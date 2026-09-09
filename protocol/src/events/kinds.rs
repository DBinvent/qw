//! QW's custom Nostr event kinds. Numbered in the 9000-9099 block: NIP-01
//! reserves 1000<=kind<10000 for regular (stored, non-replaceable,
//! non-ephemeral) events, which matches every kind here — these are
//! permanent signed records, never superseded or expired by a relay.
//!
//! Each kind's `content` is JSON of the paired struct below. Full spec
//! (rationale, tag layout, worked examples) lives in `/protocol/nips/`.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{e_tag, e_tag_marked, p_tag, r_tag, revision_tag, t_tag, Event, Tag, UnsignedEvent};

// --- job lifecycle (NIP-QW01) ---
pub const KIND_JOB_OFFER: u16 = 9000;
pub const KIND_JOB_ACCEPT: u16 = 9001;
pub const KIND_JOB_MILESTONE: u16 = 9002;
pub const KIND_JOB_COMPLETION: u16 = 9003;
pub const KIND_JOB_COUNTEROFFER: u16 = 9004;
pub const KIND_JOB_REVIEW_REQUEST: u16 = 9005;
pub const KIND_SIDE_SETTLEMENT: u16 = 9006;

// --- credit issuance (NIP-QW02) ---
pub const KIND_CREDIT_ISSUANCE: u16 = 9010;

// --- profile / skill tags (NIP-QW03) ---
/// Legacy: the original profile kind, in NIP-01's regular (never-replaced)
/// range. Frozen — nothing signs it any more, but readers fall back to the
/// most recent one when a pubkey has published no [`KIND_PROFILE`] event.
pub const KIND_PROFILE_SKILL_TAGS: u16 = 9020;
/// The current profile kind, in Nostr's **replaceable** range
/// (10000-19999): relays keep only the latest per (pubkey, kind). A
/// profile is a standing statement of intent, not evidence — unlike the
/// contract ledger it must not accumulate a permanent public history of
/// every edit. QW's replaceable kinds mirror the `90xx` block at `100xx`,
/// so 9020 -> 10020. Carries a `["revision", n]` tag; readers order by
/// `(revision, created_at, id)` — never `created_at` alone — so a replica
/// with a fast clock cannot overwrite a newer profile (NIP-QW12).
pub const KIND_PROFILE: u16 = 10020;

// --- dispute annotation (NIP-QW04) ---
pub const KIND_DISPUTE_ANNOTATION: u16 = 9030;

// --- cascade block (NIP-QW05) ---
pub const KIND_CASCADE_BLOCK_FLAG: u16 = 9040;
pub const KIND_CASCADE_BLOCK_RECORD: u16 = 9041;

// --- referral query (NIP-QW06) ---
pub const KIND_SKILL_QUERY: u16 = 9050;
pub const KIND_SKILL_ANSWER: u16 = 9051;

// --- introduction (NIP-QW07) ---
pub const KIND_INTRODUCTION: u16 = 9060;

// --- history request/response (NIP-QW08) ---
pub const KIND_HISTORY_REQUEST: u16 = 9070;
pub const KIND_HISTORY_RESPONSE: u16 = 9071;

// --- person record amendment (NIP-QW09) ---
pub const KIND_RECOVERY_POLICY: u16 = 9080;
pub const KIND_PERSON_RECORD_AMENDMENT: u16 = 9081;
/// Device subkey delegation / revocation (NIP-QW09 §"Device subkeys").
/// Controller-signed, **no quorum** — routine multi-device life (a new
/// phone, a `qw-web` box) should not need the account's trusted contacts.
/// A delegation carries `revoked_at: null`; revoking a device is a second
/// 9082 for the same `device_pubkey` with `revoked_at` set. Verifying any
/// QW event then becomes "signature valid **and** signer was
/// controller-or-delegated at `created_at`"
/// (`crate::recovery::device_authority`).
pub const KIND_DEVICE_SUBKEY: u16 = 9082;

// --- chain-calculation result (NIP-QW10) ---
pub const KIND_CHAIN_CALCULATION_RESULT: u16 = 9090;

// --- bulletin listing (NIP-QW11) ---
pub const KIND_BULLETIN_LISTING: u16 = 9091;

// --- skill recognition, by a bureau (NIP-QW15) ---
pub const KIND_RECOGNITION_REQUEST: u16 = 9092;
pub const KIND_RECOGNITION: u16 = 9093;

// --- broadcast propagation (NIP-QW14) ---
pub const KIND_BROADCAST: u16 = 9100;
pub const KIND_HOP_RATING: u16 = 9101;

/// `Hours × Rate × ko × km` per abstract.md — `ko`/`km` may be omitted to
/// simplify negotiation. For an AI-model party actor, `ko` tracks model
/// size / context window / agent-config quality and `km` the model's
/// cognition — prompt adherence, hallucination rate (abstract.md, "When a
/// party actor is an AI model").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobOffer {
    pub skill_tags: Vec<String>,
    pub hours: f64,
    pub rate: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ko: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub km: Option<f64>,
    pub terms: String,
}

/// Client offers a job to `worker_pubkey`. Signed by the client only —
/// not atomic (§4).
pub fn job_offer(
    client_pubkey_hex: &str,
    worker_pubkey_hex: &str,
    offer: &JobOffer,
) -> UnsignedEvent {
    let mut tags: Vec<Tag> = vec![p_tag(worker_pubkey_hex)];
    tags.extend(offer.skill_tags.iter().map(t_tag));
    UnsignedEvent::new(
        client_pubkey_hex,
        KIND_JOB_OFFER,
        tags,
        serde_json::to_string(offer).expect("JobOffer serializes"),
    )
}

/// The same offer as [`job_offer`], plus a record that this proposal grew
/// out of one specific NIP-QW07 introduction — the invite that preceded
/// it. Adds `["e", <introduction event id>, "", "introduction"]`; the
/// content is byte-for-byte an ordinary offer, so
/// [`crate::contract::Contract::from_events`] still anchors the contract
/// on this event's own id, and a client that predates the marker just
/// ignores the extra tag. Use it for the common invite-link case, where
/// the link existed *because* of an upcoming contract.
pub fn job_offer_from_introduction(
    client_pubkey_hex: &str,
    worker_pubkey_hex: &str,
    introduction_event_id_hex: &str,
    offer: &JobOffer,
) -> UnsignedEvent {
    let mut unsigned = job_offer(client_pubkey_hex, worker_pubkey_hex, offer);
    unsigned
        .tags
        .push(e_tag_marked(introduction_event_id_hex, "introduction"));
    unsigned
}

