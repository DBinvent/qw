//! Synthetic small-world contact graph for the §3 demo target: "a query
//! for a skill tag reaches a match in ~log(N) hops across a simulated
//! contact graph of a few hundred synthetic nodes."
//!
//! Real professional contact graphs are small-world: locally clustered
//! (your contacts know each other) with a few long-range links that
//! collapse the average path length. A plain Watts-Strogatz ring lattice
//! plus random shortcuts reproduces both properties without needing a
//! real social graph dataset. Nodes are assigned a dominant skill-tag
//! domain in contiguous ring blocks — modeling the (also realistic)
//! tendency for someone's contacts to cluster by field, which is what
//! greedy tag-similarity routing actually needs to outperform blind
//! flooding.

use rand::Rng;

use qw_protocol::identity::Identity;

use crate::contact::{Contact, ContactPolicy};
use crate::network::Network;
use crate::node::Node;

/// A handful of `/taxonomy.yaml` domains, enough to exercise greedy
/// routing's tag-similarity behavior without pulling in a YAML parser
/// just for a synthetic demo graph.
pub const DEMO_DOMAINS: &[&str] = &[
    "it/backend/languages#rust",
    "it/frontend#react",
    "it/mobile#swift",
    "it/data#sql",
    "it/devops#kubernetes",
];

/// Build a small-world graph of `n` nodes: a ring lattice (`ring_degree`
/// neighbors on each side) plus `shortcuts_per_node` random long-range
/// edges per node. Returns the network and each node's pubkey in ring
/// order (index `i`'s dominant domain is `DEMO_DOMAINS[i * len /
/// n]`-ish — contiguous blocks around the ring).
pub fn build_small_world(
    n: usize,
    ring_degree: usize,
    shortcuts_per_node: usize,
    rng: &mut impl Rng,
) -> (Network, Vec<String>) {
    assert!(
        n > 2 * ring_degree,
        "ring_degree must be small relative to n to avoid self-loops/duplicate wraparound"
    );

    let mut network = Network::new();
    let mut pubkeys = Vec::with_capacity(n);
    let block_size = n.div_ceil(DEMO_DOMAINS.len()).max(1);

    for i in 0..n {
        let domain_tag = DEMO_DOMAINS[(i / block_size).min(DEMO_DOMAINS.len() - 1)];
        let mut node = Node::new(Identity::generate());
        node.set_own_skill_tags(vec![domain_tag.to_string()]);
        pubkeys.push((node.pubkey(), domain_tag.to_string()));
        network.add_node(node);
    }

    for i in 0..n {
        for d in 1..=ring_degree {
            connect(&mut network, &pubkeys, i, (i + d) % n);
        }
    }
    for i in 0..n {
        for _ in 0..shortcuts_per_node {
            let j = rng.random_range(0..n);
            if j != i {
                connect(&mut network, &pubkeys, i, j);
            }
        }
    }

    (network, pubkeys.into_iter().map(|(pk, _)| pk).collect())
}

fn connect(network: &mut Network, pubkeys: &[(String, String)], i: usize, j: usize) {
    if i == j {
        return;
    }
    let (pi, ti) = pubkeys[i].clone();
    let (pj, tj) = pubkeys[j].clone();
    if let Some(node_i) = network.node_mut(&pi) {
        node_i.add_contact(
            Contact::new(pj.clone(), ContactPolicy::open()).with_cached_tags(vec![tj]),
        );
    }
    if let Some(node_j) = network.node_mut(&pj) {
        node_j.add_contact(Contact::new(pi, ContactPolicy::open()).with_cached_tags(vec![ti]));
    }
}

#[cfg(test)]
mod tests {
    use rand::{rngs::StdRng, SeedableRng};

    use super::*;

    // Fixed seeds: this simulation isn't tuned for a formal statistical
    // guarantee (see the demo example's own caveat), so pin the RNG for
    // reproducible, non-flaky assertions rather than asserting over a
    // random draw every run.

    #[test]
    fn builds_the_requested_node_count_with_edges() {
        let mut rng = StdRng::seed_from_u64(1);
        let (network, pubkeys) = build_small_world(200, 2, 2, &mut rng);
        assert_eq!(network.len(), 200);
        assert_eq!(pubkeys.len(), 200);
        let sample = network.node(&pubkeys[0]).unwrap();
        assert!(
            sample.contacts().count() >= 4,
            "ring alone should give at least 2*ring_degree contacts"
        );
    }

    #[test]
    fn a_query_reaches_a_cross_domain_match_via_shortcuts() {
        let mut rng = StdRng::seed_from_u64(7);
        let (mut network, pubkeys) = build_small_world(300, 2, 2, &mut rng);

        // Node in the middle of one domain's block, searching for a
        // domain whose block is on the opposite side of the ring — only
        // reachable via the random long-range shortcuts, not local ring
        // adjacency, which is what actually exercises small-world
        // reachability rather than trivial same-block neighbors.
        let requester = &pubkeys[150];
        let target_domain = DEMO_DOMAINS[0];

        let result = network.originate_query(requester, target_domain, 8, 0);
        assert!(
            !result.answers.is_empty(),
            "a 300-node small-world graph should bridge to a distant domain within 8 hops"
        );
    }
}
