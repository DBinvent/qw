//! Personal ledger replication (NIP-QW12): anti-entropy between replicas
//! of **one** identity — a phone and one or more `qw-web` boxes — so they
//! converge without a coordinator.
//!
//! This is not the offline mailbox ([`crate::sync`]). The mailbox is
//! store-and-forward *between different identities* (one recipient,
//! delete-past-cursor, 30-day expiry). This is bidirectional, whole-set,
//! permanent anti-entropy *between replicas of the same person*. The two
//! share no code, and this module must not assume any mailbox semantics.
//!
//! Transport-agnostic on the same split [`crate::sync`] draws: the logic
//! that is easy to get subtly wrong — the coverage exchange, verify-on-
//! ingest, the union merge — lives here over a [`LedgerTransport`] trait;
//! HTTP replica-to-replica, a relay, or the mailbox as a fallback carrier
//! are all just implementations.
//!
//! **The model is a grow-only set.** Events are content-addressed and
//! self-verifying (`id = sha256([0, pubkey, created_at, kind, tags,
//! content])`), so "the same event" on two replicas is a bit-identical
//! record with the same id — nothing to reconcile field by field. Merge is
//! set union keyed by id: commutative, associative, idempotent. A replica
//! with a stale view cannot break a fold — signing a new event only adds
//! one element, and every view (`profile`, contract state, `net_position`)
//! re-runs over the union.
//!
//! **Coverage is the exact set of held ids.** A replica advertises every
//! event id it holds; a peer replies with every event whose id is not in
//! that set, and its own id set so the caller can push the reverse
//! difference in the same round. NIP-QW12 permits a compact per-author
//! high-water or a negentropy-style range digest instead — those are
//! *optimisations, not required*, and a high-water alone cannot express "I
//! have everything after T but am missing something before it", which is
//! exactly what two devices that both authored for one identity produce.
//! The exact id set is O(ledger) on the wire but always correct; a digest
//! is a later optimisation for a large ledger.
//!
//! Three rules, the same ones [`crate::sync`] follows:
//!
//! - **Everything pulled is re-verified locally.** A replica is untrusted
//!   infrastructure in exactly the sense §8 gives the mailbox: it may
//!   withhold or serve stale data; it cannot inject or forge. An event
//!   that fails `Event::verify` is dropped and counted, never merged.
//! - **Dedup is by id.** A same-second sibling authored on another of the
//!   identity's own devices has a different id, so it is simply "not in
//!   the set" and is pulled like anything else — the off-by-one
//!   `crate::sync`'s inclusive cursor guards against does not arise here.
//! - **Idempotent.** Re-offering an event a replica already holds is a
//!   no-op on both sides.

use std::collections::{BTreeSet, HashSet};

use qw_protocol::events::Event;
use serde::{Deserialize, Serialize};

/// A replica's statement of exactly which events it holds, by id. Sorted,
/// so the wire form is deterministic. A peer answers a pull with every
/// event whose id is absent here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LedgerCoverage(pub BTreeSet<String>);

impl LedgerCoverage {
    /// The coverage of a held event set.
    pub fn of(events: &[Event]) -> Self {
        Self(events.iter().map(|e| e.id.clone()).collect())
    }

    /// Whether a holder with this coverage lacks `event`.
    pub fn wants(&self, event: &Event) -> bool {
        !self.0.contains(&event.id)
    }
}

/// A peer's answer to a pull: the events it holds that the puller's
/// coverage did not include, plus the peer's own coverage so the puller
/// can push back the reverse difference in the same round. Serde-derived
/// because it *is* the `/ledger/pull` wire response
/// (`qw_client_core::HttpLedger`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PullResponse {
    pub events: Vec<Event>,
    pub coverage: LedgerCoverage,
}

/// The I/O half, supplied by whatever is actually running: HTTP
/// replica-to-replica in an app, a fake in tests.
pub trait LedgerTransport {
    type Error: std::fmt::Display;

    /// Advertise `have` to `peer` and get back what the peer holds that
    /// `have` does not include, together with the peer's own coverage.
    fn pull(&mut self, peer: &str, have: &LedgerCoverage) -> Result<PullResponse, Self::Error>;

    /// Hand `events` to `peer` to merge into its ledger. Returns how many
    /// were new to it (the rest it already held — a harmless no-op).
    fn push(&mut self, peer: &str, events: &[Event]) -> Result<usize, Self::Error>;
}