/// Neither accepts nor rejects `superseded_event_id_hex` (the offer or
/// prior counteroffer it responds to) — it supersedes those terms and
/// hands the proposal back. Either party may counter repeatedly; only a
/// signed Accept ends the exchange. Reuses `JobOffer`'s shape since a
/// counteroffer *is* a full replacement set of terms, not a diff.
pub fn job_counteroffer(
    author_pubkey_hex: &str,
    counterparty_pubkey_hex: &str,
    superseded_event_id_hex: &str,
    counter: &JobOffer,
) -> UnsignedEvent {
    let mut tags: Vec<Tag> = vec![
        p_tag(counterparty_pubkey_hex),
        e_tag(superseded_event_id_hex),
    ];
    tags.extend(counter.skill_tags.iter().map(t_tag));
    UnsignedEvent::new(
        author_pubkey_hex,
        KIND_JOB_COUNTEROFFER,
        tags,
        serde_json::to_string(counter).expect("JobOffer serializes"),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobAccept {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Worker accepts. Signed by the worker only — not atomic (§4).
/// `offer_event_id_hex` is whichever event actually got agreed to: the
/// original offer if nobody countered, or the last counteroffer
/// (`KIND_JOB_COUNTEROFFER`) otherwise — no version before the accepted
/// one carries any obligation.
pub fn job_accept(
    worker_pubkey_hex: &str,
    client_pubkey_hex: &str,
    offer_event_id_hex: &str,
    accept: &JobAccept,
) -> UnsignedEvent {
    let tags = vec![p_tag(client_pubkey_hex), e_tag(offer_event_id_hex)];
    UnsignedEvent::new(
        worker_pubkey_hex,
        KIND_JOB_ACCEPT,
        tags,
        serde_json::to_string(accept).expect("JobAccept serializes"),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobMilestone {
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hours_delta: Option<f64>,
}

/// Optional; either party may post one. Signed by whoever posts it.
pub fn job_milestone(
    author_pubkey_hex: &str,
    counterparty_pubkey_hex: &str,
    offer_event_id_hex: &str,
    milestone: &JobMilestone,
) -> UnsignedEvent {
    let tags = vec![p_tag(counterparty_pubkey_hex), e_tag(offer_event_id_hex)];
    UnsignedEvent::new(
        author_pubkey_hex,
        KIND_JOB_MILESTONE,
        tags,
        serde_json::to_string(milestone).expect("JobMilestone serializes"),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobCompletion {
    /// 0-5; how the author rates the counterparty's side of the contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Each party signs their own completion/acceptance record separately —
/// not atomic (§4). Two of these (one per party) is what dual indexing
/// (`crate::dual_index`) expects to find for a contract to be complete.
pub fn job_completion(
    author_pubkey_hex: &str,
    counterparty_pubkey_hex: &str,
    offer_event_id_hex: &str,
    completion: &JobCompletion,
) -> UnsignedEvent {
    let tags = vec![p_tag(counterparty_pubkey_hex), e_tag(offer_event_id_hex)];
    UnsignedEvent::new(
        author_pubkey_hex,
        KIND_JOB_COMPLETION,
        tags,
        serde_json::to_string(completion).expect("JobCompletion serializes"),
    )
}

/// Marks a contract **settled off-system** — paid by a side payment, so no
/// credit issuance (NIP-QW02, kind 9010) will follow. The countersigned
/// completion (kind 9003) still records that the work happened and was
/// approved; this says only that the *ledger* entry is deliberately
/// absent, so a reader stops waiting for a 9010 and scores the contract
/// accordingly. Either party may post it, anchored to the offer id like
/// every other post-offer step; both posting it is the mutually-attested
/// case.
///
/// Trust effect (NIP-QW13 §2): a side-settled contract keeps its
/// rating / pass-fail signal, contributes **no** Quant-magnitude term
/// (there is no amount), and is scaled by the viewer's
/// `side_settled_factor`. It ranks **below** a credit-backed contract and
/// **above** a self-declared or commit-analysis tag (NIP-QW03 evidence
/// classes).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SideSettlement {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `["p", <counterparty>]`, `["e", <offer event id>]`, and a
/// `["settlement", "side"]` marker so a relay or coordination service can
/// filter without reading `content`.
pub fn side_settlement(
    author_pubkey_hex: &str,
    counterparty_pubkey_hex: &str,
    offer_event_id_hex: &str,
    settlement: &SideSettlement,
) -> UnsignedEvent {
    let tags = vec![
        p_tag(counterparty_pubkey_hex),
        e_tag(offer_event_id_hex),
        vec!["settlement".to_string(), "side".to_string()],
    ];
    UnsignedEvent::new(
        author_pubkey_hex,
        KIND_SIDE_SETTLEMENT,
        tags,
        serde_json::to_string(settlement).expect("SideSettlement serializes"),
    )
}

/// Request review of a completed job or a delivered milestone, with
/// optional feedback — a **pre-signature** negotiation step
/// (`abstract.md` "Basic Use Cases" §"Commit a contract", added
/// 2026-08-07), closer in spirit to `JobCounteroffer` than to the
/// after-the-fact dispute annotations of NIP-QW04 (those apply to
/// already-signed records; this precedes the countersigned Completion).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobReviewRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

/// `target_event_id_hex` is whatever's under review — a milestone
/// (`KIND_JOB_MILESTONE`) or a completion (`KIND_JOB_COMPLETION`).
pub fn job_review_request(
    author_pubkey_hex: &str,
    counterparty_pubkey_hex: &str,
    target_event_id_hex: &str,
    review: &JobReviewRequest,
) -> UnsignedEvent {
    let tags = vec![p_tag(counterparty_pubkey_hex), e_tag(target_event_id_hex)];
    UnsignedEvent::new(
        author_pubkey_hex,
        KIND_JOB_REVIEW_REQUEST,
        tags,
        serde_json::to_string(review).expect("JobReviewRequest serializes"),
    )
}

/// Q4 default: ranged/bucketed amount, full value opt-in per participant.
/// `Bucket` is a log-scale bucket index (see `/protocol/nips/NIP-QW02...`
/// for the bucket table); `Exact` is the opt-in full disclosure.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "unit", rename_all = "snake_case")]
pub enum QuantAmount {
    Bucket { index: u8 },
    Exact { quants: f64 },
}

impl QuantAmount {
    /// A numeric value usable for summation (`crate::trust::net_position`,
    /// §5). **Provisional**: the real bucket-edge table is an open item
    /// (NIP-QW02) — this is a placeholder log-scale mapping (doubling per
    /// index), good enough for relative ordering across a viewer's own
    /// history, not a committed pricing table. Revisit alongside Q4 in
    /// `todo-impl.md` once reputation-market data exists to size it for
    /// real.
    pub fn approx_value(&self) -> f64 {
        match self {
            QuantAmount::Bucket { index } => 2f64.powi(*index as i32),
            QuantAmount::Exact { quants } => *quants,
        }
    }
}

/// The one event requiring atomic dual-sign (§4): `payload_hash` is the
/// hash both parties agreed to and independently signed; either party can
/// publish once both signatures are collected, and anyone can verify both
/// against `issuer_pubkey`/`subject_pubkey` without trusting the publisher.
/// issuer = counterparty (payer), subject = worker (payee) — same roles as
/// the VC schema in `crate::vc`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditIssuance {
    pub completion_event_id: String,
    pub payload_hash: String,
    pub amount: QuantAmount,
    pub issuer_sig: String,
    pub subject_sig: String,
}

impl CreditIssuance {
    /// What both parties actually sign in the two-phase exchange (§4):
    /// everything about the issuance except the two signatures
    /// themselves. Both `issuer_sig` and `subject_sig` must be valid
    /// BIP-340 signatures over this same hash for the issuance to be
    /// honored — see `qw_protocol::contract::verify_credit_issuance`.
    pub fn payload_hash(completion_event_id: &str, amount: &QuantAmount) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(
            serde_json::json!([completion_event_id, amount])
                .to_string()
                .as_bytes(),
        );
        hasher.finalize().into()
    }
}

pub fn credit_issuance(
    issuer_pubkey_hex: &str,
    subject_pubkey_hex: &str,
    completion_event_id_hex: &str,
    issuance: &CreditIssuance,
) -> UnsignedEvent {
    let tags = vec![p_tag(subject_pubkey_hex), e_tag(completion_event_id_hex)];
    UnsignedEvent::new(
        issuer_pubkey_hex,
        KIND_CREDIT_ISSUANCE,
        tags,
        serde_json::to_string(issuance).expect("CreditIssuance serializes"),
    )
}

/// A self-assessed proficiency on one skill (NIP-QW03). A **claim**, never
/// evidence — a reader weighs it against the countersigned history (§5),
/// and `expert` from a key with no completed contracts still scores
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillLevel {
    Beginner,
    Intermediate,
    Senior,
    Expert,
}

impl SkillLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Beginner => "beginner",
            Self::Intermediate => "intermediate",
            Self::Senior => "senior",
            Self::Expert => "expert",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "beginner" => Some(Self::Beginner),
            "intermediate" | "mid" => Some(Self::Intermediate),
            "senior" => Some(Self::Senior),
            "expert" => Some(Self::Expert),
            _ => None,
        }
    }
}

/// Where a declared tag came from. Both are self-asserted; `CommitAnalysis`
/// (the client's own `qw_node::bootstrap` reading of a repo's history) is
/// a stronger *provenance*, not third-party proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SkillSource {
    /// Typed by the holder.
    #[default]
    #[serde(rename = "self")]
    SelfTyped,
    /// Suggested by commit-history analysis, then signed by the holder.
    #[serde(rename = "commit-analysis")]
    CommitAnalysis,
}

impl SkillSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SelfTyped => "self",
            Self::CommitAnalysis => "commit-analysis",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "self" => Some(Self::SelfTyped),
            "commit-analysis" => Some(Self::CommitAnalysis),
            _ => None,
        }
    }
}

/// A link to an external profile — GitHub, a personal site, LinkedIn.
/// **Unverified by the protocol**: a reader may go and check it. It is an
/// alternative signal that matters most before a first countersigned
/// contract exists (`abstract.md` §"Skills Social Network"; §2 external
/// identity links).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalLink {
    /// A free label — `github`, `gitlab`, `linkedin`, `x`, `mastodon`,
    /// `website`, … A client offers common ones; any string is allowed.
    pub network: String,
    pub url: String,
}

/// A skill tag — a `taxonomy.yaml` leaf or a custom label — is shorter
/// than this many Unicode characters. A custom skill name is a label, not
/// a sentence (NIP-QW03).
pub const MAX_SKILL_TAG_LEN: usize = 80;

/// A profile carries fewer than this many skill tags — a focused
/// self-description, not a keyword dump (NIP-QW03).
pub const MAX_PROFILE_SKILLS: usize = 80;

/// Why a [`ProfileSkillTags`] is not well-formed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSkillTagsError {
    /// A skill tag is empty, or `MAX_SKILL_TAG_LEN` characters or longer.
    SkillTagLength(String),
    /// The profile carries `MAX_PROFILE_SKILLS` or more skill tags.
    TooManySkills(usize),
}

impl fmt::Display for ProfileSkillTagsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileSkillTagsError::SkillTagLength(tag) => write!(
                f,
                "skill tag must be 1..{MAX_SKILL_TAG_LEN} characters: {tag:?}"
            ),
            ProfileSkillTagsError::TooManySkills(n) => write!(
                f,
                "a profile carries fewer than {MAX_PROFILE_SKILLS} skill tags, got {n}"
            ),
        }
    }
}

