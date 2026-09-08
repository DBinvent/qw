//! The client half of NIP-QW01's pre-signature steps: compose a contract
//! **proposal**, answer one with a **counteroffer** (reject → amend →
//! re-apply, in one signed event), and end the exchange with an
//! **Accept**. Plus a read-only [`list`] of every negotiation the held
//! events put this identity in.
//!
//! Nothing here is a new protocol rule. Kinds 9000 / 9004 / 9001 and the
//! [`qw_protocol::contract::Contract`] state machine already define all of
//! it, including the property this module leans on: every step is built
//! from purely local data (the signing identity, plus the id of whichever
//! prior event it references), so proposing, countering and accepting each
//! work with the counterparty offline. This module only does the picking
//! and packaging a UI would otherwise re-derive:
//!
//! - **A counteroffer is the whole "reject-amend-reapply" loop.** It
//!   neither accepts nor rejects the terms it references — it supersedes
//!   them and hands the proposal back (NIP-QW01 §9004). Either party may
//!   send one, repeatedly; only the worker's signed Accept ends it, and
//!   walking away is silence plus the 30-day `Expired` timeout, not an
//!   event.
//! - **The proposal that follows an invite.** When the counterparty is
//!   only reachable because they followed a public invite link, the first
//!   offer can carry an `"introduction"` marker back to that intro event
//!   (`job_offer_from_introduction`) so the record shows the negotiation
//!   grew out of the invite. The intro is still the contact edge; the
//!   offer is still a separate signed step.

use qw_protocol::contract::{Contract, ContractState};
use qw_protocol::events::kinds::{
    dispute_annotation, job_accept, job_counteroffer, job_offer, job_offer_from_introduction,
    AuditOutcome, DisputeAnnotation, JobAccept, JobOffer, KIND_JOB_COMPLETION,
    KIND_JOB_COUNTEROFFER, KIND_JOB_OFFER,
};
use qw_protocol::events::{now, p_tag, Event};
use qw_protocol::identity::Identity;
use qw_protocol::invite;
use serde::{Deserialize, Serialize};

use crate::taxonomy;

/// What went wrong composing a step. Every variant names the offending
/// input rather than failing blank — a proposal form has several fields
/// and "invalid" on its own is not actionable.
#[derive(Debug, Clone, PartialEq)]
pub enum NegotiationError {
    /// Not 64 hex chars, or it is this identity's own key — you cannot
    /// contract with yourself.
    Counterparty(String),
    /// A terms field did not validate; the string says which and why.
    Terms(String),
    /// No held offer event has this id — the contract it anchors is not
    /// visible to this client.
    UnknownOffer(String),
    /// This identity is neither the client nor the worker on that offer.
    NotAParticipant,
    /// There is already a signed Accept against the current head.
    AlreadyAccepted,
    /// The contract is past negotiation (completed, credit-issued, or
    /// expired) — the string is the state.
    NotOpen(String),
    /// Only the worker (the offer's `p`-tagged party) signs kind 9001; the
    /// proposing side cannot accept its own offer.
    WorkerOnly,
    /// A dispute-annotation input did not validate — empty note, unknown
    /// annotation kind, missing/bad audit outcome, or a target event that
    /// is not part of the named contract. The string says which.
    Annotation(String),
}

impl std::fmt::Display for NegotiationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NegotiationError::Counterparty(s) => write!(f, "counterparty: {s}"),
            NegotiationError::Terms(s) => write!(f, "terms: {s}"),
            NegotiationError::UnknownOffer(id) => {
                write!(f, "no proposal with id {id} is held by this client")
            }
            NegotiationError::NotAParticipant => {
                write!(f, "this identity is not a party to that proposal")
            }
            NegotiationError::AlreadyAccepted => {
                write!(f, "that proposal has already been accepted")
            }
            NegotiationError::NotOpen(state) => {
                write!(f, "that contract is {state}, not open for negotiation")
            }
            NegotiationError::WorkerOnly => {
                write!(f, "only the worker signs an Accept, not the proposing side")
            }
            NegotiationError::Annotation(s) => write!(f, "annotation: {s}"),
        }
    }
}

impl std::error::Error for NegotiationError {}

/// A proposal's terms as the editor collects them — the same set every
/// round, because a counteroffer is a full replacement, not a diff
/// (NIP-QW01). `ko`/`km` are the objective/subjective coefficients from
/// `abstract.md`'s `Quants = Hours × Rate × ko × km`; both may be left
/// out to keep a negotiation simple. For an AI-model party actor, `ko`
/// tracks model size / context window / agent-config quality and `km` the
/// model's cognition — prompt adherence and hallucination rate
/// (`abstract.md`, "When a party actor is an AI model").
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TermsDraft {
    /// Free text or picker leaves; resolved through the bundled taxonomy
    /// here, so an entry that will not resolve is an error naming it.
    pub skill_tags: Vec<String>,
    pub hours: f64,
    pub rate: f64,
    #[serde(default)]
    pub ko: Option<f64>,
    #[serde(default)]
    pub km: Option<f64>,
    pub terms: String,
}