#[derive(Debug, Default, PartialEq)]
pub struct LedgerRound {
    /// New, verified events merged this round, oldest first — the caller
    /// folds these into its `HistoryStore`.
    pub received: Vec<Event>,
    /// Pulled events that failed `Event::verify`. Non-zero means a peer is
    /// serving corrupt data; surfaced rather than logged-and-forgotten so a
    /// caller can rank that peer down. A duplicate id is *not* counted.
    pub rejected: usize,
    /// Events peers accepted as new to them.
    pub pushed: usize,
    /// Per-peer failures, `(peer, message)`. One unreachable peer never
    /// stops the others.
    pub errors: Vec<(String, String)>,
}

/// Drives anti-entropy rounds. Holds only the set of ids merged so far, to
/// keep [`LedgerRound::received`] free of events the caller already has;
/// the ledger itself lives in the caller's `HistoryStore`, and every round
/// is primed from it, so a `Session` that ingested events by some other
/// path (a mailbox poll) is accounted for.
///
/// `seen` grows without bound, deliberately — the ledger it mirrors does
/// too (NIP-QW12: "permanent"; records are evidence others may ask for).
#[derive(Default)]
pub struct LedgerSync {
    seen: HashSet<String>,
}

impl LedgerSync {
    pub fn new() -> Self {
        Self::default()
    }

