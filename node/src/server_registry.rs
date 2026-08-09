//! Multi-server client-side selection (§8): a node routes a query to
//! whichever coordination server its own trust config ranks best, never
//! hard-coding one as authoritative. A server is scored the same way any
//! participant is (`qw_protocol::trust`) — "the server's own net
//! position and reputation are public and priced by the same social
//! mechanism as any participant's" (`abstract.md`).

use qw_protocol::events::Event;
use qw_protocol::trust::{assess_reputation, ReputationState, ScoringWeights};

#[derive(Debug, Clone, PartialEq)]
pub struct ServerCandidate {
    pub pubkey: String,
    pub base_url: String,
    /// Advertised fee for the query being routed (Quants, per
    /// `abstract.md`) — left as a plain number since fee schedules differ
    /// per service; the caller supplies whatever it means for the
    /// specific query being routed.
    pub fee: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RankedServer<'a> {
    pub candidate: &'a ServerCandidate,
    pub reputation: ReputationState,
}

/// Rank candidates by the node's own trust view of each server's pubkey,
/// highest score first; unknown-risk servers (no trust path found at
/// all) sort after every scored one, never treated as a neutral/zero
/// score — same "unknown-risk, not neutral" rule §5 applies to any
/// pubkey (`qw_protocol::trust::ReputationState`). Ties (including
/// unknown-risk vs. unknown-risk) are broken by lower fee.
///
/// This never silently designates one server as *the* server — a caller
/// that wants a single choice still picks `ranked.first()` themselves;
/// an empty `candidates` list just produces an empty ranking, not a
/// fallback to some hard-coded default.
pub fn rank_servers<'a>(
    events: &[Event],
    self_pubkey: &str,
    candidates: &'a [ServerCandidate],
    max_hops: u8,
) -> Vec<RankedServer<'a>> {
    let weights = ScoringWeights::default();
    let mut ranked: Vec<RankedServer<'a>> = candidates
        .iter()
        .map(|c| RankedServer {
            candidate: c,
            reputation: assess_reputation(events, self_pubkey, &c.pubkey, max_hops, None, &weights),
        })
        .collect();

    ranked.sort_by(|a, b| {
        use std::cmp::Ordering;
        use ReputationState::{Scored, UnknownRisk};
        match (&a.reputation, &b.reputation) {
            (Scored(sa), Scored(sb)) => sb
                .partial_cmp(sa)
                .unwrap_or(Ordering::Equal)
                .then_with(|| fee_order(a, b)),
            (Scored(_), UnknownRisk) => Ordering::Less,
            (UnknownRisk, Scored(_)) => Ordering::Greater,
            (UnknownRisk, UnknownRisk) => fee_order(a, b),
        }
    });
    ranked
}

fn fee_order(a: &RankedServer, b: &RankedServer) -> std::cmp::Ordering {
    a.candidate
        .fee
        .partial_cmp(&b.candidate.fee)
        .unwrap_or(std::cmp::Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use qw_protocol::contract::{assemble_credit_issuance, sign_credit_issuance_payload};
    use qw_protocol::events::kinds::{job_completion, job_offer, JobCompletion, JobOffer};
    use qw_protocol::events::QuantAmount;
    use qw_protocol::identity::Identity;

    use super::*;

    fn sample_offer() -> JobOffer {
        JobOffer {
            skill_tags: vec!["it/backend/languages#rust".to_string()],
            hours: 1.0,
            rate: 1.0,
            ko: None,
            km: None,
            terms: "t".to_string(),
        }
    }

    fn issue_credit(issuer: &Identity, subject: &Identity, amount: QuantAmount) -> Vec<Event> {
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
    fn a_trusted_server_ranks_above_an_unknown_risk_one() {
        let me = Identity::generate();
        let trusted_server = Identity::generate();
        let unknown_server = Identity::generate();
        let events = issue_credit(&me, &trusted_server, QuantAmount::Exact { quants: 10.0 });

        let candidates = vec![
            ServerCandidate {
                pubkey: unknown_server.nostr_pubkey_hex(),
                base_url: "https://unknown.example".to_string(),
                fee: 0.01,
            },
            ServerCandidate {
                pubkey: trusted_server.nostr_pubkey_hex(),
                base_url: "https://trusted.example".to_string(),
                fee: 5.0,
            },
        ];

        let ranked = rank_servers(&events, &me.nostr_pubkey_hex(), &candidates, 3);
        assert_eq!(
            ranked[0].candidate.pubkey,
            trusted_server.nostr_pubkey_hex(),
            "a trusted server must outrank an unknown one even with a much higher fee"
        );
        assert_eq!(
            ranked[1].candidate.pubkey,
            unknown_server.nostr_pubkey_hex()
        );
        assert_eq!(ranked[1].reputation, ReputationState::UnknownRisk);
    }

    #[test]
    fn equal_trust_breaks_ties_by_lower_fee() {
        let me = Identity::generate();
        let server_a = Identity::generate();
        let server_b = Identity::generate();
        let mut events = issue_credit(&me, &server_a, QuantAmount::Exact { quants: 10.0 });
        events.extend(issue_credit(
            &me,
            &server_b,
            QuantAmount::Exact { quants: 10.0 },
        ));

        let candidates = vec![
            ServerCandidate {
                pubkey: server_a.nostr_pubkey_hex(),
                base_url: "https://a.example".to_string(),
                fee: 2.0,
            },
            ServerCandidate {
                pubkey: server_b.nostr_pubkey_hex(),
                base_url: "https://b.example".to_string(),
                fee: 1.0,
            },
        ];

        let ranked = rank_servers(&events, &me.nostr_pubkey_hex(), &candidates, 3);
        assert_eq!(
            ranked[0].candidate.pubkey,
            server_b.nostr_pubkey_hex(),
            "equal trust must fall back to the cheaper server"
        );
    }

    #[test]
    fn unknown_risk_servers_still_rank_by_fee_among_themselves() {
        let me = Identity::generate();
        let cheap = Identity::generate();
        let pricey = Identity::generate();
        let events: Vec<Event> = Vec::new();

        let candidates = vec![
            ServerCandidate {
                pubkey: pricey.nostr_pubkey_hex(),
                base_url: "https://pricey.example".to_string(),
                fee: 9.0,
            },
            ServerCandidate {
                pubkey: cheap.nostr_pubkey_hex(),
                base_url: "https://cheap.example".to_string(),
                fee: 0.5,
            },
        ];

        let ranked = rank_servers(&events, &me.nostr_pubkey_hex(), &candidates, 3);
        assert!(ranked
            .iter()
            .all(|r| r.reputation == ReputationState::UnknownRisk));
        assert_eq!(ranked[0].candidate.pubkey, cheap.nostr_pubkey_hex());
    }

    #[test]
    fn no_candidates_is_an_empty_ranking_not_a_hardcoded_fallback() {
        let me = Identity::generate();
        let events: Vec<Event> = Vec::new();
        let ranked = rank_servers(&events, &me.nostr_pubkey_hex(), &[], 3);
        assert!(ranked.is_empty());
    }
}
