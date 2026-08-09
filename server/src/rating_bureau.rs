//! Rating-bureau service (§8): filtered, re-signed history aggregation,
//! server-hosted. A thin wrapper — reuses NIP-QW08's `HistoryRequest`/
//! `HistoryResponse` shape exactly (§2), just with the server as
//! responder instead of a contact answering about themselves. The
//! request must be a real signed `HistoryRequest` event (not a bare HTTP
//! query) — the same reasoning as `qw_server::chain_calculation`'s
//! preference for signed artifacts applies doubly here, since a real
//! deployment would gate this on the subscription NIP-QW08 mentions.
//!
//! The "subscription, priced in Quants" billing this service is meant to
//! have isn't implemented — that's a business/billing concern layered on
//! top of this endpoint, not part of the aggregation logic itself.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::post;
use axum::Router;

use qw_protocol::dual_index::all_records_about;
use qw_protocol::events::kinds::{history_response, HistoryRequest, HistoryResponse};
use qw_protocol::events::Event;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/rating-bureau/history", post(handle_history_request))
}

async fn handle_history_request(
    State(state): State<AppState>,
    Json(request_event): Json<Event>,
) -> impl IntoResponse {
    if request_event.verify().is_err() {
        return (StatusCode::BAD_REQUEST, "request event does not verify").into_response();
    }
    let Ok(request) = serde_json::from_str::<HistoryRequest>(&request_event.content) else {
        return (
            StatusCode::BAD_REQUEST,
            "request content is not a valid HistoryRequest",
        )
            .into_response();
    };
    let Some(subject_pubkey) = request_event.first_tag_value("p") else {
        return (
            StatusCode::BAD_REQUEST,
            "request must p-tag the subject being asked about",
        )
            .into_response();
    };

    let record_event_ids: Vec<String> = {
        let events = state.events.read().expect("event store lock poisoned");
        all_records_about(&events, subject_pubkey)
            .into_iter()
            .filter(|e| within_scope(e, &request))
            .map(|e| e.id.clone())
            .collect()
    };

    let response = HistoryResponse { record_event_ids };
    let response_event = history_response(
        &state.identity.nostr_pubkey_hex(),
        &request_event.pubkey,
        &request_event.id,
        &response,
    )
    .sign(&state.identity);
    Json(response_event).into_response()
}

fn within_scope(event: &Event, request: &HistoryRequest) -> bool {
    if let Some(since) = request.since {
        if event.created_at < since {
            return false;
        }
    }
    if let Some(until) = request.until {
        if event.created_at > until {
            return false;
        }
    }
    if request.skill_tags.is_empty() {
        return true;
    }
    event
        .tag_values("t")
        .any(|t| request.skill_tags.iter().any(|rt| rt == t))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    use qw_protocol::events::kinds::{history_request, job_offer, JobOffer};
    use qw_protocol::identity::Identity;

    use super::*;

    fn post_json(uri: &str, event: &Event) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(event).unwrap()))
            .unwrap()
    }

    fn tagged_offer(
        author: &Identity,
        counterparty: &Identity,
        skill: &str,
        created_at: u64,
    ) -> Event {
        let offer = JobOffer {
            skill_tags: vec![skill.to_string()],
            hours: 1.0,
            rate: 1.0,
            ko: None,
            km: None,
            terms: "t".to_string(),
        };
        let unsigned = job_offer(
            &author.nostr_pubkey_hex(),
            &counterparty.nostr_pubkey_hex(),
            &offer,
        );
        qw_protocol::events::UnsignedEvent::with_created_at(
            unsigned.pubkey,
            unsigned.kind,
            unsigned.tags,
            unsigned.content,
            created_at,
        )
        .sign(author)
    }

    #[tokio::test]
    async fn returns_a_signed_response_referencing_the_request() {
        let server_identity = Identity::generate();
        let requester = Identity::generate();
        let subject = Identity::generate();
        let record = tagged_offer(&subject, &requester, "it/backend/languages#rust", 1_000);
        let app = router().with_state(AppState::with_events(server_identity, vec![record.clone()]));

        let request_event = history_request(
            &requester.nostr_pubkey_hex(),
            &subject.nostr_pubkey_hex(),
            &HistoryRequest {
                skill_tags: vec![],
                since: None,
                until: None,
            },
        )
        .sign(&requester);

        let response = app
            .oneshot(post_json("/rating-bureau/history", &request_event))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let response_event: Event = serde_json::from_slice(&body).unwrap();

        assert!(response_event.verify().is_ok());
        assert_eq!(
            response_event.first_tag_value("e"),
            Some(request_event.id.as_str())
        );
        assert_eq!(
            response_event.first_tag_value("p"),
            Some(requester.nostr_pubkey_hex().as_str())
        );

        let content: HistoryResponse = serde_json::from_str(&response_event.content).unwrap();
        assert_eq!(content.record_event_ids, vec![record.id]);
    }

    #[tokio::test]
    async fn scope_filters_by_skill_tag_and_time_window() {
        let server_identity = Identity::generate();
        let requester = Identity::generate();
        let subject = Identity::generate();
        let rust_record = tagged_offer(&subject, &requester, "it/backend/languages#rust", 1_000);
        let frontend_record = tagged_offer(&subject, &requester, "it/frontend#react", 2_000);
        let too_old_record = tagged_offer(&subject, &requester, "it/backend/languages#rust", 10);
        let app = router().with_state(AppState::with_events(
            server_identity,
            vec![rust_record.clone(), frontend_record, too_old_record],
        ));

        let scoped = HistoryRequest {
            skill_tags: vec!["it/backend/languages#rust".to_string()],
            since: Some(500),
            until: None,
        };
        let request_event = history_request(
            &requester.nostr_pubkey_hex(),
            &subject.nostr_pubkey_hex(),
            &scoped,
        )
        .sign(&requester);

        let response = app
            .oneshot(post_json("/rating-bureau/history", &request_event))
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let response_event: Event = serde_json::from_slice(&body).unwrap();
        let content: HistoryResponse = serde_json::from_str(&response_event.content).unwrap();

        assert_eq!(
            content.record_event_ids,
            vec![rust_record.id],
            "must exclude both the wrong-skill and the too-old record"
        );
    }

    #[tokio::test]
    async fn rejects_a_request_event_that_does_not_verify() {
        let server_identity = Identity::generate();
        let requester = Identity::generate();
        let subject = Identity::generate();
        let app = router().with_state(AppState::new(server_identity));

        let mut request_event = history_request(
            &requester.nostr_pubkey_hex(),
            &subject.nostr_pubkey_hex(),
            &HistoryRequest {
                skill_tags: vec![],
                since: None,
                until: None,
            },
        )
        .sign(&requester);
        request_event.content = "tampered".to_string();

        let response = app
            .oneshot(post_json("/rating-bureau/history", &request_event))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
