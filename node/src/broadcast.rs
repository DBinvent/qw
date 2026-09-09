//! Broadcast propagation between nodes (NIP-QW14): the echomail-style push
//! of a `proposal` / `demand` / `profile` / `news` / `review` outward hop
//! by hop, and the signed, days-valid per-hop reputation reading
//! (`HopRating`) that rides with it.
//!
//! This is node-to-node. A coordination server (`qw-server`) may hold and
//! re-serve envelopes for offline nodes, but the relay *decision* — score
//! the originator, gate on a per-type policy, fan out — is a node's, made
//! against the node's own trust of every hop on the path.
//!
//! Pure decision (`evaluate_relay`, `fold_chain`) + the per-type policy
//! table + the hop-rating cache live here; [`crate::node::Node`] wires
//! them to its contact book and trust view.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use qw_protocol::events::kinds::{BroadcastKind, HopRating, HopScore};
use qw_protocol::events::Event;

/// Multiplier per extra hop — NIP-QW13 `path.hop_decay` default.
pub const HOP_DECAY: f64 = 0.5;

const DAY: u64 = 86_400;

// --- per-type policy (NIP-QW14 §4) ---------------------------------------

/// Local, unpublished — the counterpart of [`crate::contact::ContactPolicy`]
/// for push traffic. One per [`BroadcastKind`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PropagationPolicy {
    pub max_hops: u8,
    pub max_age_secs: u64,
    pub max_bytes: u32,
    pub fanout: u8,
    pub min_hop_score: f64,
    pub hop_rating_ttl_secs: u64,
    pub rate_per_day: u32,
}

/// The stock defaults — news wide and cheap, a proposal tight, a review
/// slow and demanding (NIP-QW14 §4 table).
pub fn default_policy(kind: BroadcastKind) -> PropagationPolicy {
    let p = |max_hops, age_d: u64, kib: u32, fanout, min_hop_score, ttl_d: u64, rate| {
        PropagationPolicy {
            max_hops,
            max_age_secs: age_d * DAY,
            max_bytes: kib * 1024,
            fanout,
            min_hop_score,
            hop_rating_ttl_secs: ttl_d * DAY,
            rate_per_day: rate,
        }
    };
    match kind {
        BroadcastKind::Proposal => p(3, 7, 8, 3, 1.0, 3, 50),
        BroadcastKind::Demand => p(4, 14, 4, 3, 0.9, 3, 20),
        BroadcastKind::Profile => p(3, 30, 16, 2, 1.0, 7, 5),
        BroadcastKind::News => p(6, 30, 32, 4, 0.8, 7, 10),
        BroadcastKind::Review => p(4, 90, 8, 3, 1.1, 3, 20),
    }
}

/// The node's editable per-type policy — stock defaults until a knob is
/// overridden. Persisted the same way [`crate::contact::ContactPolicy`]
/// and NIP-QW13's `ReputationConfig` are (QW's client: the sealed
/// `SyncState`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PropagationConfig {
    overrides: HashMap<u8, PropagationPolicy>,
}

fn kind_key(kind: BroadcastKind) -> u8 {
    match kind {
        BroadcastKind::Proposal => 0,
        BroadcastKind::Demand => 1,
        BroadcastKind::Profile => 2,
        BroadcastKind::News => 3,
        BroadcastKind::Review => 4,
    }
}

impl PropagationConfig {
    pub fn for_kind(&self, kind: BroadcastKind) -> PropagationPolicy {
        self.overrides
            .get(&kind_key(kind))
            .copied()
            .unwrap_or_else(|| default_policy(kind))
    }

    pub fn set(&mut self, kind: BroadcastKind, policy: PropagationPolicy) {
        self.overrides.insert(kind_key(kind), policy);
    }

    /// The full per-type table, resolved (defaults where nothing is
    /// overridden) — what a config UI reads and writes.
    pub fn to_wire(&self) -> PropagationConfigWire {
        PropagationConfigWire {
            proposal: self.for_kind(BroadcastKind::Proposal),
            demand: self.for_kind(BroadcastKind::Demand),
            profile: self.for_kind(BroadcastKind::Profile),
            news: self.for_kind(BroadcastKind::News),
            review: self.for_kind(BroadcastKind::Review),
        }
    }

