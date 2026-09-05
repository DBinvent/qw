//! The decrypted, in-RAM side of a multi-user account (`server/multi-user.md`).
//!
//! At login the master key (from the `unlock` module, next increment) opens
//! the `accounts.blob` column into a payload — the identity secret and the
//! event ledger. [`open_account`] hands back the [`Identity`] for
//! `Session::with_identity` and an [`EncHistoryStore`] to give it as the
//! `HistoryStore`. Every `append` re-seals the whole payload under the same
//! master key and sets a [`dirty`](EncHistoryStore::dirty) flag; the caller
//! (the multi-user `Web`, next increment) writes [`sealed`](EncHistoryStore::sealed)
//! back through `AccountStore::set_blob` after each session op. The master
//! key lives only here, only for the session.
//!
//! Verify-on-ingest is unchanged from `EventStore`: an event that fails
//! `verify` is dropped on open (counted in [`rejected`](EncHistoryStore::rejected))
//! and refused on `append` — a decrypted-but-corrupt payload must not feed a
//! trust computation any more than a tampered `events.jsonl` line would.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use qw_client_core::{ClientError, HistoryStore, SyncState};
use qw_protocol::events::Event;
use qw_protocol::identity::Identity;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::envelope::{EnvelopeError, MasterKey};

/// The account ciphertext and whether it is ahead of what the store holds.
/// Shared between the [`EncHistoryStore`] (which rewrites it on every
/// `append`) and the session's owner (the multi-user `Web`), which flushes
/// it through `AccountStore::set_blob` — `Session` itself has no way to
/// hand its `HistoryStore` back.
#[derive(Default)]
pub struct SealedState {
    pub blob: Vec<u8>,
    pub dirty: bool,
}

/// A cloneable handle to one account's [`SealedState`].
pub type SealedHandle = Arc<Mutex<SealedState>>;

/// Domain separator for the account blob's AEAD, bound with `account_id`.
/// Distinct from the KEK-wrapping AAD so a wrapping can never be replayed as
/// a blob or vice versa.
const BLOB_AAD: &[u8] = b"qw-web/accounts/blob-v1";

fn blob_aad(account_id: &Uuid) -> Vec<u8> {
    let mut aad = BLOB_AAD.to_vec();
    aad.extend_from_slice(account_id.as_bytes());
    aad
}

/// What the master key seals — the write side.
#[derive(Serialize)]
struct PayloadOut<'a> {
    /// 32-byte identity secret, hex — the same spelling as the single-user
    /// `identity.key` file.
    id: &'a str,
    events: &'a [Event],
    /// Outbox ids + poll cursors, so an authored-but-unsent event and a
    /// warm mailbox cursor survive a host restart (`todo-impl.md` §7).
    sync: &'a SyncState,
}

/// The read side.
#[derive(Deserialize)]
struct PayloadIn {
    id: String,
    #[serde(default)]
    events: Vec<Event>,
    /// Absent in a blob sealed before outbox persistence landed — an
    /// empty [`SyncState`] then, same as a fresh account.
    #[serde(default)]
    sync: SyncState,
}

#[derive(Debug)]
pub enum StoreError {
    /// The blob did not decrypt — wrong master, wrong `account_id`, or
    /// tampered ciphertext.
    Envelope(EnvelopeError),
    /// Decrypted, but not the payload shape expected.
    Payload(String),
    /// The payload's `id` is not a 32-byte hex secret key.
    Identity,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Envelope(e) => write!(f, "account blob: {e}"),
            Self::Payload(m) => write!(f, "account payload: {m}"),
            Self::Identity => f.write_str("account payload carries a malformed identity key"),
        }
    }
}
impl std::error::Error for StoreError {}
impl From<EnvelopeError> for StoreError {
    fn from(e: EnvelopeError) -> Self {
        Self::Envelope(e)
    }
}

fn hex32(s: &str) -> Option<[u8; 32]> {
    hex::decode(s).ok()?.try_into().ok()
}

