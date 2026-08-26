//! Greedy/small-world routing (§3, NIP-QW06): forward a query only to
//! tag-similar contacts, never flood. Per `qw-design-faq.md` §6: "Since
//! each node already caches contacts' skill tags, relays forward
//! selectively toward tag-similar contacts... fanout 2-3 instead of 50."
//!
//! Similarity is measured against two lists, not one. A contact's declared
//! tags are what they published about themselves; their earned tags come
//! from contracts a counterparty countersigned
//! (`qw_protocol::trust::earned_skill_tags`). Matching declared tags alone
//! made the participant with the most evidence the hardest to find — ten
//! countersigned Rust contracts and no `rust` tag published meant
//! unreachable by a Rust query.
//!
//! Earned outranks declared at each precision level, because one is free to
//! assert and the other costs somebody else's signature. That is a *reach*
//! ordering only: which contact a query is forwarded to says nothing about
//! what the eventual match is worth, which stays a per-viewer computation
//! over completed work (§5).

pub use qw_protocol::events::same_domain;

use crate::contact::Contact;

/// Fanout cap — "fanout 2-3 instead of 50" (FAQ §6).
pub const GREEDY_FANOUT: usize = 3;

/// Why a contact was selected. Carried out of [`select_forward_targets_ranked`]
/// so a caller can tell a countersigned match from a self-declared one —
/// merging them into a single list would throw that away at exactly the
/// point it is worth knowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchSource {
    /// Tag attached to a contract the contact completed and a counterparty
    /// countersigned. Not inflatable.
    Earned,
    /// Tag from the contact's self-published profile. Free to assert.
    Declared,
    /// Nothing matched; selected only to keep a forward path open.
    Fallback,
}

/// Rank-then-cap contact selection for forwarding `skill_tag`.
///
/// Order, most precise and best-evidenced first:
///
/// 1. earned exact          4. declared same-domain
/// 2. declared exact        5. anything, as fallback
/// 3. earned same-domain
///
/// Proof before claim at each precision level, but precision first overall:
/// an exact declared match is a better forward guess than an earned match in
/// a merely adjacent domain, since the query still has to arrive somewhere
/// relevant before evidence matters.
///
/// The fallback tier is only added when fewer than [`GREEDY_FANOUT`]
/// contacts matched by tag at all — small-world routing still needs *some*
/// forward path to reach far-off matches, not just same-domain hops every
/// time.
pub fn select_forward_targets_ranked<'a>(
    contacts: impl Iterator<Item = &'a Contact>,
    skill_tag: &str,
) -> Vec<(&'a Contact, MatchSource)> {
    let has = |tags: &[String], want: &str| tags.iter().any(|t| t == want);
    let near = |tags: &[String], want: &str| tags.iter().any(|t| same_domain(t, want));

    let mut earned_exact = Vec::new();
    let mut declared_exact = Vec::new();
    let mut earned_domain = Vec::new();
    let mut declared_domain = Vec::new();
    let mut rest = Vec::new();

    for c in contacts {
        if has(&c.earned_skill_tags, skill_tag) {
            earned_exact.push((c, MatchSource::Earned));
        } else if has(&c.cached_skill_tags, skill_tag) {
            declared_exact.push((c, MatchSource::Declared));
        } else if near(&c.earned_skill_tags, skill_tag) {
            earned_domain.push((c, MatchSource::Earned));
        } else if near(&c.cached_skill_tags, skill_tag) {
            declared_domain.push((c, MatchSource::Declared));
        } else {
            rest.push((c, MatchSource::Fallback));
        }
    }

    let mut selected = earned_exact;
    selected.extend(declared_exact);
    selected.extend(earned_domain);
    selected.extend(declared_domain);
    if selected.len() < GREEDY_FANOUT {
        selected.extend(rest);
    }
    selected.truncate(GREEDY_FANOUT);
    selected
}

