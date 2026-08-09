//! The job-lifecycle state machine (§4, NIP-QW01/QW02): turns a set of
//! already-defined events into one [`ContractState`], so a client doesn't
//! re-derive "did both sides complete, is a countersign overdue" logic
//! independently — and the two-phase atomic credit-issuance exchange.
//!
//! Pure and read-only: nothing here mutates events or performs I/O, only
//! reads a slice of already-received events plus the two parties'
//! pubkeys and the current time. That's what makes the offline-tolerance
//! property (§4) fall out for free — every step's event is fully formed
//! from purely local data (the identity signing it, and whichever prior
//! event it references), so no step needs the counterparty, or even a
//! network, to be reachable at the moment it's created.

use secp256k1::schnorr;

use crate::dual_index::{self, DualIndexStatus};
use crate::events::kinds::{
    credit_issuance, CreditIssuance as CreditIssuanceContent, QuantAmount, KIND_CREDIT_ISSUANCE,
    KIND_DISPUTE_ANNOTATION, KIND_JOB_ACCEPT, KIND_JOB_COMPLETION, KIND_JOB_COUNTEROFFER,
    KIND_JOB_MILESTONE,
};
use crate::events::{Event, UnsignedEvent};
use crate::identity::{verify_hex_schnorr, Identity};

/// §0.7: 30 days from counterparty signature timestamp before a contract
/// flips to `unsigned/expired`.
pub const DISPUTE_TIMEOUT_SECS: u64 = 30 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq)]
pub enum ContractState {
    /// No Accept yet against the current negotiation head.
    Negotiating,
    /// Accepted; not yet dual-completed.
    Accepted,
    /// Both parties have posted their own completion (dual-indexed, §2).
    Completed,
    /// A dual-signature-verified `CreditIssuance` exists — the terminal
    /// success state.
    CreditIssued,
    /// §0.7: `DISPUTE_TIMEOUT_SECS` since the last relevant signature
    /// with no further progress — either a stale negotiation or a
    /// one-sided completion nobody countersigned.
    Expired,
}

/// One contract's current view, assembled from whatever events are
/// locally visible. Nothing here is a separate source of truth — every
/// field is a pure read of the underlying signed events, recomputed
/// fresh each time (matching `crate::dual_index`'s "no separate balance
/// store, ever" philosophy).
pub struct Contract<'a> {
    pub offer_event_id: String,
    pub client_pubkey: String,
    pub worker_pubkey: String,
    /// The original offer, or the most recent counteroffer that
    /// superseded it — see [`negotiation_head`] for how ties in a
    /// branching negotiation are resolved.
    pub negotiation_head_id: String,
    pub accept: Option<&'a Event>,
    pub milestones: Vec<&'a Event>,
    pub completion_status: DualIndexStatus,
    pub credit_issuance: Option<&'a Event>,
    pub disputes: Vec<&'a Event>,
    pub state: ContractState,
}

