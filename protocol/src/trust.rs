//! Trust graph & net_position (§5): local, per-viewer computations over
//! already-received events — no global index, no shared reputation
//! score, no separate balance store. Everything here is a pure read of
//! `crate::contract`'s verified `CreditIssuance` events, recomputed fresh
//! each time (same "no separate source of truth" philosophy as
//! `crate::dual_index` and `crate::contract`).
//!
//! What "local" means concretely: every function here takes whatever
//! events slice the caller already has visibility into (their own relays
//! plus whichever friends' relays they can reach) — there is no fetch, no
//! network call, no assumption the caller has the *complete* graph.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::contract::verify_credit_issuance;
use crate::dual_index::{check_dual_index, DualIndexStatus};
use crate::events::{
    same_domain, Event, KIND_CREDIT_ISSUANCE, KIND_JOB_COMPLETION, KIND_JOB_OFFER,
};

// --- local graph walk ---

/// A path of verified `CreditIssuance` events connecting the viewer to a
/// target pubkey. `hops.len() == edges.len()`; `hops == 0` only for the
/// degenerate "target is self" case (`edges` empty).
#[derive(Debug, Clone, PartialEq)]
pub struct TrustPath<'a> {
    pub target: String,
    pub hops: u8,
    pub edges: Vec<&'a Event>,
}

/// BFS outward from `self_pubkey` over verified `CreditIssuance` edges
/// (undirected, for reachability — direction only matters for
/// `net_position`), up to `max_hops`, optionally restricted to edges
/// whose underlying job shares a taxonomy domain with `skill_domain`
/// (`crate::events::same_domain`). Returns the *shortest* verified path,
/// or `None` if unreachable within `max_hops`.
///
/// No global index: this only ever looks at `events`, which is whatever
/// the caller already has local access to — own relay plus friends' —
/// exactly the "runs against relays the node already has access to"
/// constraint from `todo-impl.md` §5.
pub fn find_trust_path<'a>(
    events: &'a [Event],
    self_pubkey: &str,
    target_pubkey: &str,
    max_hops: u8,
    skill_domain: Option<&str>,
) -> Option<TrustPath<'a>> {
    if self_pubkey == target_pubkey {
        return Some(TrustPath {
            target: target_pubkey.to_string(),
            hops: 0,
            edges: Vec::new(),
        });
    }

    let edges = verified_edges(events, skill_domain);

    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(self_pubkey.to_string());
    let mut frontier: Vec<String> = vec![self_pubkey.to_string()];
    let mut predecessor: HashMap<String, (String, &'a Event)> = HashMap::new();

    for _ in 0..max_hops {
        if frontier.is_empty() {
            break;
        }
        let mut next_frontier = Vec::new();
        for node in &frontier {
            for (a, b, ev) in &edges {
                let neighbor = if a == node {
                    Some(b.as_str())
                } else if b == node {
                    Some(a.as_str())
                } else {
                    None
                };
                let Some(neighbor) = neighbor else { continue };
                if visited.contains(neighbor) {
                    continue;
                }
                visited.insert(neighbor.to_string());
                predecessor.insert(neighbor.to_string(), (node.clone(), ev));
                if neighbor == target_pubkey {
                    return Some(reconstruct_path(&predecessor, target_pubkey));
                }
                next_frontier.push(neighbor.to_string());
            }
        }
        frontier = next_frontier;
    }
    None
}

fn reconstruct_path<'a>(
    predecessor: &HashMap<String, (String, &'a Event)>,
    target: &str,
) -> TrustPath<'a> {
    let mut edges = Vec::new();
    let mut current = target.to_string();
    while let Some((prev, ev)) = predecessor.get(&current) {
        edges.push(*ev);
        current = prev.clone();
    }
    edges.reverse();
    TrustPath {
        target: target.to_string(),
        hops: edges.len() as u8,
        edges,
    }
}