fn seal(
    master: &MasterKey,
    account_id: &Uuid,
    id_hex: &str,
    events: &[Event],
    sync: &SyncState,
) -> Result<Vec<u8>, StoreError> {
    let bytes = serde_json::to_vec(&PayloadOut {
        id: id_hex,
        events,
        sync,
    })
    .map_err(|e| StoreError::Payload(e.to_string()))?;
    Ok(master.seal(&bytes, &blob_aad(account_id)))
}

/// The blob for a brand-new account — identity, no events, nothing queued.
/// `register` calls this, then `AccountStore::insert`.
pub fn seal_new_account(
    master: &MasterKey,
    account_id: &Uuid,
    identity: &Identity,
) -> Result<Vec<u8>, StoreError> {
    seal(
        master,
        account_id,
        &hex::encode(identity.secret_bytes()),
        &[],
        &SyncState::default(),
    )
}

/// What [`open_account`] returns: the identity for `Session::with_identity`,
/// the store to hand it, the [`SealedHandle`] to flush, the persisted
/// [`SyncState`] for `Session::restore_sync_state` (the history has moved
/// into the `Session` by then, so the caller needs its own copy), and a
/// clone of the master key so the host can wrap it under a new KEK without
/// reaching back through the `Session`.
pub struct OpenAccount {
    pub identity: Identity,
    pub history: EncHistoryStore,
    pub state: SealedHandle,
    pub sync: SyncState,
    pub master: MasterKey,
}

/// Decrypt an account blob into an identity and a live history store.
/// `master` moves in — it lives inside [`EncHistoryStore`] for the session.
pub fn open_account(
    master: MasterKey,
    account_id: Uuid,
    blob: &[u8],
) -> Result<OpenAccount, StoreError> {
    let bytes = master.open(blob, &blob_aad(&account_id))?;
    let payload: PayloadIn =
        serde_json::from_slice(&bytes).map_err(|e| StoreError::Payload(e.to_string()))?;

    let secret = hex32(&payload.id).ok_or(StoreError::Identity)?;
    let identity = Identity::from_secret_bytes(secret).map_err(|_| StoreError::Identity)?;
    let identity_hex = payload.id;

    // verify-on-load, mirroring EventStore::open — drop and count what does
    // not verify or is a duplicate id.
    let mut events = Vec::new();
    let mut ids = HashSet::new();
    let mut rejected = 0usize;
    for e in payload.events {
        if e.verify().is_ok() && ids.insert(e.id.clone()) {
            events.push(e);
        } else {
            rejected += 1;
        }
    }
    events.sort_by_key(|e| e.created_at);

    let state: SealedHandle = Arc::new(Mutex::new(SealedState {
        blob: blob.to_vec(),
        dirty: false,
    }));
    let history = EncHistoryStore {
        master: master.clone(),
        account_id,
        identity_hex,
        events,
        ids,
        rejected,
        sync: payload.sync.clone(),
        state: state.clone(),
    };
    Ok(OpenAccount {
        identity,
        history,
        state,
        sync: payload.sync,
        master,
    })
}

/// A [`HistoryStore`] whose backing bytes are one AEAD blob under the
/// session master key. `events()` is a plain in-RAM slice; every `append`
/// re-seals and marks [`dirty`](Self::dirty).
pub struct EncHistoryStore {
    master: MasterKey,
    account_id: Uuid,
    identity_hex: String,
    events: Vec<Event>,
    ids: HashSet<String>,
    rejected: usize,
    /// The last [`SyncState`] folded into the sealed blob — re-sealed only
    /// when `Session` hands over a different one (see `persist_sync_state`).
    sync: SyncState,
    state: SealedHandle,
}

impl EncHistoryStore {
    /// The current ciphertext — always consistent with
    /// [`events`](HistoryStore::events).
    pub fn sealed(&self) -> Vec<u8> {
        self.state.lock().expect("sealed state lock").blob.clone()
    }

    /// `sealed()` is ahead of what the account store holds.
    pub fn dirty(&self) -> bool {
        self.state.lock().expect("sealed state lock").dirty
    }

    /// The caller has written `sealed()` back to the account store.
    pub fn mark_persisted(&mut self) {
        self.state.lock().expect("sealed state lock").dirty = false;
    }
}