impl std::error::Error for ProfileSkillTagsError {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileSkillTags {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Leaf tags from `taxonomy.yaml`, e.g. `it/backend/languages#rust`, or
    /// a custom label. Each `< MAX_SKILL_TAG_LEN` characters and fewer than
    /// `MAX_PROFILE_SKILLS` of them — see [`ProfileSkillTags::validate`].
    pub skill_tags: Vec<String>,
    /// Self-assessed level, `tag -> level`. A tag absent from the map
    /// states no level. Additive — a pre-2026-09 profile has none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skill_levels: BTreeMap<String, SkillLevel>,
    /// Provenance, `tag -> source`. A tag absent from the map is
    /// [`SkillSource::SelfTyped`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skill_sources: BTreeMap<String, SkillSource>,
    /// External-network links, unverified (see [`ExternalLink`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<ExternalLink>,
}

impl ProfileSkillTags {
    /// Structural limits (NIP-QW03): every skill tag is `1..MAX_SKILL_TAG_LEN`
    /// characters, and there are fewer than `MAX_PROFILE_SKILLS` of them. A
    /// conforming client MUST call this before signing; a reader SHOULD
    /// reject an event whose content fails it.
    pub fn validate(&self) -> Result<(), ProfileSkillTagsError> {
        if self.skill_tags.len() >= MAX_PROFILE_SKILLS {
            return Err(ProfileSkillTagsError::TooManySkills(self.skill_tags.len()));
        }
        for tag in &self.skill_tags {
            let len = tag.chars().count();
            if len == 0 || len >= MAX_SKILL_TAG_LEN {
                return Err(ProfileSkillTagsError::SkillTagLength(tag.clone()));
            }
        }
        Ok(())
    }
}

/// Build a profile event ([`KIND_PROFILE`], replaceable). `revision` is
/// author-monotonic: pass `latest_seen_revision + 1`. A reader breaks a
/// `created_at` tie — and defends against a stale replica's fast clock —
/// with [`Event::revision`], reading the `["revision", n]` tag this
/// writes (NIP-QW12, NIP-QW03). Callers SHOULD run
/// [`ProfileSkillTags::validate`] first.
pub fn profile_skill_tags(
    pubkey_hex: &str,
    revision: u64,
    profile: &ProfileSkillTags,
) -> UnsignedEvent {
    let mut tags: Vec<Tag> = vec![revision_tag(revision)];
    tags.extend(profile.skill_tags.iter().map(t_tag));
    tags.extend(profile.links.iter().map(|l| r_tag(&l.url)));
    UnsignedEvent::new(
        pubkey_hex,
        KIND_PROFILE,
        tags,
        serde_json::to_string(profile).expect("ProfileSkillTags serializes"),
    )
}

/// Reply / audit request / audit opinion, per the FAQ's dispute table.
/// Attaches after the fact; never mutates the original signed contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "annotation_type", rename_all = "snake_case")]
pub enum DisputeAnnotation {
    /// Signed by the party being criticized. No score effect.
    Reply { body: String },
    /// Signed by either party. Marks the record "under review".
    AuditRequest { body: String },
    /// Signed by a third-party auditor; weight proportional to the
    /// auditor's own standing. The auditor stakes reputation — this
    /// opinion attaches to the auditor's own record too.
    AuditOpinion { body: String, outcome: AuditOutcome },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    FavorsClient,
    FavorsWorker,
    Split,
    Inconclusive,
}

pub fn dispute_annotation(
    author_pubkey_hex: &str,
    target_event_id_hex: &str,
    annotation: &DisputeAnnotation,
) -> UnsignedEvent {
    let tags = vec![e_tag(target_event_id_hex)];
    UnsignedEvent::new(
        author_pubkey_hex,
        KIND_DISPUTE_ANNOTATION,
        tags,
        serde_json::to_string(annotation).expect("DisputeAnnotation serializes"),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CascadeBlockFlag {
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_event_id: Option<String>,
}

/// Any WoT member may flag `target_pubkey_hex` (§0.5).
pub fn cascade_block_flag(
    flagger_pubkey_hex: &str,
    target_pubkey_hex: &str,
    flag: &CascadeBlockFlag,
) -> UnsignedEvent {
    let tags = vec![p_tag(target_pubkey_hex)];
    UnsignedEvent::new(
        flagger_pubkey_hex,
        KIND_CASCADE_BLOCK_FLAG,
        tags,
        serde_json::to_string(flag).expect("CascadeBlockFlag serializes"),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CascadeBlockRecord {
    /// Hops from the originally flagged signer at the time this vouch was
    /// published (§0.5: auto-cascade only applies at distance 1).
    pub distance: u8,
}

/// "I also block X, sourced from Y" — a node re-publishing its own vouch
/// once its local policy accepts a block signal (§6). This is what makes
/// cascade propagation social rather than a central blocklist: there is no
/// authority that enumerates every blocked account, only a chain of these.
pub fn cascade_block_record(
    voucher_pubkey_hex: &str,
    blocked_pubkey_hex: &str,
    sourced_from_event_id_hex: &str,
    record: &CascadeBlockRecord,
) -> UnsignedEvent {
    let tags = vec![
        p_tag(blocked_pubkey_hex),
        e_tag_marked(sourced_from_event_id_hex, "cascade-source"),
    ];
    UnsignedEvent::new(
        voucher_pubkey_hex,
        KIND_CASCADE_BLOCK_RECORD,
        tags,
        serde_json::to_string(record).expect("CascadeBlockRecord serializes"),
    )
}

/// §3 referral-query prototype. Privacy model (FAQ §6 "Who sees the
/// query?"): the true requester's identity is revealed only to their
/// direct contact (hop 1); every event from hop 1 onward is signed by the
/// *relaying* node and never references the requester's private ask, so
/// walking the `referral-hop` chain backward from any later hop
/// terminates at hop 1, never at the requester. `query_id` (not any event
/// id) is what correlates every hop of one logical query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillQuery {
    pub query_id: String,
    pub skill_tag: String,
    /// Hops already traveled *before* this event (0 for hop 1's own
    /// chain-head forward — hop 1 is the first hop past the requester).
    pub hops_from_origin: u8,
    /// Set once by the requester, carried unchanged by every hop, so any
    /// relay can compute its own remaining budget without a coordinator.
    pub max_hops: u8,
}

/// Build a hop's forward event. `prior_hop_event_id_hex` is `None` only
/// for hop 1's chain-head forward — it must not reference the requester's
/// private ask. Every later hop references the specific forward event it
/// received, via a `"referral-hop"`-marked `e` tag; that chain is the
/// path a receiver can vouch-walk ("2 hops via Anna").
pub fn skill_query(
    relayer_pubkey_hex: &str,
    prior_hop_event_id_hex: Option<&str>,
    query: &SkillQuery,
) -> UnsignedEvent {
    let mut tags = vec![t_tag(query.skill_tag.clone())];
    if let Some(prior) = prior_hop_event_id_hex {
        tags.push(e_tag_marked(prior, "referral-hop"));
    }
    UnsignedEvent::new(
        relayer_pubkey_hex,
        KIND_SKILL_QUERY,
        tags,
        serde_json::to_string(query).expect("SkillQuery serializes"),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillAnswer {
    pub query_id: String,
    /// The node that actually has the matching skill. Fixed at the
    /// moment of matching and carried unchanged through every relay hop
    /// back to the requester — the event's own `pubkey` field changes at
    /// each hop (whoever is currently vouching this leg), so it cannot be
    /// used to recover who originally matched; this field is what can.
    pub responder_pubkey: String,
    pub matched_skill_tag: String,
    /// Path length from hop 1 to the responder (the matching query
    /// event's `hops_from_origin`, plus this hop).
    pub hops: u8,
    /// The responder's own current profile event (kind 10020,
    /// [`KIND_PROFILE`]), attached so a requester who is **not** a contact
    /// can view what they found — the responder's other skills, display
    /// name, external links — not just `matched_skill_tag`. Set by the
    /// responder on the leg it signs; relay hops carry it through
    /// unchanged. Not a broadcast: it travels only along the vouched relay
    /// path of a query that matched. A reader **MUST** re-verify it and
    /// check `profile.pubkey == responder_pubkey` before reading a byte,
    /// and it changes nothing about trust (§5), which reads countersigned
    /// work alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Event>,
}

/// The matching node's attestation, referencing the specific forward
/// event it matched on. It is addressed to the immediate upstream hop,
/// never directly to the requester (whose identity the responder, by
/// design, never sees) — delivery back to the requester happens hop by
/// hop along the relay chain (`qw_node`'s local routing table, not a
/// signed protocol step), each hop re-signing with `signer_pubkey_hex` as
/// its own vouch while `answer.responder_pubkey` stays fixed.
pub fn skill_answer(
    signer_pubkey_hex: &str,
    upstream_pubkey_hex: &str,
    matched_event_id_hex: &str,
    answer: &SkillAnswer,
) -> UnsignedEvent {
    let tags = vec![
        p_tag(upstream_pubkey_hex),
        e_tag_marked(matched_event_id_hex, "referral-hop"),
    ];
    UnsignedEvent::new(
        signer_pubkey_hex,
        KIND_SKILL_ANSWER,
        tags,
        serde_json::to_string(answer).expect("SkillAnswer serializes"),
    )
}

/// `Introduction::via` for an edge minted by following a public invite
/// link (NIP-QW07 "Public self-introduction"). Nobody vouched for anyone
/// here — the publisher posted a link and a stranger followed it — so
/// cascade block skips these edges (`crate::cascade`).
pub const VIA_PUBLIC_LINK: &str = "public-link";

/// A contact-graph operation, not a contract one (`abstract.md` "Basic Use
/// Cases" §Introduce) — introducing a *job* is a `JobOffer`, covered
/// above. Three shapes share this kind: a self-introduction
/// (`subject_pubkey == event.pubkey`, introducing the signer to
/// `recipient_pubkey`), a mutual introduction (`subject_pubkey` is a
/// third party — one of the signer's own contacts — being introduced to
/// `recipient_pubkey`, another of the signer's own contacts), and a
/// self-introduction carrying [`VIA_PUBLIC_LINK`], which is the same
/// shape as the first but generated by following a published link
/// instead of being addressed to someone in particular.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Introduction {
    pub subject_pubkey: String,
    /// The chain of connections linking `subject_pubkey` to
    /// `recipient_pubkey`, oldest hop first, *not* including the signer's
    /// own hop (that's `event.pubkey`) — e.g. for the signer vouching for
    /// someone two hops out on their own side, the pubkeys in between.
    /// Empty for a direct (one-hop) introduction.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// How the edge came to exist. `None` for an ordinary introduction —
    /// someone chose to make it, one person at a time. [`VIA_PUBLIC_LINK`]
    /// for one minted by a stranger following a published invite link,
    /// which asserts reachability and nothing else.
    ///
    /// Kept as a free-form `Option<String>` rather than an enum so an
    /// unknown future value round-trips through a client that predates it
    /// instead of failing to parse — the same reason `chain` and `note`
    /// are `skip_serializing_if`. Anything that is not exactly
    /// [`VIA_PUBLIC_LINK`] is treated as an ordinary, vouched edge, so a
    /// new marker can never silently *weaken* a cascade.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

impl Introduction {
    /// A self-introduction produced by following a public invite link.
    pub fn public_link(subject_pubkey: impl Into<String>) -> Self {
        Self {
            subject_pubkey: subject_pubkey.into(),
            chain: Vec::new(),
            note: None,
            via: Some(VIA_PUBLIC_LINK.to_string()),
        }
    }

    /// Whether this edge was minted by a public invite link, and so
    /// carries no vouch from anyone.
    pub fn is_public_link(&self) -> bool {
        self.via.as_deref() == Some(VIA_PUBLIC_LINK)
    }
}

/// Signed and therefore attributable — the introducer's reputation is
/// behind it. Accepting one (adding `subject_pubkey` as a contact) is a
/// local decision by the recipient, not itself a signed protocol step;
/// the resulting edge asserts acquaintance, not competence — only
/// completed, countersigned work (NIP-QW01/QW02) carries trust in a
/// domain (§5).
pub fn introduction(
    introducer_pubkey_hex: &str,
    recipient_pubkey_hex: &str,
    intro: &Introduction,
) -> UnsignedEvent {
    let tags = vec![p_tag(recipient_pubkey_hex)];
    UnsignedEvent::new(
        introducer_pubkey_hex,
        KIND_INTRODUCTION,
        tags,
        serde_json::to_string(intro).expect("Introduction serializes"),
    )
}

/// Scope for a requested work history — empty `skill_tags` means all
/// domains; `since`/`until` bound the time window (unix seconds,
/// inclusive; `None` = unbounded on that side).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryRequest {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub skill_tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
}

pub fn history_request(
    requester_pubkey_hex: &str,
    contact_pubkey_hex: &str,
    request: &HistoryRequest,
) -> UnsignedEvent {
    let tags = vec![p_tag(contact_pubkey_hex)];
    UnsignedEvent::new(
        requester_pubkey_hex,
        KIND_HISTORY_REQUEST,
        tags,
        serde_json::to_string(request).expect("HistoryRequest serializes"),
    )
}

/// A signed, filtered pointer into the responder's own history: which
/// already-signed, already-dual-indexed records (job completions, credit
/// issuances) fall within the requested scope. The response doesn't
/// re-attest to their content — the requester independently fetches and
/// verifies each referenced id (`Event::verify`, `crate::dual_index`) —
/// it only attests to *which* records the responder is choosing to
/// disclose. `abstract.md`'s "the recipient may verify the signature and
/// check for omissions" means checking this list against whatever the
/// requester can independently see elsewhere, not a property this event
/// proves on its own: a responder can always choose to omit an in-scope
/// record without saying so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryResponse {
    pub record_event_ids: Vec<String>,
}

pub fn history_response(
    responder_pubkey_hex: &str,
    requester_pubkey_hex: &str,
    request_event_id_hex: &str,
    response: &HistoryResponse,
) -> UnsignedEvent {
    let tags = vec![p_tag(requester_pubkey_hex), e_tag(request_event_id_hex)];
    UnsignedEvent::new(
        responder_pubkey_hex,
        KIND_HISTORY_RESPONSE,
        tags,
        serde_json::to_string(response).expect("HistoryResponse serializes"),
    )
}

/// The account holder's advance configuration for controller-key
/// recovery — "quorum size and membership are the account holder's own
/// configuration, set in advance" (FAQ). Published (and republished to
/// change it) by the controller itself; not itself protected by a
/// quorum — see `qw_protocol::recovery`'s module docs for the resulting
/// bootstrapping/dispute limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryPolicy {
    /// M in "M-of-N".
    pub quorum_threshold: u8,
    /// N candidate signers (hex pubkeys) an amendment may draw from.
    pub trusted_pubkeys: Vec<String>,
}

pub fn recovery_policy(controller_pubkey_hex: &str, policy: &RecoveryPolicy) -> UnsignedEvent {
    UnsignedEvent::new(
        controller_pubkey_hex,
        KIND_RECOVERY_POLICY,
        vec![],
        serde_json::to_string(policy).expect("RecoveryPolicy serializes"),
    )
}

/// One quorum member's countersignature over a
/// [`PersonRecordAmendment::payload_hash`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuorumSig {
    pub signer_pubkey: String,
    pub sig: String,
}

/// Controller key rotation/recovery (FAQ "What happens when a signing key
/// is lost or compromised?"): publishes `new_controller_pubkey` as
/// continuation of `account_id` (the account's genesis controller pubkey
/// — a permanent anchor, since `did:key` itself can't rotate), revoking
/// `revoked_pubkey` from `effective_at`. Revocation is **not**
/// retroactive — signatures from `revoked_pubkey` before `effective_at`
/// stay valid. A signature under a revoked key *after* `effective_at`
/// must be surfaced as an alert by any verifier, never silently dropped —
/// it's the strongest available evidence the key is in hostile hands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonRecordAmendment {
    pub account_id: String,
    pub revoked_pubkey: String,
    pub new_controller_pubkey: String,
    pub effective_at: u64,
    pub quorum_sigs: Vec<QuorumSig>,
}

impl PersonRecordAmendment {
    /// What each quorum member actually signs — everything about the
    /// amendment except the signatures themselves.
    pub fn payload_hash(
        account_id: &str,
        revoked_pubkey: &str,
        new_controller_pubkey: &str,
        effective_at: u64,
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(
            serde_json::json!([
                account_id,
                revoked_pubkey,
                new_controller_pubkey,
                effective_at
            ])
            .to_string()
            .as_bytes(),
        );
        hasher.finalize().into()
    }
}

/// `publisher_pubkey_hex` is whoever actually broadcasts this (may or may
/// not be one of the quorum signers, or the new/old controller) — the
/// event's own NIP-01 signature only proves who published it; consent is
/// carried entirely by `amendment.quorum_sigs`, verified independently of
/// the publisher via `qw_protocol::recovery::verify_amendment`.
pub fn person_record_amendment(
    publisher_pubkey_hex: &str,
    amendment: &PersonRecordAmendment,
) -> UnsignedEvent {
    let tags = vec![vec!["account".to_string(), amendment.account_id.clone()]];
    UnsignedEvent::new(
        publisher_pubkey_hex,
        KIND_PERSON_RECORD_AMENDMENT,
        tags,
        serde_json::to_string(amendment).expect("PersonRecordAmendment serializes"),
    )
}

/// One device-key delegation, or — with `revoked_at` set — its
/// revocation (NIP-QW09 §"Device subkeys"). Unlike a controller
/// amendment, this is signed by the **current controller** alone (resolve
/// it with [`crate::recovery::controller_at`]), no quorum. Revocation is
/// **not** retroactive: a device key's signatures before `revoked_at`
/// stay valid; one after it is an alert
/// ([`crate::recovery::device_authority`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceSubkey {
    pub device_pubkey: String,
    /// A human label for the device — "pixel-8 / vlad", "qw-web home box".
    pub label: String,
    /// From when this device key may sign for the account (unix seconds).
    pub valid_from: u64,
    /// Set (in a second 9082 for the same `device_pubkey`) to revoke.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<u64>,
}

