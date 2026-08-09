//! Per-contact relay policy (§3, NIP-QW06 "Per-contact policy"). Entirely
//! local state — never published, never even shared with the contact it
//! describes.

use crate::routing::same_domain;

/// One reading of `qw-design-faq.md` §6's permission table — see
/// NIP-QW06 for the full rationale on each field's direction (inbound
/// `accept_depth` vs. outbound `relay_depth`).
#[derive(Debug, Clone, PartialEq)]
pub struct ContactPolicy {
    /// Cap on `hops_from_origin` (after I'd increment it) for queries I
    /// extend to this contact — how far downstream I'll personally push
    /// things along this edge, regardless of the query's own `max_hops`.
    pub relay_depth: u8,
    /// Cap on `hops_from_origin` as received — I won't accept a query
    /// from this contact if it already traveled further than this.
    pub accept_depth: u8,
    /// Tag-domain allowlist (`sector/domain` prefixes). Empty = unrestricted.
    pub categories: Vec<String>,
    /// Queries per day accepted from this contact.
    pub rate_limit: u32,
    /// May this contact's `cached_skill_tags` entry (what greedy routing
    /// matches against) be populated at all.
    pub share_tags: bool,
}

impl ContactPolicy {
    /// No restriction beyond the protocol's own `max_hops`/fanout — a
    /// reasonable default for a close, trusted contact.
    pub fn open() -> Self {
        Self {
            relay_depth: 3,
            accept_depth: 3,
            categories: Vec::new(),
            rate_limit: 100,
            share_tags: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Contact {
    pub pubkey: String,
    pub policy: ContactPolicy,
    /// Skill tags this contact is locally known to have — what greedy
    /// routing (`crate::routing::select_forward_targets`) matches
    /// against. Populated out of band (a prior tag-exchange/gossip step,
    /// gated by `policy.share_tags` on *their* side) — not by this NIP.
    pub cached_skill_tags: Vec<String>,
    rate_window_day: Option<u64>,
    rate_window_count: u32,
}

impl Contact {
    pub fn new(pubkey: impl Into<String>, policy: ContactPolicy) -> Self {
        Self {
            pubkey: pubkey.into(),
            policy,
            cached_skill_tags: Vec::new(),
            rate_window_day: None,
            rate_window_count: 0,
        }
    }

    pub fn with_cached_tags(mut self, tags: Vec<String>) -> Self {
        self.cached_skill_tags = tags;
        self
    }

    fn category_allows(&self, skill_tag: &str) -> bool {
        self.policy.categories.is_empty()
            || self
                .policy
                .categories
                .iter()
                .any(|c| same_domain(c, skill_tag))
    }

    /// Non-mutating eligibility check for an inbound query from this
    /// contact: depth and category only. Call [`Contact::record_incoming`]
    /// afterward (and only if this passes) to also apply the rate limit.
    pub fn accepts_incoming(&self, hops_from_origin: u8, skill_tag: &str) -> bool {
        hops_from_origin <= self.policy.accept_depth && self.category_allows(skill_tag)
    }

    /// Record one incoming query from this contact at `now` (unix
    /// seconds). Returns `false` (and does not count it) if this would
    /// exceed `rate_limit` for the day — call only once accept checks
    /// already passed, so a rejected query doesn't consume the budget.
    pub fn record_incoming(&mut self, now: u64) -> bool {
        let day = now / 86_400;
        if self.rate_window_day != Some(day) {
            self.rate_window_day = Some(day);
            self.rate_window_count = 0;
        }
        if self.rate_window_count >= self.policy.rate_limit {
            return false;
        }
        self.rate_window_count += 1;
        true
    }

    /// Outbound cap: would extending a query to `outgoing_hops_from_origin`
    /// still be within what I'm willing to relay along this contact edge?
    pub fn allows_relay_at(&self, outgoing_hops_from_origin: u8) -> bool {
        outgoing_hops_from_origin <= self.policy.relay_depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_depth_rejects_queries_that_traveled_too_far() {
        let mut policy = ContactPolicy::open();
        policy.accept_depth = 1;
        let contact = Contact::new("abc", policy);
        assert!(contact.accepts_incoming(1, "it/backend#rust"));
        assert!(!contact.accepts_incoming(2, "it/backend#rust"));
    }

    #[test]
    fn categories_allowlist_restricts_domain() {
        let mut policy = ContactPolicy::open();
        policy.categories = vec!["it/backend".to_string()];
        let contact = Contact::new("abc", policy);
        assert!(contact.accepts_incoming(0, "it/backend/languages#rust"));
        assert!(!contact.accepts_incoming(0, "it/frontend/frameworks#react"));
    }

    #[test]
    fn rate_limit_resets_on_a_new_day_and_caps_within_one() {
        let mut policy = ContactPolicy::open();
        policy.rate_limit = 2;
        let mut contact = Contact::new("abc", policy);
        assert!(contact.record_incoming(0));
        assert!(contact.record_incoming(10));
        assert!(
            !contact.record_incoming(20),
            "third query same day exceeds rate_limit=2"
        );
        assert!(
            contact.record_incoming(90_000),
            "next day resets the window"
        );
    }

    #[test]
    fn relay_depth_caps_outbound_extension() {
        let mut policy = ContactPolicy::open();
        policy.relay_depth = 1;
        let contact = Contact::new("abc", policy);
        assert!(contact.allows_relay_at(1));
        assert!(!contact.allows_relay_at(2));
    }
}
