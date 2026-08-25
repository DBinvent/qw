//! Sybil resistance: cascade block (§6, NIP-QW05). Pure, local
//! evaluation over whatever flag/introduction/block-record events a node
//! can currently see — no central blocklist, no global state. A node
//! decides to block, and re-publishes its own `CascadeBlockRecord` vouch
//! (`crate::events::kinds::cascade_block_record`); that new event is what
//! other nodes later see and can independently re-derive the same
//! decision from — cascade *is* the accumulation of these signed vouches
//! across the network over time, not a step this module performs itself.
//!
//! Two scope limitations, both consistent with how the rest of this
//! crate handles similarly hard graph problems (see NIP-QW06's own
//! deferral of true multi-path independence):
//!
//! - **"Independent flaggers" means distinct signer pubkeys only.**
//!   §0.5's "non-overlapping paths" — verifying the flaggers aren't
//!   themselves a sockpuppet cluster reachable through one shared
//!   intermediary — is not implemented.
//! - **"Relay-graph distance" is measured over the published
//!   Introduction graph (NIP-QW07), not a node's private contact list**
//!   (`qw_node::contact::Contact` is local-only and never published, per
//!   §3 — it cannot be what a *distance* claim about someone else's
//!   graph refers to; Introduction events are the one contact-adjacent
//!   graph that's actually public).

use std::collections::{HashMap, HashSet};

use crate::events::{
    Event, Introduction, KIND_CASCADE_BLOCK_FLAG, KIND_CASCADE_BLOCK_RECORD, KIND_INTRODUCTION,
};

/// §0.5 default: any WoT member may flag; a block auto-cascades to
/// accounts within `auto_cascade_distance` hops of a flagged signer once
/// `min_independent_flaggers` distinct flaggers confirm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CascadePolicy {
    pub min_independent_flaggers: usize,
    pub auto_cascade_distance: u8,
}

