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
/// Same idea for the optional email column, under its own domain tag so a
/// cipher cannot be moved between the two columns either (`field_aad`).
const EMAIL_AAD: &[u8] = b"qw-web/accounts/email-v1";
/// `normalize_email`'s length cap — RFC 5321's own bound on the whole
/// address.
const MAX_EMAIL_LEN: usize = 254;

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
    /// Optional notify-only address, sealed like the nickname. `None`
    /// until the holder adds one (self-service, post-registration — see
    /// `server/multi-user.md`). Never a login path: nothing here is ever
    /// read before a session already exists.
    pub email_cipher: Option<Vec<u8>>,
    /// Set only by `AccountStore::mark_email_verified` — the one path that
    /// moves this from `None` to `Some`. A fresh or changed address always
    /// clears it back to `None` first.
    pub email_verified_at: Option<u64>,
    /// Keyed BLAKE2b-256 of the pending confirmation code (`blind_index`
    /// reused — the code is a short secret exactly like the nickname it
    /// already hashes). Cleared once verified, replaced, or dropped.
    pub email_verify_hash: Option<Vec<u8>>,
    /// When the pending code was last (re)sent — the resend-rate-limit and
    /// 24-hour expiry both read this.
    pub email_verify_sent_at: Option<u64>,
}

impl AccountRecord {
    /// Build a record for a new account: normalise the nickname, derive the
    /// blind index, seal the nickname. `account_id` is minted by the caller
    /// (`register` needs it first, to bind it into the account blob's AAD).
    /// No email at creation — it is always a later, self-service step.
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
            email_cipher: None,
            email_verified_at: None,
            email_verify_hash: None,
            email_verify_sent_at: None,
        })
    }

    /// The stored nickname, decrypted. Only the operator (holder of
    /// `server_key`) can do this — used for re-indexing on a merge, not on
    /// the login path.
    pub fn nick(&self, server_key: &[u8; SERVER_KEY_LEN]) -> Result<String, AccountError> {
        open_nick(server_key, &self.account_id, &self.nick_cipher)
    }

    /// The stored email, decrypted, or `None` if none was ever added. The
    /// holder typed it themselves, so handing it back to their own
    /// session is not new exposure — same reasoning as `nick`, just
    /// reachable from a route rather than operator-only.
    pub fn email(&self, server_key: &[u8; SERVER_KEY_LEN]) -> Result<Option<String>, AccountError> {
        match &self.email_cipher {
            Some(c) => open_email(server_key, &self.account_id, c).map(Some),
            None => Ok(None),
        }
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

/// Trim and lowercase; reject anything that is not plausibly
/// `local@domain.tld`. Deliberately loose — the same "anything like an
/// email" bar `multi-user.md` sets for the nickname — because this
/// address is never a lookup key, only a notify channel: the cost of
/// rejecting a real but unusual address is higher than the cost of
/// accepting one that later bounces.
pub fn normalize_email(raw: &str) -> Result<String, AccountError> {
    let e = raw.trim().to_lowercase();
    if e.is_empty() || e.len() > MAX_EMAIL_LEN || e.chars().any(char::is_whitespace) {
        return Err(AccountError::EmailInvalid);
    }
    let Some((local, domain)) = e.split_once('@') else {
        return Err(AccountError::EmailInvalid);
    };
    if local.is_empty() || domain.is_empty() || !domain.contains('.') || domain.contains('@') {
        return Err(AccountError::EmailInvalid);
    }
    Ok(e)
}

/// Six digits, rejection-sampled so the low values are not marginally
/// more likely — the bias would be tiny but costs nothing to avoid. The
/// type-it-in email confirmation code (`multiuser::send_email_code`).
pub fn generate_email_code() -> String {
    loop {
        let mut bytes = [0u8; 4];
        OsRng.fill_bytes(&mut bytes);
        let n = u32::from_le_bytes(bytes);
        // 4_294_000_000 is the largest multiple of 1_000_000 below u32::MAX.
        if n < 4_294_000_000 {
            return format!("{:06}", n % 1_000_000);
        }
    }
}

/// Keyed purely by `server_key` — nothing column-specific in here, the
/// domain separation lives entirely in `row_aad`'s tag.
fn server_field_aead(server_key: &[u8; SERVER_KEY_LEN]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(Key::from_slice(server_key))
}

/// `domain` (a `*_AAD` constant) plus the row's own `account_id`, so a
/// cipher cannot be lifted onto another row *or* swapped onto the other
/// sealed column under the same key.
fn row_aad(domain: &[u8], account_id: &Uuid) -> Vec<u8> {
    let mut aad = domain.to_vec();
    aad.extend_from_slice(account_id.as_bytes());
    aad
}

fn seal_field(
    server_key: &[u8; SERVER_KEY_LEN],
    domain: &[u8],
    account_id: &Uuid,
    plaintext: &str,
) -> Vec<u8> {
    let mut nonce = [0u8; XNONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let aad = row_aad(domain, account_id);
    let mut ct = server_field_aead(server_key)
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext.as_bytes(),
                aad: &aad,
            },
        )
        .expect("XChaCha20-Poly1305 encryption of an in-memory buffer cannot fail");
    let mut out = Vec::with_capacity(XNONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.append(&mut ct);
    out
}

fn open_field(
    server_key: &[u8; SERVER_KEY_LEN],
    domain: &[u8],
    account_id: &Uuid,
    cipher: &[u8],
) -> Result<String, AccountError> {
    if cipher.len() < XNONCE_LEN + AEAD_TAG_LEN {
        return Err(AccountError::Malformed);
    }
    let (nonce, ct) = cipher.split_at(XNONCE_LEN);
    let aad = row_aad(domain, account_id);
    let pt = server_field_aead(server_key)
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad: &aad })
        .map_err(|_| AccountError::Malformed)?;
    String::from_utf8(pt).map_err(|_| AccountError::Malformed)
}

