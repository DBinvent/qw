//! One client session, and the two seams that let it run three ways off
//! one codebase (`todo-impl.md` §7).
//!
//! [`Session`] holds an identity, a [`HistoryStore`], a mailbox sync
//! cursor and the server list, and exposes **one method per operation** —
//! `identity_view`, `follow`, `sync_now`, `profile_view`, `set_profile`,
//! `negotiations`, `propose`, `counter`, `accept`. Every shell (the Tauri
//! window, a `qw-web` HTTP handler) is then a thin wrapper: take the
//! request, call the method, stringify the error. No behaviour lives in a
//! shell — that is the rule that keeps the web build reviewable as a proxy
//! for the app build.
//!
//! The two traits are where deployments differ:
//!
//! - [`KeyStore`] — load/persist the identity key. [`crate::Vault`] is the
//!   on-disk `0600` implementation; a multi-user `qw-web` host supplies a
//!   seed-unlocked, memory-only one.
//! - [`HistoryStore`] — the verified event set. [`crate::EventStore`] is
//!   the append-only `events.jsonl` implementation; a multi-user host adds
//!   an encrypted-at-rest one that decrypts into memory for the life of a
//!   session.

use std::collections::{BTreeMap, HashSet};

use qw_node::ledger::{LedgerSync, LedgerTransport};
use qw_node::node::{Delivery, FinalAnswer, Node};
use qw_node::server_registry::{rank_servers, ServerCandidate};
use qw_node::sync::{MailboxSync, MailboxTransport};
use qw_protocol::events::{
    now as unix_now, Event, Introduction, SkillAnswer, KIND_INTRODUCTION, KIND_PROFILE,
    KIND_SKILL_ANSWER, KIND_SKILL_QUERY,
};
use qw_protocol::identity::Identity;
use qw_protocol::{invite, trust};
use serde::{Deserialize, Serialize};

use crate::negotiation::{
    self, AcceptArgs, AnnotateArgs, CounterArgs, NegotiationView, ProposeArgs,
};
use crate::profile::{self, ProfileEdit, ProfileView};
use crate::{follow_invite, invite_qr_svg, ClientError};

/// Default relay-query reach — NIP-QW06 §3's default is 3.
const REFERRAL_MAX_HOPS: u8 = 3;

/// Trust-graph depth for a reputation read (§5) — the same default.
const TRUST_MAX_HOPS: u8 = 3;

/// The network's front door — fixed for the life of the network, so it is
/// a constant here rather than configuration (NIP-QW07).
const INVITE_BASE: &str = "https://knownby.work";

/// Trust-graph depth [`Session::rank_servers`] walks when scoring a
/// coordination server — the same default the referral query uses.
const SERVER_RANK_MAX_HOPS: u8 = 3;

/// "Recently completed work" for the admission position limit (abstract.md
/// §"Basic Use Cases" — the exposure ceiling "scales with how much work
/// that counterparty has recently completed"). 90 days, a quarter.
const RECENT_VOLUME_WINDOW_SECS: u64 = 90 * 24 * 60 * 60;

/// Load and persist the identity key. The only file a user must actually
/// back up (§2: there is no account to recover it from).
pub trait KeyStore {
    fn load(&self) -> Result<Option<Identity>, ClientError>;
    fn save(&self, identity: &Identity) -> Result<(), ClientError>;

    /// Load the identity, generating and saving one on first run.
    fn load_or_create(&self) -> Result<Identity, ClientError> {
        match self.load()? {
            Some(identity) => Ok(identity),
            None => {
                let identity = Identity::generate();
                self.save(&identity)?;
                Ok(identity)
            }
        }
    }
}

/// The set of signed events this identity holds — the private ledger every
/// view is a pure fold over (NIP-QW12). Implementations verify on the way
/// in; a store that trusted its input would let a hostile mailbox or
/// replica inject a record into a trust computation.
pub trait HistoryStore: Send {
    /// Everything held, oldest first.
    fn events(&self) -> &[Event];
    /// Merge in events (from a sync, or a future ledger-replication pass).
    /// Returns how many were new. Unverifiable events are refused.
    fn append(&mut self, events: &[Event]) -> Result<usize, ClientError>;
    /// Lines that did not parse or verify on load — non-zero means the
    /// backing file was edited or truncated.
    fn rejected(&self) -> usize;

    fn len(&self) -> usize {
        self.events().len()
    }
    fn is_empty(&self) -> bool {
        self.events().is_empty()
    }

    /// Persist a [`SyncState`] snapshot beside the ledger. `Session` calls
    /// this after every op that queues an event and after every
    /// `sync_now`, so a host that keeps an account across restarts does
    /// not lose an authored-but-unsent event or re-download a whole
    /// mailbox on a cold start (`todo-impl.md` §7 "Outbox persistence").
    ///
    /// It is on this trait, rather than a store of its own, because the
    /// two implementations that have somewhere to put it — the encrypted
    /// multi-user blob, and the `events.jsonl` store's `sync-state.json`
    /// sidecar — differ in *where*, not *whether*.
    fn persist_sync_state(&mut self, _state: &SyncState) -> Result<(), ClientError> {
        Ok(())
    }

    /// The [`SyncState`] this store loaded from its backing on open — a
    /// sidecar file, or (for the multi-user blob) `SyncState::default()`
    /// because the host restores that one explicitly from the sealed
    /// payload. [`Session::with_identity`] applies it at construction so a
    /// configured server list, the outbox and warm cursors survive a
    /// restart without every shell wiring `restore_sync_state` by hand.
    fn loaded_sync_state(&self) -> SyncState {
        SyncState::default()
    }
}

/// What a `sync_now` pass would otherwise lose on restart: the authored
/// events still waiting for a server to accept them, and the per-server
/// poll cursors. A host that keeps an account across restarts (the
/// multi-user `qw-web` blob) writes this beside the ledger — see
/// [`HistoryStore::persist_sync_state`] and [`Session::restore_sync_state`].
///
/// The outbox is stored as **event ids**, not events: every queued event
/// is already in the [`HistoryStore`] (authoring appends there first), so
/// keeping the bytes twice would only let the two copies drift.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SyncState {
    /// Ids of ledger events not yet accepted by any server, oldest first.
    #[serde(default)]
    pub outbox: Vec<String>,
    /// `base_url` -> newest `created_at` already collected from it.
    #[serde(default)]
    pub cursors: BTreeMap<String, u64>,
    /// The coordination servers this identity syncs its mailbox against,
    /// in try order. Empty means "not configured here" — a restore leaves
    /// whatever the host handed the constructor (`QW_SERVERS`, or the
    /// app's built-in) in place. Set through [`Session::set_servers`].
    #[serde(default)]
    pub servers: Vec<String>,
    /// The offer-time admission pre-filter (abstract.md "Basic Use Cases",
    /// §5): minimum requester reputation and a bilateral position ceiling.
    /// Both `None` by default — nothing is filtered until configured. Set
    /// through [`Session::set_admission_policy`].
    #[serde(default)]
    pub admission: trust::AdmissionPolicy,
}

/// One client's live state. `Send` (so a shell can put it behind a
/// `Mutex`), never `Sync` on its own — every operation takes `&mut self`
/// or reads a consistent snapshot under that lock.
pub struct Session {
    identity: Identity,
    history: Box<dyn HistoryStore>,
    sync: MailboxSync,
    /// NIP-QW12 anti-entropy against other replicas of *this* identity —
    /// separate from `sync` (the mailbox, between different identities).
    ledger: LedgerSync,
    /// The referral-routing node (NIP-QW06): contact book derived from
    /// held introductions, own + earned skill tags, relay policy. Kept in
    /// step with `history` by [`Session::refresh_node`].
    node: Node,
    /// Monotone within a session — namespaces referral `query_id`s minted
    /// here.
    query_seq: u64,
    /// Answers to referral queries this session originated, keyed by
    /// `query_id`, deduped by responder (lowest hop count wins).
    referral_results: BTreeMap<String, Vec<FinalAnswerView>>,
    servers: Vec<String>,
    /// The offer-time admission pre-filter (abstract.md §"Basic Use
    /// Cases"). Default-empty: nothing is filtered until the user sets a
    /// threshold. Persisted with the rest of [`SyncState`].
    admission: trust::AdmissionPolicy,
}

impl Session {
    /// Open a session, creating the identity on first run.
    pub fn open(
        keys: &dyn KeyStore,
        history: Box<dyn HistoryStore>,
        servers: Vec<String>,
    ) -> Result<Self, ClientError> {
        Ok(Self::with_identity(keys.load_or_create()?, history, servers))
    }

    /// For a host that already holds the decrypted identity (a
    /// seed-unlocked `qw-web` session) and wants no `KeyStore` round-trip.
    pub fn with_identity(
        identity: Identity,
        history: Box<dyn HistoryStore>,
        servers: Vec<String>,
    ) -> Self {
        let sync = MailboxSync::new(identity.nostr_pubkey_hex());
        // The node signs referral forwards, so it needs its own copy of
        // the key (same identity, different owner).
        let node = Node::new(
            Identity::from_secret_bytes(identity.secret_bytes())
                .expect("a valid identity's secret is valid"),
        );
        let mut session = Self {
            identity,
            history,
            sync,
            ledger: LedgerSync::new(),
            node,
            query_seq: 0,
            referral_results: BTreeMap::new(),
            servers,
            admission: trust::AdmissionPolicy::default(),
        };
        // Adopt whatever the store persisted last run (a sidecar file; the
        // multi-user host restores its sealed copy on top of this and they
        // agree). A store with nothing persisted hands back a default,
        // which leaves the constructor `servers` alone.
        let loaded = session.history.loaded_sync_state();
        session.restore_sync_state(&loaded);
        session.refresh_node();
        session
    }