/// [`select_forward_targets_ranked`] without the provenance — the shape
/// callers that only need "where do I forward this" already use.
pub fn select_forward_targets<'a>(
    contacts: impl Iterator<Item = &'a Contact>,
    skill_tag: &str,
) -> Vec<&'a Contact> {
    select_forward_targets_ranked(contacts, skill_tag)
        .into_iter()
        .map(|(c, _)| c)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contact::ContactPolicy;

    fn contact(pubkey: &str, tags: &[&str]) -> Contact {
        Contact::new(pubkey, ContactPolicy::open())
            .with_cached_tags(tags.iter().map(|s| s.to_string()).collect())
    }

    /// A contact who has published no profile at all, only finished work.
    fn earned_only(pubkey: &str, tags: &[&str]) -> Contact {
        Contact::new(pubkey, ContactPolicy::open())
            .with_earned_tags(tags.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn greedy_selection_prefers_tag_similar_contacts_over_flood() {
        let backend_a = contact("backend-a", &["it/backend/languages#rust"]);
        let backend_b = contact("backend-b", &["it/backend/frameworks#axum"]);
        let frontend = contact("frontend", &["it/frontend#react"]);
        let unrelated = contact("unrelated", &["it/mobile#swift"]);
        let contacts = vec![&backend_a, &backend_b, &frontend, &unrelated];

        let selected = select_forward_targets(contacts.into_iter(), "it/backend/languages#rust");

        assert_eq!(
            selected.len(),
            GREEDY_FANOUT,
            "must not flood all 4 contacts"
        );
        let selected_pubkeys: Vec<&str> = selected.iter().map(|c| c.pubkey.as_str()).collect();
        assert!(
            selected_pubkeys.contains(&"backend-a"),
            "exact tag match must be selected"
        );
        assert!(
            selected_pubkeys.contains(&"backend-b"),
            "same-domain match must be selected"
        );
    }

    #[test]
    fn falls_back_to_any_contact_when_nobody_matches_by_tag() {
        let a = contact("a", &["it/mobile#swift"]);
        let b = contact("b", &["it/mobile#kotlin-android"]);
        let contacts = vec![&a, &b];

        let selected = select_forward_targets(contacts.into_iter(), "it/backend/languages#rust");
        assert_eq!(
            selected.len(),
            2,
            "with nobody tag-similar, still forward to reach somewhere"
        );
    }

    /// The case this ranking exists for: someone whose evidence is real and
    /// whose profile is empty used to be invisible to the query their whole
    /// history answers.
    #[test]
    fn a_contact_with_only_countersigned_work_is_reachable() {
        let proven = earned_only("proven", &["it/backend/languages#rust"]);
        let noise_a = contact("noise-a", &["it/mobile#swift"]);
        let noise_b = contact("noise-b", &["it/frontend#react"]);
        let noise_c = contact("noise-c", &["it/mobile#kotlin-android"]);
        let contacts = vec![&proven, &noise_a, &noise_b, &noise_c];

        let selected = select_forward_targets(contacts.into_iter(), "it/backend/languages#rust");

        assert!(
            selected.iter().any(|c| c.pubkey == "proven"),
            "a countersigned Rust history must make someone findable for Rust \
             even with no profile published"
        );
    }

    /// Both are exact matches; the tie-break is which one cost a
    /// counterparty's signature.
    #[test]
    fn earned_outranks_declared_at_the_same_precision() {
        let tag = "it/backend/languages#rust";
        let claimed = contact("claimed", &[tag]);
        let proven = earned_only("proven", &[tag]);
        // Fill the fanout so ordering is what decides, not capacity.
        let filler_a = contact("filler-a", &[tag]);
        let filler_b = contact("filler-b", &[tag]);
        let contacts = vec![&claimed, &filler_a, &filler_b, &proven];

        let ranked = select_forward_targets_ranked(contacts.into_iter(), tag);

        assert_eq!(ranked[0].0.pubkey, "proven");
        assert_eq!(ranked[0].1, MatchSource::Earned);
        assert!(
            ranked[1..]
                .iter()
                .all(|(_, src)| *src == MatchSource::Declared),
            "everyone else here only claimed the tag"
        );
    }

    /// Evidence does not beat relevance: a query has to arrive somewhere
    /// on-topic before it matters who proved what.
    #[test]
    fn precision_still_wins_over_provenance_across_domains() {
        let exact_claim = contact("exact-claim", &["it/backend/languages#rust"]);
        let earned_nearby = earned_only("earned-nearby", &["it/backend/frameworks#axum"]);
        let contacts = vec![&earned_nearby, &exact_claim];

        let ranked =
            select_forward_targets_ranked(contacts.into_iter(), "it/backend/languages#rust");

        assert_eq!(
            ranked[0].0.pubkey, "exact-claim",
            "an exact declared match is a better forward guess than an earned \
             match in a merely adjacent domain"
        );
    }

    /// Selection is about reach. Nothing here may be read as a verdict on
    /// the contact — a fallback pick is not a claim that they match.
    #[test]
    fn fallback_picks_are_labelled_as_such() {
        let a = earned_only("a", &["it/mobile#swift"]);
        let b = contact("b", &["it/mobile#kotlin-android"]);
        let contacts = vec![&a, &b];

        let ranked =
            select_forward_targets_ranked(contacts.into_iter(), "it/backend/languages#rust");

        assert_eq!(ranked.len(), 2, "still forward somewhere");
        assert!(ranked.iter().all(|(_, src)| *src == MatchSource::Fallback));
    }
}
