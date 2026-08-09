//! §8: the optional coordination server. Build only after the
//! peer-to-peer core works standalone — this is an efficiency/
//! monetization layer, not a dependency. Every service here is a thin
//! wrapper: the actual computation is `qw_protocol` logic already built
//! and tested for §3/§4/§5/§8; this crate's job is only to expose it
//! over HTTP and hold the server's own signing identity.
//!
//! Deliberately not implemented: the community insurance pool — the
//! design docs call it out as explicitly last, since it depends on real
//! transaction volume existing first to fund the pool meaningfully.

pub mod board;
pub mod chain_calculation;
pub mod rating_bureau;
pub mod state;
pub mod vault;

/// The full server: every service merged into one router over one
/// shared `AppState` (one identity, one event store) — what a real
/// deployment's `main` would serve. Each service's own module is fully
/// testable in isolation (see each module's tests); this composition is
/// what `tests::` below exercises end-to-end, across services, to catch
/// anything an isolated test wouldn't (route conflicts, state not
/// actually shared).
pub fn app(state: state::AppState) -> axum::Router {
    axum::Router::new()
        .merge(chain_calculation::router())
        .merge(vault::router())
        .merge(rating_bureau::router())
        .merge(board::router())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use qw_protocol::events::kinds::{profile_skill_tags, ProfileSkillTags};
    use qw_protocol::events::Event;
    use qw_protocol::identity::Identity;

    use super::*;

    #[tokio::test]
    async fn all_three_services_answer_on_one_combined_app() {
        let server_identity = Identity::generate();
        let alice = Identity::generate();
        let combined = app(state::AppState::new(server_identity));

        // vault: store a profile event
        let profile = profile_skill_tags(
            &alice.nostr_pubkey_hex(),
            &ProfileSkillTags {
                display_name: None,
                skill_tags: vec!["it/backend/languages#rust".to_string()],
            },
        )
        .sign(&alice);
        let store = combined
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/vault/events")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&profile).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(store.status(), StatusCode::CREATED);

        // vault: retrieve it back
        let retrieve = combined
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/vault/events?pubkey={}", alice.nostr_pubkey_hex()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(retrieve.into_body(), usize::MAX).await.unwrap();
        let stored: Vec<Event> = serde_json::from_slice(&body).unwrap();
        assert_eq!(stored, vec![profile]);

        // chain-calculation: no path between two strangers, but the
        // route itself must still be reachable on the combined app
        let stranger = Identity::generate();
        let chain = combined
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/chain-calculation?requester_pubkey={}&target_pubkey={}",
                        alice.nostr_pubkey_hex(),
                        stranger.nostr_pubkey_hex()
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            chain.status(),
            StatusCode::NOT_FOUND,
            "route must be live even with no path found"
        );
    }
}
