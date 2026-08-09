//! Bulletin board service (§8, NIP-QW11): a public, browsable board of
//! undirected self-advertisements. "The server works as relay and ads
//! board — when the user is not in network... users might get online,
//! post a message and switch off, while another user can get online and
//! check messages/board like craigslist. This does not require both
//! users to be online at same time" (`todo-impl.md` §8). Unlike
//! `qw_server::vault` (retrieve by a *known* pubkey), this is filterable
//! by category without knowing who posted — the actual new capability
//! this NIP adds.
//!
//! Rate limits and monetization for board usage are left to the server
//! operator, not implemented here — same treatment as
//! `qw_server::rating_bureau`'s subscription billing.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::post;
use axum::Router;
use serde::Deserialize;

use qw_protocol::events::kinds::{BulletinListing, ListingType};
use qw_protocol::events::{now, same_domain, Event, KIND_BULLETIN_LISTING};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/board/listings", post(post_listing).get(browse_listings))
}

/// Accepts a listing only if it's actually kind 9091 and verifies — this
/// endpoint hosts listings specifically, not arbitrary events (that's
/// `qw_server::vault`'s job). Idempotent on re-submission, same reasoning
/// as the vault.
async fn post_listing(
    State(state): State<AppState>,
    Json(event): Json<Event>,
) -> impl IntoResponse {
    if event.kind != KIND_BULLETIN_LISTING {
        return (
            StatusCode::BAD_REQUEST,
            "not a bulletin listing (kind 9091) event",
        )
            .into_response();
    }
    if event.verify().is_err() {
        return (StatusCode::BAD_REQUEST, "event does not verify").into_response();
    }
    if serde_json::from_str::<BulletinListing>(&event.content).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            "content is not a valid BulletinListing",
        )
            .into_response();
    }

    let mut events = state.events.write().expect("event store lock poisoned");
    if events.iter().any(|e| e.id == event.id) {
        return StatusCode::OK.into_response();
    }
    events.push(event);
    StatusCode::CREATED.into_response()
}

#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
    pub skill_tag: Option<String>,
    pub listing_type: Option<ListingType>,
}

async fn browse_listings(
    State(state): State<AppState>,
    Query(query): Query<BrowseQuery>,
) -> impl IntoResponse {
    let events = state.events.read().expect("event store lock poisoned");
    let listings: Vec<Event> = browse(
        &events,
        query.skill_tag.as_deref(),
        query.listing_type,
        now(),
    )
    .into_iter()
    .cloned()
    .collect();
    Json(listings)
}

