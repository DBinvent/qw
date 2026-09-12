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
    profile_skill_tags, Certification, Education, Employment, Event, ExternalLink, ProfileSkillTags,
    SkillLevel, SkillSource, KIND_PROFILE, KIND_PROFILE_SKILL_TAGS, MAX_BIO_LEN, MAX_HEADLINE_LEN,
    MAX_LOCATION_LEN, MAX_NAME_LEN,
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

/// How far a free-text profile field (`headline`, `bio`) is allowed to
/// travel. See `app/profile-fields.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldVisibility {
    /// Written into the published kind-10020 profile — everyone sees it.
    #[default]
    Public,
    /// Reserved for a contacts-only channel (encrypted kind `10021`,
    /// specced in `app/profile-fields.md`, unbuilt). Until that lands it
    /// is held locally and **not** published — same as `Private`.
    Contacts,
    /// Never leaves this device.
    Private,
}

impl FieldVisibility {
    fn parse(s: &str) -> Self {
        match s {
            "private" => Self::Private,
            "contacts" => Self::Contacts,
            _ => Self::Public,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Contacts => "contacts",
            Self::Private => "private",
        }
    }
}

/// One free-text profile field held on the device: its value and how far
/// it may travel. Only `Public` reaches the network today.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProfileField {
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub visibility: FieldVisibility,
}

impl ProfileField {
    /// The trimmed value if it is `Public` and non-empty — what
    /// [`build_signed`] mirrors into the signed profile.
    fn published(&self) -> Option<String> {
        let v = self.value.trim();
        (self.visibility == FieldVisibility::Public && !v.is_empty()).then(|| v.to_string())
    }
}

fn trimmed(s: &str) -> String {
    s.trim().to_string()
}

/// One work-history entry plus its own visibility. The editor holds every
/// entry; only `Public` ones are mirrored into the signed profile.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LocalEmployment {
    #[serde(flatten)]
    pub entry: Employment,
    #[serde(default)]
    pub visibility: FieldVisibility,
}

impl LocalEmployment {
    fn is_blank(&self) -> bool {
        self.entry.title.trim().is_empty() && self.entry.org.trim().is_empty()
    }
    fn published(&self) -> Option<Employment> {
        (self.visibility == FieldVisibility::Public && !self.is_blank()).then(|| Employment {
            title: trimmed(&self.entry.title),
            org: trimmed(&self.entry.org),
            start: trimmed(&self.entry.start),
            end: trimmed(&self.entry.end),
            summary: trimmed(&self.entry.summary),
        })
    }
}

/// One education entry plus its own visibility.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LocalEducation {
    #[serde(flatten)]
    pub entry: Education,
    #[serde(default)]
    pub visibility: FieldVisibility,
}

impl LocalEducation {
    fn is_blank(&self) -> bool {
        self.entry.school.trim().is_empty()
    }
    fn published(&self) -> Option<Education> {
        (self.visibility == FieldVisibility::Public && !self.is_blank()).then(|| Education {
            school: trimmed(&self.entry.school),
            field: trimmed(&self.entry.field),
            start: trimmed(&self.entry.start),
            end: trimmed(&self.entry.end),
        })
    }
}

/// One certification plus its own visibility.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LocalCertification {
    #[serde(flatten)]
    pub entry: Certification,
    #[serde(default)]
    pub visibility: FieldVisibility,
}

impl LocalCertification {
    fn is_blank(&self) -> bool {
        self.entry.name.trim().is_empty()
    }
    fn published(&self) -> Option<Certification> {
        (self.visibility == FieldVisibility::Public && !self.is_blank()).then(|| Certification {
            name: trimmed(&self.entry.name),
            issuer: trimmed(&self.entry.issuer),
            year: trimmed(&self.entry.year),
            url: trimmed(&self.entry.url),
        })
    }
}

/// Local, mostly-unpublished profile detail (NIP-QW03 additive fields +
/// the visibility model in `app/profile-fields.md`). Every value lives
/// here on the device; `Session::set_profile` copies the `Public` fields
/// and entries into the signed kind-10020 event and leaves the rest local.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProfileLocal {
    /// The holder's name and its visibility. Seeded from a pre-existing
    /// published `display_name` the first time this record is written.
    #[serde(default)]
    pub name: ProfileField,
    /// An alternate public name (alias) and its visibility.
    #[serde(default)]
    pub alt_name: ProfileField,
    #[serde(default)]
    pub headline: ProfileField,
    #[serde(default)]
    pub bio: ProfileField,
    #[serde(default)]
    pub location: ProfileField,
    #[serde(default)]
    pub employment: Vec<LocalEmployment>,
    #[serde(default)]
    pub education: Vec<LocalEducation>,
    #[serde(default)]
    pub certifications: Vec<LocalCertification>,
}

/// What the editor sends for one free-text field.
#[derive(Debug, Clone, Deserialize)]
pub struct ProfileFieldEdit {
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub visibility: String,
}

