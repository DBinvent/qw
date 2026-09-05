//! The account secret envelope (`server/multi-user.md`).
//!
//! One **master key** per account: 32 random bytes, generated once, that
//! encrypt the account blob (identity key + event log) directly and never
//! change. It lives only in a live session's RAM — never serialised, never
//! logged, `Debug` redacted, zeroized on drop.
//!
//! Around it a **set of wrappings**: each [`Wrapping`] is one KEK's sealed
//! copy of that same master. The simple start has a single KEK kind — a
//! user secret (passphrase / raw key / seed phrase) through Argon2id — but
//! the shapes here (`Vec<Wrapping>`, a `#[non_exhaustive]` [`KekKind`],
//! per-row `created_at` / `not_after` / `rotate`) are the ones the fob /
//! KMS / approval-app / phone factors slot into later with no change to
//! the master or the stored blob.

use std::fmt;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

const MASTER_LEN: usize = 32;
const KEK_LEN: usize = 32;
const XNONCE_LEN: usize = 24;
const SALT_LEN: usize = 16;
/// The AEAD tag on `seal`/`open` covers at least this many bytes.
const AEAD_TAG_LEN: usize = 16;
/// Domain separator bound as AAD into every KEK→master wrapping, so a
/// wrapping can never be replayed as an account blob or vice versa.
const WRAP_AAD: &[u8] = b"qw-web/envelope/master-v1";

/// 32 bytes that encrypt the account blob directly. RAM-only for the life
/// of a session.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MasterKey([u8; MASTER_LEN]);

impl MasterKey {
    /// A fresh master key for a new account.
    pub fn random() -> Self {
        let mut k = [0u8; MASTER_LEN];
        OsRng.fill_bytes(&mut k);
        Self(k)
    }

    fn from_slice(b: &[u8]) -> Result<Self, EnvelopeError> {
        let arr: [u8; MASTER_LEN] = b.try_into().map_err(|_| EnvelopeError::Malformed)?;
        Ok(Self(arr))
    }

    fn aead(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(Key::from_slice(&self.0))
    }

    /// Seal a blob under the master key. Output is `nonce || ciphertext`;
    /// `aad` is bound and must match on `open` (pass the `account_id`).
    pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; XNONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let mut ct = self
            .aead()
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad })
            .expect("XChaCha20-Poly1305 encryption of an in-memory buffer cannot fail");
        let mut out = Vec::with_capacity(XNONCE_LEN + ct.len());
        out.extend_from_slice(&nonce);
        out.append(&mut ct);
        out
    }

    /// Recover a blob sealed by [`MasterKey::seal`] with the same `aad`.
    /// A wrong key, a wrong `aad`, or a tampered blob is [`EnvelopeError::Decrypt`].
    pub fn open(&self, blob: &[u8], aad: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
        if blob.len() < XNONCE_LEN + AEAD_TAG_LEN {
            return Err(EnvelopeError::Malformed);
        }
        let (nonce, ct) = blob.split_at(XNONCE_LEN);
        self.aead()
            .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
            .map_err(|_| EnvelopeError::Decrypt)
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MasterKey(redacted)")
    }
}

/// A key-encryption key: 32 bytes derived from some factor, used only to
/// wrap/unwrap the master. Never stored.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
struct Kek([u8; KEK_LEN]);

impl Kek {
    fn aead(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(Key::from_slice(&self.0))
    }
}

/// Which kind of factor a [`Wrapping`] holds. `#[non_exhaustive]`: the
/// fob / KMS / approval-app / phone / recovery kinds land here later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum KekKind {
    /// A passphrase, raw key, or seed phrase through Argon2id.
    Secret,
}

/// The KDF settings for a `Secret` wrapping — stored so a re-derive uses
/// the same cost and salt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// Only `"argon2id"` today.
    pub alg: String,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    #[serde(with = "hex_bytes")]
    pub salt: Vec<u8>,
}

