//! The client half of NIP-QW03: read the current profile out of held
//! history, and build a new signed profile event (kind `10020`,
//! replaceable, with an author-monotonic `revision`) from what the editor
//! collected. A reader prefers the greatest `(revision, created_at, id)`
//! and falls back to a legacy kind-9020 only when no `10020` exists.
//!
//! **One surface, on purpose.** The profile is the *standing*
//! self-description with no expiry. The time-scoped "available for X"
//! posting is kind 9091 (NIP-QW11) and is not this — an editor that
//! produced both from one form would turn a standing claim into an advert
//! that never lapses.
//!
//! **Every published tag is public.** Relays holding a pending referral
//! query read them to route it. There is no half-public tag; the only lever
//! is whether a tag is in this list at all, and one that is not is simply
//! unroutable. Selective disclosure lives on the *evidence* side
//! (`qw_protocol::vc`), not here.

use qw_protocol::events::{
    profile_skill_tags, Event, ProfileSkillTags, KIND_PROFILE, KIND_PROFILE_SKILL_TAGS,
};
use qw_protocol::identity::Identity;
use serde::{Deserialize, Serialize};

use crate::taxonomy;

/// `/taxonomy.yaml`'s own rule: "Max 5 skill tags".
pub const MAX_TAGS: usize = 5;

/// The most recent profile, as a shell shows it.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    pub display_name: Option<String>,
    /// The tags currently published, in the order they were signed.
    pub tags: Vec<String>,
}

/// What the editor collected — picker leaves and/or free text. Resolved
/// through the taxonomy in [`build_signed`]; an entry that will not
/// resolve is an error naming it, not a silent drop.
#[derive(Debug, Clone, Deserialize)]
pub struct ProfileEdit {
    pub display_name: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileError {
    /// More than [`MAX_TAGS`] after de-duplication.
    TooMany(usize),
    /// Nothing left to publish. An empty profile is not a profile — it is
    /// the network being unable to route to this member at all.
    Empty,
    /// One entry could not be resolved to a tag. Carries the raw input and
    /// why, so the editor can point at the offending chip.
    Tag { input: String, reason: String },
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::TooMany(n) => {
                write!(f, "{n} tags — the taxonomy allows at most {MAX_TAGS}")
            }
            ProfileError::Empty => write!(f, "a profile needs at least one skill tag"),
            ProfileError::Tag { input, reason } => write!(f, "`{input}`: {reason}"),
        }
    }
}
impl std::error::Error for ProfileError {}

/// The event behind [`current`] — the most authoritative profile this
/// pubkey has published among held events, or `None`. A replaceable
/// [`KIND_PROFILE`] event always wins over a legacy [`KIND_PROFILE_SKILL_TAGS`]
/// one; among replaceable events the order is `(revision, created_at, id)`
/// (NIP-QW12), so a stale replica's fast clock cannot overwrite a newer
/// profile.
pub fn current_event<'a>(events: &'a [Event], pubkey_hex: &str) -> Option<&'a Event> {
    events
        .iter()
        .filter(|e| e.kind == KIND_PROFILE && e.pubkey == pubkey_hex)
        .max_by(|a, b| {
            (a.revision(), a.created_at, &a.id).cmp(&(b.revision(), b.created_at, &b.id))
        })
        .or_else(|| {
            events
                .iter()
                .filter(|e| e.kind == KIND_PROFILE_SKILL_TAGS && e.pubkey == pubkey_hex)
                .max_by_key(|e| e.created_at)
        })
}

/// The current profile this pubkey has published, from held events.
/// `None` means they have never published one — the case the editor
/// exists to fix. See [`current_event`] for the selection rule.
pub fn current(events: &[Event], pubkey_hex: &str) -> Option<ProfileSkillTags> {
    serde_json::from_str(&current_event(events, pubkey_hex)?.content).ok()
}

/// The highest profile `revision` this pubkey has published on a
/// replaceable event; `0` if none. The next edit signs `+ 1`.
fn latest_revision(events: &[Event], pubkey_hex: &str) -> u64 {
    events
        .iter()
        .filter(|e| e.kind == KIND_PROFILE && e.pubkey == pubkey_hex)
        .map(|e| e.revision())
        .max()
        .unwrap_or(0)
}