/// Verified `(issuer, subject, event)` triples, optionally filtered to
/// those whose underlying job (via its completion's offer) shares a
/// domain with `skill_domain`. An edge whose domain can't be resolved
/// (the completion or offer isn't in `events`) is excluded when a filter
/// is given — we can't confirm a match, so it doesn't count as one.
fn verified_edges<'a>(
    events: &'a [Event],
    skill_domain: Option<&str>,
) -> Vec<(String, String, &'a Event)> {
    let mut edges = Vec::new();
    for e in events {
        if e.kind != KIND_CREDIT_ISSUANCE {
            continue;
        }
        let Some(subject) = e.first_tag_value("p") else {
            continue;
        };
        let issuer = e.pubkey.as_str();
        if verify_credit_issuance(e, issuer, subject).is_err() {
            continue;
        }
        if let Some(domain) = skill_domain {
            let tags = resolve_skill_tags(events, e);
            if !tags.iter().any(|t| same_domain(t, domain)) {
                continue;
            }
        }
        edges.push((issuer.to_string(), subject.to_string(), e));
    }
    edges
}

/// Walk `credit_issuance -> completion (by e-tag) -> offer (by e-tag) ->
/// skill_tags` to find what domain a `CreditIssuance` edge belongs to.
fn resolve_skill_tags(events: &[Event], credit_issuance_event: &Event) -> Vec<String> {
    let Ok(issuance) =
        serde_json::from_str::<crate::events::CreditIssuance>(&credit_issuance_event.content)
    else {
        return Vec::new();
    };
    let Some(completion) = events
        .iter()
        .find(|e| e.id == issuance.completion_event_id && e.kind == KIND_JOB_COMPLETION)
    else {
        return Vec::new();
    };
    let Some(offer_id) = completion.first_tag_value("e") else {
        return Vec::new();
    };
    let Some(offer) = events
        .iter()
        .find(|e| e.id == offer_id && e.kind == KIND_JOB_OFFER)
    else {
        return Vec::new();
    };
    serde_json::from_str::<crate::events::JobOffer>(&offer.content)
        .map(|o| o.skill_tags)
        .unwrap_or_default()
}

/// One skill, with what the contract record says about it.
///
/// The tag is the thing; this is the evidence attached to it. Deliberately
/// not a "level" or a tier — a skill can also carry a broker review, and the
/// two are independent axes rather than positions on one ladder. Anything
/// that collapsed them into a single rank would have to invent an exchange
/// rate between "a stranger looked at your GitHub" and "someone paid you and
/// signed for it", and there isn't one.
#[derive(Debug, Clone, PartialEq)]
pub struct EarnedSkill {
    pub tag: String,
    /// Countersigned contracts carrying this tag.
    pub contracts: usize,
    /// Mean of the ratings *counterparties* gave, 0-5.
    ///
    /// Read from the other party's completion, never from your own:
    /// `JobCompletion::rating` is "how the author rates the counterparty",
    /// so your own completion rates them. Taking it from the wrong side
    /// would let anyone rate themselves five stars, which is the exact
    /// failure this whole section exists to avoid.
    ///
    /// `None` when no counterparty recorded one — the field is optional, and
    /// absent is not zero.
    pub rating: Option<f64>,
}