impl From<&ProfileFieldEdit> for ProfileField {
    fn from(e: &ProfileFieldEdit) -> Self {
        ProfileField {
            value: e.value.trim().to_string(),
            visibility: FieldVisibility::parse(&e.visibility),
        }
    }
}

impl From<&ProfileField> for ProfileFieldEdit {
    fn from(f: &ProfileField) -> Self {
        ProfileFieldEdit {
            value: f.value.clone(),
            visibility: f.visibility.as_str().to_string(),
        }
    }
}

/// One free-text field as a shell shows it: value plus its visibility as
/// a lowercase string.
#[derive(Debug, Clone, Serialize)]
pub struct FieldView {
    pub value: String,
    pub visibility: String,
}

impl From<&ProfileField> for FieldView {
    fn from(f: &ProfileField) -> Self {
        FieldView {
            value: f.value.clone(),
            visibility: f.visibility.as_str().to_string(),
        }
    }
}

/// The most recent profile, as a shell shows it.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    /// The effective name for a legacy reader — the local value if the
    /// owner has one, else the published one. Prefer `name` (carries the
    /// visibility) in a new client.
    pub display_name: Option<String>,
    /// Name + visibility, and the alternate public name + visibility.
    /// Filled by `Session::profile_view` from [`ProfileLocal`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<FieldView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_name: Option<FieldView>,
    /// Free-text prose held locally, each with its visibility. Filled by
    /// `Session::profile_view` from the persisted [`ProfileLocal`], not by
    /// [`ProfileView::from_record`] (the signed event only carries the
    /// `Public` ones).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headline: Option<FieldView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bio: Option<FieldView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<FieldView>,
    /// Structured detail, the full local list with each entry's chosen
    /// visibility — the editor renders these; `Session::profile_view`
    /// fills them from [`ProfileLocal`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub employment: Vec<LocalEmployment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub education: Vec<LocalEducation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub certifications: Vec<LocalCertification>,
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
    /// Fold a held [`ProfileSkillTags`] into the shell shape. `headline` /
    /// `bio` are left `None` — the signed event carries only the `Public`
    /// copy, and `Session::profile_view` fills them from the local
    /// [`ProfileLocal`] which also knows each field's chosen visibility.
    fn from_record(p: ProfileSkillTags) -> Self {
        Self {
            display_name: p.display_name,
            name: None,
            alt_name: None,
            headline: None,
            bio: None,
            location: None,
            employment: vec![],
            education: vec![],
            certifications: vec![],
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

/// What the editor sends for `display_name`. Accepts the legacy bare
/// string (`"vk"` -> public) as well as the `{ value, visibility }` shape
/// every other free-text field uses, so an older caller is unaffected.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum NameEdit {
    Plain(String),
    Field(ProfileFieldEdit),
}

impl From<&NameEdit> for ProfileField {
    fn from(n: &NameEdit) -> Self {
        match n {
            NameEdit::Plain(s) => ProfileField {
                value: s.trim().to_string(),
                visibility: FieldVisibility::Public,
            },
            NameEdit::Field(f) => ProfileField::from(f),
        }
    }
}

/// What the editor collected. An entry that will not resolve is an error
/// naming it, not a silent drop.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProfileEdit {
    /// The holder's name plus its visibility (or a bare string, legacy).
    pub display_name: Option<NameEdit>,
    /// An alternate public name plus its visibility.
    #[serde(default)]
    pub alt_name: Option<ProfileFieldEdit>,
    pub skills: Vec<SkillEdit>,
    #[serde(default)]
    pub links: Vec<LinkEdit>,
    /// A one-line "what I do" plus its visibility. Omitted leaves the
    /// stored value untouched; sent, it replaces it.
    #[serde(default)]
    pub headline: Option<ProfileFieldEdit>,
    /// A short bio plus its visibility.
    #[serde(default)]
    pub bio: Option<ProfileFieldEdit>,
    /// A coarse location plus its visibility.
    #[serde(default)]
    pub location: Option<ProfileFieldEdit>,
    /// The full work-history list, each entry with its own visibility.
    /// Omitted leaves the stored list; sent, it replaces it wholesale.
    #[serde(default)]
    pub employment: Option<Vec<LocalEmployment>>,
    #[serde(default)]
    pub education: Option<Vec<LocalEducation>>,
    #[serde(default)]
    pub certifications: Option<Vec<LocalCertification>>,
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
    /// `headline` or `bio` (the `Public` copy) is over its character bound.
    Field { field: &'static str, reason: String },
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
            ProfileError::Field { field, reason } => write!(f, "{field}: {reason}"),
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

    // The name and its alias, like every other free-text field: published
    // only if the holder left the visibility `Public`.
    let display_name = name_published(
        edit.display_name.as_ref().map(ProfileField::from),
        "display_name",
    )?;
    let alt_name = name_published(
        edit.alt_name.as_ref().map(ProfileField::from),
        "alt_name",
    )?;

    // Free-text prose + structured lists: only the `Public` copy of a
    // field, and only `Public` entries, reach the event. `Contacts` and
    // `Private` are held locally by the caller.
    let headline = field_published(edit.headline.as_ref(), "headline", MAX_HEADLINE_LEN)?;
    let bio = field_published(edit.bio.as_ref(), "bio", MAX_BIO_LEN)?;
    let location = field_published(edit.location.as_ref(), "location", MAX_LOCATION_LEN)?;

    let employment = edit
        .employment
        .as_deref()
        .map(|v| v.iter().filter_map(LocalEmployment::published).collect())
        .unwrap_or_default();
    let education = edit
        .education
        .as_deref()
        .map(|v| v.iter().filter_map(LocalEducation::published).collect())
        .unwrap_or_default();
    let certifications = edit
        .certifications
        .as_deref()
        .map(|v| v.iter().filter_map(LocalCertification::published).collect())
        .unwrap_or_default();

    let profile = ProfileSkillTags {
        display_name,
        alt_name,
        headline,
        bio,
        location,
        employment,
        education,
        certifications,
        skill_tags,
        skill_levels,
        skill_sources,
        links,
    };
    // The protocol owns the entry bounds (count + per-field length).
    profile.validate().map_err(|e| match e {
        qw_protocol::events::ProfileSkillTagsError::FieldLength { field, .. } => {
            ProfileError::Field {
                field,
                reason: e.to_string(),
            }
        }
        other => ProfileError::Field {
            field: "profile",
            reason: other.to_string(),
        },
    })?;

    let pubkey = identity.nostr_pubkey_hex();
    let revision = latest_revision(events, &pubkey) + 1;
    Ok(profile_skill_tags(&pubkey, revision, &profile).sign(identity))
}

/// Same as [`field_published`] for a name — the value is already a
/// resolved [`ProfileField`] (a `NameEdit` may have been a bare string).
fn name_published(
    f: Option<ProfileField>,
    name: &'static str,
) -> Result<Option<String>, ProfileError> {
    let Some(v) = f.and_then(|f| f.published()) else {
        return Ok(None);
    };
    if v.chars().count() >= MAX_NAME_LEN {
        return Err(ProfileError::Field {
            field: name,
            reason: format!("must be shorter than {MAX_NAME_LEN} characters"),
        });
    }
    Ok(Some(v))
}

/// The `Public`, non-empty, in-bounds value of an optional editor field —
/// what [`build_signed`] mirrors into the signed profile. `Contacts` /
/// `Private` / empty / absent all yield `None`.
fn field_published(
    e: Option<&ProfileFieldEdit>,
    name: &'static str,
    max: usize,
) -> Result<Option<String>, ProfileError> {
    let Some(v) = e.and_then(|e| ProfileField::from(e).published()) else {
        return Ok(None);
    };
    if v.chars().count() >= max {
        return Err(ProfileError::Field {
            field: name,
            reason: format!("must be shorter than {max} characters"),
        });
    }
    Ok(Some(v))
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
            display_name: name.map(|n| NameEdit::Plain(n.to_string())),
            skills: tags
                .iter()
                .map(|t| SkillEdit {
                    tag: t.to_string(),
                    level: None,
                    source: None,
                })
                .collect(),
            links: vec![],
            ..Default::default()
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
            ..Default::default()
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
    fn headline_public_is_published_trimmed_private_bio_is_not() {
        let mut edit = edit_of(Some("vk"), &["it/backend/languages#rust"]);
        edit.headline = Some(ProfileFieldEdit {
            value: "  Backend engineer  ".into(),
            visibility: "public".into(),
        });
        edit.bio = Some(ProfileFieldEdit {
            value: "a note to self".into(),
            visibility: "private".into(),
        });
        let ev = build_signed(&id(), &[], &edit).unwrap();
        let p: ProfileSkillTags = serde_json::from_str(&ev.content).unwrap();
        assert_eq!(p.headline.as_deref(), Some("Backend engineer"));
        assert_eq!(p.bio, None, "a Private field never reaches the signed event");

        // Contacts behaves as Private until the encrypted channel is built.
        edit.headline = Some(ProfileFieldEdit {
            value: "hidden".into(),
            visibility: "contacts".into(),
        });
        let ev = build_signed(&id(), &[], &edit).unwrap();
        let p: ProfileSkillTags = serde_json::from_str(&ev.content).unwrap();
        assert_eq!(p.headline, None);
    }

    #[test]
    fn a_public_headline_at_the_protocol_bound_is_rejected() {
        let mut edit = edit_of(None, &["it/backend/languages#rust"]);
        edit.headline = Some(ProfileFieldEdit {
            value: "x".repeat(qw_protocol::events::MAX_HEADLINE_LEN),
            visibility: "public".into(),
        });
        assert!(matches!(
            build_signed(&id(), &[], &edit),
            Err(ProfileError::Field { field: "headline", .. })
        ));
        // one under the bound is fine
        edit.headline = Some(ProfileFieldEdit {
            value: "x".repeat(qw_protocol::events::MAX_HEADLINE_LEN - 1),
            visibility: "public".into(),
        });
        assert!(build_signed(&id(), &[], &edit).is_ok());
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
