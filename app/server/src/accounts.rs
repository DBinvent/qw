//! The account store — the **directory tier** of `server/multi-user.md`,
//! in Postgres.
//!
//! One row per account: `account_id` (random UUID, the stable key), the
//! nickname as an AEAD column plus a keyed-hash **blind index**, the
//! `Vec<Wrapping>` KEK set (JSONB), and the sealed account blob. A dump of
//! this table reveals only *"N accounts exist"* — the nickname is a keyed
//! hash and a ciphertext, everything identifying is inside `blob` under a
//! master key that is never at rest in the clear.
//!
//! Same stack as the sibling services (register-bo / dashta):
//! [`marg`](https://docs.rs/marg) supplies the connection string at the
//! entrypoint, [`migrate`] applies `accounts.schema.yaml` through
//! `schema_guard_tokio` at startup, [`PgAccountStore`] runs the queries on
//! `sqlx`.
//!
//! [`AccountStore`] is a trait with two implementations: [`PgAccountStore`]
//! (what a deployment runs) and an in-memory one used by the tests, so
//! `cargo test --workspace` stays hermetic — the same reason `KeyStore` /
//! `HistoryStore` / `MailboxTransport` are traits. Postgres round-trips are
//! covered by an `#[ignore]`d test that needs `QW_TEST_DATABASE_URL`.

use async_trait::async_trait;
use blake2::digest::consts::U32;
use blake2::digest::Mac;
use blake2::Blake2bMac;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::rngs::OsRng;
use rand::RngCore;
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::types::Json;
use sqlx::{PgPool, Row};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::envelope::{unix_secs, Wrapping};

/// Minimum nickname length, measured on the normalised form
/// (`server/multi-user.md`: "anything like an email", min length 8).
pub const MIN_NICK_LEN: usize = 8;
/// The server key that keys the blind index and the nickname cipher is
/// exactly this many bytes.
pub const SERVER_KEY_LEN: usize = 32;

const XNONCE_LEN: usize = 24;
const AEAD_TAG_LEN: usize = 16;
/// AAD on the nickname cipher — the row's own `account_id`, so a cipher
/// cannot be lifted onto another row.
const NICK_AAD: &[u8] = b"qw-web/accounts/nick-v1";

/// The `accounts` table, declaratively. Applied by [`migrate`].
const SCHEMA_YAML: &str = include_str!("accounts.schema.yaml");