fn seal_nick(server_key: &[u8; SERVER_KEY_LEN], account_id: &Uuid, nick: &str) -> Vec<u8> {
    seal_field(server_key, NICK_AAD, account_id, nick)
}

/// Decrypt a nickname sealed by [`seal_nick`].
pub fn open_nick(
    server_key: &[u8; SERVER_KEY_LEN],
    account_id: &Uuid,
    cipher: &[u8],
) -> Result<String, AccountError> {
    open_field(server_key, NICK_AAD, account_id, cipher)
}

/// Seal an already-[`normalize_email`]d address under `server_key`,
/// AAD-bound to `account_id` so it cannot be swapped onto another row or
/// onto the nickname column.
pub fn seal_email(server_key: &[u8; SERVER_KEY_LEN], account_id: &Uuid, email: &str) -> Vec<u8> {
    seal_field(server_key, EMAIL_AAD, account_id, email)
}

/// Decrypt an email sealed by [`seal_email`].
pub fn open_email(
    server_key: &[u8; SERVER_KEY_LEN],
    account_id: &Uuid,
    cipher: &[u8],
) -> Result<String, AccountError> {
    open_field(server_key, EMAIL_AAD, account_id, cipher)
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

    /// This instance's default coordination-server list — the "public host
    /// default list" served on `GET /servers` and pulled by fresh clients
    /// and the mobile app. `None` before it is seeded on first boot.
    async fn host_servers(&self) -> Result<Option<Vec<String>>, AccountError>;

    /// Replace that list (first-boot seed from `QW_SERVERS`, or an operator
    /// edit).
    async fn set_host_servers(&self, servers: &[String]) -> Result<(), AccountError>;

    /// Add, change, resend-for, or drop the optional notify email in one
    /// write. `cipher`/`verify_hash`/`sent_at` all `Some` means "a pending
    /// code now exists for this address"; all `None` means "remove the
    /// address entirely" (`/api/email_drop`). Either way this call
    /// **always clears `email_verified_at`** — a fresh or changed address
    /// (including re-sending its own code) returns to pending; only
    /// [`AccountStore::mark_email_verified`] may set it.
    async fn set_email(
        &self,
        id: Uuid,
        cipher: Option<Vec<u8>>,
        verify_hash: Option<Vec<u8>>,
        sent_at: Option<u64>,
    ) -> Result<(), AccountError>;

    /// Confirm the pending code: set `email_verified_at` and clear the
    /// pending-code fields. The caller has already matched the presented
    /// code against `email_verify_hash` — this method does not re-check
    /// it, so it must only be called after that comparison succeeds.
    async fn mark_email_verified(&self, id: Uuid) -> Result<(), AccountError>;
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

/// Every column `row_to_record` reads, shared by the three `SELECT`s below
/// so the list cannot drift out of sync between them.
const ACCOUNT_COLUMNS: &str = "account_id, nick_index, nick_cipher, wrappings, blob, created_at, \
     updated_at, email_cipher, email_verified_at, email_verify_hash, email_verify_sent_at";

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
        email_cipher: row.try_get("email_cipher").map_err(db)?,
        email_verified_at: row
            .try_get::<Option<i64>, _>("email_verified_at")
            .map_err(db)?
            .map(|v| v as u64),
        email_verify_hash: row.try_get("email_verify_hash").map_err(db)?,
        email_verify_sent_at: row
            .try_get::<Option<i64>, _>("email_verify_sent_at")
            .map_err(db)?
            .map(|v| v as u64),
    })
}