    /// One anti-entropy round against every peer, in order. `held` is the
    /// caller's current ledger — used to advertise coverage and to avoid
    /// re-merging what is already held. Returns the new events for the
    /// caller to append, and what each peer took.
    pub fn round<T: LedgerTransport>(
        &mut self,
        transport: &mut T,
        peers: &[&str],
        held: &[Event],
    ) -> LedgerRound {
        let mut report = LedgerRound::default();

        // Prime from the authoritative set each round: cheap insurance that
        // `received` never contains something the caller already holds.
        for e in held {
            self.seen.insert(e.id.clone());
        }
        let mine = LedgerCoverage::of(held);

        // Events learned earlier in *this* round, so a 3+-replica set can
        // converge in one round instead of one per hop: peer B is offered
        // what peer A just gave us.
        let mut fresh: Vec<Event> = Vec::new();

        for peer in peers {
            let PullResponse { events, coverage } = match transport.pull(peer, &mine) {
                Ok(resp) => resp,
                Err(e) => {
                    report.errors.push((peer.to_string(), e.to_string()));
                    continue;
                }
            };

            for event in events {
                if event.verify().is_err() {
                    report.rejected += 1;
                    continue;
                }
                if self.seen.insert(event.id.clone()) {
                    fresh.push(event);
                }
            }

            let outgoing: Vec<Event> = held
                .iter()
                .chain(fresh.iter())
                .filter(|e| coverage.wants(e))
                .cloned()
                .collect();
            if outgoing.is_empty() {
                continue;
            }
            match transport.push(peer, &outgoing) {
                Ok(n) => report.pushed += n,
                Err(e) => report.errors.push((peer.to_string(), e.to_string())),
            }
        }

        fresh.sort_by_key(|e| e.created_at);
        report.received = fresh;
        report
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use qw_protocol::events::{p_tag, UnsignedEvent, KIND_JOB_OFFER, KIND_PROFILE};
    use qw_protocol::identity::Identity;

    use super::*;

    /// An in-memory replica — stands in for the HTTP peer an app supplies.
    /// Holds a real event set and answers `pull` / `push` the way a
    /// conforming replica must.
    #[derive(Default)]
    struct FakeReplica {
        held: Vec<Event>,
        /// Return this error instead of answering.
        broken: Option<String>,
        /// Answer a pull with *everything*, ignoring the advertised
        /// coverage — what a lazy or hostile replica does. The rejection
        /// path is only real if the fake can actually misbehave.
        floods: bool,
    }

    impl FakeReplica {
        fn holding(events: Vec<Event>) -> Self {
            Self {
                held: events,
                ..Default::default()
            }
        }
        fn has(&self, id: &str) -> bool {
            self.held.iter().any(|e| e.id == id)
        }
    }

    /// A set of named peers behind one transport.
    #[derive(Default)]
    struct Peers {
        replicas: HashMap<String, FakeReplica>,
        pulls: Vec<(String, LedgerCoverage)>,
    }

    impl Peers {
        fn with(mut self, name: &str, replica: FakeReplica) -> Self {
            self.replicas.insert(name.to_string(), replica);
            self
        }
    }

    impl LedgerTransport for Peers {
        type Error = String;

        fn pull(&mut self, peer: &str, have: &LedgerCoverage) -> Result<PullResponse, String> {
            self.pulls.push((peer.to_string(), have.clone()));
            let r = self
                .replicas
                .get(peer)
                .ok_or_else(|| format!("no peer {peer}"))?;
            if let Some(err) = &r.broken {
                return Err(err.clone());
            }
            let events = r
                .held
                .iter()
                .filter(|e| r.floods || have.wants(e))
                .cloned()
                .collect();
            Ok(PullResponse {
                events,
                coverage: LedgerCoverage::of(&r.held),
            })
        }

        fn push(&mut self, peer: &str, events: &[Event]) -> Result<usize, String> {
            let r = self
                .replicas
                .get_mut(peer)
                .ok_or_else(|| format!("no peer {peer}"))?;
            if let Some(err) = &r.broken {
                return Err(err.clone());
            }
            let mut new = 0;
            for e in events {
                if !r.has(&e.id) {
                    r.held.push(e.clone());
                    new += 1;
                }
            }
            Ok(new)
        }
    }

    fn ev(author: &Identity, kind: u16, created_at: u64, body: &str) -> Event {
        UnsignedEvent {
            pubkey: author.nostr_pubkey_hex(),
            created_at,
            kind,
            tags: vec![],
            content: body.to_string(),
        }
        .sign(author)
    }

    fn offer(from: &Identity, to: &str, created_at: u64, body: &str) -> Event {
        UnsignedEvent {
            pubkey: from.nostr_pubkey_hex(),
            created_at,
            kind: KIND_JOB_OFFER,
            tags: vec![p_tag(to.to_string())],
            content: body.to_string(),
        }
        .sign(from)
    }

    #[test]
    fn coverage_is_the_exact_id_set() {
        let a = Identity::generate();
        let held = [
            ev(&a, KIND_PROFILE, 100, "a1"),
            ev(&a, KIND_PROFILE, 300, "a2"),
        ];
        let cov = LedgerCoverage::of(&held);
        assert!(!cov.wants(&held[0]) && !cov.wants(&held[1]));
        // an event below the high-water we nonetheless lack is still wanted —
        // the failure a per-author high-water summary would hide
        let missing_below_tip = ev(&a, KIND_PROFILE, 200, "a1.5");
        assert!(cov.wants(&missing_below_tip));
        assert!(cov.wants(&ev(&a, KIND_PROFILE, 400, "a3")));
    }

    #[test]
    fn two_replicas_converge_in_one_round_even_with_a_hole_below_the_tip() {
        // The real two-device case: each authored for the same identity at
        // a different time, so the phone's newest is *newer* than the web
        // box's, and neither is a prefix of the other.
        let me = Identity::generate();
        let web_evt = ev(&me, KIND_PROFILE, 100, "created on the web box");
        let phone_evt = ev(&me, KIND_PROFILE, 200, "edited on the phone");
        let mut here = vec![web_evt.clone()];

        let mut peers =
            Peers::default().with("phone", FakeReplica::holding(vec![phone_evt.clone()]));
        let mut sync = LedgerSync::new();
        let round = sync.round(&mut peers, &["phone"], &here);

        assert_eq!(round.received.len(), 1);
        assert_eq!(round.received[0].id, phone_evt.id);
        assert!(round.errors.is_empty());
        here.extend(round.received);

        // the phone got our older event despite already holding a newer one
        assert_eq!(round.pushed, 1);
        let phone = &peers.replicas["phone"];
        assert!(phone.has(&web_evt.id) && phone.has(&phone_evt.id));

        // a second round is quiet — both sides hold the same set
        let again = sync.round(&mut peers, &["phone"], &here);
        assert!(again.received.is_empty());
        assert_eq!(again.pushed, 0);
    }

    #[test]
    fn merge_is_union_not_addressed_to_anyone() {
        // A ledger holds events by *other* people too (a counterparty's
        // countersignature). Those replicate the same way — there is no
        // "addressed to me" filter, unlike the mailbox.
        let me = Identity::generate();
        let counterparty = Identity::generate();
        let theirs = offer(
            &counterparty,
            &me.nostr_pubkey_hex(),
            50,
            "their signed half",
        );

        let mut peers = Peers::default().with("web", FakeReplica::holding(vec![theirs.clone()]));
        let round = LedgerSync::new().round(&mut peers, &["web"], &[]);
        assert_eq!(round.received.len(), 1);
        assert_eq!(round.received[0].id, theirs.id);
    }

    #[test]
    fn a_forged_event_from_a_peer_is_dropped_and_counted() {
        let me = Identity::generate();
        let good = ev(&me, KIND_PROFILE, 100, "real");
        let mut forged = ev(&me, KIND_PROFILE, 100, "original");
        forged.content = "tampered after signing".to_string(); // id no longer matches

        let mut peers = Peers::default().with(
            "web",
            FakeReplica {
                held: vec![good.clone(), forged],
                floods: true,
                ..Default::default()
            },
        );
        let round = LedgerSync::new().round(&mut peers, &["web"], &[]);

        assert_eq!(round.received.len(), 1, "{:?}", round.received);
        assert_eq!(round.received[0].id, good.id);
        assert_eq!(round.rejected, 1);
    }

    #[test]
    fn a_same_second_sibling_from_another_device_is_not_lost() {
        let me = Identity::generate();
        let here = vec![ev(&me, KIND_PROFILE, 500, "a")];
        let sibling = ev(&me, KIND_PROFILE, 500, "b"); // same second, different id

        let mut peers = Peers::default().with(
            "phone",
            FakeReplica::holding(vec![here[0].clone(), sibling.clone()]),
        );
        let round = LedgerSync::new().round(&mut peers, &["phone"], &here);

        assert_eq!(
            round.received.len(),
            1,
            "the same-second sibling must arrive"
        );
        assert_eq!(round.received[0].id, sibling.id);
    }

    #[test]
    fn a_broken_peer_does_not_stop_the_others() {
        let me = Identity::generate();
        let from_b = ev(&me, KIND_PROFILE, 10, "via b");
        let mut peers = Peers::default()
            .with(
                "a",
                FakeReplica {
                    broken: Some("connection refused".into()),
                    ..Default::default()
                },
            )
            .with("b", FakeReplica::holding(vec![from_b.clone()]));

        let round = LedgerSync::new().round(&mut peers, &["a", "b"], &[]);
        assert_eq!(round.received.len(), 1);
        assert_eq!(round.received[0].id, from_b.id);
        assert_eq!(round.errors.len(), 1);
        assert_eq!(round.errors[0].0, "a");
    }

    #[test]
    fn the_same_event_from_two_peers_is_merged_once() {
        let me = Identity::generate();
        let shared = ev(&me, KIND_PROFILE, 42, "held by both peers");
        let mut peers = Peers::default()
            .with("a", FakeReplica::holding(vec![shared.clone()]))
            .with("b", FakeReplica::holding(vec![shared.clone()]));

        let round = LedgerSync::new().round(&mut peers, &["a", "b"], &[]);
        assert_eq!(round.received.len(), 1);
    }

    #[test]
    fn three_replicas_converge_in_one_round_via_the_middle() {
        let me = Identity::generate();
        let x = ev(&me, KIND_PROFILE, 100, "x");
        let mut peers = Peers::default()
            .with("a", FakeReplica::holding(vec![x.clone()]))
            .with("b", FakeReplica::default());

        let round = LedgerSync::new().round(&mut peers, &["a", "b"], &[]);
        assert_eq!(round.received.len(), 1);
        assert!(
            peers.replicas["b"].has(&x.id),
            "b got x in the same round, via here"
        );
    }

    #[test]
    fn nothing_to_push_skips_the_push_call() {
        let me = Identity::generate();
        let here = vec![ev(&me, KIND_PROFILE, 1, "only thing anyone holds")];
        let mut peers = Peers::default().with("web", FakeReplica::holding(here.clone()));

        let round = LedgerSync::new().round(&mut peers, &["web"], &here);
        assert!(round.received.is_empty());
        assert_eq!(round.pushed, 0);
        assert!(round.errors.is_empty());
    }

    #[test]
    fn the_pull_advertises_our_coverage() {
        let me = Identity::generate();
        let ours = ev(&me, KIND_PROFILE, 700, "ours");
        let here = vec![ours.clone()];
        let mut peers = Peers::default().with("web", FakeReplica::default());
        LedgerSync::new().round(&mut peers, &["web"], &here);

        let (peer, advertised) = &peers.pulls[0];
        assert_eq!(peer, "web");
        assert!(advertised.0.contains(&ours.id));
    }
}