/// Compose a fresh proposal. `from_introduction` is set when this
/// contact is only reachable because they followed a public invite link.
#[derive(Debug, Clone, Deserialize)]
pub struct ProposeArgs {
    pub counterparty: String,
    #[serde(default)]
    pub from_introduction: Option<String>,
    pub terms: TermsDraft,
}

/// Answer an open proposal — `offer_event_id` is the contract anchor.
#[derive(Debug, Clone, Deserialize)]
pub struct CounterArgs {
    pub offer_event_id: String,
    pub terms: TermsDraft,
}

/// The worker's Accept. Blank `note` is treated as absent.
#[derive(Debug, Clone, Deserialize)]
pub struct AcceptArgs {
    pub offer_event_id: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Attach a dispute annotation (kind 9030, NIP-QW04) to a contract: a
/// **reply** (the criticised party's side of it, no score effect), an
/// **audit request** (either party, marks the record under review), or an
/// **audit opinion** (a third-party auditor, who stakes their own
/// standing). It never mutates the event it targets — the record stays,
/// the annotation rides alongside.
#[derive(Debug, Clone, Deserialize)]
pub struct AnnotateArgs {
    /// The contract anchor (root offer id). The annotation attaches to
    /// this contract's current negotiation head unless `target` names
    /// another event the contract already references.
    pub offer_event_id: String,
    /// `reply` | `audit_request` | `audit_opinion`.
    pub kind: String,
    pub body: String,
    /// Required for `audit_opinion`, ignored otherwise: `favors_client` |
    /// `favors_worker` | `split` | `inconclusive`.
    #[serde(default)]
    pub outcome: Option<String>,
    /// Annotate a specific milestone / completion / prior annotation
    /// instead of the negotiation head. Must be an event id this contract
    /// already involves.
    #[serde(default)]
    pub target: Option<String>,
}

impl TermsDraft {
    fn resolve(&self) -> Result<JobOffer, NegotiationError> {
        let terms = self.terms.trim().to_string();
        if terms.is_empty() {
            return Err(NegotiationError::Terms("a description is required".into()));
        }
        if !self.hours.is_finite() || self.hours <= 0.0 {
            return Err(NegotiationError::Terms("hours must be a positive number".into()));
        }
        if !self.rate.is_finite() || self.rate < 0.0 {
            return Err(NegotiationError::Terms("rate must be zero or more".into()));
        }
        for (name, v) in [("ko", self.ko), ("km", self.km)] {
            if let Some(v) = v {
                if !v.is_finite() || v <= 0.0 {
                    return Err(NegotiationError::Terms(format!(
                        "{name}, if set, must be a positive number"
                    )));
                }
            }
        }

        let mut skill_tags: Vec<String> = Vec::new();
        for raw in &self.skill_tags {
            if raw.trim().is_empty() {
                continue;
            }
            let tag = taxonomy::resolve(raw).map_err(NegotiationError::Terms)?;
            let tag = tag.tag().to_string();
            if !skill_tags.contains(&tag) {
                skill_tags.push(tag);
            }
        }
        if skill_tags.is_empty() {
            return Err(NegotiationError::Terms("at least one skill tag is required".into()));
        }

        Ok(JobOffer {
            skill_tags,
            hours: self.hours,
            rate: self.rate,
            ko: self.ko,
            km: self.km,
            terms,
        })
    }
}

/// Propose a contract to `counterparty_pubkey_hex` (a fresh kind 9000).
/// The proposing identity is the client; `counterparty` is the worker.
/// Pass `from_introduction` when the only reason this contact is reachable
/// is a public invite link they followed — the offer then points back at
/// that introduction event.
pub fn propose(
    identity: &Identity,
    counterparty_pubkey_hex: &str,
    from_introduction: Option<&str>,
    draft: &TermsDraft,
) -> Result<Event, NegotiationError> {
    let me = identity.nostr_pubkey_hex();
    let worker = validate_counterparty(counterparty_pubkey_hex, &me)?;
    let offer = draft.resolve()?;
    let unsigned = match from_introduction {
        Some(intro_id) => job_offer_from_introduction(&me, &worker, intro_id, &offer),
        None => job_offer(&me, &worker, &offer),
    };
    Ok(unsigned.sign(identity))
}

/// Answer an open proposal with a counteroffer: supersede the current head
/// terms with `draft` and hand the proposal back (kind 9004). Either party
/// may do this, as often as it takes.
pub fn counter(
    identity: &Identity,
    events: &[Event],
    offer_event_id: &str,
    draft: &TermsDraft,
) -> Result<Event, NegotiationError> {
    let me = identity.nostr_pubkey_hex();
    let parties = parties_of(events, offer_event_id)?;
    let (client, worker) = parties.roles(&me)?;
    let counterparty = if me == client { &worker } else { &client };

    let contract = Contract::from_events(events, offer_event_id, &client, &worker, now());
    open_or_err(&contract.state)?;

    let terms = draft.resolve()?;
    Ok(job_counteroffer(&me, counterparty, &contract.negotiation_head_id, &terms).sign(identity))
}

/// End the exchange: the worker signs an Accept against the current head
/// (kind 9001). `note` is optional and blank is treated as absent.
pub fn accept(
    identity: &Identity,
    events: &[Event],
    offer_event_id: &str,
    note: Option<&str>,
) -> Result<Event, NegotiationError> {
    let me = identity.nostr_pubkey_hex();
    let parties = parties_of(events, offer_event_id)?;
    let (client, worker) = parties.roles(&me)?;
    if me != worker {
        return Err(NegotiationError::WorkerOnly);
    }

    let contract = Contract::from_events(events, offer_event_id, &client, &worker, now());
    open_or_err(&contract.state)?;

    let note = note
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(job_accept(&me, &client, &contract.negotiation_head_id, &JobAccept { note }).sign(identity))
}

/// Sign a dispute annotation (kind 9030) for the contract `args`
/// identifies. `reply` and `audit_request` require the signer to be a
/// party to the contract; `audit_opinion` does not — an auditor is by
/// definition a third party — but still must resolve a real target event
/// inside it. Any contract state is fair game: annotating a contract
/// stuck in limbo is the whole reason this kind exists (NIP-QW04).
pub fn annotate(
    identity: &Identity,
    events: &[Event],
    args: &AnnotateArgs,
) -> Result<Event, NegotiationError> {
    let me = identity.nostr_pubkey_hex();
    let parties = parties_of(events, &args.offer_event_id)?;
    let contract = Contract::from_events(
        events,
        &args.offer_event_id,
        &parties.client,
        &parties.worker,
        now(),
    );

    let body = args.body.trim().to_string();
    if body.is_empty() {
        return Err(NegotiationError::Annotation("a note is required".into()));
    }

    let annotation = match args.kind.as_str() {
        "reply" => {
            parties.roles(&me)?;
            DisputeAnnotation::Reply { body }
        }
        "audit_request" => {
            parties.roles(&me)?;
            DisputeAnnotation::AuditRequest { body }
        }
        "audit_opinion" => {
            let outcome = parse_outcome(args.outcome.as_deref())?;
            DisputeAnnotation::AuditOpinion { body, outcome }
        }
        other => {
            return Err(NegotiationError::Annotation(format!(
                "unknown annotation kind {other:?}"
            )))
        }
    };

    let target = match &args.target {
        Some(id) => {
            if !contract_involves(events, &contract, &args.offer_event_id, id) {
                return Err(NegotiationError::Annotation(
                    "target event is not part of that contract".into(),
                ));
            }
            id.clone()
        }
        None => contract.negotiation_head_id.clone(),
    };

    // NIP-QW04's wire format is `["e", <target>]` only. Add `["p", …]` for
    // the other party (both, for a third-party auditor) purely so the
    // coordination mailbox can route the annotation to them — the same
    // carriage note NIP-QW06 makes for 9050. A relay a party subscribes
    // to ignores the tag; it changes nothing about the annotation.
    let mut unsigned = dispute_annotation(&me, &target, &annotation);
    for party in [&parties.client, &parties.worker] {
        if party != &me {
            unsigned.tags.push(p_tag(party.clone()));
        }
    }
    Ok(unsigned.sign(identity))
}

fn parse_outcome(raw: Option<&str>) -> Result<AuditOutcome, NegotiationError> {
    match raw.map(str::trim) {
        Some("favors_client") => Ok(AuditOutcome::FavorsClient),
        Some("favors_worker") => Ok(AuditOutcome::FavorsWorker),
        Some("split") => Ok(AuditOutcome::Split),
        Some("inconclusive") => Ok(AuditOutcome::Inconclusive),
        _ => Err(NegotiationError::Annotation(
            "an audit opinion needs an outcome: favors_client, favors_worker, split or inconclusive"
                .into(),
        )),
    }
}

/// Is `event_id` one of the events this contract is built from — its
/// offer, negotiation head, a milestone, or a completion? These are
/// exactly the targets [`qw_protocol::contract::Contract::from_events`]
/// re-collects annotations for, so an annotation on anything else would
/// sign fine and then never surface on the contract.
fn contract_involves(
    events: &[Event],
    contract: &Contract<'_>,
    offer_event_id: &str,
    event_id: &str,
) -> bool {
    if event_id == offer_event_id || event_id == contract.negotiation_head_id {
        return true;
    }
    if contract.milestones.iter().any(|e| e.id == event_id) {
        return true;
    }
    events.iter().any(|e| {
        e.id == event_id
            && e.kind == KIND_JOB_COMPLETION
            && e.tag_values("e").any(|t| t == offer_event_id)
    })
}

/// One dispute annotation (kind 9030) attached to a contract, as a screen
/// shows it. Ordered oldest-first by the list that carries them.
#[derive(Debug, Clone, Serialize)]
pub struct DisputeView {
    pub event_id: String,
    /// The event this annotation replies to — an offer, head, milestone,
    /// completion, or an earlier annotation.
    pub target_event_id: String,
    pub author_pubkey: String,
    pub author_npub: String,
    /// This identity signed it.
    pub mine: bool,
    /// `reply` | `audit_request` | `audit_opinion`.
    pub annotation_type: String,
    pub body: String,
    /// `audit_opinion` only: `favors_client` | `favors_worker` | `split`
    /// | `inconclusive`.
    pub outcome: Option<String>,
    pub at: u64,
}

/// One negotiation as a screen would show it: the current terms, how many
/// rounds in, whose move it looks like, and which actions are legal now.
#[derive(Debug, Clone, Serialize)]
pub struct NegotiationView {
    /// The root offer's id — the contract anchor every later step
    /// references, and the handle [`counter`]/[`accept`] take.
    pub offer_event_id: String,
    pub counterparty_pubkey: String,
    /// This identity proposed it (is the client). Otherwise it is the
    /// worker, and the only side that can Accept.
    pub am_client: bool,
    /// The offer, or the latest counteroffer that superseded it.
    pub head_event_id: String,
    pub head_terms: JobOffer,
    /// Number of counteroffers on the accepted-or-current line.
    pub rounds: usize,
    /// `negotiating` | `accepted` | `completed` | `credit_issued` |
    /// `expired`.
    pub state: String,
    /// Still negotiating, and the last word was the counterparty's.
    pub your_move: bool,
    pub can_counter: bool,
    pub can_accept: bool,
    /// Dispute annotations anywhere on this contract (NIP-QW04), oldest
    /// first. Usually empty.
    pub disputes: Vec<DisputeView>,
    /// An audit request is attached with no audit opinion answering it
    /// yet — the "under review, undecided" state the FAQ describes.
    pub under_review: bool,
    /// `false` only for an inbound proposal still open that fails this
    /// viewer's admission pre-filter (abstract.md §"Basic Use Cases").
    /// `list` always sets `true`; [`crate::session::Session::negotiations`]
    /// is where the filter is actually applied, because it needs the
    /// viewer's configured thresholds.
    pub passes_filter: bool,
    /// Newest relevant signature timestamp — what the list sorts on.
    pub last_update: u64,
}

/// Every negotiation `me_pubkey_hex` is a party to, newest activity first.
pub fn list(events: &[Event], me_pubkey_hex: &str) -> Vec<NegotiationView> {
    let clock = now();
    let mut out: Vec<NegotiationView> = events
        .iter()
        .filter(|e| e.kind == KIND_JOB_OFFER)
        .filter_map(|offer| {
            let client = offer.pubkey.as_str();
            let worker = offer.first_tag_value("p")?;
            if me_pubkey_hex != client && me_pubkey_hex != worker {
                return None;
            }
            let contract = Contract::from_events(events, &offer.id, client, worker, clock);
            let head = events
                .iter()
                .find(|e| e.id == contract.negotiation_head_id)?;
            let head_terms: JobOffer = serde_json::from_str(&head.content).ok()?;

            let am_client = me_pubkey_hex == client;
            let counterparty = if am_client { worker } else { client };
            let negotiating = matches!(contract.state, ContractState::Negotiating);
            let disputes = dispute_views(&contract.disputes, me_pubkey_hex);
            let has = |t: &str| disputes.iter().any(|d| d.annotation_type == t);
            let under_review = has("audit_request") && !has("audit_opinion");
            let last_update = contract
                .accept
                .map(|a| a.created_at)
                .unwrap_or(head.created_at)
                .max(offer.created_at)
                .max(disputes.iter().map(|d| d.at).max().unwrap_or(0));

            Some(NegotiationView {
                offer_event_id: offer.id.clone(),
                counterparty_pubkey: counterparty.to_string(),
                am_client,
                head_event_id: head.id.clone(),
                head_terms,
                rounds: count_rounds(events, &offer.id),
                state: state_label(&contract.state).to_string(),
                your_move: negotiating && head.pubkey != me_pubkey_hex,
                can_counter: negotiating,
                can_accept: !am_client && negotiating,
                disputes,
                under_review,
                passes_filter: true,
                last_update,
            })
        })
        .collect();
    out.sort_by_key(|v| std::cmp::Reverse(v.last_update));
    out
}

// --- internals -------------------------------------------------------

fn validate_counterparty(input: &str, me: &str) -> Result<String, NegotiationError> {
    // Accept whatever a person pastes — bare hex or `npub1…` — the same
    // way `follow` does; `parse_invite_target` also refuses an `nsec`.
    let p = invite::parse_invite_target(input.trim())
        .map_err(|e| NegotiationError::Counterparty(e.to_string()))?;
    if p == me {
        return Err(NegotiationError::Counterparty(
            "that is this identity's own key".into(),
        ));
    }
    Ok(p)
}

struct Parties {
    client: String,
    worker: String,
}

impl Parties {
    /// `(client, worker)` if `me` is one of them, else `NotAParticipant`.
    fn roles(&self, me: &str) -> Result<(String, String), NegotiationError> {
        if me == self.client || me == self.worker {
            Ok((self.client.clone(), self.worker.clone()))
        } else {
            Err(NegotiationError::NotAParticipant)
        }
    }
}

fn parties_of(events: &[Event], offer_event_id: &str) -> Result<Parties, NegotiationError> {
    let offer = events
        .iter()
        .find(|e| e.id == offer_event_id && e.kind == KIND_JOB_OFFER)
        .ok_or_else(|| NegotiationError::UnknownOffer(offer_event_id.to_string()))?;
    let worker = offer
        .first_tag_value("p")
        .ok_or_else(|| NegotiationError::UnknownOffer(offer_event_id.to_string()))?;
    Ok(Parties {
        client: offer.pubkey.clone(),
        worker: worker.to_string(),
    })
}

/// Turn a contract's raw kind-9030 events into display rows, oldest first.
/// An annotation whose content will not parse is dropped, not surfaced as
/// a blank row.
fn dispute_views(disputes: &[&Event], me: &str) -> Vec<DisputeView> {
    let mut out: Vec<DisputeView> = disputes
        .iter()
        .filter_map(|e| {
            let (annotation_type, body, outcome) =
                match serde_json::from_str::<DisputeAnnotation>(&e.content).ok()? {
                    DisputeAnnotation::Reply { body } => ("reply", body, None),
                    DisputeAnnotation::AuditRequest { body } => ("audit_request", body, None),
                    DisputeAnnotation::AuditOpinion { body, outcome } => (
                        "audit_opinion",
                        body,
                        Some(outcome_label(outcome).to_string()),
                    ),
                };
            Some(DisputeView {
                event_id: e.id.clone(),
                target_event_id: e.first_tag_value("e").unwrap_or_default().to_string(),
                author_npub: invite::npub_encode(&e.pubkey).unwrap_or_else(|_| e.pubkey.clone()),
                author_pubkey: e.pubkey.clone(),
                mine: e.pubkey == me,
                annotation_type: annotation_type.to_string(),
                body,
                outcome,
                at: e.created_at,
            })
        })
        .collect();
    out.sort_by_key(|d| d.at);
    out
}

fn outcome_label(o: AuditOutcome) -> &'static str {
    match o {
        AuditOutcome::FavorsClient => "favors_client",
        AuditOutcome::FavorsWorker => "favors_worker",
        AuditOutcome::Split => "split",
        AuditOutcome::Inconclusive => "inconclusive",
    }
}

