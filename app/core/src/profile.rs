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
//!
//! Beyond the tag itself a profile carries, per NIP-QW03: a self-assessed
//! **level** (`beginner`..`expert`), the **provenance** of the tag (typed,
//! or suggested by commit-history analysis), and a flat list of
//! **external-network links** (GitHub, a site, …) — all self-asserted, none
//! evidence, all weighed against the countersigned history (§5).

use std::collections::BTreeMap;

use qw_protocol::events::{
    profile_skill_tags, Event, ExternalLink, ProfileSkillTags, SkillLevel, SkillSource,
    KIND_PROFILE, KIND_PROFILE_SKILL_TAGS,
};
use qw_protocol::identity::Identity;
use serde::{Deserialize, Serialize};

use crate::taxonomy;

/// NIP-QW03: a profile carries **fewer than 80** skill tags — the same
/// bound `qw_protocol::events::kinds::ProfileSkillTags::validate` enforces.
/// (The bundled taxonomy has far fewer leaves than this today; the cap is
/// the protocol limit, not a local style choice.)
pub const MAX_TAGS: usize = qw_protocol::events::MAX_PROFILE_SKILLS - 1;
/// Longest a single skill tag may be, in characters (NIP-QW03: `< 80`).
pub const MAX_TAG_LEN: usize = qw_protocol::events::MAX_SKILL_TAG_LEN - 1;
/// A profile carries a handful of links, not a link farm.
pub const MAX_LINKS: usize = 8;

/// The most recent profile, as a shell shows it.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    pub display_name: Option<String>,
    /// The tags currently published, in the order they were signed.
    pub tags: Vec<String>,
    /// Canonical tag -> self-assessed level string. A tag absent here has
    /// no stated level.
    pub levels: BTreeMap<String, String>,
    /// Canonical tag -> provenance (`"self"` | `"commit-analysis"`). A tag
    /// absent here is `"self"`.
    pub sources: BTreeMap<String, String>,
    pub links: Vec<ExternalLink>,
}

impl ProfileView {
    /// Fold a held [`ProfileSkillTags`] into the shell shape.
    fn from_record(p: ProfileSkillTags) -> Self {
        Self {
            display_name: p.display_name,
            tags: p.skill_tags,
            levels: p
                .skill_levels
                .into_iter()
                .map(|(t, l)| (t, l.as_str().to_string()))
                .collect(),
            sources: p
                .skill_sources
                .into_iter()
                .map(|(t, s)| (t, s.as_str().to_string()))
                .collect(),
            links: p.links,
        }
    }
}

/// One skill the editor collected: the raw entry (free text or a picker
/// leaf, resolved through the taxonomy in [`build_signed`]) plus an
/// optional self-assessed level and provenance.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillEdit {
    pub tag: String,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

/// An external-network link the editor collected.
#[derive(Debug, Clone, Deserialize)]
pub struct LinkEdit {
    pub network: String,
    pub url: String,
}

/// What the editor collected. An entry that will not resolve is an error
/// naming it, not a silent drop.
#[derive(Debug, Clone, Deserialize)]
pub struct ProfileEdit {
    pub display_name: Option<String>,
    pub skills: Vec<SkillEdit>,
    #[serde(default)]
    pub links: Vec<LinkEdit>,
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
    /// A level string was not one of `beginner` / `intermediate` /
    /// `senior` / `expert`.
    Level { input: String },
    /// An external link did not validate.
    Link { input: String, reason: String },
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::TooMany(n) => {
                write!(f, "{n} tags — a profile carries fewer than {} (NIP-QW03)", MAX_TAGS + 1)
            }
            ProfileError::Empty => write!(f, "a profile needs at least one skill tag"),
            ProfileError::Tag { input, reason } => write!(f, "`{input}`: {reason}"),
            ProfileError::Level { input } => write!(
                f,
                "`{input}` is not a level (beginner, intermediate, senior, expert)"
            ),
            ProfileError::Link { input, reason } => write!(f, "link `{input}`: {reason}"),
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

/// The current profile in the shell shape (`None` if never published).
pub fn current_view(events: &[Event], pubkey_hex: &str) -> Option<ProfileView> {
    current(events, pubkey_hex).map(ProfileView::from_record)
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
    edit: &ProfileEdit,
) -> Result<Event, ProfileError> {
    let mut skill_tags: Vec<String> = Vec::new();
    let mut skill_levels: BTreeMap<String, SkillLevel> = BTreeMap::new();
    let mut skill_sources: BTreeMap<String, SkillSource> = BTreeMap::new();

    for entry in &edit.skills {
        if entry.tag.trim().is_empty() {
            continue;
        }
        let tag = taxonomy::resolve(&entry.tag)
            .map_err(|reason| ProfileError::Tag {
                input: entry.tag.clone(),
                reason,
            })?
            .tag()
            .to_string();
        if !skill_tags.contains(&tag) {
            skill_tags.push(tag.clone());
        }
        if let Some(raw) = entry
            .level
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let level =
                SkillLevel::parse(&raw.to_lowercase()).ok_or_else(|| ProfileError::Level {
                    input: raw.to_string(),
                })?;
            skill_levels.insert(tag.clone(), level);
        }
        // Provenance: only `commit-analysis` is worth recording; a missing
        // or unrecognised value is the implicit `self`.
        if entry.source.as_deref() == Some("commit-analysis") {
            skill_sources.insert(tag.clone(), SkillSource::CommitAnalysis);
        }
    }
    if skill_tags.is_empty() {
        return Err(ProfileError::Empty);
    }
    if skill_tags.len() > MAX_TAGS {
        return Err(ProfileError::TooMany(skill_tags.len()));
    }
    if let Some(t) = skill_tags.iter().find(|t| t.chars().count() > MAX_TAG_LEN) {
        return Err(ProfileError::Tag {
            input: t.clone(),
            reason: format!("a skill tag is at most {MAX_TAG_LEN} characters"),
        });
    }