/// What a pubkey has *earned*: the skills of contracts they completed and a
/// counterparty countersigned, with the contract count and rating for each.
///
/// This exists for **reach and display, not trust**. §3's greedy routing
/// matched `Contact::cached_skill_tags` — a self-published profile — so a
/// person with ten countersigned Rust contracts who never published a `rust`
/// tag was unreachable by a Rust query, and the participant with the most
/// evidence was the hardest to find. None of this feeds
/// [`score_trust_path`], which still reads completed work per viewer.
///
/// "Completed" means countersigned, not self-declared: both parties must
/// have published a [`KIND_JOB_COMPLETION`] anchored to the same offer,
/// which is [`crate::dual_index`]'s own definition of a finished contract.
/// One side alone earns nothing, or the earned set would be exactly the free
/// self-assertion the declared tags already are.
pub fn earned_skills(events: &[Event], pubkey_hex: &str) -> Vec<EarnedSkill> {
    // tag -> (contracts, ratings seen)
    let mut acc: BTreeMap<String, (usize, Vec<f64>)> = BTreeMap::new();
    let mut counted_offers: HashSet<&str> = HashSet::new();

    for completion in events
        .iter()
        .filter(|e| e.kind == KIND_JOB_COMPLETION && e.pubkey == pubkey_hex)
    {
        let (Some(offer_id), Some(counterparty)) = (
            completion.first_tag_value("e"),
            completion.first_tag_value("p"),
        ) else {
            continue;
        };

        // Both sides, or it is a claim about a contract rather than one.
        if check_dual_index(
            events,
            KIND_JOB_COMPLETION,
            offer_id,
            &[pubkey_hex, counterparty],
        ) != DualIndexStatus::Complete
        {
            continue;
        }

        // A republished completion must not count the contract twice.
        if !counted_offers.insert(offer_id) {
            continue;
        }

        let Some(offer) = events
            .iter()
            .find(|e| e.id == offer_id && e.kind == KIND_JOB_OFFER)
        else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<crate::events::JobOffer>(&offer.content) else {
            continue;
        };

        // The counterparty's side of the same offer is what rates us.
        let rating = events
            .iter()
            .find(|e| {
                e.kind == KIND_JOB_COMPLETION
                    && e.pubkey == counterparty
                    && e.tag_values("e").any(|id| id == offer_id)
            })
            .and_then(|e| serde_json::from_str::<crate::events::JobCompletion>(&e.content).ok())
            .and_then(|c| c.rating)
            .map(f64::from);

        for tag in parsed.skill_tags {
            let entry = acc.entry(tag).or_insert((0, Vec::new()));
            entry.0 += 1;
            if let Some(r) = rating {
                entry.1.push(r);
            }
        }
    }

    acc.into_iter()
        .map(|(tag, (contracts, ratings))| EarnedSkill {
            tag,
            contracts,
            rating: if ratings.is_empty() {
                None
            } else {
                Some(ratings.iter().sum::<f64>() / ratings.len() as f64)
            },
        })
        .collect()
}

/// Just the tags, for routing — which cares whether a skill is proven, not
/// how well.
pub fn earned_skill_tags(events: &[Event], pubkey_hex: &str) -> Vec<String> {
    earned_skills(events, pubkey_hex)
        .into_iter()
        .map(|s| s.tag)
        .collect()
}

// --- per-viewer subjective scoring ---

/// Weights are the viewer's own configuration — "not a shared algorithm
/// output" (§5). This struct is *a* reasonable default set of knobs, not
/// *the* scoring algorithm; a different viewer plugging in different
/// weights (or an entirely different function over the same
/// [`TrustPath`]) is exactly the point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoringWeights {
    /// Multiplier applied per hop — how much a path's trust signal
    /// decays with distance. `1.0` = no decay; `0.5` halves per hop.
    pub hop_decay: f64,
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self { hop_decay: 0.5 }
    }
}

/// One default (not authoritative) way to turn a [`TrustPath`] into a
/// single number: the *closing* edge's issuance value
/// ([`crate::events::QuantAmount::approx_value`]) — the actual evidence
/// that the target completed verified work — discounted by `hop_decay`
/// raised to `hops - 1`. Earlier edges only establish how far away that
/// evidence is (via the exponent), and deliberately don't add their own
/// value on top: they're evidence about intermediate hops, not about the
/// target, so summing them in would let a long path of unrelated
/// transactions outscore a short, direct one — the opposite of what
/// "decay with distance" should mean.
pub fn score_trust_path(path: &TrustPath, weights: &ScoringWeights) -> f64 {
    let Some(closing_edge) = path.edges.last() else {
        return 0.0; // target is self (0 hops) or, defensively, an empty path
    };
    let value = serde_json::from_str::<crate::events::CreditIssuance>(&closing_edge.content)
        .map(|c| c.amount.approx_value())
        .unwrap_or(0.0);
    value * weights.hop_decay.powi((path.hops.saturating_sub(1)) as i32)
}