fn open_or_err(state: &ContractState) -> Result<(), NegotiationError> {
    match state {
        ContractState::Negotiating => Ok(()),
        ContractState::Accepted => Err(NegotiationError::AlreadyAccepted),
        other => Err(NegotiationError::NotOpen(state_label(other).to_string())),
    }
}

fn state_label(state: &ContractState) -> &'static str {
    match state {
        ContractState::Negotiating => "negotiating",
        ContractState::Accepted => "accepted",
        ContractState::Completed => "completed",
        ContractState::CreditIssued => "credit_issued",
        ContractState::Expired => "expired",
    }
}

/// Counteroffers on the line from the root offer to the current head —
/// same forward walk as `qw_protocol::contract::negotiation_head`, counted.
fn count_rounds(events: &[Event], offer_event_id: &str) -> usize {
    let mut current = offer_event_id;
    let mut n = 0;
    loop {
        match events
            .iter()
            .filter(|e| {
                e.kind == KIND_JOB_COUNTEROFFER && e.tag_values("e").any(|id| id == current)
            })
            .max_by_key(|e| e.created_at)
        {
            Some(e) => {
                n += 1;
                current = e.id.as_str();
            }
            None => return n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qw_protocol::events::kinds::KIND_DISPUTE_ANNOTATION;

    fn draft(skill: &str, hours: f64, rate: f64, terms: &str) -> TermsDraft {
        TermsDraft {
            skill_tags: vec![skill.to_string()],
            hours,
            rate,
            ko: None,
            km: None,
            terms: terms.to_string(),
        }
    }

    #[test]
    fn propose_signs_a_9000_and_resolves_skills_through_the_taxonomy() {
        let me = Identity::generate();
        let them = Identity::generate();
        let event = propose(
            &me,
            &them.nostr_pubkey_hex(),
            None,
            &draft("Rust Lang", 8.0, 40.0, "sprint 12 backend work"),
        )
        .unwrap();

        assert!(event.verify().is_ok());
        assert_eq!(event.kind, KIND_JOB_OFFER);
        assert_eq!(
            event.first_tag_value("p"),
            Some(them.nostr_pubkey_hex().as_str())
        );
        let offer: JobOffer = serde_json::from_str(&event.content).unwrap();
        assert_eq!(offer.skill_tags, vec!["it/backend/languages#rust"]);
        // no introduction marker on a plain proposal
        assert!(!event
            .tags
            .iter()
            .any(|t| t.get(3).map(String::as_str) == Some("introduction")));
    }

    #[test]
    fn propose_from_introduction_points_back_at_the_invite() {
        let me = Identity::generate();
        let them = Identity::generate();
        let event = propose(
            &me,
            &them.nostr_pubkey_hex(),
            Some("theintroeventid"),
            &draft("rust", 4.0, 30.0, "the work the invite was about"),
        )
        .unwrap();

        let intro_ref = event.tags.iter().find(|t| {
            t.first().map(String::as_str) == Some("e")
                && t.get(3).map(String::as_str) == Some("introduction")
        });
        assert_eq!(
            intro_ref.and_then(|t| t.get(1)).map(String::as_str),
            Some("theintroeventid")
        );
    }

    #[test]
    fn propose_refuses_yourself_and_a_malformed_key() {
        let me = Identity::generate();
        assert!(matches!(
            propose(
                &me,
                &me.nostr_pubkey_hex(),
                None,
                &draft("rust", 8.0, 40.0, "x")
            ),
            Err(NegotiationError::Counterparty(_))
        ));
        assert!(matches!(
            propose(&me, "not-a-key", None, &draft("rust", 8.0, 40.0, "x")),
            Err(NegotiationError::Counterparty(_))
        ));
    }

    #[test]
    fn propose_accepts_an_npub_counterparty() {
        let me = Identity::generate();
        let them = Identity::generate();
        let npub = invite::npub_encode(&them.nostr_pubkey_hex()).unwrap();
        let event = propose(&me, &npub, None, &draft("rust", 8.0, 40.0, "x")).unwrap();
        assert_eq!(
            event.first_tag_value("p"),
            Some(them.nostr_pubkey_hex().as_str()),
            "the npub must be decoded to the same hex the p tag carries"
        );
    }

    #[test]
    fn propose_rejects_an_unresolvable_skill_and_empty_terms() {
        let me = Identity::generate();
        let them = Identity::generate();
        assert!(matches!(
            propose(
                &me,
                &them.nostr_pubkey_hex(),
                None,
                &draft("underwater basket weaving", 8.0, 40.0, "x")
            ),
            Err(NegotiationError::Terms(_))
        ));
        assert!(matches!(
            propose(
                &me,
                &them.nostr_pubkey_hex(),
                None,
                &draft("rust", 8.0, 40.0, "   ")
            ),
            Err(NegotiationError::Terms(_))
        ));
        assert!(matches!(
            propose(
                &me,
                &them.nostr_pubkey_hex(),
                None,
                &draft("rust", 0.0, 40.0, "x")
            ),
            Err(NegotiationError::Terms(_))
        ));
    }

    #[test]
    fn counter_supersedes_the_head_and_addresses_the_other_party() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer = propose(
            &client,
            &worker.nostr_pubkey_hex(),
            None,
            &draft("rust", 8.0, 40.0, "sprint 12"),
        )
        .unwrap();

        // worker counters
        let c1 = counter(
            &worker,
            &[offer.clone()],
            &offer.id,
            &draft("rust", 8.0, 55.0, "sprint 12"),
        )
        .unwrap();
        assert_eq!(c1.kind, KIND_JOB_COUNTEROFFER);
        assert_eq!(c1.first_tag_value("e"), Some(offer.id.as_str()));
        assert_eq!(
            c1.first_tag_value("p"),
            Some(client.nostr_pubkey_hex().as_str())
        );

        // client counters back — must target c1, the new head, not the offer
        let c2 = counter(
            &client,
            &[offer.clone(), c1.clone()],
            &offer.id,
            &draft("rust", 8.0, 48.0, "sprint 12"),
        )
        .unwrap();
        assert_eq!(c2.first_tag_value("e"), Some(c1.id.as_str()));
    }

    #[test]
    fn a_non_party_cannot_counter() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let stranger = Identity::generate();
        let offer = propose(
            &client,
            &worker.nostr_pubkey_hex(),
            None,
            &draft("rust", 8.0, 40.0, "x"),
        )
        .unwrap();
        assert_eq!(
            counter(
                &stranger,
                &[offer.clone()],
                &offer.id,
                &draft("rust", 8.0, 40.0, "x")
            ),
            Err(NegotiationError::NotAParticipant)
        );
    }

    #[test]
    fn accept_is_worker_only_and_closes_the_exchange() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer = propose(
            &client,
            &worker.nostr_pubkey_hex(),
            None,
            &draft("rust", 8.0, 40.0, "x"),
        )
        .unwrap();

        assert_eq!(
            accept(&client, &[offer.clone()], &offer.id, None),
            Err(NegotiationError::WorkerOnly)
        );

        let ok = accept(&worker, &[offer.clone()], &offer.id, Some("  starting Monday ")).unwrap();
        assert_eq!(ok.first_tag_value("e"), Some(offer.id.as_str()));
        let body: JobAccept = serde_json::from_str(&ok.content).unwrap();
        assert_eq!(body.note.as_deref(), Some("starting Monday"));

        // and once it is accepted, neither side may counter
        assert_eq!(
            counter(
                &client,
                &[offer.clone(), ok],
                &offer.id,
                &draft("rust", 8.0, 40.0, "x")
            ),
            Err(NegotiationError::AlreadyAccepted)
        );
    }

    #[test]
    fn list_tracks_terms_rounds_state_and_whose_move_it_is() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let me = client.nostr_pubkey_hex();

        let offer = propose(
            &client,
            &worker.nostr_pubkey_hex(),
            None,
            &draft("rust", 8.0, 40.0, "sprint 12"),
        )
        .unwrap();

        // just my offer out: waiting on them
        let v = list(&[offer.clone()], &me);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].state, "negotiating");
        assert_eq!(v[0].rounds, 0);
        assert!(v[0].am_client);
        assert!(!v[0].your_move);
        assert!(!v[0].can_accept);

        // they counter: now it is my move, terms have moved
        let c1 = counter(
            &worker,
            &[offer.clone()],
            &offer.id,
            &draft("rust", 8.0, 55.0, "sprint 12"),
        )
        .unwrap();
        let v = list(&[offer.clone(), c1.clone()], &me);
        assert_eq!(v[0].rounds, 1);
        assert!(v[0].your_move);
        assert_eq!(v[0].head_terms.rate, 55.0);
        assert_eq!(v[0].head_event_id, c1.id);

        // from the worker's side, they can accept and it is not their move
        let vw = list(&[offer.clone(), c1.clone()], &worker.nostr_pubkey_hex());
        assert!(!vw[0].am_client);
        assert!(vw[0].can_accept);
        assert!(!vw[0].your_move);

        // worker accepts: state flips, no more counters, nobody's move
        let acc = accept(&worker, &[offer.clone(), c1.clone()], &offer.id, None).unwrap();
        let v = list(&[offer, c1, acc], &me);
        assert_eq!(v[0].state, "accepted");
        assert!(!v[0].can_counter);
        assert!(!v[0].your_move);
    }

    // --- NIP-QW04 dispute annotation ----------------------------------

    fn annotate_args(offer_id: &str, kind: &str, body: &str) -> AnnotateArgs {
        AnnotateArgs {
            offer_event_id: offer_id.to_string(),
            kind: kind.to_string(),
            body: body.to_string(),
            outcome: None,
            target: None,
        }
    }

    #[test]
    fn a_party_annotates_a_contract_and_the_record_is_untouched() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let offer = propose(
            &client,
            &worker.nostr_pubkey_hex(),
            None,
            &draft("rust", 8.0, 40.0, "x"),
        )
        .unwrap();

        // the worker files an audit request; it targets the head and
        // p-tags the client so a mailbox can carry it
        let ev = annotate(
            &worker,
            std::slice::from_ref(&offer),
            &annotate_args(&offer.id, "audit_request", "  milestone never landed  "),
        )
        .unwrap();
        assert!(ev.verify().is_ok());
        assert_eq!(ev.kind, KIND_DISPUTE_ANNOTATION);
        assert_eq!(ev.first_tag_value("e"), Some(offer.id.as_str()));
        assert_eq!(
            ev.first_tag_value("p"),
            Some(client.nostr_pubkey_hex().as_str())
        );
        match serde_json::from_str::<DisputeAnnotation>(&ev.content).unwrap() {
            DisputeAnnotation::AuditRequest { body } => assert_eq!(body, "milestone never landed"),
            other => panic!("{other:?}"),
        }

        // an empty note is refused, naming the problem
        assert!(matches!(
            annotate(
                &worker,
                std::slice::from_ref(&offer),
                &annotate_args(&offer.id, "reply", "   "),
            ),
            Err(NegotiationError::Annotation(_))
        ));
    }

    #[test]
    fn reply_is_party_only_but_an_audit_opinion_is_not() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let auditor = Identity::generate();
        let offer = propose(
            &client,
            &worker.nostr_pubkey_hex(),
            None,
            &draft("rust", 8.0, 40.0, "x"),
        )
        .unwrap();

        // a stranger cannot post a reply or an audit request
        assert_eq!(
            annotate(
                &auditor,
                std::slice::from_ref(&offer),
                &annotate_args(&offer.id, "reply", "not my place"),
            ),
            Err(NegotiationError::NotAParticipant)
        );

        // but a stranger *is* the right signer for a third-party opinion —
        // it just needs an outcome
        assert!(matches!(
            annotate(
                &auditor,
                std::slice::from_ref(&offer),
                &annotate_args(&offer.id, "audit_opinion", "reviewed both sides"),
            ),
            Err(NegotiationError::Annotation(_))
        ));
        let op = annotate(
            &auditor,
            std::slice::from_ref(&offer),
            &AnnotateArgs {
                outcome: Some("favors_worker".into()),
                ..annotate_args(&offer.id, "audit_opinion", "reviewed both sides")
            },
        )
        .unwrap();
        // p-tagged to *both* parties, since the auditor is neither
        let ps: Vec<&str> = op.tag_values("p").collect();
        assert!(ps.contains(&client.nostr_pubkey_hex().as_str()));
        assert!(ps.contains(&worker.nostr_pubkey_hex().as_str()));
        match serde_json::from_str::<DisputeAnnotation>(&op.content).unwrap() {
            DisputeAnnotation::AuditOpinion { outcome, .. } => {
                assert_eq!(outcome, AuditOutcome::FavorsWorker)
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn list_surfaces_disputes_and_flags_under_review_until_an_opinion_lands() {
        let client = Identity::generate();
        let worker = Identity::generate();
        let auditor = Identity::generate();
        let me = client.nostr_pubkey_hex();
        let offer = propose(
            &client,
            &worker.nostr_pubkey_hex(),
            None,
            &draft("rust", 8.0, 40.0, "x"),
        )
        .unwrap();

        let req = annotate(
            &client,
            std::slice::from_ref(&offer),
            &annotate_args(&offer.id, "audit_request", "no delivery"),
        )
        .unwrap();
        let v = list(&[offer.clone(), req.clone()], &me);
        assert_eq!(v[0].disputes.len(), 1);
        assert_eq!(v[0].disputes[0].annotation_type, "audit_request");
        assert!(v[0].disputes[0].mine, "the client signed this one");
        assert!(
            v[0].under_review,
            "an open audit request means under review"
        );

        let op = annotate(
            &auditor,
            &[offer.clone(), req.clone()],
            &AnnotateArgs {
                outcome: Some("split".into()),
                ..annotate_args(&offer.id, "audit_opinion", "both partly at fault")
            },
        )
        .unwrap();
        let v = list(&[offer, req, op], &me);
        assert_eq!(v[0].disputes.len(), 2);
        assert_eq!(v[0].disputes[1].annotation_type, "audit_opinion");
        assert_eq!(v[0].disputes[1].outcome.as_deref(), Some("split"));
        assert!(!v[0].disputes[1].mine);
        assert!(
            !v[0].under_review,
            "an opinion resolves the 'under review, undecided' state"
        );
    }
}
