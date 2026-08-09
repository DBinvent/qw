//! Dual indexing (§2): every contract-adjacent event is `p`-tagged with the
//! *other* party's pubkey, so "all records about pubkey A" is a plain
//! relay tag filter over records other people published — no self-report
//! trust needed. For a two-signature step (e.g. completion, which each
//! party signs separately per §4), both parties' events anchor to the
//! same prior event id (the offer), so "did both sides publish?" is a
//! union query over that anchor rather than requiring either event to
//! embed the other's id up front — which isn't constructible anyway, since
//! an event's id is a hash of its own fields and can't depend on a sibling
//! event that doesn't exist yet.
//!
//! This module only queries an in-memory event set; it stands in for
//! "whatever the relay client's local view currently holds" until a real
//! relay connection exists (§3/§8).

use std::collections::HashSet;

use crate::events::Event;

/// All events that tag `pubkey_hex` as the counterparty (`p` tag) —
/// records about this pubkey that someone *else* published.
pub fn records_referencing<'a>(events: &'a [Event], pubkey_hex: &str) -> Vec<&'a Event> {
    events
        .iter()
        .filter(|e| e.tag_values("p").any(|p| p == pubkey_hex))
        .collect()
}

/// Everything `pubkey_hex` signed themselves, plus everything else that
/// names them as counterparty (`records_referencing`) — the full "all
/// records about this pubkey" set dual indexing exists to make a plain
/// filter, deduped by event id. Used wherever a service needs someone's
/// complete visible history rather than just one half of it
/// (`qw_server::vault`, `qw_server::rating_bureau`, §8).
pub fn all_records_about<'a>(events: &'a [Event], pubkey_hex: &str) -> Vec<&'a Event> {
    let mut seen = HashSet::new();
    events
        .iter()
        .filter(|e| e.pubkey == pubkey_hex || e.tag_values("p").any(|p| p == pubkey_hex))
        .filter(|e| seen.insert(e.id.as_str()))
        .collect()
}

/// All events of `kind` anchored to `anchor_event_id` via an `e` tag,
/// regardless of author — the union across both parties' published sides.
pub fn union_query<'a>(events: &'a [Event], kind: u16, anchor_event_id: &str) -> Vec<&'a Event> {
    events
        .iter()
        .filter(|e| e.kind == kind && e.tag_values("e").any(|id| id == anchor_event_id))
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub enum DualIndexStatus {
    /// Every expected signer published their side.
    Complete,
    /// Expected signers (pubkeys) who have not published, in order.
    Missing(Vec<String>),
}

/// Check whether every pubkey in `expected_signers` has published an event
/// of `kind` anchored to `anchor_event_id`. This is the detection
/// mechanism for an omitted record: a query anyone can run against the
/// union of both parties' relays, needing no cooperation from whoever
/// stayed silent.
pub fn check_dual_index(
    events: &[Event],
    kind: u16,
    anchor_event_id: &str,
    expected_signers: &[&str],
) -> DualIndexStatus {
    let published: HashSet<&str> = union_query(events, kind, anchor_event_id)
        .into_iter()
        .map(|e| e.pubkey.as_str())
        .collect();
    let missing: Vec<String> = expected_signers
        .iter()
        .filter(|s| !published.contains(*s))
        .map(|s| s.to_string())
        .collect();
    if missing.is_empty() {
        DualIndexStatus::Complete
    } else {
        DualIndexStatus::Missing(missing)
    }
}