// --- new-account handling ---

/// A viewer's reputation read on some pubkey. Deliberately **not** a
/// single number with "no history" mapped to zero — an account with zero
/// signed history and an account with a long history of exactly balanced
/// give/take are different risk profiles, and collapsing them to the
/// same score would hide that (§5: "unknown-risk, not neutral/zero").
#[derive(Debug, Clone, PartialEq)]
pub enum ReputationState {
    /// No verified `CreditIssuance` evidence reaches this pubkey within
    /// the traversal at all — not "trusted a little," genuinely unknown.
    UnknownRisk,
    Scored(f64),
}

/// Combine a trust-graph walk with a scoring function into one
/// viewer-facing reputation read.
pub fn assess_reputation(
    events: &[Event],
    self_pubkey: &str,
    target_pubkey: &str,
    max_hops: u8,
    skill_domain: Option<&str>,
    weights: &ScoringWeights,
) -> ReputationState {
    match find_trust_path(events, self_pubkey, target_pubkey, max_hops, skill_domain) {
        Some(path) => ReputationState::Scored(score_trust_path(&path, weights)),
        None => ReputationState::UnknownRisk,
    }
}

// --- net_position ---

/// `Σ(delivered) − Σ(issued)`, verified: only counts `CreditIssuance`
/// events that actually pass [`verify_credit_issuance`] against the
/// pubkeys the event itself claims (`event.pubkey` = issuer, `p` tag =
/// subject) — an unverified `p`-tag match alone is trivially spoofable
/// (the same class of gap `crate::contract::derive_state` had to close).
/// No separate balance store, ever — recomputed from `events` every call.
pub fn net_position(events: &[Event], self_pubkey: &str) -> f64 {
    net_position_with(events, self_pubkey, None)
}

/// Bilateral variant: the same figure restricted to one counterparty —
/// what the admission filters (below) actually need, since the FAQ's
/// position limit is "Quants given versus taken **with that
/// counterparty**," not the viewer's global balance.
pub fn net_position_with(
    events: &[Event],
    self_pubkey: &str,
    counterparty_pubkey: Option<&str>,
) -> f64 {
    let mut delivered = 0.0;
    let mut issued = 0.0;
    for e in events {
        if e.kind != KIND_CREDIT_ISSUANCE {
            continue;
        }
        let Ok(content) = serde_json::from_str::<crate::events::CreditIssuance>(&e.content) else {
            continue;
        };
        let Some(subject) = e.first_tag_value("p") else {
            continue;
        };
        let issuer = e.pubkey.as_str();

        if subject == self_pubkey {
            if let Some(cp) = counterparty_pubkey {
                if issuer != cp {
                    continue;
                }
            }
            if verify_credit_issuance(e, issuer, subject).is_ok() {
                delivered += content.amount.approx_value();
            }
        } else if issuer == self_pubkey {
            if let Some(cp) = counterparty_pubkey {
                if subject != cp {
                    continue;
                }
            }
            if verify_credit_issuance(e, issuer, subject).is_ok() {
                issued += content.amount.approx_value();
            }
        }
    }
    delivered - issued
}

/// Sum of verified `CreditIssuance` value involving `counterparty_pubkey`
/// (either side) at or after `since` — a building block for an
/// admission-filter position limit that "scales with how much work that
/// counterparty has recently completed" (FAQ, added 2026-08-07). The
/// actual scaling formula is left to the caller/client config — see
/// `todo-impl.md` §0.8.
pub fn counterparty_recent_volume(events: &[Event], counterparty_pubkey: &str, since: u64) -> f64 {
    events
        .iter()
        .filter(|e| e.kind == KIND_CREDIT_ISSUANCE && e.created_at >= since)
        .filter(|e| {
            e.pubkey == counterparty_pubkey || e.first_tag_value("p") == Some(counterparty_pubkey)
        })
        .filter_map(|e| {
            let content = serde_json::from_str::<crate::events::CreditIssuance>(&e.content).ok()?;
            let subject = e.first_tag_value("p")?;
            verify_credit_issuance(e, &e.pubkey, subject).ok()?;
            Some(content.amount.approx_value())
        })
        .sum()
}