impl<'a> Contract<'a> {
    pub fn from_events(
        events: &'a [Event],
        offer_event_id: &str,
        client_pubkey: &str,
        worker_pubkey: &str,
        now: u64,
    ) -> Self {
        let negotiation_head_id = negotiation_head(events, offer_event_id).to_string();
        let accept = find_accept(events, &negotiation_head_id);
        let milestones: Vec<&Event> = events
            .iter()
            .filter(|e| {
                e.kind == KIND_JOB_MILESTONE && e.tag_values("e").any(|id| id == offer_event_id)
            })
            .collect();
        let completion_status = dual_index::check_dual_index(
            events,
            KIND_JOB_COMPLETION,
            offer_event_id,
            &[client_pubkey, worker_pubkey],
        );
        let credit_issuance = events.iter().find(|e| {
            e.kind == KIND_CREDIT_ISSUANCE && e.tag_values("p").any(|p| p == worker_pubkey)
        });

        let completions = dual_index::union_query(events, KIND_JOB_COMPLETION, offer_event_id);
        let mut dispute_target_ids: Vec<&str> = vec![offer_event_id, negotiation_head_id.as_str()];
        dispute_target_ids.extend(milestones.iter().map(|e| e.id.as_str()));
        dispute_target_ids.extend(completions.iter().map(|e| e.id.as_str()));
        let disputes: Vec<&Event> = events
            .iter()
            .filter(|e| {
                e.kind == KIND_DISPUTE_ANNOTATION
                    && e.tag_values("e").any(|id| dispute_target_ids.contains(&id))
            })
            .collect();

        let state = derive_state(
            events,
            &negotiation_head_id,
            accept,
            &completion_status,
            &completions,
            credit_issuance,
            client_pubkey,
            worker_pubkey,
            now,
        );

        Self {
            offer_event_id: offer_event_id.to_string(),
            client_pubkey: client_pubkey.to_string(),
            worker_pubkey: worker_pubkey.to_string(),
            negotiation_head_id,
            accept,
            milestones,
            completion_status,
            credit_issuance,
            disputes,
            state,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn derive_state(
    events: &[Event],
    negotiation_head_id: &str,
    accept: Option<&Event>,
    completion_status: &DualIndexStatus,
    completions: &[&Event],
    credit_issuance: Option<&Event>,
    client_pubkey: &str,
    worker_pubkey: &str,
    now: u64,
) -> ContractState {
    if let Some(issuance) = credit_issuance {
        // Full authorization check, not just "does this look
        // well-formed": a `p`-tag match alone (how `credit_issuance` was
        // found) is trivially spoofable by anyone, so the state view must
        // not call a contract settled without confirming both embedded
        // signatures actually belong to this contract's real issuer
        // (client) and subject (worker).
        if verify_credit_issuance(issuance, client_pubkey, worker_pubkey).is_ok() {
            return ContractState::CreditIssued;
        }
    }

    if matches!(completion_status, DualIndexStatus::Complete) {
        return ContractState::Completed;
    }

    if let DualIndexStatus::Missing(missing) = completion_status {
        if missing.len() == 1 {
            if let Some(existing) = completions.iter().find(|e| !missing.contains(&e.pubkey)) {
                if now.saturating_sub(existing.created_at) > DISPUTE_TIMEOUT_SECS {
                    return ContractState::Expired;
                }
            }
        }
    }

    if accept.is_some() {
        return ContractState::Accepted;
    }

    if let Some(head) = events.iter().find(|e| e.id == negotiation_head_id) {
        if now.saturating_sub(head.created_at) > DISPUTE_TIMEOUT_SECS {
            return ContractState::Expired;
        }
    }

    ContractState::Negotiating
}

/// Walk forward from `offer_event_id` through counteroffers
/// (`KIND_JOB_COUNTEROFFER`) referencing each successive head. If more
/// than one counteroffer references the same head (both parties
/// countered before seeing each other's), the most recently created one
/// wins — concurrent branches aren't merged, just resolved by recency.
pub fn negotiation_head<'a>(events: &'a [Event], offer_event_id: &'a str) -> &'a str {
    let mut current = offer_event_id;
    loop {
        let next = events
            .iter()
            .filter(|e| {
                e.kind == KIND_JOB_COUNTEROFFER && e.tag_values("e").any(|id| id == current)
            })
            .max_by_key(|e| e.created_at);
        match next {
            Some(e) => current = e.id.as_str(),
            None => return current,
        }
    }
}

pub fn find_accept<'a>(events: &'a [Event], head_event_id: &str) -> Option<&'a Event> {
    events
        .iter()
        .find(|e| e.kind == KIND_JOB_ACCEPT && e.tag_values("e").any(|id| id == head_event_id))
}

// --- two-phase atomic credit issuance (§4) ---

/// Phase 1: each party independently signs the same payload hash.
/// Neither signature alone authorizes anything; only once both exist
/// does [`assemble_credit_issuance`] produce a publishable event. How the
/// two signatures actually reach each other (a direct message, a shared
/// draft, ...) is a transport concern this function doesn't address.
pub fn sign_credit_issuance_payload(
    identity: &Identity,
    completion_event_id: &str,
    amount: &QuantAmount,
) -> schnorr::Signature {
    let hash = CreditIssuanceContent::payload_hash(completion_event_id, amount);
    identity.sign_schnorr(&hash)
}

/// Phase 2: once both signatures are collected, assemble the publishable
/// event. The caller still signs+publishes the resulting `UnsignedEvent`
/// (`UnsignedEvent::sign`) themselves — that outer NIP-01 signature only
/// proves who published it; `issuer_sig`/`subject_sig` are what carry
/// consent (see [`verify_credit_issuance`]).
pub fn assemble_credit_issuance(
    issuer_pubkey_hex: &str,
    subject_pubkey_hex: &str,
    completion_event_id_hex: &str,
    amount: QuantAmount,
    issuer_sig: schnorr::Signature,
    subject_sig: schnorr::Signature,
) -> UnsignedEvent {
    let payload_hash = CreditIssuanceContent::payload_hash(completion_event_id_hex, &amount);
    let issuance = CreditIssuanceContent {
        completion_event_id: completion_event_id_hex.to_string(),
        payload_hash: hex::encode(payload_hash),
        amount,
        issuer_sig: hex::encode(issuer_sig.to_byte_array()),
        subject_sig: hex::encode(subject_sig.to_byte_array()),
    };
    credit_issuance(
        issuer_pubkey_hex,
        subject_pubkey_hex,
        completion_event_id_hex,
        &issuance,
    )
}

#[derive(Debug, PartialEq)]
pub enum CreditIssuanceError {
    EventVerificationFailed,
    Malformed,
    PayloadHashMismatch,
    InvalidIssuerSig,
    InvalidSubjectSig,
}

/// Confirm consent for a published `CreditIssuance` event without
/// trusting whoever published it: recompute `payload_hash`, then verify
/// both embedded signatures against the *known* issuer/subject pubkeys
/// (unlike [`derive_state`]'s internal-consistency-only check, this is
/// the real authorization check a client should run before treating an
/// issuance as final).
pub fn verify_credit_issuance(
    event: &Event,
    issuer_pubkey_hex: &str,
    subject_pubkey_hex: &str,
) -> Result<(), CreditIssuanceError> {
    event
        .verify()
        .map_err(|_| CreditIssuanceError::EventVerificationFailed)?;
    let content: CreditIssuanceContent =
        serde_json::from_str(&event.content).map_err(|_| CreditIssuanceError::Malformed)?;

    let expected_hash =
        CreditIssuanceContent::payload_hash(&content.completion_event_id, &content.amount);
    let claimed_hash =
        hex::decode(&content.payload_hash).map_err(|_| CreditIssuanceError::Malformed)?;
    if claimed_hash != expected_hash {
        return Err(CreditIssuanceError::PayloadHashMismatch);
    }

    if !verify_hex_schnorr(issuer_pubkey_hex, &content.issuer_sig, &expected_hash) {
        return Err(CreditIssuanceError::InvalidIssuerSig);
    }
    if !verify_hex_schnorr(subject_pubkey_hex, &content.subject_sig, &expected_hash) {
        return Err(CreditIssuanceError::InvalidSubjectSig);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::kinds::{
        job_accept, job_completion, job_counteroffer, job_offer, JobAccept, JobCompletion, JobOffer,
    };
    use crate::identity::Identity;

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

    #[test]
    fn negotiating_before_any_accept() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &sample_offer(),
        )
        .sign(&client);
        let events = vec![offer.clone()];

        let contract = Contract::from_events(
            &events,
            &offer.id,
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            100,
        );
        assert_eq!(contract.state, ContractState::Negotiating);
        assert_eq!(contract.negotiation_head_id, offer.id);
    }

