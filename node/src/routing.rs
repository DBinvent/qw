//! Greedy/small-world routing (§3, NIP-QW06): forward a query only to
//! tag-similar contacts, never flood. Per `qw-design-faq.md` §6: "Since
//! each node already caches contacts' skill tags, relays forward
//! selectively toward tag-similar contacts... fanout 2-3 instead of 50."

pub use qw_protocol::events::same_domain;

use crate::contact::Contact;

/// Fanout cap — "fanout 2-3 instead of 50" (FAQ §6).
pub const GREEDY_FANOUT: usize = 3;

/// Rank-then-cap contact selection for forwarding `skill_tag`: contacts
/// with an exact cached-tag match first, then same-domain matches, and
/// only pad with the rest if fewer than [`GREEDY_FANOUT`] matched by tag
/// at all — small-world routing still needs *some* forward path to reach
/// far-off matches, not just same-domain hops every time.
pub fn select_forward_targets<'a>(
    contacts: impl Iterator<Item = &'a Contact>,
    skill_tag: &str,
) -> Vec<&'a Contact> {
    let mut exact = Vec::new();
    let mut domain_matches = Vec::new();
    let mut rest = Vec::new();

    for c in contacts {
        if c.cached_skill_tags.iter().any(|t| t == skill_tag) {
            exact.push(c);
        } else if c
            .cached_skill_tags
            .iter()
            .any(|t| same_domain(t, skill_tag))
        {
            domain_matches.push(c);
        } else {
            rest.push(c);
        }
    }

    let mut selected = exact;
    selected.extend(domain_matches);
    if selected.len() < GREEDY_FANOUT {
        selected.extend(rest);
    }
    selected.truncate(GREEDY_FANOUT);
    selected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contact::ContactPolicy;

    fn contact(pubkey: &str, tags: &[&str]) -> Contact {
        Contact::new(pubkey, ContactPolicy::open())
            .with_cached_tags(tags.iter().map(|s| s.to_string()).collect())
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
}
