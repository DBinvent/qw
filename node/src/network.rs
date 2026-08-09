//! In-memory simulated relay network: delivers events between [`Node`]s
//! directly (a function call, not a socket) and drives propagation to
//! completion. Stands in for "whatever relays the node's own contacts are
//! reachable through" until a real relay connection exists (§3 later
//! milestone, §8) — the routing/policy logic in `crate::node` doesn't
//! know or care that delivery is simulated.

use std::collections::{HashMap, VecDeque};

use qw_protocol::events::{Event, SkillAnswer as SkillAnswerContent};

use crate::node::{Delivery, FinalAnswer, Node};
use crate::routing::select_forward_targets;

pub struct Network {
    nodes: HashMap<String, Node>,
}

#[derive(Debug, Default)]
pub struct PropagationStats {
    pub query_messages: usize,
    pub answer_messages: usize,
}

#[derive(Debug, Default)]
pub struct QueryResult {
    pub answers: Vec<FinalAnswer>,
    pub stats: PropagationStats,
}

impl Network {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    pub fn add_node(&mut self, node: Node) {
        self.nodes.insert(node.pubkey(), node);
    }

    pub fn node(&self, pubkey: &str) -> Option<&Node> {
        self.nodes.get(pubkey)
    }

    pub fn node_mut(&mut self, pubkey: &str) -> Option<&mut Node> {
        self.nodes.get_mut(pubkey)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Originate a query from `requester_pubkey` (must already be a node
    /// in the network) and run propagation to completion. Mirrors
    /// `Node::begin_relay_chain`'s greedy fanout for the *requester's own*
    /// choice of which direct contacts to privately ask first.
    pub fn originate_query(
        &mut self,
        requester_pubkey: &str,
        skill_tag: &str,
        max_hops: u8,
        now: u64,
    ) -> QueryResult {
        let query_id = format!(
            "{:016x}{:016x}",
            now,
            self.nodes.len() as u64 ^ 0x9E3779B97F4A7C15u64
        );
        let mut queue: VecDeque<Delivery> = VecDeque::new();
        let mut stats = PropagationStats::default();
        let mut answers: HashMap<String, FinalAnswer> = HashMap::new();

        let Some(requester) = self.nodes.get(requester_pubkey) else {
            return QueryResult::default();
        };
        let hop1_candidates: Vec<String> = select_forward_targets(requester.contacts(), skill_tag)
            .into_iter()
            .map(|c| c.pubkey.clone())
            .collect();

        for hop1 in hop1_candidates {
            if let Some(node) = self.nodes.get_mut(&hop1) {
                let outcome =
                    node.begin_relay_chain(&query_id, skill_tag, max_hops, requester_pubkey);
                for d in outcome.deliveries {
                    queue.push_back(d);
                }
            }
        }

        // `answers` is populated only from `Delivery::Answer` actually
        // reaching the requester below — not from `outcome.own_match`
        // observed in passing here — so results reflect what the relay
        // chain really delivered, the same signal a real requester would
        // have (`RelayOutcome::own_match` is exposed for direct
        // `Node`-level testing, not as a simulation-only shortcut here).
        while let Some(delivery) = queue.pop_front() {
            match delivery {
                Delivery::Query { to, event } => {
                    stats.query_messages += 1;
                    let from = event.pubkey.clone();
                    if let Some(node) = self.nodes.get_mut(&to) {
                        let outcome = node.receive_query(&from, &event, now);
                        for d in outcome.deliveries {
                            queue.push_back(d);
                        }
                    }
                }
                Delivery::Answer { to, event } => {
                    stats.answer_messages += 1;
                    if to == requester_pubkey {
                        if let Some(final_answer) = decode_final_answer(&event) {
                            self.absorb(Some(final_answer), &mut answers);
                        }
                    } else if let Some(node) = self.nodes.get_mut(&to) {
                        if let Some(next) = node.receive_answer(&event) {
                            queue.push_back(next);
                        }
                    }
                }
            }
        }

        QueryResult {
            answers: answers.into_values().collect(),
            stats,
        }
    }

    fn absorb(&self, candidate: Option<FinalAnswer>, answers: &mut HashMap<String, FinalAnswer>) {
        if let Some(a) = candidate {
            answers
                .entry(a.responder_pubkey.clone())
                .and_modify(|existing| {
                    if a.hops < existing.hops {
                        *existing = a.clone();
                    }
                })
                .or_insert(a);
        }
    }
}

impl Default for Network {
    fn default() -> Self {
        Self::new()
    }
}

fn decode_final_answer(event: &Event) -> Option<FinalAnswer> {
    if event.verify().is_err() {
        return None;
    }
    // `event.pubkey` is whoever signed this last relay leg (hop 1), not
    // the original responder — `content.responder_pubkey` is fixed at
    // the point of matching and carried unchanged through every hop.
    let content: SkillAnswerContent = serde_json::from_str(&event.content).ok()?;
    Some(FinalAnswer {
        responder_pubkey: content.responder_pubkey,
        matched_skill_tag: content.matched_skill_tag,
        hops: content.hops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contact::{Contact, ContactPolicy};
    use qw_protocol::identity::Identity;

    fn node_with_tags(tags: &[&str]) -> Node {
        let mut n = Node::new(Identity::generate());
        n.set_own_skill_tags(tags.iter().map(|s| s.to_string()).collect());
        n
    }

    fn connect(network: &mut Network, a: &str, b: &str, tag_hint: &[&str]) {
        let tags: Vec<String> = tag_hint.iter().map(|s| s.to_string()).collect();
        let policy = ContactPolicy::open();
        network
            .nodes
            .get_mut(a)
            .unwrap()
            .add_contact(Contact::new(b, policy.clone()).with_cached_tags(tags.clone()));
        network
            .nodes
            .get_mut(b)
            .unwrap()
            .add_contact(Contact::new(a, policy).with_cached_tags(tags));
    }

    #[test]
    fn propagation_across_three_nodes_reaches_a_distant_match() {
        let mut network = Network::new();
        let requester = node_with_tags(&[]);
        let hop1 = node_with_tags(&[]);
        let hop2 = node_with_tags(&["it/backend/languages#rust"]);
        let (rp, h1p, h2p) = (requester.pubkey(), hop1.pubkey(), hop2.pubkey());
        network.add_node(requester);
        network.add_node(hop1);
        network.add_node(hop2);

        connect(&mut network, &rp, &h1p, &["it/backend/languages#rust"]);
        connect(&mut network, &h1p, &h2p, &["it/backend/languages#rust"]);

        let result = network.originate_query(&rp, "it/backend/languages#rust", 3, 0);
        assert_eq!(result.answers.len(), 1);
        assert_eq!(result.answers[0].responder_pubkey, h2p);
        // `hops` counts from hop 1, not from the requester (NIP-QW06):
        // hop2 is 1 edge past hop1's own chain-head forward.
        assert_eq!(result.answers[0].hops, 1);
    }

    #[test]
    fn no_match_within_max_hops_yields_no_answers() {
        let mut network = Network::new();
        let requester = node_with_tags(&[]);
        let hop1 = node_with_tags(&[]);
        let (rp, h1p) = (requester.pubkey(), hop1.pubkey());
        network.add_node(requester);
        network.add_node(hop1);
        connect(&mut network, &rp, &h1p, &["it/backend/languages#rust"]);

        let result = network.originate_query(&rp, "it/backend/languages#rust", 3, 0);
        assert!(result.answers.is_empty());
    }

    #[test]
    fn greedy_routing_sends_far_fewer_messages_than_full_flood() {
        // A hub connected to many contacts across unrelated domains, plus
        // one real path to a rust match several hops out.
        let mut network = Network::new();
        let requester = node_with_tags(&[]);
        let rp = requester.pubkey();
        network.add_node(requester);

        let mut prev = rp.clone();
        for hop in 0..4 {
            let is_last = hop == 3;
            let n = node_with_tags(if is_last {
                &["it/backend/languages#rust"]
            } else {
                &[]
            });
            let np = n.pubkey();
            network.add_node(n);
            connect(&mut network, &prev, &np, &["it/backend/languages#rust"]);

            // Pad this hop with unrelated contacts a flood would also hit.
            for _ in 0..10 {
                let noise = node_with_tags(&["it/mobile#swift"]);
                let noise_p = noise.pubkey();
                network.add_node(noise);
                connect(&mut network, &np, &noise_p, &["it/mobile#swift"]);
            }
            prev = np;
        }

        let result = network.originate_query(&rp, "it/backend/languages#rust", 4, 0);
        assert_eq!(result.answers.len(), 1, "the match must still be found");
        // A full flood at fanout ~11 per hop over 4 hops would be
        // thousands of messages; greedy routing should stay tiny.
        assert!(
            result.stats.query_messages < 20,
            "expected greedy routing to stay well under flood volume, got {}",
            result.stats.query_messages
        );
    }
}