    #[test]
    fn stale_negotiation_expires_after_the_timeout_with_no_accept() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let unsigned = crate::events::UnsignedEvent::with_created_at(
            client.nostr_pubkey_hex(),
            crate::events::KIND_JOB_OFFER,
            vec![crate::events::p_tag(worker.nostr_pubkey_hex())],
            serde_json::to_string(&sample_offer()).unwrap(),
            1_000,
        );
        let offer = unsigned.sign(&client);

        let events = vec![offer.clone()];
        let still_within = Contract::from_events(
            &events,
            &offer.id,
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            1_000 + DISPUTE_TIMEOUT_SECS,
        );
        assert_eq!(
            still_within.state,
            ContractState::Negotiating,
            "exactly at the boundary must not yet be expired"
        );

        let past_timeout = 1_000 + DISPUTE_TIMEOUT_SECS + 1;
        let expired = Contract::from_events(
            &events,
            &offer.id,
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            past_timeout,
        );
        assert_eq!(
            expired.state,
            ContractState::Expired,
            "an offer nobody ever accepted must expire after the timeout"
        );
    }

    #[test]
    fn dispute_annotation_is_wired_into_the_contract_view() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &sample_offer(),
        )
        .sign(&client);
        let worker_completion = job_completion(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer.id,
            &JobCompletion {
                rating: Some(1),
                note: Some("not what we agreed".to_string()),
            },
        )
        .sign(&worker);

        let annotation = crate::events::kinds::dispute_annotation(
            &client.nostr_pubkey_hex(),
            &worker_completion.id,
            &crate::events::kinds::DisputeAnnotation::AuditRequest {
                body: "worker's completion note doesn't match delivered work".to_string(),
            },
        )
        .sign(&client);

        let events = vec![offer.clone(), worker_completion, annotation.clone()];
        let contract = Contract::from_events(
            &events,
            &offer.id,
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            100,
        );

        assert_eq!(contract.disputes.len(), 1);
        assert_eq!(contract.disputes[0].id, annotation.id);
    }

    #[test]
    fn counteroffer_moves_the_negotiation_head_and_accept_targets_it() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &sample_offer(),
        )
        .sign(&client);
        let counter_terms = JobOffer {
            rate: 45.0,
            ..sample_offer()
        };
        let counter = job_counteroffer(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer.id,
            &counter_terms,
        )
        .sign(&worker);
        let accept = job_accept(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &counter.id,
            &JobAccept { note: None },
        )
        .sign(&worker);

        let events = vec![offer.clone(), counter.clone(), accept.clone()];
        let contract = Contract::from_events(
            &events,
            &offer.id,
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            100,
        );

        assert_eq!(
            contract.negotiation_head_id, counter.id,
            "head must move to the counteroffer, not stay on the original offer"
        );
        assert_eq!(contract.state, ContractState::Accepted);
        assert_eq!(contract.accept.unwrap().id, accept.id);
    }

    #[test]
    fn accept_against_the_original_offer_when_nobody_counters() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &sample_offer(),
        )
        .sign(&client);
        let accept = job_accept(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer.id,
            &JobAccept { note: None },
        )
        .sign(&worker);

        let events = vec![offer.clone(), accept];
        let contract = Contract::from_events(
            &events,
            &offer.id,
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            100,
        );
        assert_eq!(contract.state, ContractState::Accepted);
    }

    #[test]
    fn both_completions_present_is_completed_state() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &sample_offer(),
        )
        .sign(&client);
        let client_completion = job_completion(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer.id,
            &JobCompletion {
                rating: Some(5),
                note: None,
            },
        )
        .sign(&client);
        let worker_completion = job_completion(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer.id,
            &JobCompletion {
                rating: None,
                note: None,
            },
        )
        .sign(&worker);

        let events = vec![offer.clone(), client_completion, worker_completion];
        let contract = Contract::from_events(
            &events,
            &offer.id,
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            100,
        );
        assert_eq!(contract.state, ContractState::Completed);
    }

    #[test]
    fn one_sided_completion_expires_after_the_timeout() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &sample_offer(),
        )
        .sign(&client);
        let unsigned = crate::events::UnsignedEvent::with_created_at(
            client.nostr_pubkey_hex(),
            crate::events::KIND_JOB_COMPLETION,
            vec![
                crate::events::p_tag(worker.nostr_pubkey_hex()),
                crate::events::e_tag(offer.id.clone()),
            ],
            serde_json::to_string(&JobCompletion {
                rating: Some(4),
                note: None,
            })
            .unwrap(),
            1_000,
        );
        let client_completion = unsigned.sign(&client);

        let events = vec![offer.clone(), client_completion];
        let now = 1_000 + DISPUTE_TIMEOUT_SECS + 1;
        let contract = Contract::from_events(
            &events,
            &offer.id,
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            now,
        );
        assert_eq!(contract.state, ContractState::Expired);
    }

    #[test]
    fn two_phase_credit_issuance_round_trips_and_verifies() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &sample_offer(),
        )
        .sign(&client);
        let worker_completion = job_completion(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer.id,
            &JobCompletion {
                rating: None,
                note: None,
            },
        )
        .sign(&worker);

        let amount = QuantAmount::Bucket { index: 3 };
        // each party signs independently — order doesn't matter, and
        // neither needs the other online at the moment they sign
        let subject_sig = sign_credit_issuance_payload(&worker, &worker_completion.id, &amount);
        let issuer_sig = sign_credit_issuance_payload(&client, &worker_completion.id, &amount);

        let unsigned = assemble_credit_issuance(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &worker_completion.id,
            amount,
            issuer_sig,
            subject_sig,
        );
        let event = unsigned.sign(&client);

        assert!(verify_credit_issuance(
            &event,
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex()
        )
        .is_ok());

        let events = vec![offer.clone(), worker_completion, event];
        let contract = Contract::from_events(
            &events,
            &offer.id,
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            100,
        );
        assert_eq!(contract.state, ContractState::CreditIssued);
    }

    #[test]
    fn mismatched_subject_sig_fails_verification() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let attacker = Identity::generate();
        let offer = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &sample_offer(),
        )
        .sign(&client);
        let worker_completion = job_completion(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer.id,
            &JobCompletion {
                rating: None,
                note: None,
            },
        )
        .sign(&worker);

        let amount = QuantAmount::Bucket { index: 3 };
        // attacker forges the "subject" signature instead of the real worker
        let bogus_subject_sig =
            sign_credit_issuance_payload(&attacker, &worker_completion.id, &amount);
        let issuer_sig = sign_credit_issuance_payload(&client, &worker_completion.id, &amount);

        let unsigned = assemble_credit_issuance(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &worker_completion.id,
            amount,
            issuer_sig,
            bogus_subject_sig,
        );
        let event = unsigned.sign(&client);

        assert_eq!(
            verify_credit_issuance(
                &event,
                &client.nostr_pubkey_hex(),
                &worker.nostr_pubkey_hex()
            ),
            Err(CreditIssuanceError::InvalidSubjectSig)
        );
    }

    #[test]
    fn forged_credit_issuance_does_not_flip_contract_state() {
        // A CreditIssuance whose payload_hash is internally consistent
        // but whose signatures don't actually belong to this contract's
        // real issuer/subject must not be enough to show the contract as
        // settled — only a `p` tag (trivially spoofable) points at it.
        let client = Identity::generate();
        let worker = Identity::generate();
        let attacker = Identity::generate();
        let offer = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &sample_offer(),
        )
        .sign(&client);
        let worker_completion = job_completion(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer.id,
            &JobCompletion {
                rating: None,
                note: None,
            },
        )
        .sign(&worker);

        let amount = QuantAmount::Bucket { index: 3 };
        // both "signatures" are the attacker's own, not the real parties' —
        // and since `UnsignedEvent::sign` refuses to sign under a pubkey
        // it doesn't hold the key for, the attacker can only self-publish
        // (pubkey = attacker), not impersonate the client as publisher;
        // the actual attack surface is the forged embedded sigs, not the
        // outer NIP-01 signer.
        let issuer_sig = sign_credit_issuance_payload(&attacker, &worker_completion.id, &amount);
        let subject_sig = sign_credit_issuance_payload(&attacker, &worker_completion.id, &amount);
        let forged_content = CreditIssuanceContent {
            completion_event_id: worker_completion.id.clone(),
            payload_hash: hex::encode(CreditIssuanceContent::payload_hash(
                &worker_completion.id,
                &amount,
            )),
            amount,
            issuer_sig: hex::encode(issuer_sig.to_byte_array()),
            subject_sig: hex::encode(subject_sig.to_byte_array()),
        };
        let forged = crate::events::UnsignedEvent::with_created_at(
            attacker.nostr_pubkey_hex(),
            KIND_CREDIT_ISSUANCE,
            vec![
                crate::events::p_tag(worker.nostr_pubkey_hex()),
                crate::events::e_tag(worker_completion.id.clone()),
            ],
            serde_json::to_string(&forged_content).unwrap(),
            100,
        )
        .sign(&attacker);

        let events = vec![offer.clone(), worker_completion, forged];
        let contract = Contract::from_events(
            &events,
            &offer.id,
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            100,
        );
        assert_ne!(
            contract.state,
            ContractState::CreditIssued,
            "a forged issuance must not be treated as settled"
        );
    }

    #[test]
    fn offline_tolerance_every_step_composes_from_purely_local_data() {
        // Simulate weeks-long gaps between steps — nothing about
        // constructing/signing/verifying any one step needs the
        // counterparty or a network reachable at that moment, only the
        // prior step's already-received event id.
        let client = Identity::generate();
        let worker = Identity::generate();

        let offer = crate::events::UnsignedEvent::with_created_at(
            client.nostr_pubkey_hex(),
            crate::events::KIND_JOB_OFFER,
            vec![crate::events::p_tag(worker.nostr_pubkey_hex())],
            serde_json::to_string(&sample_offer()).unwrap(),
            0,
        )
        .sign(&client);
        // client goes offline for 10 days here — worker only needs the
        // offer's id, already locally held, to accept.
        let accept = crate::events::UnsignedEvent::with_created_at(
            worker.nostr_pubkey_hex(),
            KIND_JOB_ACCEPT,
            vec![
                crate::events::p_tag(client.nostr_pubkey_hex()),
                crate::events::e_tag(offer.id.clone()),
            ],
            serde_json::to_string(&JobAccept { note: None }).unwrap(),
            10 * 86_400,
        )
        .sign(&worker);
        // worker goes offline for 20 more days — client only needs the
        // offer id (not the accept, and not the worker) to complete.
        let client_completion = crate::events::UnsignedEvent::with_created_at(
            client.nostr_pubkey_hex(),
            KIND_JOB_COMPLETION,
            vec![
                crate::events::p_tag(worker.nostr_pubkey_hex()),
                crate::events::e_tag(offer.id.clone()),
            ],
            serde_json::to_string(&JobCompletion {
                rating: Some(5),
                note: None,
            })
            .unwrap(),
            30 * 86_400,
        )
        .sign(&client);

        for event in [&offer, &accept, &client_completion] {
            assert!(
                event.verify().is_ok(),
                "each step verifies independently of when the others happened"
            );
        }

        let events = vec![offer.clone(), accept, client_completion];
        // still within the timeout as of a month in — not yet expired,
        // just legitimately waiting on the worker's own completion.
        let contract = Contract::from_events(
            &events,
            &offer.id,
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            31 * 86_400,
        );
        assert_eq!(contract.state, ContractState::Accepted);
    }
}
