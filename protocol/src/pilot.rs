//! Pilot success metric (§10): "N signed contracts completed end-to-end
//! (offer → countersigned credit) inside the cohort, not signups" — a
//! direct implementation of the plan's own stated metric, so measuring
//! it doesn't need to be reinvented once real pilot data exists.

use std::collections::HashSet;

use crate::contract::verify_credit_issuance;
use crate::events::{Event, KIND_CREDIT_ISSUANCE};

/// Count verified `CreditIssuance` events where both the issuer and the
/// subject are members of `cohort`. Each one is, by construction, one
/// contract that made it all the way from offer to countersigned credit
/// (§4's `ContractState::CreditIssued`) — exactly what the pilot is
/// meant to measure. An unverified event (the same forged-signature
/// class `crate::contract`/`crate::trust` already guard against) doesn't
/// count, and neither does one involving anyone outside the cohort.
pub fn completed_contracts_in_cohort(events: &[Event], cohort: &HashSet<String>) -> usize {
    events
        .iter()
        .filter(|e| e.kind == KIND_CREDIT_ISSUANCE)
        .filter(|e| cohort.contains(&e.pubkey))
        .filter(|e| {
            e.first_tag_value("p")
                .map(|p| cohort.contains(p))
                .unwrap_or(false)
        })
        .filter(|e| {
            let Some(subject) = e.first_tag_value("p") else {
                return false;
            };
            verify_credit_issuance(e, &e.pubkey, subject).is_ok()
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{assemble_credit_issuance, sign_credit_issuance_payload};
    use crate::events::kinds::{job_completion, job_offer, JobCompletion, JobOffer};
    use crate::events::QuantAmount;
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

    fn complete_contract(issuer: &Identity, subject: &Identity) -> Vec<Event> {
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

    #[test]
    fn counts_completed_contracts_within_the_cohort() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let cohort: HashSet<String> = [alice.nostr_pubkey_hex(), bob.nostr_pubkey_hex()]
            .into_iter()
            .collect();

        let mut events = complete_contract(&alice, &bob);
        events.extend(complete_contract(&bob, &alice));

        assert_eq!(completed_contracts_in_cohort(&events, &cohort), 2);
    }

    #[test]
    fn excludes_contracts_touching_someone_outside_the_cohort() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let outsider = Identity::generate();
        let cohort: HashSet<String> = [alice.nostr_pubkey_hex(), bob.nostr_pubkey_hex()]
            .into_iter()
            .collect();

        let mut events = complete_contract(&alice, &bob);
        events.extend(complete_contract(&alice, &outsider));

        assert_eq!(
            completed_contracts_in_cohort(&events, &cohort),
            1,
            "the outsider's contract must not count toward the pilot metric"
        );
    }

    #[test]
    fn excludes_unverified_issuances() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let attacker = Identity::generate();
        let cohort: HashSet<String> = [alice.nostr_pubkey_hex(), bob.nostr_pubkey_hex()]
            .into_iter()
            .collect();

        let offer = job_offer(
            &alice.nostr_pubkey_hex(),
            &bob.nostr_pubkey_hex(),
            &sample_offer(),
        )
        .sign(&alice);
        let completion = job_completion(
            &bob.nostr_pubkey_hex(),
            &alice.nostr_pubkey_hex(),
            &offer.id,
            &JobCompletion {
                rating: None,
                note: None,
            },
        )
        .sign(&bob);
        let amount = QuantAmount::Bucket { index: 1 };
        let bogus_sig = sign_credit_issuance_payload(&attacker, &completion.id, &amount);
        let forged_content = crate::events::CreditIssuance {
            completion_event_id: completion.id.clone(),
            payload_hash: hex::encode(crate::events::CreditIssuance::payload_hash(
                &completion.id,
                &amount,
            )),
            amount,
            issuer_sig: hex::encode(bogus_sig.to_byte_array()),
            subject_sig: hex::encode(bogus_sig.to_byte_array()),
        };
        let forged = crate::events::UnsignedEvent::with_created_at(
            attacker.nostr_pubkey_hex(),
            KIND_CREDIT_ISSUANCE,
            vec![
                crate::events::p_tag(bob.nostr_pubkey_hex()),
                crate::events::e_tag(completion.id.clone()),
            ],
            serde_json::to_string(&forged_content).unwrap(),
            100,
        )
        .sign(&attacker);

        let events = vec![offer, completion, forged];
        assert_eq!(completed_contracts_in_cohort(&events, &cohort), 0);
    }
}