impl KdfParams {
    /// Argon2id at the `argon2` crate's current recommended cost, with a
    /// fresh random salt.
    pub fn argon2id() -> Self {
        let mut salt = vec![0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        Self {
            alg: "argon2id".into(),
            m_cost: Params::DEFAULT_M_COST,
            t_cost: Params::DEFAULT_T_COST,
            p_cost: Params::DEFAULT_P_COST,
            salt,
        }
    }
}

fn derive_kek(secret: &[u8], p: &KdfParams) -> Result<Kek, EnvelopeError> {
    if p.alg != "argon2id" {
        return Err(EnvelopeError::Kdf(format!("unsupported kdf {:?}", p.alg)));
    }
    let params = Params::new(p.m_cost, p.t_cost, p.p_cost, Some(KEK_LEN))
        .map_err(|e| EnvelopeError::Kdf(e.to_string()))?;
    let mut out = [0u8; KEK_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(secret, &p.salt, &mut out)
        .map_err(|e| EnvelopeError::Kdf(e.to_string()))?;
    Ok(Kek(out))
}

/// One KEK's sealed copy of the account master key, plus the metadata that
/// makes it independently datable and rotatable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wrapping {
    /// A user-facing label ("passphrase", "seed", "yubikey-blue").
    pub tag: String,
    pub kind: KekKind,
    pub kdf: KdfParams,
    #[serde(with = "hex_bytes")]
    pub nonce: Vec<u8>,
    /// `AEAD(master)` under the KEK.
    #[serde(with = "hex_bytes")]
    pub wrapped: Vec<u8>,
    /// Unix seconds.
    pub created_at: u64,
    /// Unix seconds; past this the wrapping is refused for new logins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<u64>,
    /// Operator- or user-set: re-wrap or drop this factor at next
    /// opportunity.
    #[serde(default)]
    pub rotate: bool,
}

impl Wrapping {
    /// Wrap `master` under a KEK derived from `secret`. Fresh salt and
    /// nonce every call, so two wrappings of one master never collide.
    pub fn seal_secret(
        tag: impl Into<String>,
        secret: &[u8],
        master: &MasterKey,
    ) -> Result<Self, EnvelopeError> {
        let kdf = KdfParams::argon2id();
        let kek = derive_kek(secret, &kdf)?;
        let mut nonce = [0u8; XNONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let wrapped = kek
            .aead()
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload { msg: &master.0, aad: WRAP_AAD },
            )
            .map_err(|_| EnvelopeError::Encrypt)?;
        Ok(Self {
            tag: tag.into(),
            kind: KekKind::Secret,
            kdf,
            nonce: nonce.to_vec(),
            wrapped,
            created_at: unix_secs(),
            not_after: None,
            rotate: false,
        })
    }

    /// Recover the master key, given the secret for this wrapping.
    pub fn unwrap_secret(&self, secret: &[u8]) -> Result<MasterKey, EnvelopeError> {
        if self.kind != KekKind::Secret {
            return Err(EnvelopeError::Kind);
        }
        if self.nonce.len() != XNONCE_LEN {
            return Err(EnvelopeError::Malformed);
        }
        let kek = derive_kek(secret, &self.kdf)?;
        let plain = kek
            .aead()
            .decrypt(
                XNonce::from_slice(&self.nonce),
                Payload { msg: &self.wrapped, aad: WRAP_AAD },
            )
            .map_err(|_| EnvelopeError::Decrypt)?;
        MasterKey::from_slice(&plain)
    }

    /// Hard-expired by `not_after`.
    pub fn expired(&self, now: u64) -> bool {
        self.not_after.is_some_and(|t| now >= t)
    }