/// Events whose id/signature don't verify against their own fields — a
/// tampered record (id or content mutated after signing) always shows up
/// here, whichever query surfaced it.
pub fn find_invalid(events: &[Event]) -> Vec<&Event> {
    events.iter().filter(|e| e.verify().is_err()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{job_completion, job_offer, JobCompletion, JobOffer, KIND_JOB_COMPLETION};
    use crate::identity::Identity;

    fn sample_offer_id(client: &Identity, worker: &Identity) -> String {
        let offer = JobOffer {
            skill_tags: vec!["it/backend/languages#rust".to_string()],
            hours: 4.0,
            rate: 30.0,
            ko: None,
            km: None,
            terms: "fix the flaky test".to_string(),
        };
        let unsigned = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer,
        );
        unsigned.sign(client).id
    }

    #[test]
    fn all_records_about_combines_self_authored_and_referenced_without_duplicates() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer_id = sample_offer_id(&client, &worker);

        let client_completion = job_completion(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer_id,
            &JobCompletion {
                rating: Some(4),
                note: None,
            },
        )
        .sign(&client);
        let worker_completion = job_completion(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer_id,
            &JobCompletion {
                rating: None,
                note: None,
            },
        )
        .sign(&worker);

        let events = vec![client_completion.clone(), worker_completion.clone()];

        let about_worker = all_records_about(&events, &worker.nostr_pubkey_hex());
        assert_eq!(
            about_worker.len(),
            2,
            "worker's own completion plus client's completion naming worker"
        );

        let about_client = all_records_about(&events, &client.nostr_pubkey_hex());
        assert_eq!(about_client.len(), 2);

        let ids: std::collections::HashSet<&str> =
            about_worker.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(worker_completion.id.as_str()));
        assert!(ids.contains(client_completion.id.as_str()));
    }

    #[test]
    fn dual_index_surfaces_records_authored_by_the_counterparty() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer_id = sample_offer_id(&client, &worker);

        let worker_completion = job_completion(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer_id,
            &JobCompletion {
                rating: Some(5),
                note: None,
            },
        )
        .sign(&worker);

        let events = vec![worker_completion];
        // Querying "records about the client" finds the worker's record,
        // even though the client never published anything themselves.
        let about_client = records_referencing(&events, &client.nostr_pubkey_hex());
        assert_eq!(about_client.len(), 1);
        assert_eq!(about_client[0].pubkey, worker.nostr_pubkey_hex());
    }

    #[test]
    fn union_query_returns_both_sides_completion() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer_id = sample_offer_id(&client, &worker);

        let client_completion = job_completion(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer_id,
            &JobCompletion {
                rating: Some(4),
                note: Some("good work".to_string()),
            },
        )
        .sign(&client);
        let worker_completion = job_completion(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer_id,
            &JobCompletion {
                rating: None,
                note: None,
            },
        )
        .sign(&worker);

        let events = vec![client_completion.clone(), worker_completion.clone()];
        let both = union_query(&events, KIND_JOB_COMPLETION, &offer_id);
        assert_eq!(both.len(), 2);

        let status = check_dual_index(
            &events,
            KIND_JOB_COMPLETION,
            &offer_id,
            &[&client.nostr_pubkey_hex(), &worker.nostr_pubkey_hex()],
        );
        assert_eq!(status, DualIndexStatus::Complete);
    }

    #[test]
    fn omitted_record_is_detected_via_union_query() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer_id = sample_offer_id(&client, &worker);

        // Only the client ever completes their side.
        let client_completion = job_completion(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer_id,
            &JobCompletion {
                rating: Some(3),
                note: None,
            },
        )
        .sign(&client);

        let events = vec![client_completion];
        let status = check_dual_index(
            &events,
            KIND_JOB_COMPLETION,
            &offer_id,
            &[&client.nostr_pubkey_hex(), &worker.nostr_pubkey_hex()],
        );
        assert_eq!(
            status,
            DualIndexStatus::Missing(vec![worker.nostr_pubkey_hex()])
        );
    }

    #[test]
    fn tampered_record_is_detected_via_union_query() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer_id = sample_offer_id(&client, &worker);

        let mut client_completion = job_completion(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer_id,
            &JobCompletion {
                rating: Some(5),
                note: None,
            },
        )
        .sign(&client);
        // Someone rewrites the rating after the fact.
        client_completion.content = client_completion.content.replace("5", "1");

        let events = vec![client_completion];
        let matched = union_query(&events, KIND_JOB_COMPLETION, &offer_id);
        assert_eq!(
            matched.len(),
            1,
            "tampering must not remove the record from the query result"
        );
        assert_eq!(
            find_invalid(&events).len(),
            1,
            "but it must fail signature verification"
        );
    }
}