    let mut links: Vec<ExternalLink> = Vec::new();
    for l in &edit.links {
        let url = l.url.trim().to_string();
        if url.is_empty() {
            continue;
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(ProfileError::Link {
                input: url,
                reason: "must be an http(s) URL".to_string(),
            });
        }
        let network = l.network.trim().to_string();
        let network = if network.is_empty() {
            "website".to_string()
        } else {
            network.to_lowercase()
        };
        if !links.iter().any(|e: &ExternalLink| e.url == url) {
            links.push(ExternalLink { network, url });
        }
    }
    if links.len() > MAX_LINKS {
        return Err(ProfileError::Link {
            input: format!("{} links", links.len()),
            reason: format!("at most {MAX_LINKS}"),
        });
    }

    let display_name = edit
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let profile = ProfileSkillTags {
        display_name,
        skill_tags,
        skill_levels,
        skill_sources,
        links,
    };
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

    /// A `ProfileEdit` with bare tags, no levels/links — the common case.
    fn edit_of(name: Option<&str>, tags: &[&str]) -> ProfileEdit {
        ProfileEdit {
            display_name: name.map(str::to_string),
            skills: tags
                .iter()
                .map(|t| SkillEdit {
                    tag: t.to_string(),
                    level: None,
                    source: None,
                })
                .collect(),
            links: vec![],
        }
    }

    fn build(me: &Identity, held: &[Event], tags: &[&str]) -> Event {
        build_signed(me, held, &edit_of(None, tags)).unwrap()
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
                ..Default::default()
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
            &edit_of(
                Some("  vk  "),
                &["it/backend/languages#rust", "Go Lang", "rust"],
            ),
        )
        .unwrap();