    /// Store every type from the wire form. A field equal to
    /// [`default_policy`] is dropped, so a config that was never touched
    /// serializes back to nothing.
    pub fn from_wire(wire: &PropagationConfigWire) -> Self {
        let mut cfg = Self::default();
        for (kind, policy) in [
            (BroadcastKind::Proposal, wire.proposal),
            (BroadcastKind::Demand, wire.demand),
            (BroadcastKind::Profile, wire.profile),
            (BroadcastKind::News, wire.news),
            (BroadcastKind::Review, wire.review),
        ] {
            if policy != default_policy(kind) {
                cfg.set(kind, policy);
            }
        }
        cfg
    }
}

/// The full per-message-type table — every type present, so a UI never has
/// to know which defaults are in force. `PropagationConfig` serializes as
/// this and deserializes from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropagationConfigWire {
    pub proposal: PropagationPolicy,
    pub demand: PropagationPolicy,
    pub profile: PropagationPolicy,
    pub news: PropagationPolicy,
    pub review: PropagationPolicy,
}

impl Default for PropagationConfigWire {
    fn default() -> Self {
        PropagationConfig::default().to_wire()
    }
}

impl Serialize for PropagationConfig {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_wire().serialize(s)
    }
}

impl<'de> Deserialize<'de> for PropagationConfig {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        PropagationConfigWire::deserialize(d).map(|w| PropagationConfig::from_wire(&w))
    }
}

// --- the hop-rating cache ----------------------------------------------

/// A node's own signed [`HopRating`] events, one per `(subject, domain)`,
/// kept until each expires. Re-attached to every broadcast the node
/// relays from that originator — the whole reason to sign once and cache.
#[derive(Debug, Default)]
pub struct HopRatingCache {
    by_key: HashMap<(String, String), (Event, u64)>,
}

impl HopRatingCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The still-valid rating for `(subject, domain)`, if one is cached.
    pub fn valid(&self, subject: &str, domain: &str, now: u64) -> Option<&Event> {
        self.by_key
            .get(&(subject.to_string(), domain.to_string()))
            .filter(|(_, valid_until)| now < *valid_until)
            .map(|(event, _)| event)
    }

    pub fn store(&mut self, subject: &str, domain: &str, event: Event, valid_until: u64) {
        self.by_key
            .insert((subject.to_string(), domain.to_string()), (event, valid_until));
    }

    /// Drop expired entries — a caller may run this on a timer.
    pub fn prune(&mut self, now: u64) {
        self.by_key.retain(|_, (_, valid_until)| now < *valid_until);
    }
}

// --- the relay decision (NIP-QW14 §5) ---------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum RelayAdvice {
    /// Forward to up to `fanout` onward peers.
    Relay { fanout: u8 },
    /// Keep and surface locally, but do not forward.
    Hold { reason: &'static str },
    /// Do not even store.
    Drop { reason: &'static str },
}

/// Fold the hop ratings along `path` into one score, NIP-QW13 §4 style:
/// start from the reading nearest the originator (hop 1's), then for each
/// relayer closer to us multiply by `min(1.0, our trust of that relayer) *
/// HOP_DECAY` — a voucher we rate poorly can only lower the result. An
/// unknown relayer, or an `unknown-risk` base, collapses the whole thing.
///
/// `path` is origin-first and **includes this node last**: `path[0]` is the
/// originator, `path[1..]` the relayers in order ending with us. `ratings`
/// maps a signer's pubkey to their (domain-matched, still-valid) rating of
/// the originator. `our_trust` returns this node's score of a pubkey, or
/// `None` for unknown-risk. `self_pubkey` is skipped in the fold — a node
/// does not weigh its own rating by its trust of itself.
pub fn fold_chain(
    path: &[String],
    ratings: &HashMap<String, HopRating>,
    our_trust: &dyn Fn(&str) -> Option<f64>,
    self_pubkey: &str,
) -> Option<HopScore> {
    let relayers = path.get(1..).unwrap_or(&[]);
    if relayers.is_empty() {
        return None;
    }
    let base_idx = relayers.iter().position(|r| ratings.contains_key(r))?;
    let base = &ratings[&relayers[base_idx]];
    let mut adjusted = match base.score {
        HopScore::UnknownRisk => return Some(HopScore::UnknownRisk),
        HopScore::Score(s) => s,
    };
    for relayer in &relayers[base_idx + 1..] {
        if relayer == self_pubkey {
            continue;
        }
        match our_trust(relayer) {
            None => return Some(HopScore::UnknownRisk),
            Some(t) => adjusted *= t.min(1.0) * HOP_DECAY,
        }
    }
    Some(HopScore::Score(adjusted))
}

/// The §5 gate. `path` is origin-first and includes this node last (the
/// path as it would be if this node forwards); `envelope_bytes` is the
/// envelope event's `content` length.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_relay(
    created_at: u64,
    expires_at: u64,
    envelope_bytes: usize,
    path: &[String],
    ratings: &HashMap<String, HopRating>,
    our_trust: &dyn Fn(&str) -> Option<f64>,
    self_pubkey: &str,
    policy: &PropagationPolicy,
    now: u64,
) -> RelayAdvice {
    if now >= expires_at {
        return RelayAdvice::Drop { reason: "past expires_at" };
    }
    if now.saturating_sub(created_at) > policy.max_age_secs {
        return RelayAdvice::Drop { reason: "older than max_age for this type" };
    }
    if envelope_bytes as u64 > policy.max_bytes as u64 {
        return RelayAdvice::Drop { reason: "over max_bytes for this type" };
    }
    let relayer_count = path.len().saturating_sub(1);
    if relayer_count >= policy.max_hops as usize {
        return RelayAdvice::Hold { reason: "at hop limit for this type" };
    }
    match fold_chain(path, ratings, our_trust, self_pubkey) {
        None => RelayAdvice::Hold { reason: "no valid hop rating yet" },
        Some(HopScore::UnknownRisk) => RelayAdvice::Hold { reason: "unknown-risk on the path" },
        Some(HopScore::Score(s)) if s < policy.min_hop_score => {
            RelayAdvice::Hold { reason: "below the score floor for this type" }
        }
        Some(HopScore::Score(_)) => RelayAdvice::Relay { fanout: policy.fanout },
    }
}