impl HistoryStore for EncHistoryStore {
    fn events(&self) -> &[Event] {
        &self.events
    }

    fn rejected(&self) -> usize {
        self.rejected
    }

    fn append(&mut self, incoming: &[Event]) -> Result<usize, ClientError> {
        let mut fresh: Vec<Event> = Vec::new();
        for e in incoming {
            if e.verify().is_ok()
                && !self.ids.contains(&e.id)
                && !fresh.iter().any(|k| k.id == e.id)
            {
                fresh.push(e.clone());
            }
        }
        if fresh.is_empty() {
            return Ok(0);
        }

        // Build the next ledger and its ciphertext before mutating self, so
        // a serialize failure leaves the store and its blob agreeing
        // (EventStore writes to disk before memory for the same reason).
        let mut next = self.events.clone();
        next.extend(fresh.iter().cloned());
        next.sort_by_key(|e| e.created_at);
        let sealed = seal(
            &self.master,
            &self.account_id,
            &self.identity_hex,
            &next,
            &self.sync,
        )
        .map_err(|e| ClientError::Http(e.to_string()))?;

        for e in &fresh {
            self.ids.insert(e.id.clone());
        }
        self.events = next;
        {
            let mut s = self.state.lock().expect("sealed state lock");
            s.blob = sealed;
            s.dirty = true;
        }
        Ok(fresh.len())
    }

