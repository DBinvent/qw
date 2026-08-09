//! §3 demo target: "a query for a skill tag reaches a match in ~log(N)
//! hops across a simulated contact graph of a few hundred synthetic
//! nodes," and the "CLI... to fire a query... and collect deduped-by-
//! pubkey responses with path count" bullet.
//!
//! Usage: `cargo run -p qw-node --example referral_demo -- [n] [skill_tag] [max_hops]`
//!
//! Caveat: this is a demo/prototype, not a statistical proof. The graph
//! generator (`qw_node::graph`) isn't tuned or run over many trials —
//! see NIP-QW06's scope note. What it does show: greedy routing finds a
//! match without flooding, using a small fraction of a full flood's
//! message count.

use std::time::{SystemTime, UNIX_EPOCH};

use qw_node::graph::{build_small_world, DEMO_DOMAINS};

fn main() {
    let mut args = std::env::args().skip(1);
    let n: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(300);
    let skill_tag = args.next().unwrap_or_else(|| DEMO_DOMAINS[0].to_string());
    let max_hops: u8 = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);

    const RING_DEGREE: usize = 2;
    // Watts-Strogatz only needs a small fraction of long-range edges to
    // collapse average path length — keep this low so a match typically
    // takes a few real hops to reach, rather than a direct shortcut
    // making hop 1 trivially likely.
    const SHORTCUTS_PER_NODE: usize = 1;

    let mut rng = rand::rng();
    let (mut network, pubkeys) = build_small_world(n, RING_DEGREE, SHORTCUTS_PER_NODE, &mut rng);
    // Default skill_tag is DEMO_DOMAINS[0], whose block sits at the start
    // of the ring — start the requester on the opposite side so a match
    // (if any) has to travel through shortcuts, not trivial same-block
    // neighbors. A caller-supplied skill_tag still fires from here; it's
    // just no longer guaranteed to be "the far side" of that particular tag.
    let requester = pubkeys
        .get(n / 2)
        .or_else(|| pubkeys.first())
        .expect("at least one node");

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before 1970")
        .as_secs();
    let result = network.originate_query(requester, &skill_tag, max_hops, now);

    println!("Simulated small-world contact graph: {n} nodes, ring degree {RING_DEGREE}, {SHORTCUTS_PER_NODE} shortcuts/node.");
    println!(
        "Query: skill_tag=\"{skill_tag}\", max_hops={max_hops}, fired from node {}",
        short(requester)
    );
    println!();

    if result.answers.is_empty() {
        println!("No match found within {max_hops} hops.");
    } else {
        let mut answers = result.answers;
        answers.sort_by_key(|a| a.hops);
        println!(
            "First match at {} hop(s); {} total matches found within {max_hops} hops, deduped by pubkey:",
            answers[0].hops,
            answers.len()
        );
        const SHOWN: usize = 8;
        for a in answers.iter().take(SHOWN) {
            println!(
                "  - {} — {} hops via the relay chain",
                short(&a.responder_pubkey),
                a.hops
            );
        }
        if answers.len() > SHOWN {
            println!("  ...and {} more", answers.len() - SHOWN);
        }
    }

    println!();
    println!(
        "Messages sent: {} queries, {} answers",
        result.stats.query_messages, result.stats.answer_messages
    );
    let avg_degree = 2 * RING_DEGREE + SHORTCUTS_PER_NODE;
    let naive_flood_estimate = (avg_degree as u64).saturating_pow(max_hops as u32);
    println!(
        "Naive full-flood estimate at avg degree ~{avg_degree} over {max_hops} hops: ~{naive_flood_estimate} messages \
         (FAQ §6: greedy routing should land near ~1% of flood traffic)"
    );
}

fn short(pubkey: &str) -> &str {
    &pubkey[..12.min(pubkey.len())]
}