        assert_eq!(
            ev.kind, KIND_PROFILE,
            "replaceable range, not the legacy 9020"
        );
        assert_eq!(ev.revision(), 1, "first edit signs revision 1");
        assert_eq!(ev.pubkey, me.nostr_pubkey_hex());
        let p: ProfileSkillTags = serde_json::from_str(&ev.content).unwrap();
        assert_eq!(p.display_name.as_deref(), Some("vk"));
        assert_eq!(
            p.skill_tags,
            vec!["it/backend/languages#rust", "it/backend/languages#go"]
        );
        assert_eq!(
            ev.tags
                .iter()
                .filter(|t| t.first().map(String::as_str) == Some("t"))
                .count(),
            2
        );
    }

    #[test]
    fn carries_levels_sources_and_links() {
        let me = id();
        let edit = ProfileEdit {
            display_name: None,
            skills: vec![
                SkillEdit {
                    tag: "Rust Lang".into(),
                    level: Some("Expert".into()),
                    source: Some("commit-analysis".into()),
                },
                SkillEdit {
                    tag: "it/backend/languages#go".into(),
                    level: Some("mid".into()), // alias for intermediate
                    source: None,
                },
            ],
            links: vec![
                LinkEdit {
                    network: "GitHub".into(),
                    url: "  https://github.com/vk  ".into(),
                },
                LinkEdit {
                    network: "".into(),
                    url: "https://vk.dev".into(),
                },
                LinkEdit {
                    network: "x".into(),
                    url: "https://github.com/vk".into(),
                }, // dup url
            ],
        };
        let ev = build_signed(&me, &[], &edit).unwrap();
        let p: ProfileSkillTags = serde_json::from_str(&ev.content).unwrap();

        assert_eq!(
            p.skill_levels["it/backend/languages#rust"],
            SkillLevel::Expert
        );
        assert_eq!(
            p.skill_levels["it/backend/languages#go"],
            SkillLevel::Intermediate
        );
        assert_eq!(
            p.skill_sources["it/backend/languages#rust"],
            SkillSource::CommitAnalysis
        );
        assert!(
            !p.skill_sources.contains_key("it/backend/languages#go"),
            "plain self is implicit"
        );
        assert_eq!(p.links.len(), 2, "the duplicate url is dropped");
        assert_eq!(
            p.links[0],
            ExternalLink {
                network: "github".into(),
                url: "https://github.com/vk".into()
            }
        );
        assert_eq!(p.links[1].network, "website", "a blank network defaults");
        // one ["r", url] per link, for relay discoverability
        assert_eq!(
            ev.tags
                .iter()
                .filter(|t| t.first().map(String::as_str) == Some("r"))
                .count(),
            2
        );

        let view = current_view(std::slice::from_ref(&ev), &me.nostr_pubkey_hex()).unwrap();
        assert_eq!(view.levels["it/backend/languages#rust"], "expert");
        assert_eq!(view.sources["it/backend/languages#rust"], "commit-analysis");
        assert_eq!(view.links.len(), 2);
    }

    #[test]
    fn rejects_a_bad_level_and_a_bad_link() {
        let me = id();
        let mut edit = edit_of(None, &["rust"]);
        edit.skills[0].level = Some("wizard".into());
        assert!(matches!(
            build_signed(&me, &[], &edit),
            Err(ProfileError::Level { .. })
        ));

        let mut edit = edit_of(None, &["rust"]);
        edit.links = vec![LinkEdit {
            network: "ftp".into(),
            url: "ftp://nope".into(),
        }];
        assert!(matches!(
            build_signed(&me, &[], &edit),
            Err(ProfileError::Link { .. })
        ));
    }

    #[test]
    fn tag_count_cap_is_the_nip_qw03_limit_not_five() {
        assert_eq!(MAX_TAGS, 79, "NIP-QW03: fewer than 80 per profile");
        let all: Vec<&str> = taxonomy::leaves().iter().map(String::as_str).collect();
        // 6 distinct leaves used to be rejected; now they are fine.
        assert!(build_signed(&id(), &[], &edit_of(None, &all[..6])).is_ok());
        if all.len() > MAX_TAGS {
            // MAX_TAGS + 1 distinct leaves are rejected as TooMany.
            let over: Vec<&str> = all[..MAX_TAGS + 1].to_vec();
            assert_eq!(
                build_signed(&id(), &[], &edit_of(None, &over)).unwrap_err(),
                ProfileError::TooMany(MAX_TAGS + 1)
            );
        }
    }

    #[test]
    fn rejects_an_unresolvable_tag_naming_it() {
        match build_signed(&id(), &[], &edit_of(None, &["definitely not a skill"])) {
            Err(ProfileError::Tag { input, .. }) => assert_eq!(input, "definitely not a skill"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_all_empty_list_is_empty_not_ok() {
        assert_eq!(
            build_signed(&id(), &[], &edit_of(Some("vk"), &["", "   "])),
            Err(ProfileError::Empty)
        );
    }

    #[test]
    fn each_edit_bumps_the_revision() {
        let me = id();
        let first = build(&me, &[], &["it/backend/languages#rust"]);
        assert_eq!(first.revision(), 1);
        let second = build(
            &me,
            std::slice::from_ref(&first),
            &["it/backend/languages#go"],
        );
        assert_eq!(second.revision(), 2);
        let third = build(&me, &[first, second], &["it/backend/languages#python"]);
        assert_eq!(third.revision(), 3);
    }

    #[test]
    fn current_takes_the_highest_revision_even_past_a_later_clock() {
        let me = id();
        let pk = me.nostr_pubkey_hex();
        let mut fast_clock_rev1 = build(&me, &[], &["it/backend/languages#rust"]);
        fast_clock_rev1.created_at = 9_999;
        let mut real_rev2 = build(
            &me,
            std::slice::from_ref(&fast_clock_rev1),
            &["it/backend/languages#go"],
        );
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
        replaceable.created_at = 1;

        assert_eq!(
            current(std::slice::from_ref(&legacy), &pk)
                .unwrap()
                .skill_tags,
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