/// `controller_pubkey_hex` must be the account's current controller — the
/// signer of the resulting event. `account_id` is the account's genesis
/// controller pubkey, the permanent anchor. Tags: `["p", device_pubkey]`,
/// `["account", account_id]`.
pub fn device_subkey(
    controller_pubkey_hex: &str,
    account_id: &str,
    subkey: &DeviceSubkey,
) -> UnsignedEvent {
    let tags = vec![
        p_tag(subkey.device_pubkey.clone()),
        vec!["account".to_string(), account_id.to_string()],
    ];
    UnsignedEvent::new(
        controller_pubkey_hex,
        KIND_DEVICE_SUBKEY,
        tags,
        serde_json::to_string(subkey).expect("DeviceSubkey serializes"),
    )
}

/// A coordination server's answer to a trust-graph query (§8): "server
/// must never be the only source of truth for a result it returns" —
/// `edge_event_ids` are the real `CreditIssuance` ids
/// (`qw_protocol::trust::TrustPath`) forming the path, in order from
/// requester to target, so the requester can spot-check by fetching any
/// or all of them directly from relays and re-verifying, rather than
/// trusting the server's `score`/`hops` fields blindly. Signed by the
/// server's own identity — a bad or lying server accrues visible,
/// checkable reputation damage the same way any participant would.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainCalculationResult {
    pub target_pubkey: String,
    pub hops: u8,
    pub edge_event_ids: Vec<String>,
    pub score: f64,
}

