//! Chain-calculation service (§8, NIP-QW10): HTTP wrapper around
//! `qw_protocol::trust::find_trust_path` + `score_trust_path`. The trust-
//! graph logic itself already lives in, and is tested by,
//! `qw_protocol::trust` (§5) — this module only turns an HTTP query into
//! that call and signs the result, per NIP-QW10's "server must never be
//! the only source of truth" requirement.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use qw_protocol::events::kinds::{chain_calculation_result, ChainCalculationResult};
use qw_protocol::events::Event;
use qw_protocol::trust::{find_trust_path, score_trust_path, ScoringWeights};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/chain-calculation", get(handle_chain_calculation))
}

#[derive(Debug, Deserialize)]
pub struct ChainCalculationQuery {
    pub requester_pubkey: String,
    pub target_pubkey: String,
    #[serde(default = "default_max_hops")]
    pub max_hops: u8,
    pub skill_domain: Option<String>,
}

fn default_max_hops() -> u8 {
    3
}

/// The response body is just the signed `Event` (NIP-QW10, kind 9090) —
/// no separate envelope. A client that only reads `event.content`'s
/// `edge_event_ids`/`score` without calling `event.verify()` first, and
/// without independently checking at least some of those edges against
/// its own relay data, has skipped the entire point of this NIP.
async fn handle_chain_calculation(
    State(state): State<AppState>,
    Query(query): Query<ChainCalculationQuery>,
) -> impl IntoResponse {
    // `TrustPath` borrows from the guard, so compute everything owned
    // needs before it (and the guard) go out of scope.
    let result = {
        let events = state.events.read().expect("event store lock poisoned");
        let Some(path) = find_trust_path(
            &events,
            &query.requester_pubkey,
            &query.target_pubkey,
            query.max_hops,
            query.skill_domain.as_deref(),
        ) else {
            return (
                StatusCode::NOT_FOUND,
                "no verified path found within max_hops",
            )
                .into_response();
        };
        let score = score_trust_path(&path, &ScoringWeights::default());
        ChainCalculationResult {
            target_pubkey: path.target.clone(),
            hops: path.hops,
            edge_event_ids: path.edges.iter().map(|e| e.id.clone()).collect(),
            score,
        }
    };
    let event: Event = chain_calculation_result(
        &state.identity.nostr_pubkey_hex(),
        &query.requester_pubkey,
        &result,
    )
    .sign(&state.identity);

    Json(event).into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    use qw_protocol::contract::{assemble_credit_issuance, sign_credit_issuance_payload};
    use qw_protocol::events::kinds::{job_completion, job_offer, JobCompletion, JobOffer};
    use qw_protocol::events::{Event, QuantAmount};
    use qw_protocol::identity::Identity;

    use super::*;

    fn sample_offer() -> JobOffer {
        JobOffer {
            skill_tags: vec!["it/backend/languages#rust".to_string()],
            hours: 4.0,
            rate: 30.0,
            ko: None,
            km: None,
            terms: "fix the flaky test".to_string(),
        }
    }

    fn issue_credit(issuer: &Identity, subject: &Identity) -> Vec<Event> {
        let offer = job_offer(
            &issuer.nostr_pubkey_hex(),
            &subject.nostr_pubkey_hex(),
            &sample_offer(),
        )
        .sign(issuer);
        let completion = job_completion(
            &subject.nostr_pubkey_hex(),
            &issuer.nostr_pubkey_hex(),
            &offer.id,
            &JobCompletion {
                rating: None,
                note: None,
            },
        )
        .sign(subject);
        let amount = QuantAmount::Bucket { index: 2 };
        let issuer_sig = sign_credit_issuance_payload(issuer, &completion.id, &amount);
        let subject_sig = sign_credit_issuance_payload(subject, &completion.id, &amount);
        let issuance = assemble_credit_issuance(
            &issuer.nostr_pubkey_hex(),
            &subject.nostr_pubkey_hex(),
            &completion.id,
            amount,
            issuer_sig,
            subject_sig,
        )
        .sign(issuer);
        vec![offer, completion, issuance]
    }

    #[tokio::test]
    async fn returns_a_signed_verifiable_chain_result() {
        let server_identity = Identity::generate();
        let alice = Identity::generate();
        let bob = Identity::generate();
        let events = issue_credit(&alice, &bob);
        let app = router().with_state(AppState::with_events(server_identity, events));

        let uri = format!(
            "/chain-calculation?requester_pubkey={}&target_pubkey={}&max_hops=3",
            alice.nostr_pubkey_hex(),
            bob.nostr_pubkey_hex()
        );
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let event: Event = serde_json::from_slice(&body).unwrap();

        // the client's actual verification step — never trust the HTTP
        // 200 alone
        assert!(event.verify().is_ok());
        let content: ChainCalculationResult = serde_json::from_str(&event.content).unwrap();
        assert_eq!(content.target_pubkey, bob.nostr_pubkey_hex());
        assert_eq!(content.hops, 1);
        assert_eq!(content.edge_event_ids.len(), 1);
    }

    #[tokio::test]
    async fn returns_404_when_no_path_exists() {
        let server_identity = Identity::generate();
        let alice = Identity::generate();
        let stranger = Identity::generate();
        let app = router().with_state(AppState::new(server_identity));

        let uri = format!(
            "/chain-calculation?requester_pubkey={}&target_pubkey={}",
            alice.nostr_pubkey_hex(),
            stranger.nostr_pubkey_hex()
        );
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