/// One stored account. `blob` and each `Wrapping` are opaque here — this
/// layer never holds a master key.
#[derive(Debug, Clone)]
pub struct AccountRecord {
    pub account_id: Uuid,
    pub nick_index: Vec<u8>,
    pub nick_cipher: Vec<u8>,
    pub wrappings: Vec<Wrapping>,
    pub blob: Vec<u8>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl AccountRecord {
    /// Build a record for a new account: normalise the nickname, derive the
    /// blind index, seal the nickname. `account_id` is minted by the caller
    /// (`register` needs it first, to bind it into the account blob's AAD).
    pub fn new(
        server_key: &[u8; SERVER_KEY_LEN],
        account_id: Uuid,
        nick: &str,
        wrappings: Vec<Wrapping>,
        blob: Vec<u8>,
    ) -> Result<Self, AccountError> {
        let nick = normalize_nick(nick)?;
        let now = unix_secs();
        Ok(Self {
            nick_index: blind_index(server_key, &nick),
            nick_cipher: seal_nick(server_key, &account_id, &nick),
            account_id,
            wrappings,
            blob,
            created_at: now,
            updated_at: now,
        })
    }

    /// The stored nickname, decrypted. Only the operator (holder of
    /// `server_key`) can do this — used for re-indexing on a merge, not on
    /// the login path.
    pub fn nick(&self, server_key: &[u8; SERVER_KEY_LEN]) -> Result<String, AccountError> {
        open_nick(server_key, &self.account_id, &self.nick_cipher)
    }
}

/// Trim, NFC, casefold; reject empty or shorter than [`MIN_NICK_LEN`].
///
/// Casefold is `str::to_lowercase` for now — full Unicode caseless
/// matching (`i` vs `İ`, ß) is a later refinement, called out in
/// `multi-user.md`.
pub fn normalize_nick(raw: &str) -> Result<String, AccountError> {
    let n: String = raw.trim().nfc().collect::<String>().to_lowercase();
    if n.is_empty() {
        return Err(AccountError::NickEmpty);
    }
    if n.chars().count() < MIN_NICK_LEN {
        return Err(AccountError::NickTooShort);
    }
    Ok(n)
}

/// Keyed BLAKE2b-256 over the normalised nickname. Deterministic for a
/// given `server_key`, useless without it.
pub fn blind_index(server_key: &[u8; SERVER_KEY_LEN], normalized_nick: &str) -> Vec<u8> {
    let mut mac = <Blake2bMac<U32> as Mac>::new_from_slice(server_key)
        .expect("a 32-byte key is valid for BLAKE2b");
    mac.update(normalized_nick.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Normalise then blind-index in one step — the login lookup path.
pub fn nick_blind_index(
    server_key: &[u8; SERVER_KEY_LEN],
    raw_nick: &str,
) -> Result<Vec<u8>, AccountError> {
    Ok(blind_index(server_key, &normalize_nick(raw_nick)?))
}

fn nick_aead(server_key: &[u8; SERVER_KEY_LEN]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(Key::from_slice(server_key))
}

fn row_aad(account_id: &Uuid) -> Vec<u8> {
    let mut aad = NICK_AAD.to_vec();
    aad.extend_from_slice(account_id.as_bytes());
    aad
}

fn seal_nick(server_key: &[u8; SERVER_KEY_LEN], account_id: &Uuid, nick: &str) -> Vec<u8> {
    let mut nonce = [0u8; XNONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let aad = row_aad(account_id);
    let mut ct = nick_aead(server_key)
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload { msg: nick.as_bytes(), aad: &aad },
        )
        .expect("XChaCha20-Poly1305 encryption of an in-memory buffer cannot fail");
    let mut out = Vec::with_capacity(XNONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.append(&mut ct);
    out
}

/// Decrypt a nickname sealed by [`seal_nick`].
pub fn open_nick(
    server_key: &[u8; SERVER_KEY_LEN],
    account_id: &Uuid,
    cipher: &[u8],
) -> Result<String, AccountError> {
    if cipher.len() < XNONCE_LEN + AEAD_TAG_LEN {
        return Err(AccountError::Malformed);
    }
    let (nonce, ct) = cipher.split_at(XNONCE_LEN);
    let aad = row_aad(account_id);
    let pt = nick_aead(server_key)
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad: &aad })
        .map_err(|_| AccountError::Malformed)?;
    String::from_utf8(pt).map_err(|_| AccountError::Malformed)
}

/// The store seam. Async because Postgres is; `Send + Sync` so it lives in
/// `Arc<dyn AccountStore>` on the shared web state.
#[async_trait]
pub trait AccountStore: Send + Sync {
    /// Insert a fresh record. A blind-index collision in closed mode is
    /// [`AccountError::Conflict`].
    async fn insert(&self, rec: &AccountRecord) -> Result<(), AccountError>;

    /// Every account sharing a blind index, oldest first, capped at 5 —
    /// one in closed mode, the trial-decrypt fan-out set after a merge.
    async fn by_blind_index(&self, idx: &[u8]) -> Result<Vec<AccountRecord>, AccountError>;

    /// One account by its stable id — the key-management path, where a
    /// live session already knows which account it is and only needs the
    /// current `wrappings` to add / drop / rotate a factor.
    async fn by_account_id(&self, id: Uuid) -> Result<Option<AccountRecord>, AccountError>;

    /// Replace the KEK set (add / drop / rotate a factor).
    async fn set_wrappings(&self, id: Uuid, wrappings: &[Wrapping]) -> Result<(), AccountError>;

    /// Replace the sealed blob (a ledger change re-seals under the same
    /// master).
    async fn set_blob(&self, id: Uuid, blob: &[u8]) -> Result<(), AccountError>;
}

/// Apply `accounts.schema.yaml` to `db_url`. Call once at the entrypoint,
/// before [`PgAccountStore::connect`] — same `schema_guard_tokio` mechanism
/// the sibling services use, safe on every boot (it diffs against what is
/// already there). `db_url` is whatever `marg::ArgConfig::db_url()` hands
/// back — on the host, peer auth over the unix socket
/// (`postgresql://<user>@/<db>?host=/var/run/postgresql`).
pub async fn migrate(db_url: &str) -> Result<(), AccountError> {
    let schema = schema_guard_tokio::load_schema_from_src(SCHEMA_YAML.to_string())
        .map_err(AccountError::Migrate)?;
    schema_guard_tokio::migrate_opt(
        schema,
        db_url,
        &schema_guard_tokio::MigrationOptions::default(),
    )
    .await
    .map_err(AccountError::Migrate)?;
    Ok(())
}

/// The deployment store.
pub struct PgAccountStore {
    pub(crate) pool: PgPool,
}

impl PgAccountStore {
    /// Open the connection pool. Run [`migrate`] first — this does not
    /// touch the schema.
    pub async fn connect(url: &str) -> Result<Self, AccountError> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(url)
            .await
            .map_err(db)?;
        Ok(Self { pool })
    }
}

fn row_to_record(row: &PgRow) -> Result<AccountRecord, AccountError> {
    Ok(AccountRecord {
        account_id: row.try_get("account_id").map_err(db)?,
        nick_index: row.try_get("nick_index").map_err(db)?,
        nick_cipher: row.try_get("nick_cipher").map_err(db)?,
        wrappings: row
            .try_get::<Json<Vec<Wrapping>>, _>("wrappings")
            .map_err(db)?
            .0,
        blob: row.try_get("blob").map_err(db)?,
        created_at: row.try_get::<i64, _>("created_at").map_err(db)? as u64,
        updated_at: row.try_get::<i64, _>("updated_at").map_err(db)? as u64,
    })
}

#[async_trait]
impl AccountStore for PgAccountStore {
    async fn insert(&self, rec: &AccountRecord) -> Result<(), AccountError> {
        sqlx::query(
            "INSERT INTO accounts \
             (account_id, nick_index, nick_cipher, wrappings, blob, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(rec.account_id)
        .bind(&rec.nick_index)
        .bind(&rec.nick_cipher)
        .bind(Json(&rec.wrappings))
        .bind(&rec.blob)
        .bind(rec.created_at as i64)
        .bind(rec.updated_at as i64)
        .execute(&self.pool)
        .await
        .map_err(insert_err)?;
        Ok(())
    }

    async fn by_blind_index(&self, idx: &[u8]) -> Result<Vec<AccountRecord>, AccountError> {
        let rows = sqlx::query(
            "SELECT account_id, nick_index, nick_cipher, wrappings, blob, created_at, updated_at \
             FROM accounts WHERE nick_index = $1 ORDER BY created_at LIMIT 5",
        )
        .bind(idx)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter().map(row_to_record).collect()
    }

    async fn by_account_id(&self, id: Uuid) -> Result<Option<AccountRecord>, AccountError> {
        let row = sqlx::query(
            "SELECT account_id, nick_index, nick_cipher, wrappings, blob, created_at, updated_at \
             FROM accounts WHERE account_id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.as_ref().map(row_to_record).transpose()
    }

    async fn set_wrappings(&self, id: Uuid, wrappings: &[Wrapping]) -> Result<(), AccountError> {
        let n = sqlx::query("UPDATE accounts SET wrappings = $2, updated_at = $3 WHERE account_id = $1")
            .bind(id)
            .bind(Json(wrappings))
            .bind(unix_secs() as i64)
            .execute(&self.pool)
            .await
            .map_err(db)?
            .rows_affected();
        if n == 0 {
            return Err(AccountError::NotFound);
        }
        Ok(())
    }

    async fn set_blob(&self, id: Uuid, blob: &[u8]) -> Result<(), AccountError> {
        let n = sqlx::query("UPDATE accounts SET blob = $2, updated_at = $3 WHERE account_id = $1")
            .bind(id)
            .bind(blob)
            .bind(unix_secs() as i64)
            .execute(&self.pool)
            .await
            .map_err(db)?
            .rows_affected();
        if n == 0 {
            return Err(AccountError::NotFound);
        }
        Ok(())
    }
}

fn db(e: sqlx::Error) -> AccountError {
    AccountError::Db(e.to_string())
}

fn insert_err(e: sqlx::Error) -> AccountError {
    if let sqlx::Error::Database(dbe) = &e {
        if dbe.is_unique_violation() {
            return AccountError::Conflict;
        }
    }
    db(e)
}

#[derive(Debug)]
pub enum AccountError {
    NickEmpty,
    NickTooShort,
    /// Blind-index collision while the instance is closed.
    Conflict,
    NotFound,
    Malformed,
    /// `schema_guard_tokio` failed to parse or apply the schema.
    Migrate(String),
    Db(String),
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NickEmpty => f.write_str("nickname is empty after trimming"),
            Self::NickTooShort => write!(f, "nickname must be at least {MIN_NICK_LEN} characters"),
            Self::Conflict => f.write_str("that nickname is taken on this instance"),
            Self::NotFound => f.write_str("no such account"),
            Self::Malformed => f.write_str("stored nickname is malformed"),
            Self::Migrate(m) => write!(f, "schema migration: {m}"),
            Self::Db(m) => write!(f, "account store: {m}"),
        }
    }
}

impl std::error::Error for AccountError {}

/// Test double for [`AccountStore`] — keeps `cargo test --workspace` free
/// of a Postgres dependency (used here and by `multiuser`'s tests). Allows
/// a shared blind index (models the post-merge world) so the fan-out path
/// is exercised.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemAccountStore(std::sync::Mutex<Vec<AccountRecord>>);

#[cfg(test)]
#[async_trait]
impl AccountStore for MemAccountStore {
    async fn insert(&self, rec: &AccountRecord) -> Result<(), AccountError> {
        let mut g = self.0.lock().unwrap();
        if g.iter().any(|r| r.account_id == rec.account_id) {
            return Err(AccountError::Conflict);
        }
        g.push(rec.clone());
        Ok(())
    }
    async fn by_blind_index(&self, idx: &[u8]) -> Result<Vec<AccountRecord>, AccountError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.nick_index == idx)
            .take(5)
            .cloned()
            .collect())
    }
    async fn by_account_id(&self, id: Uuid) -> Result<Option<AccountRecord>, AccountError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.account_id == id)
            .cloned())
    }
    async fn set_wrappings(&self, id: Uuid, w: &[Wrapping]) -> Result<(), AccountError> {
        let mut g = self.0.lock().unwrap();
        let r = g
            .iter_mut()
            .find(|r| r.account_id == id)
            .ok_or(AccountError::NotFound)?;
        r.wrappings = w.to_vec();
        r.updated_at = unix_secs();
        Ok(())
    }
    async fn set_blob(&self, id: Uuid, blob: &[u8]) -> Result<(), AccountError> {
        let mut g = self.0.lock().unwrap();
        let r = g
            .iter_mut()
            .find(|r| r.account_id == id)
            .ok_or(AccountError::NotFound)?;
        r.blob = blob.to_vec();
        r.updated_at = unix_secs();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SK: [u8; SERVER_KEY_LEN] = [7u8; SERVER_KEY_LEN];

    fn record(nick: &str) -> AccountRecord {
        AccountRecord::new(
            &SK,
            Uuid::new_v4(),
            nick,
            Vec::new(),
            b"sealed-blob".to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn normalize_trims_lowercases_and_nfc() {
        assert_eq!(normalize_nick("  Alice.Doe  ").unwrap(), "alice.doe");
        // "e" + U+0301 combining acute (NFD) collapses to precomposed "é".
        assert_eq!(
            normalize_nick("Cafe\u{0301}longname").unwrap(),
            "caf\u{e9}longname"
        );
    }

    #[test]
    fn normalize_rejects_empty_and_short() {
        assert!(matches!(normalize_nick("   "), Err(AccountError::NickEmpty)));
        assert!(matches!(
            normalize_nick("short7 "),
            Err(AccountError::NickTooShort)
        ));
        assert!(normalize_nick("exactly8").is_ok());
    }

    #[test]
    fn blind_index_is_stable_and_key_dependent() {
        let a = blind_index(&SK, "alice.doe");
        assert_eq!(a.len(), 32);
        assert_eq!(a, blind_index(&SK, "alice.doe"), "deterministic");
        assert_ne!(a, blind_index(&SK, "bob.somebody"), "nick matters");
        assert_ne!(a, blind_index(&[9u8; 32], "alice.doe"), "key matters");
    }

    #[test]
    fn nick_cipher_round_trips_under_its_own_account_id() {
        let rec = record("alice.doe");
        assert_eq!(rec.nick(&SK).unwrap(), "alice.doe");
        // bound to the row: another id does not open it
        assert!(open_nick(&SK, &Uuid::new_v4(), &rec.nick_cipher).is_err());
    }

    #[tokio::test]
    async fn insert_then_lookup_returns_the_record() {
        let store = MemAccountStore::default();
        let rec = record("alice.doe");
        let id = rec.account_id;
        store.insert(&rec).await.unwrap();

        let hit = store
            .by_blind_index(&nick_blind_index(&SK, "  ALICE.DOE ").unwrap())
            .await
            .unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].account_id, id);

        assert!(store
            .by_blind_index(&blind_index(&SK, "nobody.here"))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn a_shared_blind_index_fans_out() {
        let store = MemAccountStore::default();
        let a = record("shared.nickname");
        let mut b = record("shared.nickname"); // distinct account_id, same index
        b.nick_index = a.nick_index.clone();
        store.insert(&a).await.unwrap();
        store.insert(&b).await.unwrap();

        let hits = store.by_blind_index(&a.nick_index).await.unwrap();
        assert_eq!(hits.len(), 2, "login trial-decrypts both");
    }

    #[tokio::test]
    async fn set_wrappings_and_blob_persist() {
        let store = MemAccountStore::default();
        let rec = record("alice.doe");
        let id = rec.account_id;
        store.insert(&rec).await.unwrap();

        let m = crate::envelope::MasterKey::random();
        let w = Wrapping::seal_secret("passphrase", b"hunter2hunter2", &m).unwrap();
        store.set_wrappings(id, std::slice::from_ref(&w)).await.unwrap();
        store.set_blob(id, b"resealed").await.unwrap();

        let got = &store.by_blind_index(&rec.nick_index).await.unwrap()[0];
        assert_eq!(got.wrappings.len(), 1);
        assert_eq!(got.blob, b"resealed");
        assert!(got.updated_at >= got.created_at);

        assert!(matches!(
            store.set_blob(Uuid::new_v4(), b"x").await,
            Err(AccountError::NotFound)
        ));
    }

    // Both drivers (schema_guard_tokio's tokio-postgres and sqlx) accept
    // the three-slash socket spelling; sqlx rejects `//user:@/db?host=...`
    // ("empty host"), so the entrypoint feeds marg a `--db` of this shape:
    //   postgres:///<db>?host=/var/run/postgresql&user=<role>
    #[tokio::test]
    #[ignore = "needs QW_TEST_DATABASE_URL, e.g. postgres:///qw_ci?host=/var/run/postgresql&user=$USER"]
    async fn pg_round_trip() {
        let Ok(url) = std::env::var("QW_TEST_DATABASE_URL") else {
            return;
        };
        // schema_guard is safe to run every boot — do it twice, the second
        // pass must be a no-op diff, not an error.
        migrate(&url).await.unwrap();
        migrate(&url).await.unwrap();

        let store = PgAccountStore::connect(&url).await.unwrap();
        let rec = record("integration.nick");
        let id = rec.account_id;

        store.insert(&rec).await.unwrap();
        let hit = store.by_blind_index(&rec.nick_index).await.unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].account_id, id);
        assert_eq!(hit[0].nick(&SK).unwrap(), "integration.nick");

        store.set_blob(id, b"blob-1").await.unwrap();
        assert_eq!(
            store.by_blind_index(&rec.nick_index).await.unwrap()[0].blob,
            b"blob-1"
        );

        sqlx::query("DELETE FROM accounts WHERE account_id = $1")
            .bind(id)
            .execute(&store.pool)
            .await
            .unwrap();
    }
}