#[async_trait]
impl AccountStore for PgAccountStore {
    async fn insert(&self, rec: &AccountRecord) -> Result<(), AccountError> {
        // Email columns are always NULL on insert — `AccountRecord::new`
        // never sets them; adding one is always a later `set_email` call.
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
        let rows = sqlx::query(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE nick_index = $1 ORDER BY created_at LIMIT 5"
        ))
        .bind(idx)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter().map(row_to_record).collect()
    }

    async fn by_account_id(&self, id: Uuid) -> Result<Option<AccountRecord>, AccountError> {
        let row = sqlx::query(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE account_id = $1"
        ))
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

    async fn set_email(
        &self,
        id: Uuid,
        cipher: Option<Vec<u8>>,
        verify_hash: Option<Vec<u8>>,
        sent_at: Option<u64>,
    ) -> Result<(), AccountError> {
        let n = sqlx::query(
            "UPDATE accounts SET email_cipher = $2, email_verify_hash = $3, \
             email_verify_sent_at = $4, email_verified_at = NULL, updated_at = $5 \
             WHERE account_id = $1",
        )
        .bind(id)
        .bind(&cipher)
        .bind(&verify_hash)
        .bind(sent_at.map(|v| v as i64))
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

    async fn mark_email_verified(&self, id: Uuid) -> Result<(), AccountError> {
        let now = unix_secs() as i64;
        let n = sqlx::query(
            "UPDATE accounts SET email_verified_at = $2, email_verify_hash = NULL, \
             email_verify_sent_at = NULL, updated_at = $2 WHERE account_id = $1",
        )
        .bind(id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db)?
        .rows_affected();
        if n == 0 {
            return Err(AccountError::NotFound);
        }
        Ok(())
    }

    async fn host_servers(&self) -> Result<Option<Vec<String>>, AccountError> {
        let row: Option<(Json<Vec<String>>,)> =
            sqlx::query_as("SELECT servers FROM host_config WHERE id = 1")
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        Ok(row.map(|(Json(v),)| v))
    }

    async fn set_host_servers(&self, servers: &[String]) -> Result<(), AccountError> {
        sqlx::query(
            "INSERT INTO host_config (id, servers, updated_at) VALUES (1, $1, $2) \
             ON CONFLICT (id) DO UPDATE SET servers = $1, updated_at = $2",
        )
        .bind(Json(servers))
        .bind(unix_secs() as i64)
        .execute(&self.pool)
        .await
        .map_err(db)?;
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
    /// `normalize_email` rejected the input — too short, no `@`, no `.`
    /// in the domain part, or over the length cap.
    EmailInvalid,
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
            Self::EmailInvalid => f.write_str("that does not look like an email address"),
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
pub(crate) struct MemAccountStore(
    std::sync::Mutex<Vec<AccountRecord>>,
    std::sync::Mutex<Option<Vec<String>>>,
);

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
    async fn host_servers(&self) -> Result<Option<Vec<String>>, AccountError> {
        Ok(self.1.lock().unwrap().clone())
    }
    async fn set_host_servers(&self, servers: &[String]) -> Result<(), AccountError> {
        *self.1.lock().unwrap() = Some(servers.to_vec());
        Ok(())
    }
    async fn set_email(
        &self,
        id: Uuid,
        cipher: Option<Vec<u8>>,
        verify_hash: Option<Vec<u8>>,
        sent_at: Option<u64>,
    ) -> Result<(), AccountError> {
        let mut g = self.0.lock().unwrap();
        let r = g
            .iter_mut()
            .find(|r| r.account_id == id)
            .ok_or(AccountError::NotFound)?;
        r.email_cipher = cipher;
        r.email_verify_hash = verify_hash;
        r.email_verify_sent_at = sent_at;
        r.email_verified_at = None;
        r.updated_at = unix_secs();
        Ok(())
    }
    async fn mark_email_verified(&self, id: Uuid) -> Result<(), AccountError> {
        let mut g = self.0.lock().unwrap();
        let r = g
            .iter_mut()
            .find(|r| r.account_id == id)
            .ok_or(AccountError::NotFound)?;
        r.email_verified_at = Some(unix_secs());
        r.email_verify_hash = None;
        r.email_verify_sent_at = None;
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

    #[test]
    fn normalize_email_trims_lowercases_and_rejects_the_obviously_wrong() {
        assert_eq!(
            normalize_email("  Alice@Example.COM ").unwrap(),
            "alice@example.com"
        );
        for bad in [
            "",
            "   ",
            "no-at-sign",
            "@example.com",
            "alice@",
            "alice@nodot",
            "a b@example.com",
        ] {
            assert!(
                matches!(normalize_email(bad), Err(AccountError::EmailInvalid)),
                "{bad:?}"
            );
        }
        assert!(
            normalize_email(&format!("a@{}.com", "x".repeat(260))).is_err(),
            "over length cap"
        );
    }

    #[test]
    fn email_code_is_six_digits() {
        for _ in 0..50 {
            let code = generate_email_code();
            assert_eq!(code.len(), 6, "{code}");
            assert!(code.chars().all(|c| c.is_ascii_digit()), "{code}");
        }
    }

    #[test]
    fn email_cipher_round_trips_and_cannot_be_opened_as_a_nickname() {
        let id = Uuid::new_v4();
        let cipher = seal_email(&SK, &id, "alice@example.com");
        assert_eq!(open_email(&SK, &id, &cipher).unwrap(), "alice@example.com");
        // bound to the row
        assert!(open_email(&SK, &Uuid::new_v4(), &cipher).is_err());
        // bound to the field — the AAD differs from the nickname's, so a
        // cipher cannot be swapped onto the other sealed column
        assert!(open_nick(&SK, &id, &cipher).is_err());
    }

    #[tokio::test]
    async fn set_email_is_pending_then_mark_verified_confirms_it() {
        let store = MemAccountStore::default();
        let rec = record("alice.doe");
        let id = rec.account_id;
        store.insert(&rec).await.unwrap();

        let email = normalize_email("alice@example.com").unwrap();
        let cipher = seal_email(&SK, &id, &email);
        let code = generate_email_code();
        let hash = blind_index(&SK, &code);
        store
            .set_email(id, Some(cipher), Some(hash.clone()), Some(1_000))
            .await
            .unwrap();

        let got = store.by_account_id(id).await.unwrap().unwrap();
        assert_eq!(
            got.email(&SK).unwrap().as_deref(),
            Some("alice@example.com")
        );
        assert_eq!(got.email_verified_at, None, "pending, not yet confirmed");
        assert_eq!(got.email_verify_hash, Some(hash));

        store.mark_email_verified(id).await.unwrap();
        let got = store.by_account_id(id).await.unwrap().unwrap();
        assert!(got.email_verified_at.is_some());
        assert_eq!(
            got.email_verify_hash, None,
            "the pending code is cleared on success"
        );

        // set_email again (a changed address) returns to pending.
        let cipher2 = seal_email(&SK, &id, "alice2@example.com");
        store
            .set_email(
                id,
                Some(cipher2),
                Some(blind_index(&SK, "000000")),
                Some(2_000),
            )
            .await
            .unwrap();
        let got = store.by_account_id(id).await.unwrap().unwrap();
        assert_eq!(
            got.email_verified_at, None,
            "a changed address is pending again"
        );

        // set_email(None, None, None) drops it entirely.
        store.set_email(id, None, None, None).await.unwrap();
        let got = store.by_account_id(id).await.unwrap().unwrap();
        assert_eq!(got.email(&SK).unwrap(), None);
        assert_eq!(got.email_verify_hash, None);

        assert!(matches!(
            store.set_email(Uuid::new_v4(), None, None, None).await,
            Err(AccountError::NotFound)
        ));
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

    #[tokio::test]
    async fn host_servers_round_trips() {
        let store = MemAccountStore::default();
        assert_eq!(store.host_servers().await.unwrap(), None);

        store
            .set_host_servers(&["https://a.example".into(), "https://b.example".into()])
            .await
            .unwrap();
        assert_eq!(
            store.host_servers().await.unwrap(),
            Some(vec!["https://a.example".into(), "https://b.example".into()])
        );

        store
            .set_host_servers(&["https://c.example".into()])
            .await
            .unwrap();
        assert_eq!(
            store.host_servers().await.unwrap(),
            Some(vec!["https://c.example".into()])
        );
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