pub fn chain_calculation_result(
    server_pubkey_hex: &str,
    requester_pubkey_hex: &str,
    result: &ChainCalculationResult,
) -> UnsignedEvent {
    let tags = vec![p_tag(requester_pubkey_hex)];
    UnsignedEvent::new(
        server_pubkey_hex,
        KIND_CHAIN_CALCULATION_RESULT,
        tags,
        serde_json::to_string(result).expect("ChainCalculationResult serializes"),
    )
}

/// Which side of the board a listing is on — mirrors a classifieds
/// board's "offered" vs. "wanted" split.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingType {
    Offering,
    Seeking,
}

/// A public, **undirected** self-advertisement (§8 "offline bulletin
/// board"): unlike `JobOffer` (NIP-QW01, addressed to one specific
/// worker) or `ProfileSkillTags` (NIP-QW03, a standing self-description),
/// this is a browsable, time-scoped posting — "I'm available for X" or
/// "I need X" — meant to be discovered by someone who doesn't know the
/// poster's pubkey in advance, the way a Craigslist post works: the two
/// sides don't need to be online at the same time, only for the server
/// hosting the board to be reachable when each of them happens to be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BulletinListing {
    pub listing_type: ListingType,
    pub skill_tags: Vec<String>,
    pub description: String,
    /// Unix seconds; a board should stop surfacing this listing after
    /// this point. `None` = no expiry set by the poster (a board
    /// operator may still enforce its own retention limit — see
    /// `todo-impl.md` §8's note that usage limits/monetization here are
    /// left to the operator, not fixed by this NIP).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

/// No `p` tag — a listing has no addressed counterparty yet, that's the
/// entire point. `t` tags carry `skill_tags` for board-side filtering,
/// same convention as every other tagged kind.
pub fn bulletin_listing(author_pubkey_hex: &str, listing: &BulletinListing) -> UnsignedEvent {
    let tags: Vec<Tag> = listing.skill_tags.iter().map(t_tag).collect();
    UnsignedEvent::new(
        author_pubkey_hex,
        KIND_BULLETIN_LISTING,
        tags,
        serde_json::to_string(listing).expect("BulletinListing serializes"),
    )
}

// --- broadcast propagation (NIP-QW14) ---

/// Which kind of thing a [`Broadcast`] carries. Each has its own
/// propagation policy (hop cap, age, size, score floor, …) — a client
/// concern, not on the wire — see NIP-QW14 §4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BroadcastKind {
    /// An open job call — a `JobOffer` with no addressed worker.
    Proposal,
    /// The wanted side — "I need work in X".
    Demand,
    /// A push of a (replaceable) profile toward tag-similar contacts.
    Profile,
    /// An update. Domain-agnostic; travels furthest and cheapest.
    News,
    /// A third-party opinion about someone's work in a domain. A reach
    /// signal, not evidence — the reviewer stakes their own standing.
    Review,
}

impl BroadcastKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposal => "proposal",
            Self::Demand => "demand",
            Self::Profile => "profile",
            Self::News => "news",
            Self::Review => "review",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "proposal" => Some(Self::Proposal),
            "demand" => Some(Self::Demand),
            "profile" => Some(Self::Profile),
            "news" => Some(Self::News),
            "review" => Some(Self::Review),
            _ => None,
        }
    }
}

/// An originator-signed message meant to be pushed outward hop by hop
/// (NIP-QW14 §1). Immutable in flight: every mutable fact — who relayed
/// it, how far, what each hop scores the originator — lives in the
/// [`HopRating`] events that reference this one, never here. The event
/// `id` is the dedup key (FidoNet's MSGID).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Broadcast {
    #[serde(rename = "type")]
    pub kind: BroadcastKind,
    /// Shape depends on `kind` — NIP-QW14 §3.
    pub body: serde_json::Value,
    /// Hard stop: no node relays or surfaces the envelope past this, no
    /// matter what its policy allows.
    pub expires_at: u64,
}

/// `["broadcast", <type>]` echo selector, one `["t", <tag>]` per routing
/// tag, and `["expiration", <expires_at>]` (NIP-40).
pub fn broadcast(
    originator_pubkey_hex: &str,
    envelope: &Broadcast,
    routing_tags: &[String],
) -> UnsignedEvent {
    let mut tags: Vec<Tag> = vec![vec![
        "broadcast".to_string(),
        envelope.kind.as_str().to_string(),
    ]];
    tags.extend(routing_tags.iter().map(t_tag));
    tags.push(vec!["expiration".to_string(), envelope.expires_at.to_string()]);
    UnsignedEvent::new(
        originator_pubkey_hex,
        KIND_BROADCAST,
        tags,
        serde_json::to_string(envelope).expect("Broadcast serializes"),
    )
}

/// A NIP-QW13 reputation reading, or `"unknown-risk"` when no verified
/// path reaches the subject in the relevant domain (NIP-QW13 §"score").
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HopScore {
    Score(f64),
    #[serde(with = "unknown_risk")]
    UnknownRisk,
}

mod unknown_risk {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str("unknown-risk")
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<(), D::Error> {
        let s = String::deserialize(d)?;
        if s == "unknown-risk" {
            Ok(())
        } else {
            Err(serde::de::Error::custom("expected \"unknown-risk\""))
        }
    }
}

/// One hop's signed, time-boxed reputation reading of some pubkey in a
/// domain (NIP-QW14 §2). **Envelope-independent**: a hop computes one per
/// `(subject_pubkey, domain)` and re-attaches this same signed event to
/// every broadcast it relays from that originator until `valid_until` —
/// which is the point, since a signature-per-envelope defeats the caching.
/// The *path* a given broadcast took is carried separately (§2), not here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HopRating {
    pub subject_pubkey: String,
    /// `sector/domain`, or `""` for a non-domain-scoped broadcast.
    pub domain: String,
    pub score: HopScore,
    pub computed_at: u64,
    /// `computed_at + hop_rating_ttl` for the relayed type. Past this the
    /// rating is dead — a verifier ignores it, a relayer recomputes.
    pub valid_until: u64,
}

impl HopRating {
    pub fn is_valid_at(&self, now: u64) -> bool {
        now < self.valid_until
    }
}

/// `["p", <subject>]`, `["expiration", <valid_until>]`.
pub fn hop_rating(hop_pubkey_hex: &str, rating: &HopRating) -> UnsignedEvent {
    let tags = vec![
        p_tag(&rating.subject_pubkey),
        vec!["expiration".to_string(), rating.valid_until.to_string()],
    ];
    UnsignedEvent::new(
        hop_pubkey_hex,
        KIND_HOP_RATING,
        tags,
        serde_json::to_string(rating).expect("HopRating serializes"),
    )
}

// --- skill recognition, by a bureau (NIP-QW15) ---

/// A subject asks a bureau to corroborate skills against their QW work
/// record (and, with `append_public_profile`, a parse of profiles the
/// bureau has verified they control). `skill_tags` may be empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecognitionRequest {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub skill_tags: Vec<String>,
    #[serde(default)]
    pub append_public_profile: bool,
}