/// Resolve, de-duplicate and bound the editor's entries, then sign a
/// replaceable [`KIND_PROFILE`] event whose `revision` is one past the
/// highest this identity has published in `events`. The caller queues the
/// returned event like any other.
pub fn build_signed(
    identity: &Identity,
    events: &[Event],
    display_name: Option<&str>,
    raw_tags: &[String],
) -> Result<Event, ProfileError> {
    let mut skill_tags: Vec<String> = Vec::new();
    for raw in raw_tags {
        if raw.trim().is_empty() {
            continue;
        }
        let tag = taxonomy::resolve(raw)
            .map_err(|reason| ProfileError::Tag { input: raw.clone(), reason })?
            .tag()
            .to_string();
        if !skill_tags.contains(&tag) {
            skill_tags.push(tag);
        }
    }
    if skill_tags.is_empty() {
        return Err(ProfileError::Empty);
    }
    if skill_tags.len() > MAX_TAGS {
        return Err(ProfileError::TooMany(skill_tags.len()));
    }

    let display_name = display_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let profile = ProfileSkillTags { display_name, skill_tags };
    let pubkey = identity.nostr_pubkey_hex();
    let revision = latest_revision(events, &pubkey) + 1;
    Ok(profile_skill_tags(&pubkey, revision, &profile).sign(identity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use qw_protocol::events::UnsignedEvent;

    fn id() -> Identity {
        Identity::generate()
    }

    fn build(me: &Identity, held: &[Event], tags: &[&str]) -> Event {
        let raw: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        build_signed(me, held, None, &raw).unwrap()
    }

    /// A frozen legacy kind-9020 event, as a pre-migration client signed
    /// it: no `revision` tag, regular range.
    fn legacy_9020(me: &Identity, created_at: u64, tag: &str) -> Event {
        UnsignedEvent::with_created_at(
            me.nostr_pubkey_hex(),
            KIND_PROFILE_SKILL_TAGS,
            vec![],
            serde_json::to_string(&ProfileSkillTags {
                display_name: None,
                skill_tags: vec![tag.to_string()],
            })
            .unwrap(),
            created_at,
        )
        .sign(me)
    }

    #[test]
    fn builds_a_signed_replaceable_profile_from_mixed_input() {
        let me = id();
        let ev = build_signed(
            &me,
            &[],
            Some("  vk  "),
            &[
                "it/backend/languages#rust".to_string(),
                "Go Lang".to_string(), // free text -> it/backend/languages#go
                "rust".to_string(),    // duplicate of the first -> dropped
            ],
        )
        .unwrap();

        assert_eq!(ev.kind, KIND_PROFILE, "replaceable range, not the legacy 9020");
        assert_eq!(ev.revision(), 1, "first edit signs revision 1");
        assert_eq!(ev.pubkey, me.nostr_pubkey_hex());
        let p: ProfileSkillTags = serde_json::from_str(&ev.content).unwrap();
        assert_eq!(p.display_name.as_deref(), Some("vk"));
        assert_eq!(
            p.skill_tags,
            vec!["it/backend/languages#rust", "it/backend/languages#go"]
        );
        // one ["t", tag] per skill, for relay filtering
        assert_eq!(ev.tags.iter().filter(|t| t.first().map(String::as_str) == Some("t")).count(), 2);
    }

    #[test]
    fn rejects_more_than_five() {
        let err = build_signed(
            &id(),
            &[],
            None,
            &[
                "it/backend/languages#rust".into(),
                "it/backend/languages#go".into(),
                "it/backend/languages#python".into(),
                "it/backend/languages#java".into(),
                "it/backend/languages#kotlin".into(),
                "it/backend/languages#ruby".into(),
            ],
        )
        .unwrap_err();
        assert_eq!(err, ProfileError::TooMany(6));
    }

    #[test]
    fn rejects_an_unresolvable_tag_naming_it() {
        match build_signed(&id(), &[], None, &["definitely not a skill".to_string()]) {
            Err(ProfileError::Tag { input, .. }) => assert_eq!(input, "definitely not a skill"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_all_empty_list_is_empty_not_ok() {
        assert_eq!(
            build_signed(&id(), &[], Some("vk"), &["".to_string(), "   ".to_string()]),
            Err(ProfileError::Empty)
        );
    }

    #[test]
    fn each_edit_bumps_the_revision() {
        let me = id();
        let first = build(&me, &[], &["it/backend/languages#rust"]);
        assert_eq!(first.revision(), 1);
        let second = build(&me, std::slice::from_ref(&first), &["it/backend/languages#go"]);
        assert_eq!(second.revision(), 2);
        let third = build(&me, &[first, second], &["it/backend/languages#python"]);
        assert_eq!(third.revision(), 3);
    }

    #[test]
    fn current_takes_the_highest_revision_even_past_a_later_clock() {
        let me = id();
        let pk = me.nostr_pubkey_hex();
        // A stale replica re-signs an old profile with a fast clock: newer
        // `created_at`, but it never saw revision 2, so it is still rev 1.
        let mut fast_clock_rev1 = build(&me, &[], &["it/backend/languages#rust"]);
        fast_clock_rev1.created_at = 9_999;
        let mut real_rev2 = build(&me, std::slice::from_ref(&fast_clock_rev1), &["it/backend/languages#go"]);
        real_rev2.created_at = 100;

        let store = vec![fast_clock_rev1, real_rev2];
        assert_eq!(
            current(&store, &pk).unwrap().skill_tags,
            vec!["it/backend/languages#go"],
            "(revision, created_at, id) order — rev 2 wins the later clock"
        );
        assert!(current(&store, "someone-else").is_none());
    }

    #[test]
    fn a_replaceable_profile_always_beats_a_legacy_9020() {
        let me = id();
        let pk = me.nostr_pubkey_hex();
        let legacy = legacy_9020(&me, 5_000, "it/backend/languages#rust");
        let mut replaceable = build(&me, &[], &["it/backend/languages#go"]);
        replaceable.created_at = 1; // deliberately older than the legacy one

        assert_eq!(
            current(std::slice::from_ref(&legacy), &pk).unwrap().skill_tags,
            vec!["it/backend/languages#rust"],
            "with no replaceable event, the legacy one is the fallback"
        );
        assert_eq!(
            current(&[legacy, replaceable], &pk).unwrap().skill_tags,
            vec!["it/backend/languages#go"],
            "once a replaceable event exists it is authoritative, newer clock or not"
        );
    }
}