    /// Should not be offered for a new login: flagged for rotation, or
    /// expired.
    pub fn stale(&self, now: u64) -> bool {
        self.rotate || self.expired(now)
    }
}

/// Unix seconds, shared by the envelope and the account store.
pub(crate) fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Hex string <-> `Vec<u8>` for the on-disk record.
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        hex::decode(s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug)]
pub enum EnvelopeError {
    Kdf(String),
    Encrypt,
    Decrypt,
    Kind,
    Malformed,
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kdf(m) => write!(f, "key derivation failed: {m}"),
            Self::Encrypt => f.write_str("wrapping the master key failed"),
            Self::Decrypt => f.write_str("wrong secret, or the wrapping was tampered with"),
            Self::Kind => f.write_str("this wrapping is not a user-secret KEK"),
            Self::Malformed => f.write_str("the wrapping or sealed blob is malformed"),
        }
    }
}

impl std::error::Error for EnvelopeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_seal_open_round_trips() {
        let m = MasterKey::random();
        let blob = m.seal(b"ledger bytes", b"acct-1");
        assert_eq!(m.open(&blob, b"acct-1").unwrap(), b"ledger bytes");
    }

    #[test]
    fn open_rejects_a_flipped_bit() {
        let m = MasterKey::random();
        let mut blob = m.seal(b"ledger bytes", b"acct-1");
        let last = blob.len() - 1;
        blob[last] ^= 1;
        assert!(matches!(m.open(&blob, b"acct-1"), Err(EnvelopeError::Decrypt)));
    }

    #[test]
    fn open_rejects_a_different_aad() {
        let m = MasterKey::random();
        let blob = m.seal(b"ledger bytes", b"acct-1");
        assert!(m.open(&blob, b"acct-2").is_err());
    }

    #[test]
    fn a_wrapping_round_trips_the_master() {
        let m = MasterKey::random();
        let w = Wrapping::seal_secret("passphrase", b"hunter2hunter2", &m).unwrap();
        assert_eq!(w.unwrap_secret(b"hunter2hunter2").unwrap().0, m.0);
    }

    #[test]
    fn the_wrong_secret_does_not_unwrap() {
        let m = MasterKey::random();
        let w = Wrapping::seal_secret("passphrase", b"hunter2hunter2", &m).unwrap();
        assert!(matches!(
            w.unwrap_secret(b"nope"),
            Err(EnvelopeError::Decrypt)
        ));
    }

    #[test]
    fn two_wrappings_of_one_master_both_open_it() {
        // Adding a KEK is additive: same master, a second independent row.
        let m = MasterKey::random();
        let a = Wrapping::seal_secret("passphrase", b"first-secret-xxx", &m).unwrap();
        let b = Wrapping::seal_secret("seed", b"second-secret-yyy", &m).unwrap();
        assert_eq!(a.unwrap_secret(b"first-secret-xxx").unwrap().0, m.0);
        assert_eq!(b.unwrap_secret(b"second-secret-yyy").unwrap().0, m.0);
        assert_ne!(a.kdf.salt, b.kdf.salt, "each wrapping gets a fresh salt");
        assert_ne!(a.nonce, b.nonce, "and a fresh nonce");
    }

    #[test]
    fn stale_covers_the_rotate_flag_and_expiry() {
        let m = MasterKey::random();
        let mut w = Wrapping::seal_secret("passphrase", b"hunter2hunter2", &m).unwrap();
        let t = w.created_at;
        assert!(!w.stale(t));

        w.rotate = true;
        assert!(w.stale(t));

        w.rotate = false;
        w.not_after = Some(t);
        assert!(w.expired(t));
        assert!(w.stale(t));
    }

    #[test]
    fn wrapping_serialises_to_json_and_back() {
        let m = MasterKey::random();
        let w = Wrapping::seal_secret("passphrase", b"hunter2hunter2", &m).unwrap();
        let js = serde_json::to_string(&w).unwrap();
        assert!(js.contains("\"kind\":\"secret\""));
        assert!(!js.contains("not_after"), "None is skipped");
        let w2: Wrapping = serde_json::from_str(&js).unwrap();
        assert_eq!(w2.unwrap_secret(b"hunter2hunter2").unwrap().0, m.0);
    }

    #[test]
    fn master_key_debug_is_redacted() {
        let m = MasterKey::random();
        assert_eq!(format!("{m:?}"), "MasterKey(redacted)");
    }
}