// --- what a Node hands back ------------------------------------------

/// One onward send. `envelope` is the immutable kind-9100, `ratings` the
/// kind-9101 events gathered along the path plus this node's own, `path`
/// the ordered pubkeys (origin-first) for the next hop's loop-avoidance
/// and hop count.
#[derive(Debug, Clone, PartialEq)]
pub struct BroadcastDelivery {
    pub to: String,
    pub envelope: Event,
    pub ratings: Vec<Event>,
    pub path: Vec<String>,
}

#[derive(Debug, Default)]
pub struct BroadcastOutcome {
    pub deliveries: Vec<BroadcastDelivery>,
    /// The envelope this node now holds and surfaces locally (whether or
    /// not it also forwards).
    pub held: Option<Event>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rating(subject: &str, domain: &str, score: HopScore) -> HopRating {
        HopRating {
            subject_pubkey: subject.to_string(),
            domain: domain.to_string(),
            score,
            computed_at: 0,
            valid_until: 1_000_000,
        }
    }

    #[test]
    fn default_policy_matches_the_nip_table() {
        assert_eq!(default_policy(BroadcastKind::News).max_hops, 6);
        assert_eq!(default_policy(BroadcastKind::News).min_hop_score, 0.8);
        assert_eq!(default_policy(BroadcastKind::Review).min_hop_score, 1.1);
        assert_eq!(
            default_policy(BroadcastKind::Profile).hop_rating_ttl_secs,
            7 * DAY
        );
    }

    #[test]
    fn config_overrides_one_kind_and_leaves_the_rest_at_stock() {
        let mut cfg = PropagationConfig::default();
        let mut tight = default_policy(BroadcastKind::News);
        tight.max_hops = 2;
        cfg.set(BroadcastKind::News, tight);
        assert_eq!(cfg.for_kind(BroadcastKind::News).max_hops, 2);
        assert_eq!(cfg.for_kind(BroadcastKind::Proposal).max_hops, 3);
    }