    /// Re-seal the payload with a new outbox/cursor snapshot. A no-op when
    /// it matches what is already sealed, so a `sync_now` that changed
    /// nothing does not rewrite the blob or dirty it.
    fn persist_sync_state(&mut self, state: &SyncState) -> Result<(), ClientError> {
        if *state == self.sync {
            return Ok(());
        }
        let sealed = seal(
            &self.master,
            &self.account_id,
            &self.identity_hex,
            &self.events,
            state,
        )
        .map_err(|e| ClientError::Http(e.to_string()))?;
        self.sync = state.clone();
        let mut s = self.state.lock().expect("sealed state lock");
        s.blob = sealed;
        s.dirty = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qw_protocol::events::UnsignedEvent;

    fn signed(id: &Identity, content: &str, created_at: u64) -> Event {
        UnsignedEvent::with_created_at(id.nostr_pubkey_hex(), 1, vec![], content, created_at)
            .sign(id)
    }

    #[test]
    fn seal_then_open_round_trips_identity_and_orders_events() {
        let m = MasterKey::random();
        let acct = Uuid::new_v4();
        let id = Identity::generate();
        let early = signed(&id, "one", 100);
        let late = signed(&id, "two", 200);

        let blob = seal(
            &m,
            &acct,
            &hex::encode(id.secret_bytes()),
            &[late, early], // out of order on purpose
            &SyncState::default(),
        )
        .unwrap();
        let o = open_account(m, acct, &blob).unwrap();

        assert_eq!(o.identity.nostr_pubkey_hex(), id.nostr_pubkey_hex());
        assert_eq!(o.history.events().len(), 2);
        assert_eq!(o.history.events()[0].content, "one");
        assert_eq!(o.history.rejected(), 0);
        assert!(!o.history.dirty());
    }

    #[test]
    fn seal_new_account_carries_the_identity_and_no_events() {
        let m = MasterKey::random();
        let acct = Uuid::new_v4();
        let id = Identity::generate();

        let blob = seal_new_account(&m, &acct, &id).unwrap();
        let o = open_account(m, acct, &blob).unwrap();

        assert_eq!(o.identity.secret_bytes(), id.secret_bytes());
        assert!(o.history.events().is_empty());
    }

    #[test]
    fn open_rejects_wrong_master_wrong_account_and_a_flipped_bit() {
        let m = MasterKey::random();
        let acct = Uuid::new_v4();
        let id = Identity::generate();
        let mut blob = seal_new_account(&m, &acct, &id).unwrap();

        assert!(matches!(
            open_account(MasterKey::random(), acct, &blob),
            Err(StoreError::Envelope(_))
        ));
        assert!(matches!(
            open_account(m.clone(), Uuid::new_v4(), &blob),
            Err(StoreError::Envelope(_))
        ));
        let last = blob.len() - 1;
        blob[last] ^= 1;
        assert!(matches!(
            open_account(m, acct, &blob),
            Err(StoreError::Envelope(_))
        ));
    }

    #[test]
    fn open_drops_and_counts_a_corrupt_event_in_the_payload() {
        let m = MasterKey::random();
        let acct = Uuid::new_v4();
        let id = Identity::generate();
        let good = signed(&id, "good", 100);
        let mut bad = signed(&id, "bad", 200);
        bad.sig = "0".repeat(128); // parses as a sig, fails verification

        let blob = seal(
            &m,
            &acct,
            &hex::encode(id.secret_bytes()),
            &[good, bad],
            &SyncState::default(),
        )
        .unwrap();
        let o = open_account(m, acct, &blob).unwrap();

        assert_eq!(o.history.events().len(), 1);
        assert_eq!(o.history.events()[0].content, "good");
        assert_eq!(o.history.rejected(), 1);
    }

    #[test]
    fn append_verifies_dedupes_reseals_and_survives_a_reopen() {
        let m = MasterKey::random();
        let acct = Uuid::new_v4();
        let id = Identity::generate();
        let blob0 = seal_new_account(&m, &acct, &id).unwrap();
        let mut o = open_account(m.clone(), acct, &blob0).unwrap();

        let e = signed(&id, "hello", 100);
        assert_eq!(o.history.append(std::slice::from_ref(&e)).unwrap(), 1);
        assert_eq!(o.history.events().len(), 1);
        assert!(o.history.dirty());
        assert_ne!(o.history.sealed(), blob0);

        // a duplicate id is a no-op and does not re-dirty
        o.history.mark_persisted();
        assert_eq!(o.history.append(std::slice::from_ref(&e)).unwrap(), 0);
        assert!(!o.history.dirty());

        // a fresh session opens the resealed blob and sees the event
        let o2 = open_account(m, acct, &o.history.sealed()).unwrap();
        assert_eq!(o2.history.events().len(), 1);
        assert_eq!(o2.history.events()[0].content, "hello");
        assert_eq!(o2.identity.secret_bytes(), id.secret_bytes());
    }

    #[test]
    fn append_refuses_an_unverifiable_event() {
        let m = MasterKey::random();
        let acct = Uuid::new_v4();
        let id = Identity::generate();
        let mut o = open_account(m.clone(), acct, &seal_new_account(&m, &acct, &id).unwrap()).unwrap();

        let mut bad = signed(&id, "original", 100);
        bad.content = "swapped after signing".into(); // id no longer matches

        assert_eq!(o.history.append(std::slice::from_ref(&bad)).unwrap(), 0);
        assert!(o.history.events().is_empty());
        assert!(!o.history.dirty());
    }

    #[test]
    fn persist_sync_state_folds_into_the_blob_and_survives_a_reopen() {
        let m = MasterKey::random();
        let acct = Uuid::new_v4();
        let id = Identity::generate();
        let mut o =
            open_account(m.clone(), acct, &seal_new_account(&m, &acct, &id).unwrap()).unwrap();
        assert!(o.sync.outbox.is_empty());

        let e = signed(&id, "queued", 100);
        o.history.append(std::slice::from_ref(&e)).unwrap();
        o.history.mark_persisted();

        let st = SyncState {
            outbox: vec![e.id.clone()],
            cursors: [("http://s".to_string(), 100u64)].into_iter().collect(),
        };
        o.history.persist_sync_state(&st).unwrap();
        assert!(o.history.dirty(), "a changed snapshot re-seals");

        // Idempotent: the same snapshot again does not rewrite the blob.
        o.history.mark_persisted();
        o.history.persist_sync_state(&st).unwrap();
        assert!(!o.history.dirty());

        // A blob sealed before this feature has no `sync` key; `#[serde(default)]`
        // gives it an empty one. The resealed blob carries the real state.
        let o2 = open_account(m, acct, &o.history.sealed()).unwrap();
        assert_eq!(o2.sync, st);
        assert_eq!(o2.history.events().len(), 1);
    }
}