    pub fn pubkey_hex(&self) -> String {
        self.identity.nostr_pubkey_hex()
    }

    /// The coordination servers, in the order a sync will try them.
    pub fn servers(&self) -> &[String] {
        &self.servers
    }

    /// Replace the coordination-server list — the mailbox relays a sync
    /// flushes to and polls from. Each entry is trimmed; blank lines are
    /// dropped; every survivor must be an `http://` or `https://` URL; the
    /// list is de-duplicated in first-seen order and must end non-empty (a
    /// client with no server cannot send or collect mail). The new list is
    /// folded into the [`SyncState`] the host persists, so it survives a
    /// restart wherever that host keeps one. It is *not* re-ranked — order
    /// as given is order tried; run [`Session::rank_servers`] after if you
    /// want the trust ordering.
    pub fn set_servers(&mut self, urls: Vec<String>) -> Result<(), ClientError> {
        self.servers = crate::clean_server_list(urls)?;
        let snapshot = self.sync_state();
        self.history.persist_sync_state(&snapshot)?;
        Ok(())
    }

    /// Fetch a web host's advertised coordination-server list from
    /// `GET {host_url}/servers` and adopt it (validated exactly as
    /// [`Session::set_servers`] would). This is the "public host default
    /// list" bootstrap — the phone or a fresh client points at a `qw-web`
    /// it trusts and takes that operator's list as its own. Returns the
    /// adopted list.
    pub fn bootstrap_servers(&mut self, host_url: &str) -> Result<Vec<String>, ClientError> {
        let url = format!("{}/servers", host_url.trim_end_matches('/'));
        let resp = reqwest::blocking::Client::new()
            .get(&url)
            .send()
            .map_err(|e| ClientError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ClientError::UnexpectedStatus(resp.status().as_u16()));
        }
        let list: Vec<String> = resp
            .json()
            .map_err(|e| ClientError::Http(format!("{url}: {e}")))?;
        self.set_servers(list)?;
        Ok(self.servers.clone())
    }

    /// The offer-time admission pre-filter as configured —
    /// `(min_reputation, position_limit)`, each `None` when that check is
    /// off (abstract.md §"Basic Use Cases"). Private thresholds: a client
    /// never tells a declined requester which one it failed, or that one
    /// is set at all.
    pub fn admission_policy(&self) -> (Option<f64>, Option<f64>) {
        (self.admission.min_reputation, self.admission.position_limit)
    }

    /// Set that pre-filter. `None` on a field turns its check off; a
    /// negative or non-finite value is rejected. Folded into the persisted
    /// [`SyncState`]. It is never protocol-mandated (§0.8: no enforced
    /// default) and never auto-rejects silently here — `negotiations()`
    /// flags a failing inbound proposal so the human can still admit it.
    pub fn set_admission_policy(
        &mut self,
        min_reputation: Option<f64>,
        position_limit: Option<f64>,
    ) -> Result<(), ClientError> {
        for (name, v) in [
            ("minimum reputation", min_reputation),
            ("position limit", position_limit),
        ] {
            if let Some(v) = v {
                if !v.is_finite() || v < 0.0 {
                    return Err(ClientError::Config(format!("{name} must be zero or more")));
                }
            }
        }
        self.admission = trust::AdmissionPolicy {
            min_reputation,
            position_limit,
        };
        let snapshot = self.sync_state();
        self.history.persist_sync_state(&snapshot)?;
        Ok(())
    }

    /// Does an inbound request from `requester` pass this identity's
    /// admission pre-filter? `true` when no filter is set.
    ///
    /// Both checks are the abstract.md §"Basic Use Cases" computations:
    ///
    /// - **Minimum reputation** — `trust::assess_reputation`, i.e. the
    ///   score of the shortest verified `CreditIssuance` path from us to
    ///   `requester` in `skill_domain` (closing-edge value × hop decay),
    ///   or unknown-risk when no path reaches them. A fresh key is
    ///   unknown-risk, not neutral, so it falls below any threshold.
    /// - **Position limit** — the configured number is a *multiplier* of
    ///   `requester`'s verified completed volume in the recent window
    ///   ([`RECENT_VOLUME_WINDOW_SECS`]); the effective ceiling is that
    ///   product, so it "scales with how much work that counterparty has
    ///   recently completed". Someone with none has an effective ceiling
    ///   of zero.
    fn admits(&self, requester: &str, skill_domain: Option<&str>) -> bool {
        let events = self.history.events();
        let scaled = trust::AdmissionPolicy {
            min_reputation: self.admission.min_reputation,
            position_limit: self.admission.position_limit.map(|mult| {
                let since = unix_now().saturating_sub(RECENT_VOLUME_WINDOW_SECS);
                mult * trust::counterparty_recent_volume(events, requester, since)
            }),
        };
        trust::evaluate_admission(
            events,
            &self.pubkey_hex(),
            requester,
            &scaled,
            TRUST_MAX_HOPS,
            skill_domain,
            &trust::ScoringWeights::default(),
        ) == trust::AdmissionDecision::Admit
    }

    /// Everything this identity holds — the input to a NIP-QW12 ledger
    /// pull a peer replica makes against this instance.
    pub fn held_events(&self) -> &[Event] {
        self.history.events()
    }

    /// Re-order the coordination-server list by this identity's own trust
    /// view of each server (`qw_node::server_registry::rank_servers`): best
    /// first, unknown-risk servers last, ties broken on lower fee. §8
    /// forbids hard-coding one server as authoritative — this is how a
    /// client picks among several without doing that. A `candidate` whose
    /// `pubkey` is unknown to this ledger scores as unknown-risk, so until
    /// servers advertise a pubkey this just fee-orders; the call site is
    /// wired so real ranking arrives for free once they do. Re-run as held
    /// history grows and scores move. An empty `candidates` list is left as
    /// a no-op rather than clearing the existing server list.
    pub fn rank_servers(&mut self, candidates: &[ServerCandidate]) {
        if candidates.is_empty() {
            return;
        }
        let pubkey = self.pubkey_hex();
        let ranked = rank_servers(
            self.history.events(),
            &pubkey,
            candidates,
            SERVER_RANK_MAX_HOPS,
        );
        self.servers = ranked
            .into_iter()
            .map(|r| r.candidate.base_url.clone())
            .collect();
    }

    /// The 32-byte identity secret as hex — the same spelling as the
    /// single-user `identity.key` file, so it can be carried to another
    /// client verbatim. This *is* the account (§2: there is no server to
    /// recover it from), so a shell exposes it only behind a deliberate
    /// action and never on a main screen.
    pub fn identity_secret_hex(&self) -> String {
        self.identity
            .secret_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Records held on disk — the running total every later computation
    /// reads.
    pub fn held(&self) -> usize {
        self.history.len()
    }

    /// Signed events written but not yet accepted by any server. A UI can
    /// show the count; a `qw-web` host must persist these across a restart
    /// (NIP-QW12) or a signed step the user believes they sent is lost.
    pub fn outbox(&self) -> &[Event] {
        self.sync.pending()
    }

    /// Merge externally-obtained events into the ledger — what `sync_now`
    /// does with a mailbox delivery, and what a ledger-replication pass
    /// will do with a peer's. Returns how many were new.
    pub fn ingest(&mut self, events: &[Event]) -> Result<usize, ClientError> {
        let n = self.history.append(events)?;
        if n > 0 {
            self.refresh_node();
        }
        Ok(n)
    }

    /// An event this identity just signed is part of its ledger *now*, not
    /// only once a server echoes it back — a profile event carries no `p`
    /// tag and an offer is `p`-tagged to the *other* party, so neither ever
    /// returns through this client's own mailbox. Record it locally, then
    /// queue it for delivery. Returns the event id.
    fn author(&mut self, event: Event) -> Result<String, ClientError> {
        let id = event.id.clone();
        self.history.append(std::slice::from_ref(&event))?;
        self.sync.queue(event);
        let snapshot = self.sync_state();
        self.history.persist_sync_state(&snapshot)?;
        self.refresh_node();
        Ok(id)
    }

    /// Rebuild the routing node's view from held history: own declared
    /// tags, a contact for every introduction partner (their declared
    /// tags cached as this client currently holds them), and every
    /// contact's earned tags recomputed (NIP-QW06). Contacts only grow —
    /// there is no "unfollow" edge — but a re-add never resets a contact's
    /// rate-limit window ([`Node::note_contact`]).
    fn refresh_node(&mut self) {
        let me = self.pubkey_hex();
        let events = self.history.events();
        let own_tags = profile::current(events, &me)
            .map(|p| p.skill_tags)
            .unwrap_or_default();
        // The signed profile event itself, to ride back on any referral
        // answer this identity gives — so a non-contact who reaches us
        // sees the whole self-description, not just the matched tag.
        let own_profile = profile::current_event(events, &me).cloned();
        let contacts = introduction_partners(events, &me);
        let cached: Vec<(String, Vec<String>)> = contacts
            .into_iter()
            .map(|pk| {
                let tags = profile::current(events, &pk)
                    .map(|p| p.skill_tags)
                    .unwrap_or_default();
                (pk, tags)
            })
            .collect();

        self.node.set_own_skill_tags(own_tags);
        self.node.set_own_profile(own_profile);
        for (pk, tags) in cached {
            self.node.note_contact(&pk, tags);
        }
        self.node.refresh_earned_skill_tags(self.history.events());
    }

    /// Feed inbound referral traffic (kinds 9050 / 9051) through the node,
    /// returning the events it wants sent onward. An answer to a query
    /// *this* session originated is absorbed into `referral_results`
    /// instead of relayed further.
    fn route_inbound(&mut self, delivered: &[Event]) -> Vec<Event> {
        let me = self.pubkey_hex();
        let now = unix_now();
        let mut out = Vec::new();
        for e in delivered {
            match e.kind {
                KIND_SKILL_QUERY => {
                    let outcome = self.node.receive_query(&e.pubkey, e, now);
                    out.extend(outcome.deliveries.into_iter().map(delivery_event));
                }
                KIND_SKILL_ANSWER if e.first_tag_value("p") == Some(me.as_str()) => {
                    let mine = serde_json::from_str::<SkillAnswer>(&e.content)
                        .ok()
                        .filter(|a| self.referral_results.contains_key(&a.query_id));
                    if let Some(a) = mine {
                        self.absorb_answer(&a);
                        continue;
                    }
                    if let Some(d) = self.node.receive_answer(e) {
                        out.push(delivery_event(d));
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn absorb_answer(&mut self, a: &SkillAnswer) {
        // A referral answer from someone this identity has no direct edge
        // to may carry that responder's own signed profile (NIP-QW06
        // "Profile on the answer") — the whole self-description of a person
        // reached only along the vouched relay path, not a broadcast.
        // Merge it, re-verified by the store on the way in, only when it is
        // unmistakably theirs: a kind-`10020` event whose `pubkey` is the
        // responder and whose signature checks out.
        if let Some(ev) = &a.profile {
            if ev.kind == KIND_PROFILE && ev.pubkey == a.responder_pubkey && ev.verify().is_ok() {
                let _ = self.history.append(std::slice::from_ref(ev));
            }
        }

        let held = profile::current(self.history.events(), &a.responder_pubkey);
        let bucket = self.referral_results.entry(a.query_id.clone()).or_default();
        let view = FinalAnswerView {
            responder_pubkey: a.responder_pubkey.clone(),
            responder_npub: invite::npub_encode(&a.responder_pubkey)
                .unwrap_or_else(|_| a.responder_pubkey.clone()),
            matched_skill_tag: a.matched_skill_tag.clone(),
            hops: a.hops,
            display_name: held.as_ref().and_then(|p| p.display_name.clone()),
            declared_skill_tags: held.map(|p| p.skill_tags).unwrap_or_default(),
        };
        match bucket.iter_mut().find(|v| v.responder_pubkey == view.responder_pubkey) {
            Some(existing) if view.hops < existing.hops => *existing = view,
            Some(_) => {}
            None => bucket.push(view),
        }
    }

    /// The outbox ids, poll cursors and configured server list as a
    /// [`SyncState`] — what a host that persists an account writes beside
    /// the ledger.
    pub fn sync_state(&self) -> SyncState {
        SyncState {
            outbox: self.sync.pending().iter().map(|e| e.id.clone()).collect(),
            cursors: self.sync.cursor_snapshot().into_iter().collect(),
            servers: self.servers.clone(),
            admission: self.admission,
        }
    }

    /// Rehydrate a [`SyncState`] saved by an earlier process: re-queue
    /// each outbox event still held in the ledger (an id no longer there
    /// was pruned or the blob was edited — drop it), restore every cursor,
    /// and adopt a saved server list. Call once, right after opening the
    /// session and before the first `sync_now`.
    pub fn restore_sync_state(&mut self, state: &SyncState) {
        for id in &state.outbox {
            if let Some(event) = self.history.events().iter().find(|e| &e.id == id) {
                self.sync.queue(event.clone());
            }
        }
        for (base_url, created_at) in &state.cursors {
            self.sync.restore_cursor(base_url.clone(), *created_at);
        }
        // A saved list overrides the constructor default; an empty one
        // leaves that default (`QW_SERVERS` / the app's built-in) alone.
        if !state.servers.is_empty() {
            self.servers = state.servers.clone();
        }
        self.admission = state.admission;
    }

    /// Identity, invite link + QR, and one row per skill — declared
    /// (the replaceable profile) merged with what contract history earned.
    /// `reviewed` stays `None` until reviewed skills are built.
    pub fn identity_view(&self) -> Result<IdentityView, ClientError> {
        let pubkey = self.pubkey_hex();
        let npub = invite::npub_encode(&pubkey).map_err(ClientError::Invite)?;
        let invite_link = invite::invite_url(INVITE_BASE, &pubkey).map_err(ClientError::Invite)?;
        let invite_qr = invite_qr_svg(&invite_link)?;
        let events = self.history.events();

        let declared: HashSet<String> = profile::current(events, &pubkey)
            .map(|p| p.skill_tags.into_iter().collect())
            .unwrap_or_default();
        let mut skills: Vec<SkillView> = trust::earned_skills(events, &pubkey)
            .into_iter()
            .map(|s| SkillView {
                declared: declared.contains(&s.tag),
                tag: s.tag,
                reviewed: None,
                contracts: s.contracts,
                rating: s.rating,
            })
            .collect();
        for tag in &declared {
            if !skills.iter().any(|s| &s.tag == tag) {
                skills.push(SkillView {
                    tag: tag.clone(),
                    declared: true,
                    reviewed: None,
                    contracts: 0,
                    rating: None,
                });
            }
        }
        skills.sort_by(|a, b| a.tag.cmp(&b.tag));

        Ok(IdentityView {
            pubkey,
            npub,
            invite_link,
            skills,
            invite_qr,
        })
    }

    /// Follow an invite link: sign our half of the introduction, queue it,
    /// and hand back the contact so the caller can walk straight into a
    /// contract proposal for them (NIP-QW07 → NIP-QW01).
    pub fn follow(&mut self, link: &str) -> Result<FollowResult, ClientError> {
        let event = follow_invite(&self.identity, link)?;
        let contact_pubkey = event.first_tag_value("p").unwrap_or_default().to_string();
        let contact_npub =
            invite::npub_encode(&contact_pubkey).unwrap_or_else(|_| contact_pubkey.clone());
        let event_id = self.author(event)?;
        Ok(FollowResult {
            event_id,
            contact_pubkey,
            contact_npub,
        })
    }

    /// One sync pass: flush the outbox, poll for mail, persist what
    /// arrived. Send first so a reply just written is on its way before we
    /// block on downloads.
    pub fn sync_now<T: MailboxTransport>(
        &mut self,
        transport: &mut T,
    ) -> Result<SyncView, ClientError> {
        let servers: Vec<&str> = self.servers.iter().map(String::as_str).collect();
        let flushed = self.sync.flush(transport, &servers);
        let polled = self.sync.poll(transport, &servers);

        // Persist before reporting — a delivered count for events no
        // restart will find is the bug this guards.
        self.history.append(&polled.delivered)?;
        if !polled.delivered.is_empty() {
            self.refresh_node();
        }
        // A delivered 9050/9051 is referral traffic to route on: queue
        // whatever the node wants forwarded, so the next sync sends it.
        let onward = self.route_inbound(&polled.delivered);
        let routed = onward.len();
        for e in onward {
            self.sync.queue(e);
        }
        // The flush may have drained the outbox and the poll advanced a
        // cursor; fold both into whatever the host persists.
        let snapshot = self.sync_state();
        self.history.persist_sync_state(&snapshot)?;

        let mut errors: Vec<String> = flushed
            .errors
            .iter()
            .chain(polled.errors.iter())
            .map(|(server, message)| format!("{server}: {message}"))
            .collect();
        errors.dedup();

        Ok(SyncView {
            delivered: polled.delivered.len(),
            held: self.history.len(),
            rejected: polled.rejected,
            published: flushed.published,
            still_queued: flushed.still_queued,
            routed,
            errors,
        })
    }

    /// Originate a NIP-QW06 referral query for `skill` (free text or a
    /// taxonomy leaf — resolved through the bundled taxonomy, so a bad
    /// entry is an error naming it). This session is its own hop 1:
    /// forwards are signed by it and go to its own tag-similar contacts,
    /// so those contacts see it is asking — the fully-hidden-requester
    /// path needs an encrypted DM to hop 1, a later item. `sync_now`
    /// carries the forwards out and folds answers back into
    /// [`Session::referral_results`].
    pub fn find_by_skill(&mut self, skill: &str) -> Result<ReferralView, ClientError> {
        let skill_tag = crate::taxonomy::resolve(skill)
            .map_err(|reason| {
                ClientError::Profile(profile::ProfileError::Tag {
                    input: skill.to_string(),
                    reason,
                })
            })?
            .tag()
            .to_string();
        self.query_seq += 1;
        let query_id = format!("{}#{}", self.pubkey_hex(), self.query_seq);
        let outcome = self.node.begin_relay_chain(
            &query_id,
            &skill_tag,
            REFERRAL_MAX_HOPS,
            &self.pubkey_hex(),
        );
        let mut forwarded = 0;
        for d in outcome.deliveries {
            self.sync.queue(delivery_event(d));
            forwarded += 1;
        }
        // Register the query so its answers are absorbed, not relayed.
        self.referral_results.entry(query_id.clone()).or_default();
        let own_match = outcome.own_match.map(FinalAnswerView::from).map(|mut v| {
            // Our own row carries our own current profile, same as a
            // non-contact's row carries theirs.
            let mine = self.profile_view();
            v.display_name = mine.display_name;
            v.declared_skill_tags = mine.tags;
            v
        });
        Ok(ReferralView {
            query_id,
            forwarded,
            own_match,
        })
    }

    /// Answers collected so far for a query [`Session::find_by_skill`]
    /// originated, deduped by responder (lowest hop count).
    pub fn referral_results(&self, query_id: &str) -> &[FinalAnswerView] {
        self.referral_results
            .get(query_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// This identity's hop-1 contacts (everyone it has exchanged an
    /// introduction with), with the skill tags routing knows them by and
    /// this viewer's own trust read on each (§5 — always per-viewer, from
    /// held `CreditIssuance` evidence only).
    pub fn contacts(&self) -> Vec<ContactView> {
        let me = self.pubkey_hex();
        let events = self.history.events();
        let weights = trust::ScoringWeights::default();
        self.node
            .contacts()
            .map(|c| {
                let path = trust::find_trust_path(events, &me, &c.pubkey, TRUST_MAX_HOPS, None);
                ContactView {
                    npub: invite::npub_encode(&c.pubkey).unwrap_or_else(|_| c.pubkey.clone()),
                    pubkey: c.pubkey.clone(),
                    declared_skill_tags: c.cached_skill_tags.clone(),
                    earned_skill_tags: c.earned_skill_tags.clone(),
                    trust_hops: path.as_ref().map(|p| p.hops),
                    trust_score: path
                        .as_ref()
                        .map(|p| trust::score_trust_path(p, &weights)),
                    net_position: trust::net_position_with(events, &me, Some(&c.pubkey)),
                }
            })
            .collect()
    }

    /// This viewer's trust read on any pubkey: the shortest verified
    /// `CreditIssuance` path (`None` = unknown-risk, genuinely no
    /// evidence — never mapped to zero), a default decaying score over it,
    /// the bilateral net position, and the path's edge ids so a reader can
    /// spot-check each against raw relay data (§8). Never a global number.
    pub fn trust(&self, counterparty_pubkey: &str) -> TrustView {
        let me = self.pubkey_hex();
        let events = self.history.events();
        let weights = trust::ScoringWeights::default();
        let path = trust::find_trust_path(events, &me, counterparty_pubkey, TRUST_MAX_HOPS, None);
        TrustView {
            npub: invite::npub_encode(counterparty_pubkey)
                .unwrap_or_else(|_| counterparty_pubkey.to_string()),
            trust_hops: path.as_ref().map(|p| p.hops),
            trust_score: path.as_ref().map(|p| trust::score_trust_path(p, &weights)),
            net_position: trust::net_position_with(events, &me, Some(counterparty_pubkey)),
            path_edge_ids: path
                .map(|p| p.edges.iter().map(|e| e.id.clone()).collect())
                .unwrap_or_default(),
        }
    }

    /// This identity's own global net position: `Σ(delivered) − Σ(issued)`
    /// over verified `CreditIssuance`, recomputed from held events.
    pub fn net_position(&self) -> f64 {
        trust::net_position(self.history.events(), &self.pubkey_hex())
    }

    /// One NIP-QW12 anti-entropy round against `peers` — other replicas of
    /// *this same identity* (a phone, other `qw-web` boxes), never
    /// coordination servers. Verified events pulled from a peer are merged
    /// into the ledger; this replica's set is offered back so the peers
    /// converge too. Merge is set union, every view is a pure fold over the
    /// union, so a stale peer can only make a view temporarily incomplete —
    /// it heals on the next round.
    pub fn ledger_round<T: LedgerTransport>(
        &mut self,
        transport: &mut T,
        peers: &[&str],
    ) -> Result<LedgerView, ClientError> {
        // A snapshot, not a live borrow: `self.ledger` is mutated by the
        // round while `self.history` is read for coverage.
        let held: Vec<Event> = self.history.events().to_vec();
        let round = self.ledger.round(transport, peers, &held);

        self.history.append(&round.received)?;
        if !round.received.is_empty() {
            self.refresh_node();
        }
        let snapshot = self.sync_state();
        self.history.persist_sync_state(&snapshot)?;

        let mut errors: Vec<String> = round
            .errors
            .iter()
            .map(|(peer, message)| format!("{peer}: {message}"))
            .collect();
        errors.dedup();

        Ok(LedgerView {
            received: round.received.len(),
            pushed: round.pushed,
            rejected: round.rejected,
            held: self.history.len(),
            errors,
        })
    }

    /// The most recent profile this identity published, or an empty one.
    pub fn profile_view(&self) -> ProfileView {
        let current = profile::current(self.history.events(), &self.pubkey_hex());
        ProfileView {
            display_name: current.as_ref().and_then(|p| p.display_name.clone()),
            tags: current.map(|p| p.skill_tags).unwrap_or_default(),
        }
    }

    /// The profile this viewer holds for some *other* pubkey — a contact's,
    /// or a non-contact's that rode back on a referral answer (NIP-QW06).
    /// `None` when none is held: the network never resolves an npub to a
    /// profile from a central directory, only from evidence in hand.
    pub fn profile_of(&self, pubkey: &str) -> Option<ProfileView> {
        profile::current(self.history.events(), pubkey).map(|p| ProfileView {
            display_name: p.display_name,
            tags: p.skill_tags,
        })
    }

    /// Resolve, sign and queue a new replaceable profile event, its
    /// `revision` one past the last this identity published. `sync_now`
    /// publishes it.
    pub fn set_profile(&mut self, edit: ProfileEdit) -> Result<String, ClientError> {
        let event = profile::build_signed(
            &self.identity,
            self.history.events(),
            edit.display_name.as_deref(),
            &edit.tags,
        )?;
        self.author(event)
    }

    /// Every negotiation this identity is a party to, newest activity
    /// first.
    pub fn negotiations(&self) -> Vec<NegotiationView> {
        let mut rows = negotiation::list(self.history.events(), &self.pubkey_hex());
        // The admission pre-filter is an *inbound* gate: apply it only to a
        // proposal still open that the counterparty sent us. A failing row
        // is flagged, not dropped — the human may always admit it.
        if self.admission != trust::AdmissionPolicy::default() {
            for n in &mut rows {
                if !n.am_client && n.state == "negotiating" {
                    let domain = n.head_terms.skill_tags.first().map(String::as_str);
                    n.passes_filter = self.admits(&n.counterparty_pubkey, domain);
                }
            }
        }
        rows
    }

    /// Sign and queue a fresh proposal (kind 9000). Returns the offer id.
    pub fn propose(&mut self, args: ProposeArgs) -> Result<String, ClientError> {
        let event = negotiation::propose(
            &self.identity,
            &args.counterparty,
            args.from_introduction.as_deref(),
            &args.terms,
        )?;
        self.author(event)
    }

    /// Reject-amend-reapply in one event: supersede the current head terms
    /// and hand the proposal back (kind 9004).
    pub fn counter(&mut self, args: CounterArgs) -> Result<String, ClientError> {
        let event = negotiation::counter(
            &self.identity,
            self.history.events(),
            &args.offer_event_id,
            &args.terms,
        )?;
        self.author(event)
    }

    /// The worker's Accept against the current head (kind 9001) — the only
    /// event that ends the exchange.
    pub fn accept(&mut self, args: AcceptArgs) -> Result<String, ClientError> {
        let event = negotiation::accept(
            &self.identity,
            self.history.events(),
            &args.offer_event_id,
            args.note.as_deref(),
        )?;
        self.author(event)
    }

    /// Attach a dispute annotation (kind 9030, NIP-QW04) — a reply, an
    /// audit request, or a third-party audit opinion — to a contract. The
    /// record it targets is never touched; the annotation is queued for
    /// delivery like any authored event. Returns the annotation's id.
    pub fn annotate(&mut self, args: AnnotateArgs) -> Result<String, ClientError> {
        let event = negotiation::annotate(&self.identity, self.history.events(), &args)?;
        self.author(event)
    }
}

// --- views handed back to a shell (Serialize; identical shape for Tauri
//     IPC and an HTTP response) --------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct IdentityView {
    pub pubkey: String,
    pub npub: String,
    pub invite_link: String,
    /// One row per skill, whatever the evidence — not three lists. A skill
    /// is the row; the evidence is decoration on it.
    pub skills: Vec<SkillView>,
    /// The invite link as a scannable SVG, built in Rust because the UI
    /// has no bundler.
    pub invite_qr: String,
}

/// A skill and its badges. Both badge fields are absent for most skills.
#[derive(Debug, Clone, Serialize)]
pub struct SkillView {
    pub tag: String,
    /// Declared by the holder in their published profile. A skill that
    /// only shows up in contract history is `false`.
    pub declared: bool,
    /// A broker's review. Unpopulated — reviewed skills are specced and
    /// unbuilt; the field exists so the badge renderer need not change
    /// shape when they land.
    pub reviewed: Option<String>,
    /// Countersigned contracts carrying this tag.
    pub contracts: usize,
    /// Mean counterparty rating; `None` when nobody recorded one — absent
    /// is not zero.
    pub rating: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncView {
    pub delivered: usize,
    pub rejected: usize,
    /// Records held after this pass — the running total, not the delta. It
    /// is what every later computation reads, so watching it stay put is
    /// how you notice nothing landed.
    pub held: usize,
    pub published: usize,
    pub still_queued: usize,
    /// Referral events (9050/9051) this pass queued to forward on a
    /// contact's behalf — the client acting as a relay hop.
    pub routed: usize,
    pub errors: Vec<String>,
}

/// What [`Session::find_by_skill`] kicked off.
#[derive(Debug, Clone, Serialize)]
pub struct ReferralView {
    /// The correlator every hop of this query carries; pass it to
    /// [`Session::referral_results`].
    pub query_id: String,
    /// Contacts the first forwards went to.
    pub forwarded: usize,
    /// Set only if this identity's *own* skills match the query.
    pub own_match: Option<FinalAnswerView>,
}

/// A responder a referral query turned up. `display_name` and
/// `declared_skill_tags` are populated from that responder's own signed
/// profile (kind `10020`) when the client holds one — for a non-contact,
/// that is the profile the answer carried back (NIP-QW06 "Profile on the
/// answer"). Both are empty until such a profile is held; the identity is
/// always `responder_npub`.
#[derive(Debug, Clone, Serialize)]
pub struct FinalAnswerView {
    pub responder_pubkey: String,
    pub responder_npub: String,
    pub matched_skill_tag: String,
    /// Path length from this identity to the responder.
    pub hops: u8,
    /// From the responder's held profile; `None` when none is held.
    pub display_name: Option<String>,
    /// Every skill tag on that profile — the "open to view" self-
    /// description, not just the one tag the query matched.
    pub declared_skill_tags: Vec<String>,
}

impl From<FinalAnswer> for FinalAnswerView {
    fn from(a: FinalAnswer) -> Self {
        Self {
            responder_npub: invite::npub_encode(&a.responder_pubkey)
                .unwrap_or_else(|_| a.responder_pubkey.clone()),
            responder_pubkey: a.responder_pubkey,
            matched_skill_tag: a.matched_skill_tag,
            hops: a.hops,
            display_name: None,
            declared_skill_tags: Vec::new(),
        }
    }
}

/// One hop-1 contact and the skill tags routing knows them by, plus this
/// viewer's own trust read.
#[derive(Debug, Clone, Serialize)]
pub struct ContactView {
    pub pubkey: String,
    pub npub: String,
    /// From their published profile, as this client currently holds it.
    pub declared_skill_tags: Vec<String>,
    /// From contracts they completed and a counterparty countersigned —
    /// not inflatable.
    pub earned_skill_tags: Vec<String>,
    /// Hops on the shortest verified `CreditIssuance` path; `None` =
    /// unknown-risk (no evidence, *not* zero).
    pub trust_hops: Option<u8>,
    /// A default decaying score over that path; `None` when unknown-risk.
    pub trust_score: Option<f64>,
    /// `Σ(they delivered to me) − Σ(I issued to them)`, verified.
    pub net_position: f64,
}

/// This viewer's trust read on some pubkey (§5 — never a global number).
#[derive(Debug, Clone, Serialize)]
pub struct TrustView {
    pub npub: String,
    pub trust_hops: Option<u8>,
    pub trust_score: Option<f64>,
    pub net_position: f64,
    /// The `CreditIssuance` event ids forming the path — spot-check each
    /// against raw relay data rather than trusting the score (§8).
    pub path_edge_ids: Vec<String>,
}

/// What one [`Session::ledger_round`] did.
#[derive(Debug, Clone, Serialize)]
pub struct LedgerView {
    /// New events merged from peers this round.
    pub received: usize,
    /// Events peers accepted as new to them.
    pub pushed: usize,
    /// Events a peer served that failed verification — a peer to rank down.
    pub rejected: usize,
    /// Running total held after the merge.
    pub held: usize,
    pub errors: Vec<String>,
}

/// What following a link leaves a shell holding: the queued intro's id and
/// the contact it connects to.
#[derive(Debug, Clone, Serialize)]
pub struct FollowResult {
    pub event_id: String,
    pub contact_pubkey: String,
    pub contact_npub: String,
}

// --- helpers ------------------------------------------------------------

/// The pubkeys `me` has exchanged a NIP-QW07 introduction with, from held
/// kind-9060 events: the recipient of an intro `me` signed, the signer of
/// one addressed to `me`, and a third party introduced *to* `me`.
fn introduction_partners(events: &[Event], me: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut push = |pk: &str, out: &mut Vec<String>| {
        if pk != me && !pk.is_empty() && seen.insert(pk.to_string()) {
            out.push(pk.to_string());
        }
    };
    for e in events.iter().filter(|e| e.kind == KIND_INTRODUCTION) {
        let recipient = e.first_tag_value("p").unwrap_or_default();
        let subject = serde_json::from_str::<Introduction>(&e.content)
            .ok()
            .map(|i| i.subject_pubkey)
            .unwrap_or_default();
        if e.pubkey == me {
            push(recipient, &mut out);
        } else if recipient == me {
            push(&e.pubkey, &mut out);
            push(&subject, &mut out); // a mutual intro names a third party
        }
    }
    out
}

fn delivery_event(d: Delivery) -> Event {
    match d {
        Delivery::Query { event, .. } | Delivery::Answer { event, .. } => event,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::negotiation::TermsDraft;
    use crate::{EventStore, Vault};
    use qw_node::sync::PublishOutcome;

    fn session_at(dir: &std::path::Path) -> Session {
        Session::open(
            &Vault::at(dir),
            Box::new(EventStore::open(dir).unwrap()),
            vec!["http://unused.invalid".to_string()],
        )
        .unwrap()
    }

    fn terms(skill: &str, hours: f64, rate: f64, text: &str) -> TermsDraft {
        TermsDraft {
            skill_tags: vec![skill.to_string()],
            hours,
            rate,
            ko: None,
            km: None,
            terms: text.to_string(),
        }
    }

    #[test]
    fn the_identity_survives_reopening_the_same_dir() {
        let dir = tempfile::tempdir().unwrap();
        let a = session_at(dir.path()).pubkey_hex();
        let b = session_at(dir.path()).pubkey_hex();
        assert_eq!(a, b);
    }

    #[test]
    fn set_profile_records_the_event_locally_bumps_revision_and_queues_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = session_at(dir.path());

        s.set_profile(ProfileEdit {
            display_name: Some("Vlad".to_string()),
            tags: vec!["Rust Lang".to_string()],
        })
        .unwrap();

        // A profile event carries no `p` tag, so a mailbox would never
        // return it — it has to be in the ledger the moment it is signed,
        // not after a round-trip. And still queued for delivery.
        assert_eq!(s.outbox().len(), 1);
        assert_eq!(s.outbox()[0].revision(), 1);
        assert_eq!(s.profile_view().tags, vec!["it/backend/languages#rust"]);
        assert_eq!(s.profile_view().display_name.as_deref(), Some("Vlad"));
        assert!(s
            .identity_view()
            .unwrap()
            .skills
            .iter()
            .any(|sk| sk.tag == "it/backend/languages#rust" && sk.declared));

        // A second edit is revision 2 and supersedes the first everywhere
        // a view reads the current profile.
        s.set_profile(ProfileEdit {
            display_name: Some("Vlad".to_string()),
            tags: vec!["Go Lang".to_string()],
        })
        .unwrap();
        assert_eq!(s.outbox().last().unwrap().revision(), 2);
        assert_eq!(s.profile_view().tags, vec!["it/backend/languages#go"]);
        let skills = s.identity_view().unwrap().skills;
        assert!(skills.iter().any(|sk| sk.tag == "it/backend/languages#go" && sk.declared));
        assert!(
            !skills.iter().any(|sk| sk.tag == "it/backend/languages#rust"),
            "the superseded tag is gone from the declared set"
        );
    }

    #[test]
    fn propose_then_counter_then_accept_across_two_sessions() {
        let cdir = tempfile::tempdir().unwrap();
        let wdir = tempfile::tempdir().unwrap();
        let mut client = session_at(cdir.path());
        let mut worker = session_at(wdir.path());

        client
            .propose(ProposeArgs {
                counterparty: worker.pubkey_hex(),
                from_introduction: None,
                terms: terms("rust", 8.0, 40.0, "sprint 12"),
            })
            .unwrap();

        // The initiator sees its own negotiation at once — the offer is
        // `p`-tagged to the worker, so it would never come back through
        // the client's own mailbox.
        let cn = client.negotiations();
        assert!(cn[0].am_client && !cn[0].your_move && !cn[0].can_accept);
        let offer = client.outbox()[0].clone();

        // Worker receives the offer and counters.
        worker.ingest(&[offer.clone()]).unwrap();
        let wn = worker.negotiations();
        assert!(!wn[0].am_client && wn[0].can_accept && wn[0].your_move);
        let counter_id = worker
            .counter(CounterArgs {
                offer_event_id: offer.id.clone(),
                terms: terms("rust", 8.0, 55.0, "sprint 12"),
            })
            .unwrap();
        let counter_evt = worker
            .outbox()
            .iter()
            .find(|e| e.id == counter_id)
            .unwrap()
            .clone();

        // Client sees the new head; it is now the client's move.
        client.ingest(&[counter_evt]).unwrap();
        let cn = client.negotiations();
        assert!(cn[0].your_move);
        assert_eq!(cn[0].head_terms.rate, 55.0);
        assert_eq!(cn[0].rounds, 1);

        // Worker accepts its own counter; both sides converge.
        worker
            .accept(AcceptArgs {
                offer_event_id: offer.id.clone(),
                note: None,
            })
            .unwrap();
        assert_eq!(worker.negotiations()[0].state, "accepted");
        let accept_evt = worker.outbox().last().unwrap().clone();

        client.ingest(&[accept_evt]).unwrap();
        assert_eq!(client.negotiations()[0].state, "accepted");
    }

    #[test]
    fn a_dispute_annotation_rides_to_the_counterparty_and_shows_on_both_sides() {
        use crate::negotiation::AnnotateArgs;

        let cdir = tempfile::tempdir().unwrap();
        let wdir = tempfile::tempdir().unwrap();
        let mut client = session_at(cdir.path());
        let mut worker = session_at(wdir.path());

        client
            .propose(ProposeArgs {
                counterparty: worker.pubkey_hex(),
                from_introduction: None,
                terms: terms("rust", 8.0, 40.0, "the milestone that never came"),
            })
            .unwrap();
        let offer = client.outbox()[0].clone();
        worker.ingest(std::slice::from_ref(&offer)).unwrap();

        // The worker files an audit request against the contract.
        let ann_id = worker
            .annotate(AnnotateArgs {
                offer_event_id: offer.id.clone(),
                kind: "audit_request".into(),
                body: "milestone was never delivered".into(),
                outcome: None,
                target: None,
            })
            .unwrap();
        let wn = &worker.negotiations()[0];
        assert_eq!(wn.disputes.len(), 1);
        assert!(wn.disputes[0].mine && wn.under_review);

        // It is p-tagged to the client, so a plain mailbox delivers it.
        let ann_evt = worker
            .outbox()
            .iter()
            .find(|e| e.id == ann_id)
            .unwrap()
            .clone();
        assert_eq!(
            ann_evt.first_tag_value("p"),
            Some(client.pubkey_hex().as_str())
        );
        client.ingest(&[ann_evt]).unwrap();
        let cn = &client.negotiations()[0];
        assert_eq!(cn.disputes.len(), 1);
        assert_eq!(cn.disputes[0].annotation_type, "audit_request");
        assert!(
            !cn.disputes[0].mine,
            "the worker signed it, not this client"
        );
        assert!(cn.under_review);

        // A stranger cannot reply, but the client (a party) can.
        assert!(client
            .annotate(AnnotateArgs {
                offer_event_id: offer.id.clone(),
                kind: "reply".into(),
                body: "it shipped on the 3rd, evidence attached".into(),
                outcome: None,
                target: None,
            })
            .is_ok());
        assert_eq!(client.negotiations()[0].disputes.len(), 2);
    }

    #[test]
    fn sync_now_flushes_the_outbox_and_persists_delivered() {
        #[derive(Default)]
        struct Fake {
            held: Vec<Event>,
        }
        impl MailboxTransport for Fake {
            type Error = String;
            fn fetch(
                &mut self,
                _base: &str,
                pubkey: &str,
                _since: Option<u64>,
            ) -> Result<Vec<Event>, String> {
                Ok(self
                    .held
                    .iter()
                    .filter(|e| e.first_tag_value("p") == Some(pubkey))
                    .cloned()
                    .collect())
            }
            fn publish(&mut self, _base: &str, event: &Event) -> Result<PublishOutcome, String> {
                if self.held.iter().any(|e| e.id == event.id) {
                    return Ok(PublishOutcome::AlreadyHeld);
                }
                self.held.push(event.clone());
                Ok(PublishOutcome::Accepted)
            }
        }

        let cdir = tempfile::tempdir().unwrap();
        let wdir = tempfile::tempdir().unwrap();
        let mut client = Session::with_identity(
            Identity::generate(),
            Box::new(EventStore::open(cdir.path()).unwrap()),
            vec!["http://one".to_string()],
        );
        let mut worker = Session::with_identity(
            Identity::generate(),
            Box::new(EventStore::open(wdir.path()).unwrap()),
            vec!["http://one".to_string()],
        );

        client
            .propose(ProposeArgs {
                counterparty: worker.pubkey_hex(),
                from_introduction: None,
                terms: terms("rust", 4.0, 30.0, "fix the flaky test"),
            })
            .unwrap();

        let mut wire = Fake::default();
        let sent = client.sync_now(&mut wire).unwrap();
        assert_eq!(sent.published, 1);
        assert!(client.outbox().is_empty());

        let got = worker.sync_now(&mut wire).unwrap();
        assert_eq!(got.delivered, 1);
        assert_eq!(got.held, 1);
        assert_eq!(worker.negotiations().len(), 1);
    }

    #[test]
    fn sync_state_snapshots_and_restores_the_outbox_and_cursors() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = session_at(dir.path());
        a.propose(ProposeArgs {
            counterparty: "b".repeat(64),
            from_introduction: None,
            terms: terms("rust", 4.0, 30.0, "one"),
        })
        .unwrap();
        assert_eq!(a.outbox().len(), 1);

        let mut snap = a.sync_state();
        assert_eq!(snap.outbox, vec![a.outbox()[0].id.clone()]);
        snap.cursors.insert("http://srv".into(), 4242);

        // A fresh Session over the same dir now auto-restores from the
        // `sync-state.json` sidecar `author()` wrote — the queued offer
        // (`p`-tagged to the counterparty, so no sync would bring it back)
        // is there without an explicit restore.
        let mut b = session_at(dir.path());
        assert_eq!(b.outbox().len(), 1);
        assert_eq!(b.outbox()[0].id, a.outbox()[0].id);
        // a cursor set only in the in-memory snapshot still needs one
        b.restore_sync_state(&snap);
        assert_eq!(b.sync_state().cursors.get("http://srv"), Some(&4242));
    }

    #[test]
    fn restore_sync_state_drops_an_outbox_id_no_longer_in_the_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = session_at(dir.path());
        s.restore_sync_state(&SyncState {
            outbox: vec!["deadbeef".repeat(8)],
            cursors: Default::default(),
            servers: Vec::new(),
            admission: Default::default(),
        });
        assert!(s.outbox().is_empty(), "an id with no matching event is skipped");
    }

    #[test]
    fn the_event_store_sidecar_carries_servers_and_the_outbox_across_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut s = session_at(dir.path());
            s.set_servers(vec!["https://relay.one".into(), "https://relay.two".into()])
                .unwrap();
            s.set_admission_policy(Some(2.0), None).unwrap();
            s.propose(ProposeArgs {
                counterparty: "b".repeat(64),
                from_introduction: None,
                terms: terms("rust", 1.0, 10.0, "x"),
            })
            .unwrap();
            assert!(dir.path().join("sync-state.json").exists());
        } // session dropped — nothing in memory survives

        let s2 = session_at(dir.path());
        assert_eq!(s2.servers(), ["https://relay.one", "https://relay.two"]);
        assert_eq!(s2.admission_policy(), (Some(2.0), None));
        assert_eq!(s2.outbox().len(), 1, "the queued offer came back from the sidecar");
    }

    #[test]
    fn set_servers_validates_dedups_and_rides_the_sync_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = session_at(dir.path());

        // non-http entries are refused, naming the offender
        assert!(matches!(
            s.set_servers(vec!["ftp://nope".into()]),
            Err(ClientError::Config(_))
        ));
        // an all-empty list is refused — a client with no server is mute
        assert!(matches!(
            s.set_servers(vec!["   ".into()]),
            Err(ClientError::Config(_))
        ));

        s.set_servers(vec![
            "  https://a.example  ".into(),
            "http://b.example".into(),
            "https://a.example".into(), // dup of the first, trimmed
            String::new(),              // dropped
        ])
        .unwrap();
        assert_eq!(s.servers(), ["https://a.example", "http://b.example"]);
        // it is in the snapshot a persisting host writes
        assert_eq!(
            s.sync_state().servers,
            vec!["https://a.example".to_string(), "http://b.example".to_string()]
        );

        // a fresh session over the SAME dir now auto-adopts the saved list
        // from the sidecar — that is the whole point of the feature
        let b = session_at(dir.path());
        assert_eq!(b.servers(), ["https://a.example", "http://b.example"]);

        // a session over a DIFFERENT dir is on its constructor default;
        // `restore_sync_state` with an empty `servers` leaves it alone
        let other = tempfile::tempdir().unwrap();
        let mut c = session_at(other.path());
        assert_eq!(c.servers(), ["http://unused.invalid"]);
        c.restore_sync_state(&SyncState::default());
        assert_eq!(c.servers(), ["http://unused.invalid"]);
    }

    #[test]
    fn admission_filter_flags_an_inbound_proposal_and_rides_the_sync_state() {
        let cdir = tempfile::tempdir().unwrap();
        let wdir = tempfile::tempdir().unwrap();
        let mut client = session_at(cdir.path());
        let mut worker = session_at(wdir.path());

        client
            .propose(ProposeArgs {
                counterparty: worker.pubkey_hex(),
                from_introduction: None,
                terms: terms("rust", 4.0, 30.0, "fix the flaky test"),
            })
            .unwrap();
        let offer = client.outbox()[0].clone();
        worker.ingest(std::slice::from_ref(&offer)).unwrap();

        // no filter: the inbound proposal passes
        assert_eq!(worker.admission_policy(), (None, None));
        assert!(worker.negotiations()[0].passes_filter);

        // a min-reputation threshold: the client is unknown-risk to the
        // worker (no credit path), so the row is flagged — but still shown
        worker.set_admission_policy(Some(1.0), None).unwrap();
        let n = &worker.negotiations()[0];
        assert!(!n.passes_filter);
        assert!(!n.am_client && n.state == "negotiating");

        // negative thresholds are refused
        assert!(matches!(
            worker.set_admission_policy(Some(-1.0), None),
            Err(ClientError::Config(_))
        ));

        // it persists: a reopen over the same dir adopts it from the
        // sidecar automatically
        let snap = worker.sync_state();
        assert_eq!(snap.admission.min_reputation, Some(1.0));
        let w2 = session_at(wdir.path());
        assert_eq!(w2.admission_policy(), (Some(1.0), None));

        // the *client's* own view of the same proposal is never filtered —
        // the gate is inbound only
        assert!(client.negotiations()[0].passes_filter);
    }

    #[test]
    fn position_limit_scales_with_the_requesters_recent_completed_volume() {
        let cdir = tempfile::tempdir().unwrap();
        let wdir = tempfile::tempdir().unwrap();
        let client = Identity::generate();
        let worker = Identity::generate();
        let mut cs = Session::with_identity(
            Identity::from_secret_bytes(client.secret_bytes()).unwrap(),
            Box::new(EventStore::open(cdir.path()).unwrap()),
            vec![],
        );
        let mut ws = Session::with_identity(
            Identity::from_secret_bytes(worker.secret_bytes()).unwrap(),
            Box::new(EventStore::open(wdir.path()).unwrap()),
            vec![],
        );

        // A verified 10-Quant CreditIssuance worker -> client (worker is
        // the issuer). Worker's bilateral net_position with client is -10
        // (|10| of exposure), and client's recent completed volume is 10.
        // The issuance is `p`-tagged to *client*, so it does not attach to
        // the fresh client -> worker proposal, which stays `negotiating`.
        let edge = credit(&worker, &client, 10.0);
        cs.ingest(&edge).unwrap();
        ws.ingest(&edge).unwrap();

        // client sends a fresh proposal; worker holds it
        cs.propose(ProposeArgs {
            counterparty: worker.nostr_pubkey_hex(),
            from_introduction: None,
            terms: terms("rust", 4.0, 30.0, "more work"),
        })
        .unwrap();
        let offer = cs.outbox().last().unwrap().clone();
        ws.ingest(std::slice::from_ref(&offer)).unwrap();

        let open = |s: &Session| {
            s.negotiations()
                .into_iter()
                .find(|n| n.state == "negotiating")
                .unwrap()
        };

        // limit = 1× client's recent volume (10) → ceiling 10 → |10| not
        // over → admitted
        ws.set_admission_policy(None, Some(1.0)).unwrap();
        assert!(open(&ws).passes_filter);

        // limit = 0.5× → ceiling 5 → |10| over → flagged
        ws.set_admission_policy(None, Some(0.5)).unwrap();
        assert!(!open(&ws).passes_filter);

        // 0× → ceiling 0 → any standing imbalance flags — a fresh key with
        // no completed work gets no exposure headroom at all
        ws.set_admission_policy(None, Some(0.0)).unwrap();
        assert!(!open(&ws).passes_filter);
    }

    // --- NIP-QW06 referral routing (Node in the client) -------------

    /// A p-tag-routing fake mailbox that ferries every published event to
    /// whoever it is addressed to.
    #[derive(Default)]
    struct Wire {
        held: Vec<Event>,
    }
    impl MailboxTransport for Wire {
        type Error = String;
        fn fetch(
            &mut self,
            _b: &str,
            pubkey: &str,
            _s: Option<u64>,
        ) -> Result<Vec<Event>, String> {
            Ok(self
                .held
                .iter()
                .filter(|e| e.first_tag_value("p") == Some(pubkey))
                .cloned()
                .collect())
        }
        fn publish(&mut self, _b: &str, event: &Event) -> Result<PublishOutcome, String> {
            if self.held.iter().any(|e| e.id == event.id) {
                return Ok(PublishOutcome::AlreadyHeld);
            }
            self.held.push(event.clone());
            Ok(PublishOutcome::Accepted)
        }
    }

    fn peer_session(dir: &std::path::Path, id: Identity) -> Session {
        Session::with_identity(
            id,
            Box::new(EventStore::open(dir).unwrap()),
            vec!["wire".to_string()],
        )
    }

    #[test]
    fn introduction_partners_reads_both_directions_and_a_third_party() {
        let me = Identity::generate();
        let followed = Identity::generate();
        let follower = Identity::generate();
        let third = Identity::generate();

        // one I signed, one addressed to me, one addressed to me that
        // names a third party
        let mine = follow_invite(
            &me,
            &format!("https://knownby.work/i/{}", invite::npub_encode(&followed.nostr_pubkey_hex()).unwrap()),
        )
        .unwrap();
        let to_me = follow_invite(
            &follower,
            &format!("https://knownby.work/i/{}", invite::npub_encode(&me.nostr_pubkey_hex()).unwrap()),
        )
        .unwrap();
        let mutual = qw_protocol::events::introduction(
            &follower.nostr_pubkey_hex(),
            &me.nostr_pubkey_hex(),
            &Introduction {
                subject_pubkey: third.nostr_pubkey_hex(),
                chain: vec![],
                note: None,
                via: None,
            },
        )
        .sign(&follower);

        let partners = introduction_partners(&[mine, to_me, mutual], &me.nostr_pubkey_hex());
        assert!(partners.contains(&followed.nostr_pubkey_hex()));
        assert!(partners.contains(&follower.nostr_pubkey_hex()));
        assert!(partners.contains(&third.nostr_pubkey_hex()));
        assert!(!partners.contains(&me.nostr_pubkey_hex()));
    }

    #[test]
    fn a_referral_query_reaches_a_contact_with_the_skill_and_the_answer_comes_back() {
        let adir = tempfile::tempdir().unwrap();
        let bdir = tempfile::tempdir().unwrap();
        let mut a = peer_session(adir.path(), Identity::generate());
        let mut b = peer_session(bdir.path(), Identity::generate());

        // B does Rust; A follows B.
        b.set_profile(ProfileEdit { display_name: None, tags: vec!["Rust Lang".into()] })
            .unwrap();
        a.follow(&format!(
            "https://knownby.work/i/{}",
            invite::npub_encode(&b.pubkey_hex()).unwrap()
        ))
        .unwrap();

        let mut wire = Wire::default();
        // exchange introductions: A's follow reaches B, both derive a contact
        a.sync_now(&mut wire).unwrap();
        b.sync_now(&mut wire).unwrap();
        assert!(b.contacts().iter().any(|c| c.pubkey == a.pubkey_hex()));
        assert!(a.contacts().iter().any(|c| c.pubkey == b.pubkey_hex()));

        // A asks its network for a Rust dev — it does not hold B's profile
        // (a 10020 has no `p` tag), so B is a fallback forward target.
        let rust = "it/backend/languages#rust";
        let referral = a.find_by_skill(rust).unwrap();
        assert_eq!(referral.forwarded, 1);
        assert!(referral.own_match.is_none());
        assert!(a.referral_results(&referral.query_id).is_empty());

        a.sync_now(&mut wire).unwrap(); // flush the 9050
        let bsync = b.sync_now(&mut wire).unwrap(); // B answers
        assert_eq!(bsync.routed, 1, "B queued an answer to relay back");
        b.sync_now(&mut wire).unwrap(); // flush the 9051
        a.sync_now(&mut wire).unwrap(); // A absorbs it

        let results = a.referral_results(&referral.query_id);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].responder_pubkey, b.pubkey_hex());
        assert_eq!(results[0].matched_skill_tag, rust);
        assert_eq!(results[0].hops, 1);
        assert!(results[0].responder_npub.starts_with("npub1"));
        // The answer carried B's own profile, so A — which never held it —
        // now sees B's whole declared self-description, not just the tag
        // the query matched (NIP-QW06 "Profile on the answer").
        assert_eq!(results[0].declared_skill_tags, vec![rust.to_string()]);
        assert_eq!(
            a.profile_of(&b.pubkey_hex()).unwrap().tags,
            vec![rust.to_string()]
        );
    }

    #[test]
    fn a_non_contact_two_hops_away_becomes_viewable_through_the_answer() {
        // A — B — C: A and C share no edge; A reaches C only through B.
        let adir = tempfile::tempdir().unwrap();
        let bdir = tempfile::tempdir().unwrap();
        let cdir = tempfile::tempdir().unwrap();
        let mut a = peer_session(adir.path(), Identity::generate());
        let mut b = peer_session(bdir.path(), Identity::generate());
        let mut c = peer_session(cdir.path(), Identity::generate());

        // C publishes a two-skill profile with a display name — the
        // "open to view in the network" surface.
        c.set_profile(ProfileEdit {
            display_name: Some("Dana".into()),
            tags: vec!["Rust Lang".into(), "Go Lang".into()],
        })
        .unwrap();

        // A follows B; B follows C. Nobody follows across the gap.
        a.follow(&format!(
            "https://knownby.work/i/{}",
            invite::npub_encode(&b.pubkey_hex()).unwrap()
        ))
        .unwrap();
        b.follow(&format!(
            "https://knownby.work/i/{}",
            invite::npub_encode(&c.pubkey_hex()).unwrap()
        ))
        .unwrap();

        let mut wire = Wire::default();
        for _ in 0..2 {
            a.sync_now(&mut wire).unwrap();
            b.sync_now(&mut wire).unwrap();
            c.sync_now(&mut wire).unwrap();
        }
        assert!(a.contacts().iter().any(|x| x.pubkey == b.pubkey_hex()));
        assert!(c.contacts().iter().any(|x| x.pubkey == b.pubkey_hex()));
        assert!(
            !a.contacts().iter().any(|x| x.pubkey == c.pubkey_hex()),
            "A and C must not be contacts — the whole point of the test"
        );

        let rust = "it/backend/languages#rust";
        let referral = a.find_by_skill(rust).unwrap();
        assert!(referral.own_match.is_none());

        // A -> B (9050), B -> C (relayed 9050), C -> B (9051 + profile),
        // B -> A (relayed 9051 + profile). A node publishes what it queued
        // only on its *next* sync, so the round trip needs a pass per hop.
        for _ in 0..6 {
            a.sync_now(&mut wire).unwrap();
            b.sync_now(&mut wire).unwrap();
            c.sync_now(&mut wire).unwrap();
        }

        let results = a.referral_results(&referral.query_id);
        assert_eq!(results.len(), 1, "C came back through the chain");
        assert_eq!(results[0].responder_pubkey, c.pubkey_hex());
        assert_eq!(results[0].matched_skill_tag, rust);
        assert!(results[0].hops >= 2, "C is at least two hops from A");
        // A holds no edge to C, yet now sees C's full self-description.
        assert_eq!(results[0].display_name.as_deref(), Some("Dana"));
        assert!(results[0].declared_skill_tags.contains(&rust.to_string()));
        assert!(results[0]
            .declared_skill_tags
            .contains(&"it/backend/languages#go".to_string()));
        let held = a
            .profile_of(&c.pubkey_hex())
            .expect("A now holds C's profile");
        assert_eq!(held.display_name.as_deref(), Some("Dana"));
        assert_eq!(held.tags.len(), 2);
    }

    #[test]
    fn find_by_skill_matches_own_earned_skills() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = session_at(dir.path());
        s.set_profile(ProfileEdit { display_name: None, tags: vec!["Rust Lang".into()] })
            .unwrap();
        let r = s.find_by_skill("it/backend/languages#rust").unwrap();
        assert!(r.own_match.is_some());
        assert_eq!(r.own_match.unwrap().matched_skill_tag, "it/backend/languages#rust");
    }

    // --- §5 trust display -----------------------------------------------

    /// A verified `CreditIssuance` from `issuer` to `subject` for `quants`,
    /// plus the offer + completion it rests on.
    fn credit(issuer: &Identity, subject: &Identity, quants: f64) -> Vec<Event> {
        use qw_protocol::contract::{assemble_credit_issuance, sign_credit_issuance_payload};
        use qw_protocol::events::kinds::{job_completion, job_offer, JobCompletion, JobOffer};
        use qw_protocol::events::QuantAmount;

        let offer = job_offer(
            &issuer.nostr_pubkey_hex(),
            &subject.nostr_pubkey_hex(),
            &JobOffer {
                skill_tags: vec!["it/backend/languages#rust".into()],
                hours: 1.0,
                rate: 1.0,
                ko: None,
                km: None,
                terms: "t".into(),
            },
        )
        .sign(issuer);
        let completion = job_completion(
            &subject.nostr_pubkey_hex(),
            &issuer.nostr_pubkey_hex(),
            &offer.id,
            &JobCompletion { rating: None, note: None },
        )
        .sign(subject);
        let amount = QuantAmount::Exact { quants };
        let issuance = assemble_credit_issuance(
            &issuer.nostr_pubkey_hex(),
            &subject.nostr_pubkey_hex(),
            &completion.id,
            amount,
            sign_credit_issuance_payload(issuer, &completion.id, &amount),
            sign_credit_issuance_payload(subject, &completion.id, &amount),
        )
        .sign(issuer);
        vec![offer, completion, issuance]
    }

    #[test]
    fn trust_reads_a_verified_path_and_is_unknown_risk_without_one() {
        let dir = tempfile::tempdir().unwrap();
        let me = Identity::generate();
        let mut s = Session::with_identity(
            Identity::from_secret_bytes(me.secret_bytes()).unwrap(),
            Box::new(EventStore::open(dir.path()).unwrap()),
            vec![],
        );

        let worked_with_me = Identity::generate();
        let stranger = Identity::generate();

        // no evidence yet: unknown-risk, never zero
        let t = s.trust(&worked_with_me.nostr_pubkey_hex());
        assert_eq!(t.trust_hops, None);
        assert_eq!(t.trust_score, None);
        assert_eq!(t.net_position, 0.0);
        assert!(t.path_edge_ids.is_empty());

        // they delivered 10 Quants of verified work to me
        s.ingest(&credit(&worked_with_me, &me, 10.0)).unwrap();
        let t = s.trust(&worked_with_me.nostr_pubkey_hex());
        assert_eq!(t.trust_hops, Some(1));
        assert!(t.trust_score.unwrap() > 0.0);
        assert_eq!(t.net_position, 10.0, "they delivered to me, nothing back");
        assert_eq!(t.path_edge_ids.len(), 1);
        assert_eq!(s.net_position(), 10.0);

        // the stranger has no path
        assert_eq!(s.trust(&stranger.nostr_pubkey_hex()).trust_hops, None);
    }

    #[test]
    fn contacts_carry_this_viewers_trust_read() {
        let dir = tempfile::tempdir().unwrap();
        let me = Identity::generate();
        let mut s = Session::with_identity(
            Identity::from_secret_bytes(me.secret_bytes()).unwrap(),
            Box::new(EventStore::open(dir.path()).unwrap()),
            vec![],
        );
        let peer = Identity::generate();

        // become contacts (an intro I signed), and hold a credit edge
        s.follow(&format!(
            "https://knownby.work/i/{}",
            invite::npub_encode(&peer.nostr_pubkey_hex()).unwrap()
        ))
        .unwrap();
        s.ingest(&credit(&peer, &me, 5.0)).unwrap();

        let c = s
            .contacts()
            .into_iter()
            .find(|c| c.pubkey == peer.nostr_pubkey_hex())
            .expect("peer is a contact");
        assert_eq!(c.trust_hops, Some(1));
        assert_eq!(c.net_position, 5.0);
    }

    // --- NIP-QW12 ledger sync ---------------------------------------

    /// An in-memory hub the ledger tests sync every replica against — it
    /// answers `pull`/`push` like a conforming replica, keyed by slot name.
    #[derive(Default)]
    struct Hub(std::collections::HashMap<String, Vec<Event>>);

    impl LedgerTransport for Hub {
        type Error = String;
        fn pull(
            &mut self,
            slot: &str,
            have: &crate::LedgerCoverage,
        ) -> Result<crate::PullResponse, String> {
            let held = self.0.get(slot).cloned().unwrap_or_default();
            let events = held.iter().filter(|e| have.wants(e)).cloned().collect();
            Ok(crate::PullResponse {
                events,
                coverage: crate::LedgerCoverage::of(&held),
            })
        }
        fn push(&mut self, slot: &str, events: &[Event]) -> Result<usize, String> {
            let held = self.0.entry(slot.to_string()).or_default();
            let mut new = 0;
            for e in events {
                if !held.iter().any(|h| h.id == e.id) {
                    held.push(e.clone());
                    new += 1;
                }
            }
            Ok(new)
        }
    }

    /// Two `Session`s that are replicas of one identity converge: each
    /// authored a different event offline, and one anti-entropy round each
    /// leaves both — and the hub — holding the union.
    #[test]
    fn ledger_round_converges_two_replicas_of_one_identity() {
        let secret = Identity::generate().secret_bytes();
        let pdir = tempfile::tempdir().unwrap();
        let wdir = tempfile::tempdir().unwrap();
        let mut phone = Session::with_identity(
            Identity::from_secret_bytes(secret).unwrap(),
            Box::new(EventStore::open(pdir.path()).unwrap()),
            vec![],
        );
        let mut web = Session::with_identity(
            Identity::from_secret_bytes(secret).unwrap(),
            Box::new(EventStore::open(wdir.path()).unwrap()),
            vec![],
        );

        // each edits its own profile while the other is unreachable
        phone
            .set_profile(ProfileEdit { display_name: Some("vk".into()), tags: vec!["Rust Lang".into()] })
            .unwrap();
        web.follow(&format!(
            "https://knownby.work/i/{}",
            invite::npub_encode(&Identity::generate().nostr_pubkey_hex()).unwrap()
        ))
        .unwrap();
        let phone_evt = phone.history.events()[0].id.clone();
        let web_evt = web.history.events()[0].id.clone();
        assert_ne!(phone_evt, web_evt);

        // seed the hub with the web replica's ledger, then sync the phone
        let mut hub = Hub::default();
        hub.0.insert("hub".into(), web.history.events().to_vec());
        let r = phone.ledger_round(&mut hub, &["hub"]).unwrap();
        assert_eq!(r.received, 1, "pulled the web replica's event");
        assert_eq!(r.pushed, 1, "and pushed its own to the hub");
        assert!(r.errors.is_empty());
        assert_eq!(phone.held(), 2);

        // now sync the web replica against the (now complete) hub
        let r = web.ledger_round(&mut hub, &["hub"]).unwrap();
        assert_eq!(r.received, 1);
        assert_eq!(web.held(), 2);

        // all three hold the union
        for held in [phone.history.events(), web.history.events(), hub.0["hub"].as_slice()] {
            let ids: std::collections::HashSet<&str> = held.iter().map(|e| e.id.as_str()).collect();
            assert!(ids.contains(phone_evt.as_str()) && ids.contains(web_evt.as_str()));
        }

        // a second round each is quiet
        assert_eq!(phone.ledger_round(&mut hub, &["hub"]).unwrap().received, 0);
        assert_eq!(web.ledger_round(&mut hub, &["hub"]).unwrap().received, 0);
    }

    #[test]
    fn rank_servers_orders_the_list_and_no_ops_on_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = session_at(dir.path());
        let before = s.servers().to_vec();

        s.rank_servers(&[]);
        assert_eq!(s.servers(), before.as_slice(), "empty candidates change nothing");

        // two unknown-risk servers (blank pubkey) fall back to lower fee —
        // trust ordering itself is `qw_node::server_registry`'s tested job.
        s.rank_servers(&[
            ServerCandidate { pubkey: String::new(), base_url: "http://pricey".into(), fee: 9.0 },
            ServerCandidate { pubkey: String::new(), base_url: "http://cheap".into(), fee: 0.5 },
        ]);
        assert_eq!(
            s.servers(),
            ["http://cheap".to_string(), "http://pricey".to_string()]
        );
    }

    #[test]
    fn ledger_round_reports_a_broken_peer_without_failing() {
        let mut s = session_at(tempfile::tempdir().unwrap().path());

        struct Dead;
        impl LedgerTransport for Dead {
            type Error = String;
            fn pull(&mut self, _: &str, _: &crate::LedgerCoverage) -> Result<crate::PullResponse, String> {
                Err("connection refused".into())
            }
            fn push(&mut self, _: &str, _: &[Event]) -> Result<usize, String> {
                Err("connection refused".into())
            }
        }

        let view = s.ledger_round(&mut Dead, &["offline-box"]).unwrap();
        assert_eq!(view.received, 0);
        assert_eq!(view.errors.len(), 1);
        assert!(view.errors[0].contains("offline-box"));
    }
}