// --- admission filters (§5, added 2026-08-07) ---

/// Per-participant configuration — never protocol-mandated (§0.8: no
/// enforced default). `None` on either field means that filter isn't
/// enforced at all.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AdmissionPolicy {
    pub min_reputation: Option<f64>,
    /// Ceiling on the *magnitude* of bilateral net_position with the
    /// requesting counterparty — how the caller derived this number
    /// (e.g. scaled by [`counterparty_recent_volume`]) is up to them.
    pub position_limit: Option<f64>,
}

/// A decline carries no reason — "thresholds stay private precisely so
/// they cannot be probed and tuned around" (FAQ). Callers must not
/// surface *why* a request was declined, only that it was.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AdmissionDecision {
    Admit,
    Decline,
}

/// Apply both local pre-filters to an inbound request from
/// `requester_pubkey`, before it ever surfaces to a human. Reuses
/// [`assess_reputation`] and [`net_position_with`] as the pre-request
/// gate the FAQ describes ("the same market-pricing mechanism as record
/// acceptance, moved earlier in the lifecycle"), not just a display
/// value.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_admission(
    events: &[Event],
    self_pubkey: &str,
    requester_pubkey: &str,
    policy: &AdmissionPolicy,
    max_hops: u8,
    skill_domain: Option<&str>,
    weights: &ScoringWeights,
) -> AdmissionDecision {
    if let Some(min) = policy.min_reputation {
        match assess_reputation(
            events,
            self_pubkey,
            requester_pubkey,
            max_hops,
            skill_domain,
            weights,
        ) {
            ReputationState::UnknownRisk => return AdmissionDecision::Decline,
            ReputationState::Scored(score) if score < min => return AdmissionDecision::Decline,
            ReputationState::Scored(_) => {}
        }
    }

    if let Some(limit) = policy.position_limit {
        let bilateral = net_position_with(events, self_pubkey, Some(requester_pubkey));
        if bilateral.abs() > limit {
            return AdmissionDecision::Decline;
        }
    }

    AdmissionDecision::Admit
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

    /// Build one verified `CreditIssuance` edge from `issuer` to
    /// `subject`, plus the offer/completion events it chains from, so
    /// domain resolution works too.
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
    fn direct_edge_is_one_hop() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let events = issue_credit(&alice, &bob, QuantAmount::Bucket { index: 2 });

        let path = find_trust_path(
            &events,
            &alice.nostr_pubkey_hex(),
            &bob.nostr_pubkey_hex(),
            3,
            None,
        )
        .unwrap();
        assert_eq!(path.hops, 1);
        assert_eq!(path.edges.len(), 1);
    }

    #[test]
    fn transitive_edge_is_two_hops() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let carol = Identity::generate();
        let mut events = issue_credit(&alice, &bob, QuantAmount::Bucket { index: 2 });
        events.extend(issue_credit(&bob, &carol, QuantAmount::Bucket { index: 2 }));

        let path = find_trust_path(
            &events,
            &alice.nostr_pubkey_hex(),
            &carol.nostr_pubkey_hex(),
            3,
            None,
        )
        .unwrap();
        assert_eq!(path.hops, 2);

        assert!(
            find_trust_path(
                &events,
                &alice.nostr_pubkey_hex(),
                &carol.nostr_pubkey_hex(),
                1,
                None
            )
            .is_none(),
            "must respect max_hops"
        );
    }

    #[test]
    fn unreachable_target_is_none_not_a_zero_score() {
        let alice = Identity::generate();
        let stranger = Identity::generate();
        let events: Vec<Event> = Vec::new();
        assert!(find_trust_path(
            &events,
            &alice.nostr_pubkey_hex(),
            &stranger.nostr_pubkey_hex(),
            3,
            None
        )
        .is_none());
    }

    #[test]
    fn domain_filter_excludes_unrelated_edges() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let events = issue_credit(&alice, &bob, QuantAmount::Bucket { index: 2 }); // it/backend

        let matching = find_trust_path(
            &events,
            &alice.nostr_pubkey_hex(),
            &bob.nostr_pubkey_hex(),
            3,
            Some("it/backend/frameworks#axum"),
        );
        assert!(matching.is_some(), "same-domain filter should still match");

        let non_matching = find_trust_path(
            &events,
            &alice.nostr_pubkey_hex(),
            &bob.nostr_pubkey_hex(),
            3,
            Some("it/frontend#react"),
        );
        assert!(
            non_matching.is_none(),
            "different-domain filter should exclude the edge"
        );
    }

    #[test]
    fn score_decays_with_hop_distance() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let carol = Identity::generate();
        let mut events = issue_credit(&alice, &bob, QuantAmount::Exact { quants: 10.0 });
        events.extend(issue_credit(
            &bob,
            &carol,
            QuantAmount::Exact { quants: 10.0 },
        ));

        let weights = ScoringWeights::default();
        let direct = find_trust_path(
            &events,
            &alice.nostr_pubkey_hex(),
            &bob.nostr_pubkey_hex(),
            3,
            None,
        )
        .unwrap();
        let transitive = find_trust_path(
            &events,
            &alice.nostr_pubkey_hex(),
            &carol.nostr_pubkey_hex(),
            3,
            None,
        )
        .unwrap();

        assert!(
            score_trust_path(&direct, &weights) > score_trust_path(&transitive, &weights),
            "a closer path must score higher under decay"
        );
    }

    #[test]
    fn unknown_pubkey_is_unknown_risk_not_zero() {
        let alice = Identity::generate();
        let stranger = Identity::generate();
        let events: Vec<Event> = Vec::new();
        let weights = ScoringWeights::default();
        assert_eq!(
            assess_reputation(
                &events,
                &alice.nostr_pubkey_hex(),
                &stranger.nostr_pubkey_hex(),
                3,
                None,
                &weights
            ),
            ReputationState::UnknownRisk
        );
    }

    #[test]
    fn net_position_nets_delivered_against_issued() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let mut events = issue_credit(&alice, &bob, QuantAmount::Exact { quants: 100.0 }); // alice issues to bob
        events.extend(issue_credit(
            &bob,
            &alice,
            QuantAmount::Exact { quants: 30.0 },
        )); // bob issues to alice

        // alice: delivered 30 (received from bob), issued 100 (paid bob) -> net = 30 - 100 = -70
        assert_eq!(
            net_position(&events, &alice.nostr_pubkey_hex()),
            30.0 - 100.0
        );
        // bob: delivered 100, issued 30 -> net = 70
        assert_eq!(net_position(&events, &bob.nostr_pubkey_hex()), 100.0 - 30.0);
    }

    #[test]
    fn net_position_ignores_unverified_events() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let attacker = Identity::generate();

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
        let amount = QuantAmount::Exact { quants: 500.0 };
        // attacker self-publishes (can't sign as alice without her key);
        // the actual attack surface is the forged embedded sigs, tagged
        // to look like a real alice->bob issuance.
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
        assert_eq!(
            net_position(&events, &bob.nostr_pubkey_hex()),
            0.0,
            "an unverified issuance must not count"
        );
    }

    #[test]
    fn admission_declines_unknown_risk_when_min_reputation_set() {
        let alice = Identity::generate();
        let stranger = Identity::generate();
        let events: Vec<Event> = Vec::new();
        let policy = AdmissionPolicy {
            min_reputation: Some(1.0),
            position_limit: None,
        };

        let decision = evaluate_admission(
            &events,
            &alice.nostr_pubkey_hex(),
            &stranger.nostr_pubkey_hex(),
            &policy,
            3,
            None,
            &ScoringWeights::default(),
        );
        assert_eq!(decision, AdmissionDecision::Decline);
    }

    #[test]
    fn admission_admits_when_no_thresholds_configured() {
        let alice = Identity::generate();
        let stranger = Identity::generate();
        let events: Vec<Event> = Vec::new();
        let policy = AdmissionPolicy::default();

        let decision = evaluate_admission(
            &events,
            &alice.nostr_pubkey_hex(),
            &stranger.nostr_pubkey_hex(),
            &policy,
            3,
            None,
            &ScoringWeights::default(),
        );
        assert_eq!(decision, AdmissionDecision::Admit);
    }

    #[test]
    fn admission_declines_over_position_limit() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        // alice has issued 1000 to bob, received nothing back -> alice's
        // bilateral net_position with bob is -1000
        let events = issue_credit(&alice, &bob, QuantAmount::Exact { quants: 1000.0 });
        let policy = AdmissionPolicy {
            min_reputation: None,
            position_limit: Some(500.0),
        };

        let decision = evaluate_admission(
            &events,
            &alice.nostr_pubkey_hex(),
            &bob.nostr_pubkey_hex(),
            &policy,
            3,
            None,
            &ScoringWeights::default(),
        );
        assert_eq!(decision, AdmissionDecision::Decline);
    }

    // --- earned skill tags (routing input, not scoring) ---

    fn rust_offer() -> crate::events::JobOffer {
        crate::events::JobOffer {
            skill_tags: vec!["it/backend/languages#rust".to_string()],
            hours: 8.0,
            rate: 40.0,
            ko: None,
            km: None,
            terms: "backend work".to_string(),
        }
    }

    /// Client offers, both parties sign a completion anchored to it.
    /// `client_rates_worker` is what the client puts on their own
    /// completion — i.e. the worker's rating.
    /// `terms` distinguishes two contracts between the same pair. It has to:
    /// a Nostr event id is a hash of its own fields, so two offers with
    /// identical content in the same second *are* the same event, and a test
    /// that reused the terms would be building one contract twice rather
    /// than two contracts.
    fn rated_contract(
        client: &Identity,
        worker: &Identity,
        terms: &str,
        client_rates_worker: Option<u8>,
        worker_rates_client: Option<u8>,
    ) -> Vec<Event> {
        let mut offer_content = rust_offer();
        offer_content.terms = terms.to_string();
        let offer = crate::events::job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer_content,
        )
        .sign(client);
        let worker_side = crate::events::job_completion(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer.id,
            &crate::events::JobCompletion {
                rating: worker_rates_client,
                note: None,
            },
        )
        .sign(worker);
        let client_side = crate::events::job_completion(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer.id,
            &crate::events::JobCompletion {
                rating: client_rates_worker,
                note: None,
            },
        )
        .sign(client);
        vec![offer, worker_side, client_side]
    }

    fn completed_contract(client: &Identity, worker: &Identity) -> Vec<Event> {
        let offer = crate::events::job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &rust_offer(),
        )
        .sign(client);
        let done = crate::events::JobCompletion {
            rating: Some(5),
            note: None,
        };
        let worker_side = crate::events::job_completion(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer.id,
            &done,
        )
        .sign(worker);
        let client_side = crate::events::job_completion(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer.id,
            &done,
        )
        .sign(client);
        vec![offer, worker_side, client_side]
    }

    #[test]
    fn countersigned_work_yields_the_offers_skill_tags() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let events = completed_contract(&client, &worker);

        assert_eq!(
            earned_skill_tags(&events, &worker.nostr_pubkey_hex()),
            vec!["it/backend/languages#rust".to_string()],
        );
        // Both sides did the contract, so both sides earned the tag — the
        // client's own history of commissioning Rust work is real too.
        assert_eq!(
            earned_skill_tags(&events, &client.nostr_pubkey_hex()),
            vec!["it/backend/languages#rust".to_string()],
        );
    }

    /// The whole point of "countersigned": one party declaring a contract
    /// finished is worth exactly as much as a self-published profile tag,
    /// which is to say it must not become an earned one.
    #[test]
    fn a_one_sided_completion_earns_nothing() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let mut events = completed_contract(&client, &worker);
        // Drop the client's half.
        events
            .retain(|e| !(e.kind == KIND_JOB_COMPLETION && e.pubkey == client.nostr_pubkey_hex()));

        assert!(
            earned_skill_tags(&events, &worker.nostr_pubkey_hex()).is_empty(),
            "a completion nobody countersigned is a claim, not evidence"
        );
    }

    /// Someone else's finished contract must not leak into your earned set
    /// just because the events are in the same local store.
    #[test]
    fn earned_tags_do_not_bleed_between_pubkeys() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let bystander = Identity::generate();
        let events = completed_contract(&client, &worker);

        assert!(earned_skill_tags(&events, &bystander.nostr_pubkey_hex()).is_empty());
    }

    #[test]
    fn repeated_contracts_in_one_skill_do_not_duplicate_the_tag() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let mut events = completed_contract(&client, &worker);
        events.extend(completed_contract(&client, &worker));

        assert_eq!(
            earned_skill_tags(&events, &worker.nostr_pubkey_hex()).len(),
            1,
            "routing wants the set of skills, not a tally of contracts"
        );
    }

    /// The rating on your record is the one the *counterparty* gave. Taking
    /// it from your own completion would let anyone award themselves five
    /// stars, which is the failure this whole section exists to avoid.
    #[test]
    fn quality_comes_from_the_counterpartys_rating_not_your_own() {
        let client = Identity::generate();
        let worker = Identity::generate();
        // Client thinks the worker was a 5; the worker thinks the client was a 1.
        let events = rated_contract(&client, &worker, "sprint 1", Some(5), Some(1));

        let worker_skills = earned_skills(&events, &worker.nostr_pubkey_hex());
        assert_eq!(
            worker_skills[0].rating,
            Some(5.0),
            "worker is rated by the client"
        );

        let client_skills = earned_skills(&events, &client.nostr_pubkey_hex());
        assert_eq!(
            client_skills[0].rating,
            Some(1.0),
            "client is rated by the worker"
        );
    }

    #[test]
    fn contracts_accumulate_and_ratings_average() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let mut events = rated_contract(&client, &worker, "sprint 1", Some(5), None);
        events.extend(rated_contract(&client, &worker, "sprint 2", Some(3), None));

        let skills = earned_skills(&events, &worker.nostr_pubkey_hex());
        assert_eq!(skills.len(), 1, "one skill, not one row per contract");
        assert_eq!(skills[0].contracts, 2);
        assert_eq!(skills[0].rating, Some(4.0));
    }

    /// Ratings are optional. Absent must stay absent — rendering it as zero
    /// would turn "nobody said" into "they said it was terrible".
    #[test]
    fn an_unrated_contract_still_counts_but_carries_no_score() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let events = rated_contract(&client, &worker, "sprint 1", None, None);

        let skills = earned_skills(&events, &worker.nostr_pubkey_hex());
        assert_eq!(skills[0].contracts, 1);
        assert_eq!(skills[0].rating, None);
    }

    /// A relay serving the same completion twice must not inflate the count.
    #[test]
    fn a_republished_completion_does_not_count_the_contract_twice() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let mut events = rated_contract(&client, &worker, "sprint 1", Some(4), None);
        let dupe = events.clone();
        events.extend(dupe);

        let skills = earned_skills(&events, &worker.nostr_pubkey_hex());
        assert_eq!(skills[0].contracts, 1);
    }
}
