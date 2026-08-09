//! QW's custom Nostr event kinds. Numbered in the 9000-9099 block: NIP-01
//! reserves 1000<=kind<10000 for regular (stored, non-replaceable,
//! non-ephemeral) events, which matches every kind here — these are
//! permanent signed records, never superseded or expired by a relay.
//!
//! Each kind's `content` is JSON of the paired struct below. Full spec
//! (rationale, tag layout, worked examples) lives in `/protocol/nips/`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{e_tag, e_tag_marked, p_tag, t_tag, Tag, UnsignedEvent};

// --- job lifecycle (NIP-QW01) ---
pub const KIND_JOB_OFFER: u16 = 9000;
pub const KIND_JOB_ACCEPT: u16 = 9001;
pub const KIND_JOB_MILESTONE: u16 = 9002;
pub const KIND_JOB_COMPLETION: u16 = 9003;
pub const KIND_JOB_COUNTEROFFER: u16 = 9004;
pub const KIND_JOB_REVIEW_REQUEST: u16 = 9005;

// --- credit issuance (NIP-QW02) ---
pub const KIND_CREDIT_ISSUANCE: u16 = 9010;

// --- profile / skill tags (NIP-QW03) ---
pub const KIND_PROFILE_SKILL_TAGS: u16 = 9020;

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

// --- chain-calculation result (NIP-QW10) ---
pub const KIND_CHAIN_CALCULATION_RESULT: u16 = 9090;

// --- bulletin listing (NIP-QW11) ---
pub const KIND_BULLETIN_LISTING: u16 = 9091;

/// `Hours × Rate × ko × km` per abstract.md — `ko`/`km` may be omitted to
/// simplify negotiation.
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileSkillTags {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Leaf tags from `taxonomy.yaml`, e.g. `it/backend/languages#rust`.
    pub skill_tags: Vec<String>,
}

pub fn profile_skill_tags(pubkey_hex: &str, profile: &ProfileSkillTags) -> UnsignedEvent {
    let tags: Vec<Tag> = profile.skill_tags.iter().map(t_tag).collect();
    UnsignedEvent::new(
        pubkey_hex,
        KIND_PROFILE_SKILL_TAGS,
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

/// A contact-graph operation, not a contract one (`abstract.md` "Basic Use
/// Cases" §Introduce) — introducing a *job* is a `JobOffer`, covered
/// above. Two shapes share this kind: a self-introduction
/// (`subject_pubkey == event.pubkey`, introducing the signer to
/// `recipient_pubkey`) and a mutual introduction (`subject_pubkey` is a
/// third party — one of the signer's own contacts — being introduced to
/// `recipient_pubkey`, another of the signer's own contacts).
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
    fn self_introduction_names_the_signer_as_subject() {
        let introducer = Identity::generate();
        let recipient = Identity::generate();
        let intro = Introduction {
            subject_pubkey: introducer.nostr_pubkey_hex(),
            chain: vec![],
            note: Some("we met at the meetup".to_string()),
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
    fn mutual_introduction_names_a_third_party_subject() {
        let introducer = Identity::generate();
        let recipient = Identity::generate();
        let subject = Identity::generate();
        let intro = Introduction {
            subject_pubkey: subject.nostr_pubkey_hex(),
            chain: vec![],
            note: None,
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
}