    #[test]
    fn config_round_trips_through_its_wire_form() {
        let mut cfg = PropagationConfig::default();
        let mut tight = default_policy(BroadcastKind::News);
        tight.max_hops = 2;
        tight.min_hop_score = 1.3;
        cfg.set(BroadcastKind::News, tight);

        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"news\""), "wire form is the per-type table");
        let back: PropagationConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.for_kind(BroadcastKind::News).max_hops, 2);
        assert_eq!(back.for_kind(BroadcastKind::News).min_hop_score, 1.3);
        assert_eq!(
            back.for_kind(BroadcastKind::Proposal),
            default_policy(BroadcastKind::Proposal)
        );
        // an untouched config serializes to the full default table and back
        let d = PropagationConfig::default();
        let d2: PropagationConfig =
            serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert!(d2.overrides.is_empty());
    }

    #[test]
    fn cache_returns_a_rating_until_it_expires() {
        let mut cache = HopRatingCache::new();
        // a placeholder event — the cache does not inspect it
        let ev = Event {
            id: "x".into(),
            pubkey: "hop".into(),
            created_at: 0,
            kind: 9101,
            tags: vec![],
            content: "{}".into(),
            sig: "0".into(),
        };
        cache.store("orig", "it/backend", ev, 500);
        assert!(cache.valid("orig", "it/backend", 499).is_some());
        assert!(cache.valid("orig", "it/backend", 500).is_none());
        assert!(cache.valid("orig", "it/frontend", 100).is_none());
    }

    #[test]
    fn fold_takes_the_base_then_decays_by_relayers_but_not_self() {
        // origin → hop1 (rated) → hop2 → me
        let path = vec!["origin".into(), "hop1".into(), "hop2".into(), "me".into()];
        let mut ratings = HashMap::new();
        ratings.insert("hop1".to_string(), rating("origin", "it/backend", HopScore::Score(1.4)));

        // hop2 is a stranger to us → unknown-risk collapses the fold
        let unknown = |_: &str| None;
        assert_eq!(
            fold_chain(&path, &ratings, &unknown, "me"),
            Some(HopScore::UnknownRisk)
        );

        // hop2 we trust at 0.8 → 1.4 * 0.8 * 0.5; "me" is skipped
        let trust_08 = |p: &str| if p == "hop2" { Some(0.8) } else { None };
        match fold_chain(&path, &ratings, &trust_08, "me").unwrap() {
            HopScore::Score(s) => assert!((s - 1.4 * 0.8 * 0.5).abs() < 1e-9),
            _ => panic!("expected a score"),
        }
    }

    #[test]
    fn fold_is_none_without_any_relayer_rating() {
        let path = vec!["origin".into(), "hop1".into(), "me".into()];
        assert_eq!(fold_chain(&path, &HashMap::new(), &|_| None, "me"), None);
    }

    #[test]
    fn evaluate_relay_covers_the_gate() {
        let pol = default_policy(BroadcastKind::Proposal); // floor 1.0, max_hops 3
        // origin → hop1 (rated) → me
        let path = vec!["o".to_string(), "h1".to_string(), "me".to_string()];
        let mut ratings = HashMap::new();
        ratings.insert("h1".to_string(), rating("o", "it/backend", HopScore::Score(1.5)));
        let trust = |_: &str| Some(1.0);
        let g = |created, expires, bytes, path: &[String], r, now| {
            evaluate_relay(created, expires, bytes, path, r, &trust, "me", &pol, now)
        };

        assert_eq!(
            g(0, 10, 100, &path, &ratings, 20),
            RelayAdvice::Drop { reason: "past expires_at" }
        );
        assert_eq!(
            g(0, 10_000, 9_000, &path, &ratings, 5),
            RelayAdvice::Drop { reason: "over max_bytes for this type" }
        );
        assert_eq!(
            g(0, 10_000_000, 100, &path, &ratings, 8 * DAY),
            RelayAdvice::Drop { reason: "older than max_age for this type" }
        );
        // 3 relayers incl. me, max_hops 3 → at limit
        let deep = vec!["o".into(), "a".into(), "b".into(), "me".into()];
        assert_eq!(
            g(0, 10_000_000, 100, &deep, &ratings, 100),
            RelayAdvice::Hold { reason: "at hop limit for this type" }
        );
        // below floor: base 0.9 < 1.0
        let mut low = HashMap::new();
        low.insert("h1".to_string(), rating("o", "it/backend", HopScore::Score(0.9)));
        assert_eq!(
            g(0, 10_000_000, 100, &path, &low, 100),
            RelayAdvice::Hold { reason: "below the score floor for this type" }
        );
        // clears everything
        assert_eq!(
            g(0, 10_000_000, 100, &path, &ratings, 100),
            RelayAdvice::Relay { fanout: 3 }
        );
        // no rating yet
        assert_eq!(
            g(0, 10_000_000, 100, &path, &HashMap::new(), 100),
            RelayAdvice::Hold { reason: "no valid hop rating yet" }
        );
    }
}
