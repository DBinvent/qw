//! Vault/neighbor storage service (§8): signed-record backup for
//! participants without always-on nodes. A relay in miniature — accept a
//! signed event only if it verifies, retrieve by pubkey — and it reuses
//! the same event store `chain_calculation` reads from, so vaulting a
//! record is also what makes it available to that service.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::post;
use axum::Router;
use serde::Deserialize;

use qw_protocol::dual_index::all_records_about;
use qw_protocol::events::Event;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/vault/events", post(store_event).get(retrieve_events))
}

/// Accepts a signed event only if it verifies — the vault holds no
/// keys and cannot forge a record, so this is the one gate that matters.
/// Idempotent: re-submitting an already-stored event id is a no-op, not
/// an error (a participant backing up the same record from two devices
/// shouldn't need to coordinate).
async fn store_event(State(state): State<AppState>, Json(event): Json<Event>) -> impl IntoResponse {
    if event.verify().is_err() {
        return (StatusCode::BAD_REQUEST, "event does not verify").into_response();
    }
    let mut events = state.events.write().expect("event store lock poisoned");
    if events.iter().any(|e| e.id == event.id) {
        return StatusCode::OK.into_response();
    }
    events.push(event);
    StatusCode::CREATED.into_response()
}

#[derive(Debug, Deserialize)]
pub struct RetrieveQuery {
    pub pubkey: String,
}

/// Everything `pubkey` signed themselves, plus everything else that
/// names them as counterparty (`qw_protocol::dual_index::all_records_about`
/// — the same dual-indexing property from §2, now served by a vault
/// instead of requiring the participant's own relay access).
async fn retrieve_events(
    State(state): State<AppState>,
    Query(query): Query<RetrieveQuery>,
) -> impl IntoResponse {
    let events = state.events.read().expect("event store lock poisoned");
    let result: Vec<Event> = all_records_about(&events, &query.pubkey)
        .into_iter()
        .cloned()
        .collect();
    Json(result)
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    use qw_protocol::events::kinds::{profile_skill_tags, ProfileSkillTags};
    use qw_protocol::events::{p_tag, UnsignedEvent, KIND_JOB_OFFER};
    use qw_protocol::identity::Identity;

    use super::*;

    fn post_event(event: &Event) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/vault/events")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(event).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn stores_and_retrieves_a_verified_event() {
        let server_identity = Identity::generate();
        let alice = Identity::generate();
        let app = router().with_state(AppState::new(server_identity));

        let event = profile_skill_tags(
            &alice.nostr_pubkey_hex(),
            &ProfileSkillTags {
                display_name: None,
                skill_tags: vec!["it/backend/languages#rust".to_string()],
            },
        )
        .sign(&alice);

        let store_response = app.clone().oneshot(post_event(&event)).await.unwrap();
        assert_eq!(store_response.status(), StatusCode::CREATED);

        let get_response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/vault/events?pubkey={}", alice.nostr_pubkey_hex()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);
        let body = to_bytes(get_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let stored: Vec<Event> = serde_json::from_slice(&body).unwrap();
        assert_eq!(stored, vec![event]);
    }

    #[tokio::test]
    async fn rejects_an_event_that_does_not_verify() {
        let server_identity = Identity::generate();
        let alice = Identity::generate();
        let app = router().with_state(AppState::new(server_identity));

        let mut event = profile_skill_tags(
            &alice.nostr_pubkey_hex(),
            &ProfileSkillTags {
                display_name: None,
                skill_tags: vec![],
            },
        )
        .sign(&alice);
        event.content = "tampered".to_string();

        let response = app.oneshot(post_event(&event)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn resubmitting_the_same_event_is_idempotent_not_duplicated() {
        let server_identity = Identity::generate();
        let alice = Identity::generate();
        let app = router().with_state(AppState::new(server_identity));
        let event = profile_skill_tags(
            &alice.nostr_pubkey_hex(),
            &ProfileSkillTags {
                display_name: None,
                skill_tags: vec![],
            },
        )
        .sign(&alice);

        let first = app.clone().oneshot(post_event(&event)).await.unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);
        let second = app.clone().oneshot(post_event(&event)).await.unwrap();
        assert_eq!(second.status(), StatusCode::OK);

        let get_response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/vault/events?pubkey={}", alice.nostr_pubkey_hex()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(get_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let stored: Vec<Event> = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            stored.len(),
            1,
            "re-submitting the same event must not duplicate it"
        );
    }

    #[tokio::test]
    async fn retrieval_includes_records_naming_the_pubkey_as_counterparty() {
        let server_identity = Identity::generate();
        let client = Identity::generate();
        let worker = Identity::generate();
        let app = router().with_state(AppState::new(server_identity));

        // an offer authored by `client`, but tagging `worker` as counterparty
        let offer = UnsignedEvent::new(
            client.nostr_pubkey_hex(),
            KIND_JOB_OFFER,
            vec![p_tag(worker.nostr_pubkey_hex())],
            "{}",
        )
        .sign(&client);
        app.clone().oneshot(post_event(&offer)).await.unwrap();

        let get_response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/vault/events?pubkey={}",
                        worker.nostr_pubkey_hex()
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(get_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let stored: Vec<Event> = serde_json::from_slice(&body).unwrap();
        assert_eq!(stored, vec![offer], "worker never signed anything themselves but is named in this record, per dual indexing (§2)");
    }
}