/// Pure filter, so expiry can be tested without depending on real time:
/// `kind == KIND_BULLETIN_LISTING`, not expired as of `now`, and matching
/// `skill_tag`/`listing_type` when given. Skill matching is domain-aware
/// (`qw_protocol::events::same_domain`) — a "rust" listing is found
/// browsing "backend", not only by an exact tag.
fn browse<'a>(
    events: &'a [Event],
    skill_tag: Option<&str>,
    listing_type: Option<ListingType>,
    now: u64,
) -> Vec<&'a Event> {
    let mut result = Vec::new();
    for e in events {
        if e.kind != KIND_BULLETIN_LISTING {
            continue;
        }
        let Ok(listing) = serde_json::from_str::<BulletinListing>(&e.content) else {
            continue;
        };
        if let Some(expires_at) = listing.expires_at {
            if expires_at < now {
                continue;
            }
        }
        if let Some(lt) = listing_type {
            if lt != listing.listing_type {
                continue;
            }
        }
        if let Some(tag) = skill_tag {
            if !listing
                .skill_tags
                .iter()
                .any(|t| t == tag || same_domain(t, tag))
            {
                continue;
            }
        }
        result.push(e);
    }
    result
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    use qw_protocol::events::kinds::bulletin_listing;
    use qw_protocol::events::kinds::{profile_skill_tags, ProfileSkillTags};
    use qw_protocol::identity::Identity;

    use super::*;

    fn sample_listing(
        listing_type: ListingType,
        skill_tags: &[&str],
        expires_at: Option<u64>,
    ) -> BulletinListing {
        BulletinListing {
            listing_type,
            skill_tags: skill_tags.iter().map(|s| s.to_string()).collect(),
            description: "test listing".to_string(),
            expires_at,
        }
    }

    fn post_json(event: &Event) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/board/listings")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(event).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn posts_and_browses_a_listing_end_to_end() {
        let server_identity = Identity::generate();
        let poster = Identity::generate();
        let app = router().with_state(AppState::new(server_identity));

        let listing = sample_listing(ListingType::Offering, &["it/backend/languages#rust"], None);
        let event = bulletin_listing(&poster.nostr_pubkey_hex(), &listing).sign(&poster);

        let store = app.clone().oneshot(post_json(&event)).await.unwrap();
        assert_eq!(store.status(), StatusCode::CREATED);

        let browse_response = app
            .oneshot(
                Request::builder()
                    .uri("/board/listings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(browse_response.status(), StatusCode::OK);
        let body = to_bytes(browse_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let listed: Vec<Event> = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            listed,
            vec![event],
            "browsing requires no knowledge of the poster's pubkey"
        );
    }

    #[tokio::test]
    async fn rejects_a_non_listing_kind() {
        let server_identity = Identity::generate();
        let poster = Identity::generate();
        let app = router().with_state(AppState::new(server_identity));

        let wrong_kind = profile_skill_tags(
            &poster.nostr_pubkey_hex(),
            &ProfileSkillTags {
                display_name: None,
                skill_tags: vec![],
            },
        )
        .sign(&poster);
        let response = app.oneshot(post_json(&wrong_kind)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejects_an_unverified_listing() {
        let server_identity = Identity::generate();
        let poster = Identity::generate();
        let app = router().with_state(AppState::new(server_identity));

        let mut event = bulletin_listing(
            &poster.nostr_pubkey_hex(),
            &sample_listing(ListingType::Seeking, &[], None),
        )
        .sign(&poster);
        event.content = "tampered".to_string();
        let response = app.oneshot(post_json(&event)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn browse_filters_by_domain_aware_skill_tag() {
        let poster = Identity::generate();
        let rust_listing = bulletin_listing(
            &poster.nostr_pubkey_hex(),
            &sample_listing(ListingType::Offering, &["it/backend/languages#rust"], None),
        )
        .sign(&poster);
        let frontend_listing = bulletin_listing(
            &poster.nostr_pubkey_hex(),
            &sample_listing(ListingType::Offering, &["it/frontend#react"], None),
        )
        .sign(&poster);
        let events = vec![rust_listing.clone(), frontend_listing];

        let found = browse(&events, Some("it/backend/frameworks#axum"), None, 0);
        assert_eq!(
            found,
            vec![&rust_listing],
            "same-domain match (backend) must find the rust listing without an exact tag match"
        );
    }

    #[test]
    fn browse_filters_by_listing_type() {
        let poster = Identity::generate();
        let offering = bulletin_listing(
            &poster.nostr_pubkey_hex(),
            &sample_listing(ListingType::Offering, &[], None),
        )
        .sign(&poster);
        let seeking = bulletin_listing(
            &poster.nostr_pubkey_hex(),
            &sample_listing(ListingType::Seeking, &[], None),
        )
        .sign(&poster);
        let events = vec![offering.clone(), seeking];

        let found = browse(&events, None, Some(ListingType::Offering), 0);
        assert_eq!(found, vec![&offering]);
    }

    #[test]
    fn browse_excludes_expired_listings() {
        let poster = Identity::generate();
        let fresh = bulletin_listing(
            &poster.nostr_pubkey_hex(),
            &sample_listing(ListingType::Offering, &[], Some(2_000)),
        )
        .sign(&poster);
        let expired = bulletin_listing(
            &poster.nostr_pubkey_hex(),
            &sample_listing(ListingType::Offering, &[], Some(500)),
        )
        .sign(&poster);
        let events = vec![fresh.clone(), expired];

        let found = browse(&events, None, None, 1_000);
        assert_eq!(found, vec![&fresh]);
    }
}