impl Default for CascadePolicy {
    fn default() -> Self {
        Self {
            min_independent_flaggers: 2,
            auto_cascade_distance: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockReason {
    /// Directly flagged by at least `min_independent_flaggers` distinct
    /// pubkeys.
    DirectlyFlagged { flaggers: Vec<String> },
    /// Within `auto_cascade_distance` hops, via the Introduction graph,
    /// of a directly-flagged signer.
    Cascaded {
        flagged_signer: String,
        distance: u8,
    },
    /// A `CascadeBlockRecord` from elsewhere already vouches for this
    /// pubkey — adopting/re-affirming someone else's already-published
    /// decision. This is the actual "social propagation, not a central
    /// blocklist" mechanism: re-publishing this decision (via
    /// `crate::events::kinds::cascade_block_record`) is what lets a
    /// third node later re-derive the same thing from *this* node's
    /// vouch instead.
    Vouched { sourced_from_pubkey: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockDecision {
    pub pubkey: String,
    pub reason: BlockReason,
}

/// Evaluate every locally-visible flag/introduction/block-record event
/// and return every pubkey this node's policy says to block, and why.
/// Purely a read of `events` — no fetch, no global index, matching
/// `crate::trust`/`crate::contract`'s "no separate source of truth"
/// philosophy. A caller who accepts a decision still has to actually
/// build+sign+publish the resulting `cascade_block_record` themselves
/// (picking whichever specific evidence event to reference) — this
/// function only decides, it doesn't publish.
pub fn evaluate_flags(events: &[Event], policy: &CascadePolicy) -> Vec<BlockDecision> {
    let mut decisions: HashMap<String, BlockDecision> = HashMap::new();

    let mut flaggers_by_target: HashMap<String, HashSet<String>> = HashMap::new();
    for e in events {
        if e.kind != KIND_CASCADE_BLOCK_FLAG || e.verify().is_err() {
            continue;
        }
        let Some(target) = e.first_tag_value("p") else {
            continue;
        };
        flaggers_by_target
            .entry(target.to_string())
            .or_default()
            .insert(e.pubkey.clone());
    }
    for (target, flaggers) in &flaggers_by_target {
        if flaggers.len() >= policy.min_independent_flaggers {
            decisions.insert(
                target.clone(),
                BlockDecision {
                    pubkey: target.clone(),
                    reason: BlockReason::DirectlyFlagged {
                        flaggers: flaggers.iter().cloned().collect(),
                    },
                },
            );
        }
    }

    if policy.auto_cascade_distance > 0 {
        let adjacency = introduction_adjacency(events);
        let flagged_signers: Vec<String> = decisions.keys().cloned().collect();
        for signer in flagged_signers {
            for (pubkey, distance) in bfs_within(&adjacency, &signer, policy.auto_cascade_distance)
            {
                decisions.entry(pubkey.clone()).or_insert(BlockDecision {
                    pubkey,
                    reason: BlockReason::Cascaded {
                        flagged_signer: signer.clone(),
                        distance,
                    },
                });
            }
        }
    }

    for e in events {
        if e.kind != KIND_CASCADE_BLOCK_RECORD || e.verify().is_err() {
            continue;
        }
        let Some(target) = e.first_tag_value("p") else {
            continue;
        };
        decisions
            .entry(target.to_string())
            .or_insert(BlockDecision {
                pubkey: target.to_string(),
                reason: BlockReason::Vouched {
                    sourced_from_pubkey: e.pubkey.clone(),
                },
            });
    }

    decisions.into_values().collect()
}

/// Undirected adjacency from published `KIND_INTRODUCTION` events:
/// introducer<->recipient, and introducer<->subject when introducing a
/// third party (a mutual introduction).
///
/// Edges marked `via: "public-link"` are **skipped**. Cascade block rests
/// on a real signing account standing behind each edge — that is what
/// makes "block the accounts behind the farm and the farm falls with
/// them" work. A public invite link (NIP-QW07) is exactly the edge where
/// nobody stands behind anything: the publisher posted a URL and a
/// stranger followed it. Counting those would mean publishing an ad makes
/// every reader who clicks it distance-1 from the publisher, so two flags
/// against any one of them would cascade onto the publisher — punishing
/// distribution, which the network needs, on evidence about strangers,
/// which it has none of.
fn introduction_adjacency(events: &[Event]) -> HashMap<String, HashSet<String>> {
    let mut adjacency: HashMap<String, HashSet<String>> = HashMap::new();
    let mut connect = |a: String, b: String| {
        adjacency.entry(a.clone()).or_default().insert(b.clone());
        adjacency.entry(b).or_default().insert(a);
    };

    for e in events {
        if e.kind != KIND_INTRODUCTION || e.verify().is_err() {
            continue;
        }
        let Some(recipient) = e.first_tag_value("p") else {
            continue;
        };
        let Ok(intro) = serde_json::from_str::<Introduction>(&e.content) else {
            continue;
        };
        if intro.is_public_link() {
            continue;
        }
        let introducer = e.pubkey.clone();
        connect(introducer.clone(), recipient.to_string());
        if intro.subject_pubkey != introducer {
            connect(introducer, intro.subject_pubkey);
        }
    }
    adjacency
}

fn bfs_within(
    adjacency: &HashMap<String, HashSet<String>>,
    start: &str,
    max_distance: u8,
) -> Vec<(String, u8)> {
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(start.to_string());
    let mut frontier = vec![start.to_string()];
    let mut result = Vec::new();

    for distance in 1..=max_distance {
        let mut next = Vec::new();
        for node in &frontier {
            let Some(neighbors) = adjacency.get(node) else {
                continue;
            };
            for n in neighbors {
                if visited.insert(n.clone()) {
                    result.push((n.clone(), distance));
                    next.push(n.clone());
                }
            }
        }
        frontier = next;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::kinds::{
        cascade_block_flag, cascade_block_record, introduction, CascadeBlockFlag,
        CascadeBlockRecord,
    };
    use crate::identity::Identity;

    fn flag(flagger: &Identity, target: &Identity) -> Event {
        cascade_block_flag(
            &flagger.nostr_pubkey_hex(),
            &target.nostr_pubkey_hex(),
            &CascadeBlockFlag {
                reason: "spam pattern".to_string(),
                evidence_event_id: None,
            },
        )
        .sign(flagger)
    }

    fn intro(introducer: &Identity, recipient: &Identity) -> Event {
        introduction(
            &introducer.nostr_pubkey_hex(),
            &recipient.nostr_pubkey_hex(),
            &Introduction {
                subject_pubkey: introducer.nostr_pubkey_hex(),
                chain: vec![],
                note: None,
                via: None,
            },
        )
        .sign(introducer)
    }

    /// The same edge, minted by following a published invite link.
    fn public_link_intro(follower: &Identity, publisher: &Identity) -> Event {
        introduction(
            &follower.nostr_pubkey_hex(),
            &publisher.nostr_pubkey_hex(),
            &Introduction::public_link(follower.nostr_pubkey_hex()),
        )
        .sign(follower)
    }

    #[test]
    fn single_flagger_does_not_meet_default_threshold() {
        let target = Identity::generate();
        let flagger = Identity::generate();
        let events = vec![flag(&flagger, &target)];

        let decisions = evaluate_flags(&events, &CascadePolicy::default());
        assert!(
            decisions.is_empty(),
            "1 flagger must not meet the default threshold of 2"
        );
    }

    #[test]
    fn two_independent_flaggers_trigger_a_direct_block() {
        let target = Identity::generate();
        let (flagger_a, flagger_b) = (Identity::generate(), Identity::generate());
        let events = vec![flag(&flagger_a, &target), flag(&flagger_b, &target)];

        let decisions = evaluate_flags(&events, &CascadePolicy::default());
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].pubkey, target.nostr_pubkey_hex());
        match &decisions[0].reason {
            BlockReason::DirectlyFlagged { flaggers } => assert_eq!(flaggers.len(), 2),
            other => panic!("expected DirectlyFlagged, got {other:?}"),
        }
    }

    #[test]
    fn same_flagger_twice_does_not_double_count() {
        let target = Identity::generate();
        let flagger = Identity::generate();
        // two flags, same signer — must still count as 1 distinct flagger
        let events = vec![flag(&flagger, &target), flag(&flagger, &target)];
        assert!(evaluate_flags(&events, &CascadePolicy::default()).is_empty());
    }

    #[test]
    fn cascade_reaches_distance_one_introduction_neighbor() {
        let signer = Identity::generate();
        let neighbor = Identity::generate();
        let (flagger_a, flagger_b) = (Identity::generate(), Identity::generate());

        let mut events = vec![flag(&flagger_a, &signer), flag(&flagger_b, &signer)];
        events.push(intro(&signer, &neighbor)); // signer <-> neighbor edge

        let decisions = evaluate_flags(&events, &CascadePolicy::default());
        let neighbor_decision = decisions
            .iter()
            .find(|d| d.pubkey == neighbor.nostr_pubkey_hex())
            .expect("neighbor must be cascaded");
        assert_eq!(
            neighbor_decision.reason,
            BlockReason::Cascaded {
                flagged_signer: signer.nostr_pubkey_hex(),
                distance: 1
            }
        );
    }

    #[test]
    fn cascade_skips_public_link_edges_but_not_vouched_ones() {
        // A flagged publisher with two neighbours at distance 1: one who
        // was actually introduced, one who merely followed the published
        // invite link. Only the first is a vouch, so only the first
        // cascades — otherwise posting a link on LinkedIn would make every
        // reader who clicked it blockable on evidence about the publisher.
        let publisher = Identity::generate();
        let vouched = Identity::generate();
        let stranger = Identity::generate();
        let (flagger_a, flagger_b) = (Identity::generate(), Identity::generate());

        let events = vec![
            flag(&flagger_a, &publisher),
            flag(&flagger_b, &publisher),
            intro(&publisher, &vouched),
            public_link_intro(&stranger, &publisher),
        ];

        let decisions = evaluate_flags(&events, &CascadePolicy::default());
        assert!(
            decisions
                .iter()
                .any(|d| d.pubkey == vouched.nostr_pubkey_hex()),
            "an ordinary introduction neighbour must still cascade"
        );
        assert!(
            !decisions
                .iter()
                .any(|d| d.pubkey == stranger.nostr_pubkey_hex()),
            "a public-link neighbour must not be cascaded"
        );
    }

    #[test]
    fn public_link_edge_does_not_relay_a_cascade_onward() {
        // The skip has to apply to the *path*, not just the last hop: a
        // stranger who followed the link must not become a bridge that
        // carries a block from the publisher to their own contacts.
        let publisher = Identity::generate();
        let stranger = Identity::generate();
        let strangers_contact = Identity::generate();
        let (flagger_a, flagger_b) = (Identity::generate(), Identity::generate());

        let events = vec![
            flag(&flagger_a, &publisher),
            flag(&flagger_b, &publisher),
            public_link_intro(&stranger, &publisher),
            intro(&stranger, &strangers_contact),
        ];

        let decisions = evaluate_flags(
            &events,
            &CascadePolicy {
                auto_cascade_distance: 2,
                ..CascadePolicy::default()
            },
        );
        assert!(
            !decisions
                .iter()
                .any(|d| d.pubkey == strangers_contact.nostr_pubkey_hex()),
            "a cascade must not travel through a public-link edge"
        );
    }

    #[test]
    fn cascade_does_not_reach_distance_two_by_default() {
        let signer = Identity::generate();
        let hop1 = Identity::generate();
        let hop2 = Identity::generate();
        let (flagger_a, flagger_b) = (Identity::generate(), Identity::generate());

        let events = vec![
            flag(&flagger_a, &signer),
            flag(&flagger_b, &signer),
            intro(&signer, &hop1),
            intro(&hop1, &hop2),
        ];

        let decisions = evaluate_flags(&events, &CascadePolicy::default());
        assert!(
            decisions
                .iter()
                .any(|d| d.pubkey == hop1.nostr_pubkey_hex()),
            "distance 1 must cascade"
        );
        assert!(
            !decisions
                .iter()
                .any(|d| d.pubkey == hop2.nostr_pubkey_hex()),
            "distance 2 must not auto-cascade by default"
        );
    }

    #[test]
    fn an_existing_block_record_is_adopted_as_vouched() {
        let target = Identity::generate();
        let other_node = Identity::generate();
        let some_evidence = Identity::generate(); // stand-in for whatever event other_node originally sourced from
        let record = cascade_block_record(
            &other_node.nostr_pubkey_hex(),
            &target.nostr_pubkey_hex(),
            &some_evidence.nostr_pubkey_hex(),
            &CascadeBlockRecord { distance: 1 },
        )
        .sign(&other_node);

        let events = vec![record];
        let decisions = evaluate_flags(&events, &CascadePolicy::default());
        assert_eq!(decisions.len(), 1);
        assert_eq!(
            decisions[0].reason,
            BlockReason::Vouched {
                sourced_from_pubkey: other_node.nostr_pubkey_hex()
            }
        );
    }

    #[test]
    fn bot_farm_reachability_collapses_by_blocking_the_real_signers() {
        // Two real signers, each introduced to their own cluster of 10
        // sockpuppets. Flagging only the two real signers (with 2
        // independent flaggers each) must cascade-block their entire
        // farms without anyone having to flag each bot individually.
        let signer_a = Identity::generate();
        let signer_b = Identity::generate();
        let (flagger_1, flagger_2) = (Identity::generate(), Identity::generate());

        let farm_a: Vec<Identity> = (0..10).map(|_| Identity::generate()).collect();
        let farm_b: Vec<Identity> = (0..10).map(|_| Identity::generate()).collect();

        let mut events = vec![
            flag(&flagger_1, &signer_a),
            flag(&flagger_2, &signer_a),
            flag(&flagger_1, &signer_b),
            flag(&flagger_2, &signer_b),
        ];
        for bot in &farm_a {
            events.push(intro(&signer_a, bot));
        }
        for bot in &farm_b {
            events.push(intro(&signer_b, bot));
        }

        let decisions = evaluate_flags(&events, &CascadePolicy::default());
        let blocked: HashSet<String> = decisions.into_iter().map(|d| d.pubkey).collect();

        assert!(blocked.contains(&signer_a.nostr_pubkey_hex()));
        assert!(blocked.contains(&signer_b.nostr_pubkey_hex()));
        for bot in farm_a.iter().chain(farm_b.iter()) {
            assert!(
                blocked.contains(&bot.nostr_pubkey_hex()),
                "every bot must be reachable via cascade without being individually flagged"
            );
        }
        // exactly the 2 signers + 20 bots, nothing extra
        assert_eq!(blocked.len(), 22);
    }
}