/// How strong the evidence behind one endorsed / suggested skill is —
/// mirrors the NIP-QW03 evidence classes. `QwEntitled` is a contract
/// settled through a credit issuance (NIP-QW02); `SideSettled` a closed
/// contract with nothing on the ledger (NIP-QW01 kind 9006); `Profile` an
/// external-profile parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTier {
    QwEntitled,
    SideSettled,
    Profile,
}

/// One skill the bureau will endorse or suggest, with a plain-language
/// note on what backs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Corroboration {
    pub skill_tag: String,
    pub corroborated_by: String,
    pub tier: EvidenceTier,
}

/// The bureau's signed reply (kind 9093). Corroboration, not
/// certification: it says what it checked and by what method, never "this
/// person is legit".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Recognition {
    pub subject_pubkey: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub endorsements: Vec<Corroboration>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub suggestions: Vec<Corroboration>,
    /// Listed tags the bureau found no evidence for.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub unverified: Vec<String>,
    /// Profile findings that mapped to no taxonomy tag.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub unmapped: Vec<String>,
    pub checked_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

/// Build a `RecognitionRequest` event addressed to `bureau_pubkey_hex`.
/// The requester is the subject — recognition is always about yourself.
pub fn recognition_request(
    subject_pubkey_hex: &str,
    bureau_pubkey_hex: &str,
    request: &RecognitionRequest,
) -> UnsignedEvent {
    UnsignedEvent::new(
        subject_pubkey_hex,
        KIND_RECOGNITION_REQUEST,
        vec![p_tag(bureau_pubkey_hex)],
        serde_json::to_string(request).expect("RecognitionRequest serializes"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    #[test]
    fn job_offer_round_trips_content() {
        let offer = JobOffer {
            skill_tags: vec!["it/backend/languages#rust".to_string()],
            hours: 8.0,
            rate: 40.0,
            ko: Some(1.1),
            km: None,
            terms: "sprint 12 backend work".to_string(),
        };
        let client = Identity::generate();
        let worker = Identity::generate();
        let unsigned = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer,
        );
        let event = unsigned.sign(&client);
        assert!(event.verify().is_ok());
        assert_eq!(event.kind, KIND_JOB_OFFER);
        assert_eq!(
            event.first_tag_value("p"),
            Some(worker.nostr_pubkey_hex().as_str())
        );

        let decoded: JobOffer = serde_json::from_str(&event.content).unwrap();
        assert_eq!(decoded, offer);
    }

    #[test]
    fn counteroffer_supersedes_the_offer_it_references() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer = JobOffer {
            skill_tags: vec!["it/backend/languages#rust".to_string()],
            hours: 8.0,
            rate: 40.0,
            ko: None,
            km: None,
            terms: "sprint 12 backend work".to_string(),
        };
        let offer_event = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer,
        )
        .sign(&client);

        let counter = JobOffer {
            rate: 55.0,
            ..offer
        };
        let counter_event = job_counteroffer(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer_event.id,
            &counter,
        )
        .sign(&worker);

        assert!(counter_event.verify().is_ok());
        assert_eq!(counter_event.kind, KIND_JOB_COUNTEROFFER);
        assert_eq!(
            counter_event.first_tag_value("e"),
            Some(offer_event.id.as_str())
        );
        assert_eq!(
            counter_event.first_tag_value("p"),
            Some(client.nostr_pubkey_hex().as_str())
        );

        let decoded: JobOffer = serde_json::from_str(&counter_event.content).unwrap();
        assert_eq!(decoded.rate, 55.0);
    }

    #[test]
    fn offer_from_introduction_links_the_invite_without_changing_the_content() {
        let offer = JobOffer {
            skill_tags: vec!["it/backend/languages#rust".to_string()],
            hours: 8.0,
            rate: 40.0,
            ko: None,
            km: None,
            terms: "the work the invite was about".to_string(),
        };
        let client = Identity::generate();
        let worker = Identity::generate();

        let plain = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer,
        );
        let linked = job_offer_from_introduction(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            "introeventid",
            &offer,
        );

        // Provenance is a marked `e` tag, never a content field — the
        // contract anchor logic must see an ordinary offer.
        assert_eq!(linked.content, plain.content);
        assert_eq!(linked.kind, KIND_JOB_OFFER);
        let event = linked.sign(&client);
        assert!(event.verify().is_ok());
        assert_eq!(
            event.first_tag_value("p"),
            Some(worker.nostr_pubkey_hex().as_str())
        );
        let intro_ref = event.tags.iter().find(|t| {
            t.first().map(String::as_str) == Some("e")
                && t.get(3).map(String::as_str) == Some("introduction")
        });
        assert_eq!(
            intro_ref.and_then(|t| t.get(1)).map(String::as_str),
            Some("introeventid")
        );

        let decoded: JobOffer = serde_json::from_str(&event.content).unwrap();
        assert_eq!(decoded, offer);
    }

    #[test]
    fn review_request_targets_a_milestone_or_completion() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let review = JobReviewRequest {
            feedback: Some("looks close, one nit".to_string()),
        };
        let event = job_review_request(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            "milestoneeventid",
            &review,
        )
        .sign(&client);

        assert!(event.verify().is_ok());
        assert_eq!(event.kind, KIND_JOB_REVIEW_REQUEST);
        assert_eq!(event.first_tag_value("e"), Some("milestoneeventid"));
    }

    #[test]
    fn side_settlement_anchors_to_the_offer_and_carries_the_marker() {
        let worker = Identity::generate();
        let client = Identity::generate();
        let event = side_settlement(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            "offereventid",
            &SideSettlement { note: Some("paid in cash".to_string()) },
        )
        .sign(&worker);

        assert!(event.verify().is_ok());
        assert_eq!(event.kind, KIND_SIDE_SETTLEMENT);
        assert_eq!(event.first_tag_value("e"), Some("offereventid"));
        assert_eq!(
            event.first_tag_value("p"),
            Some(client.nostr_pubkey_hex().as_str())
        );
        assert_eq!(event.first_tag_value("settlement"), Some("side"));

        let decoded: SideSettlement = serde_json::from_str(&event.content).unwrap();
        assert_eq!(decoded.note.as_deref(), Some("paid in cash"));

        // the empty settlement round-trips too (note is optional)
        let bare = side_settlement(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            "offereventid",
            &SideSettlement::default(),
        )
        .sign(&worker);
        assert_eq!(bare.content, "{}");
    }

    #[test]
    fn credit_issuance_payload_hash_is_stable_and_amount_sensitive() {
        let a = CreditIssuance::payload_hash("completion1", &QuantAmount::Bucket { index: 3 });
        let b = CreditIssuance::payload_hash("completion1", &QuantAmount::Bucket { index: 3 });
        let c = CreditIssuance::payload_hash("completion1", &QuantAmount::Bucket { index: 4 });
        assert_eq!(a, b, "same inputs must hash the same");
        assert_ne!(a, c, "a different amount must change the hash");
    }

    #[test]
    fn quant_amount_approx_value_is_monotonic_and_exact_passes_through() {
        let low = QuantAmount::Bucket { index: 2 }.approx_value();
        let high = QuantAmount::Bucket { index: 5 }.approx_value();
        assert!(
            high > low,
            "a higher bucket must approximate a larger value"
        );
        assert_eq!(QuantAmount::Exact { quants: 42.5 }.approx_value(), 42.5);
    }

    #[test]
    fn quant_amount_bucket_is_the_representable_default() {
        let bucketed = QuantAmount::Bucket { index: 5 };
        let json = serde_json::to_string(&bucketed).unwrap();
        assert!(json.contains("\"bucket\""));
        let back: QuantAmount = serde_json::from_str(&json).unwrap();
        assert_eq!(back, bucketed);
    }

    #[test]
    fn dispute_annotation_variants_round_trip() {
        let opinion = DisputeAnnotation::AuditOpinion {
            body: "reviewed both sides".to_string(),
            outcome: AuditOutcome::Split,
        };
        let json = serde_json::to_string(&opinion).unwrap();
        let back: DisputeAnnotation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, opinion);
    }

    #[test]
    fn hop1_skill_query_has_no_backreference_to_origin() {
        let hop1 = Identity::generate();
        let query = SkillQuery {
            query_id: "q1".to_string(),
            skill_tag: "it/backend/languages#rust".to_string(),
            hops_from_origin: 0,
            max_hops: 3,
        };
        let event = skill_query(&hop1.nostr_pubkey_hex(), None, &query).sign(&hop1);
        assert!(event.verify().is_ok());
        assert_eq!(event.kind, KIND_SKILL_QUERY);
        assert_eq!(
            event.first_tag_value("e"),
            None,
            "hop 1's chain-head must not reference the requester's private ask"
        );
    }

    #[test]
    fn later_hop_skill_query_references_prior_forward() {
        let hop2 = Identity::generate();
        let query = SkillQuery {
            query_id: "q1".to_string(),
            skill_tag: "it/backend/languages#rust".to_string(),
            hops_from_origin: 1,
            max_hops: 3,
        };
        let event = skill_query(&hop2.nostr_pubkey_hex(), Some("hop1eventid"), &query).sign(&hop2);
        assert_eq!(event.first_tag_value("e"), Some("hop1eventid"));
    }

    #[test]
    fn skill_answer_addresses_upstream_hop_not_the_requester() {
        let responder = Identity::generate();
        let upstream = Identity::generate();
        let answer = SkillAnswer {
            query_id: "q1".to_string(),
            responder_pubkey: responder.nostr_pubkey_hex(),
            matched_skill_tag: "it/backend/languages#rust".to_string(),
            hops: 2,
            profile: None,
        };
        let event = skill_answer(
            &responder.nostr_pubkey_hex(),
            &upstream.nostr_pubkey_hex(),
            "matchedeventid",
            &answer,
        )
        .sign(&responder);
        assert!(event.verify().is_ok());
        assert_eq!(
            event.first_tag_value("p"),
            Some(upstream.nostr_pubkey_hex().as_str())
        );
    }

    #[test]
    fn skill_answer_profile_is_optional_and_round_trips_whole() {
        let responder = Identity::generate();
        let bare = SkillAnswer {
            query_id: "q1".to_string(),
            responder_pubkey: responder.nostr_pubkey_hex(),
            matched_skill_tag: "it/backend/languages#rust".to_string(),
            hops: 1,
            profile: None,
        };
        // An absent profile stays off the wire — a matched contact who is
        // already known needs nothing extra attached.
        let json = serde_json::to_string(&bare).unwrap();
        assert!(!json.contains("profile"), "None serialises to nothing");
        let decoded: SkillAnswer = serde_json::from_str(&json).unwrap();
        assert!(decoded.profile.is_none());

        // A whole signed profile event rides along untouched and still
        // verifies after the round trip — this is what lets a non-contact
        // see the responder's full self-description (NIP-QW06).
        let prof = profile_skill_tags(
            &responder.nostr_pubkey_hex(),
            1,
            &ProfileSkillTags {
                display_name: Some("Dana".to_string()),
                skill_tags: vec![
                    "it/backend/languages#rust".to_string(),
                    "it/backend/languages#go".to_string(),
                ],
                ..Default::default()
            },
        )
        .sign(&responder);
        let with_profile = SkillAnswer {
            profile: Some(prof.clone()),
            ..bare
        };
        let back: SkillAnswer =
            serde_json::from_str(&serde_json::to_string(&with_profile).unwrap()).unwrap();
        let carried = back.profile.expect("profile survives the round trip");
        assert_eq!(carried.id, prof.id);
        assert!(carried.verify().is_ok());
        assert_eq!(carried.kind, KIND_PROFILE);
    }

    #[test]
    fn self_introduction_names_the_signer_as_subject() {
        let introducer = Identity::generate();
        let recipient = Identity::generate();
        let intro = Introduction {
            subject_pubkey: introducer.nostr_pubkey_hex(),
            chain: vec![],
            note: Some("we met at the meetup".to_string()),
            via: None,
        };
        let event = introduction(
            &introducer.nostr_pubkey_hex(),
            &recipient.nostr_pubkey_hex(),
            &intro,
        )
        .sign(&introducer);

        assert!(event.verify().is_ok());
        assert_eq!(event.kind, KIND_INTRODUCTION);
        assert_eq!(
            event.first_tag_value("p"),
            Some(recipient.nostr_pubkey_hex().as_str())
        );
        let decoded: Introduction = serde_json::from_str(&event.content).unwrap();
        assert_eq!(decoded.subject_pubkey, introducer.nostr_pubkey_hex());
    }

    #[test]
    fn public_link_introduction_marks_the_edge_and_stays_wire_compatible() {
        let follower = Identity::generate();
        let publisher = Identity::generate();
        let intro = Introduction::public_link(follower.nostr_pubkey_hex());
        assert!(intro.is_public_link());

        let event = introduction(
            &follower.nostr_pubkey_hex(),
            &publisher.nostr_pubkey_hex(),
            &intro,
        )
        .sign(&follower);
        assert!(event.verify().is_ok());
        assert!(event.content.contains(r#""via":"public-link""#));

        let decoded: Introduction = serde_json::from_str(&event.content).unwrap();
        assert_eq!(decoded, intro);

        // An ordinary introduction must not start carrying the field: an
        // older client parses `via` as absent, and absent means vouched.
        // Emitting `"via":null` instead would also change every existing
        // event's id, since the id is a hash over the serialized content.
        let ordinary = Introduction {
            subject_pubkey: follower.nostr_pubkey_hex(),
            chain: vec![],
            note: None,
            via: None,
        };
        let json = serde_json::to_string(&ordinary).unwrap();
        assert!(!json.contains("via"), "unexpected field in {json}");
        let reparsed: Introduction = serde_json::from_str(&json).unwrap();
        assert!(!reparsed.is_public_link());
    }

    #[test]
    fn mutual_introduction_names_a_third_party_subject() {
        let introducer = Identity::generate();
        let recipient = Identity::generate();
        let subject = Identity::generate();
        let intro = Introduction {
            subject_pubkey: subject.nostr_pubkey_hex(),
            chain: vec![],
            note: None,
            via: None,
        };
        let event = introduction(
            &introducer.nostr_pubkey_hex(),
            &recipient.nostr_pubkey_hex(),
            &intro,
        )
        .sign(&introducer);

        let decoded: Introduction = serde_json::from_str(&event.content).unwrap();
        assert_eq!(decoded.subject_pubkey, subject.nostr_pubkey_hex());
        assert_ne!(
            decoded.subject_pubkey, event.pubkey,
            "subject differs from the introducer for a mutual introduction"
        );
    }

    #[test]
    fn history_response_references_the_request_it_answers() {
        let requester = Identity::generate();
        let contact = Identity::generate();
        let request = HistoryRequest {
            skill_tags: vec!["it/backend/languages#rust".to_string()],
            since: Some(1_700_000_000),
            until: None,
        };
        let request_event = history_request(
            &requester.nostr_pubkey_hex(),
            &contact.nostr_pubkey_hex(),
            &request,
        )
        .sign(&requester);
        assert!(request_event.verify().is_ok());
        assert_eq!(request_event.kind, KIND_HISTORY_REQUEST);

        let response = HistoryResponse {
            record_event_ids: vec!["deadbeef".to_string(), "cafef00d".to_string()],
        };
        let response_event = history_response(
            &contact.nostr_pubkey_hex(),
            &requester.nostr_pubkey_hex(),
            &request_event.id,
            &response,
        )
        .sign(&contact);

        assert!(response_event.verify().is_ok());
        assert_eq!(response_event.kind, KIND_HISTORY_RESPONSE);
        assert_eq!(
            response_event.first_tag_value("e"),
            Some(request_event.id.as_str())
        );
        assert_eq!(
            response_event.first_tag_value("p"),
            Some(requester.nostr_pubkey_hex().as_str())
        );

        let decoded: HistoryResponse = serde_json::from_str(&response_event.content).unwrap();
        assert_eq!(decoded.record_event_ids.len(), 2);
    }

    #[test]
    fn person_record_amendment_event_shape_and_tag() {
        let genesis = Identity::generate();
        let new_controller = Identity::generate();
        let publisher = Identity::generate();

        let amendment = PersonRecordAmendment {
            account_id: genesis.nostr_pubkey_hex(),
            revoked_pubkey: genesis.nostr_pubkey_hex(),
            new_controller_pubkey: new_controller.nostr_pubkey_hex(),
            effective_at: 1_700_000_000,
            quorum_sigs: vec![],
        };
        let event =
            person_record_amendment(&publisher.nostr_pubkey_hex(), &amendment).sign(&publisher);

        assert!(event.verify().is_ok());
        assert_eq!(event.kind, KIND_PERSON_RECORD_AMENDMENT);
        assert_eq!(
            event.first_tag_value("account"),
            Some(genesis.nostr_pubkey_hex().as_str())
        );

        let decoded: PersonRecordAmendment = serde_json::from_str(&event.content).unwrap();
        assert_eq!(decoded, amendment);
    }

    #[test]
    fn device_subkey_delegation_and_revocation_shape() {
        let controller = Identity::generate();
        let device = Identity::generate();
        let account_id = controller.nostr_pubkey_hex();

        let delegation = DeviceSubkey {
            device_pubkey: device.nostr_pubkey_hex(),
            label: "pixel-8 / vlad".to_string(),
            valid_from: 1_730_000_000,
            revoked_at: None,
        };
        let ev = device_subkey(&controller.nostr_pubkey_hex(), &account_id, &delegation)
            .sign(&controller);

        assert!(ev.verify().is_ok());
        assert_eq!(ev.kind, KIND_DEVICE_SUBKEY);
        assert_eq!(ev.first_tag_value("p"), Some(device.nostr_pubkey_hex().as_str()));
        assert_eq!(ev.first_tag_value("account"), Some(account_id.as_str()));
        // a delegation omits revoked_at entirely on the wire
        assert!(!ev.content.contains("revoked_at"));
        assert_eq!(
            serde_json::from_str::<DeviceSubkey>(&ev.content).unwrap(),
            delegation
        );

        let revocation = DeviceSubkey { revoked_at: Some(1_740_000_000), ..delegation };
        let rev_ev = device_subkey(&controller.nostr_pubkey_hex(), &account_id, &revocation)
            .sign(&controller);
        assert_eq!(
            serde_json::from_str::<DeviceSubkey>(&rev_ev.content).unwrap().revoked_at,
            Some(1_740_000_000)
        );
    }

    #[test]
    fn chain_calculation_result_addresses_the_requester() {
        let server = Identity::generate();
        let requester = Identity::generate();
        let target = Identity::generate();
        let result = ChainCalculationResult {
            target_pubkey: target.nostr_pubkey_hex(),
            hops: 2,
            edge_event_ids: vec!["e1".to_string(), "e2".to_string()],
            score: 0.75,
        };
        let event = chain_calculation_result(
            &server.nostr_pubkey_hex(),
            &requester.nostr_pubkey_hex(),
            &result,
        )
        .sign(&server);

        assert!(event.verify().is_ok());
        assert_eq!(event.kind, KIND_CHAIN_CALCULATION_RESULT);
        assert_eq!(
            event.first_tag_value("p"),
            Some(requester.nostr_pubkey_hex().as_str())
        );
        let decoded: ChainCalculationResult = serde_json::from_str(&event.content).unwrap();
        assert_eq!(decoded, result);
    }

    #[test]
    fn bulletin_listing_is_undirected_and_carries_skill_tags() {
        let poster = Identity::generate();
        let listing = BulletinListing {
            listing_type: ListingType::Offering,
            skill_tags: vec![
                "it/backend/languages#rust".to_string(),
                "it/backend/languages#go".to_string(),
            ],
            description: "Rust/Go contractor, available evenings".to_string(),
            expires_at: Some(2_000_000_000),
        };
        let event = bulletin_listing(&poster.nostr_pubkey_hex(), &listing).sign(&poster);

        assert!(event.verify().is_ok());
        assert_eq!(event.kind, KIND_BULLETIN_LISTING);
        assert_eq!(
            event.first_tag_value("p"),
            None,
            "a listing must not be addressed to anyone"
        );
        let tags: Vec<&str> = event.tag_values("t").collect();
        assert_eq!(
            tags,
            vec!["it/backend/languages#rust", "it/backend/languages#go"]
        );

        let decoded: BulletinListing = serde_json::from_str(&event.content).unwrap();
        assert_eq!(decoded, listing);
    }

    #[test]
    fn profile_is_replaceable_and_carries_a_revision() {
        let me = Identity::generate();
        let profile = ProfileSkillTags {
            display_name: Some("vk".to_string()),
            skill_tags: vec![
                "it/backend/languages#rust".to_string(),
                "it/backend/frameworks#axum".to_string(),
            ],
            ..Default::default()
        };
        let event = profile_skill_tags(&me.nostr_pubkey_hex(), 3, &profile).sign(&me);

        assert!(event.verify().is_ok());
        assert_eq!(event.kind, KIND_PROFILE, "in the replaceable range, not 9020");
        assert_eq!(event.revision(), 3);
        assert_eq!(event.first_tag_value("revision"), Some("3"));
        assert_eq!(event.first_tag_value("p"), None, "a profile is not addressed");
        let tags: Vec<&str> = event.tag_values("t").collect();
        assert_eq!(
            tags,
            vec!["it/backend/languages#rust", "it/backend/frameworks#axum"]
        );
        let decoded: ProfileSkillTags = serde_json::from_str(&event.content).unwrap();
        assert_eq!(decoded, profile);
        assert!(decoded.validate().is_ok());
    }

    #[test]
    fn profile_skill_tag_limits() {
        let ok = ProfileSkillTags {
            skill_tags: vec!["it/backend/languages#rust".to_string(), "a".repeat(79)],
            ..Default::default()
        };
        assert!(ok.validate().is_ok());

        let long = ProfileSkillTags {
            skill_tags: vec!["x".repeat(MAX_SKILL_TAG_LEN)],
            ..Default::default()
        };
        assert_eq!(
            long.validate(),
            Err(ProfileSkillTagsError::SkillTagLength("x".repeat(MAX_SKILL_TAG_LEN)))
        );

        let empty_tag = ProfileSkillTags {
            skill_tags: vec![String::new()],
            ..Default::default()
        };
        assert!(matches!(
            empty_tag.validate(),
            Err(ProfileSkillTagsError::SkillTagLength(_))
        ));

        let too_many = ProfileSkillTags {
            skill_tags: (0..MAX_PROFILE_SKILLS).map(|i| format!("s{i}")).collect(),
            ..Default::default()
        };
        assert_eq!(
            too_many.validate(),
            Err(ProfileSkillTagsError::TooManySkills(MAX_PROFILE_SKILLS))
        );

        let just_under = ProfileSkillTags {
            skill_tags: (0..MAX_PROFILE_SKILLS - 1).map(|i| format!("s{i}")).collect(),
            ..Default::default()
        };
        assert!(just_under.validate().is_ok());
    }

    #[test]
    fn revision_defaults_to_zero_when_the_tag_is_absent_or_junk() {
        let me = Identity::generate();
        // No revision tag at all (a legacy 9020, or any other kind).
        let bare = UnsignedEvent::new(me.nostr_pubkey_hex(), KIND_PROFILE_SKILL_TAGS, vec![], "{}")
            .sign(&me);
        assert_eq!(bare.revision(), 0);
        // Present but not a number.
        let junk = UnsignedEvent::new(
            me.nostr_pubkey_hex(),
            KIND_PROFILE,
            vec![vec!["revision".to_string(), "soon".to_string()]],
            "{}",
        )
        .sign(&me);
        assert_eq!(junk.revision(), 0);
    }

    #[test]
    fn broadcast_envelope_carries_the_echo_selector_and_routing_tags() {
        let originator = Identity::generate();
        let envelope = Broadcast {
            kind: BroadcastKind::Proposal,
            body: serde_json::json!({ "terms": "sprint 12 backend", "hours": 8.0 }),
            expires_at: 1_737_000_000,
        };
        let event = broadcast(
            &originator.nostr_pubkey_hex(),
            &envelope,
            &["it/backend/languages#rust".to_string()],
        )
        .sign(&originator);

        assert!(event.verify().is_ok());
        assert_eq!(event.kind, KIND_BROADCAST);
        assert_eq!(event.first_tag_value("broadcast"), Some("proposal"));
        assert_eq!(
            event.first_tag_value("t"),
            Some("it/backend/languages#rust")
        );
        assert_eq!(event.first_tag_value("expiration"), Some("1737000000"));

        let decoded: Broadcast = serde_json::from_str(&event.content).unwrap();
        assert_eq!(decoded, envelope);
        assert_eq!(decoded.kind, BroadcastKind::Proposal);
    }

    #[test]
    fn hop_rating_is_envelope_independent_and_expires() {
        let hop = Identity::generate();
        let originator = Identity::generate();
        let rating = HopRating {
            subject_pubkey: originator.nostr_pubkey_hex(),
            domain: "it/backend".to_string(),
            score: HopScore::Score(1.15),
            computed_at: 1_736_400_000,
            valid_until: 1_736_660_000,
        };
        let event = hop_rating(&hop.nostr_pubkey_hex(), &rating).sign(&hop);

        assert!(event.verify().is_ok());
        assert_eq!(event.kind, KIND_HOP_RATING);
        assert_eq!(event.first_tag_value("e"), None, "not tied to one envelope");
        assert_eq!(
            event.first_tag_value("p"),
            Some(originator.nostr_pubkey_hex().as_str())
        );
        assert_eq!(event.first_tag_value("expiration"), Some("1736660000"));

        let decoded: HopRating = serde_json::from_str(&event.content).unwrap();
        assert_eq!(decoded, rating);
        assert!(decoded.is_valid_at(1_736_500_000));
        assert!(!decoded.is_valid_at(1_736_660_000));
    }

    #[test]
    fn hop_score_serializes_number_or_the_unknown_risk_string() {
        assert_eq!(
            serde_json::to_string(&HopScore::Score(0.9)).unwrap(),
            "0.9"
        );
        assert_eq!(
            serde_json::to_string(&HopScore::UnknownRisk).unwrap(),
            "\"unknown-risk\""
        );
        assert_eq!(
            serde_json::from_str::<HopScore>("\"unknown-risk\"").unwrap(),
            HopScore::UnknownRisk
        );
        assert_eq!(
            serde_json::from_str::<HopScore>("1.2").unwrap(),
            HopScore::Score(1.2)
        );
        assert!(serde_json::from_str::<HopScore>("\"nope\"").is_err());
    }

    #[test]
    fn broadcast_kind_round_trips_through_str() {
        for k in [
            BroadcastKind::Proposal,
            BroadcastKind::Demand,
            BroadcastKind::Profile,
            BroadcastKind::News,
            BroadcastKind::Review,
        ] {
            assert_eq!(BroadcastKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(BroadcastKind::parse("gossip"), None);
    }
}
