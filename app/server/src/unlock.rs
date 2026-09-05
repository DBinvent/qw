//! The KEK set — presenting, adding, dropping and rotating the wrappings
//! that seal an account's master key (`server/multi-user.md`).
//!
//! One kind today: a user secret (passphrase / raw key / seed phrase)
//! through Argon2id, `envelope::KekKind::Secret`. The fob / KMS /
//! approval-app / phone factors add a `present_*` / `add_*` pair each and
//! wrap the *same* master, so everything here stays as is.
//!
//! The master key is only ever produced here (from [`present_secret`]) or
//! at `register`; callers hold it for the life of a session and zeroize it
//! on the way out.

use serde::Serialize;

use crate::envelope::{unix_secs, EnvelopeError, KekKind, MasterKey, Wrapping};

#[derive(Debug)]
pub enum UnlockError {
    /// No wrapping in the set accepted the secret.
    NoMatch,
    /// Refused: this would remove the last way into the account
    /// (identity loss — that path is the recovery code, not `drop`).
    WouldOrphan,
    /// No wrapping carries that tag.
    UnknownTag(String),
    /// A wrapping with that tag already exists.
    DuplicateTag(String),
    /// Sealing a new wrapping failed (bad Argon2 params).
    Envelope(EnvelopeError),
}

impl std::fmt::Display for UnlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMatch => f.write_str("no key matched that secret"),
            Self::WouldOrphan => f.write_str("that is the only remaining key — cannot remove it"),
            Self::UnknownTag(t) => write!(f, "no key tagged {t:?}"),
            Self::DuplicateTag(t) => write!(f, "a key tagged {t:?} already exists"),
            Self::Envelope(e) => write!(f, "wrapping the key failed: {e}"),
        }
    }
}
impl std::error::Error for UnlockError {}
impl From<EnvelopeError> for UnlockError {
    fn from(e: EnvelopeError) -> Self {
        Self::Envelope(e)
    }
}

/// One KEK row for the UI — metadata only, never key material.
#[derive(Debug, Clone, Serialize)]
pub struct WrappingInfo {
    pub tag: String,
    pub kind: KekKind,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_after: Option<u64>,
    pub rotate: bool,
    /// Flagged for rotation, or past `not_after`.
    pub stale: bool,
}

/// The KEK set as the account's key page would show it.
pub fn enumerate(wrappings: &[Wrapping]) -> Vec<WrappingInfo> {
    let now = unix_secs();
    wrappings
        .iter()
        .map(|w| WrappingInfo {
            tag: w.tag.clone(),
            kind: w.kind,
            created_at: w.created_at,
            not_after: w.not_after,
            rotate: w.rotate,
            stale: w.stale(now),
        })
        .collect()
}

/// A master key just recovered from the KEK set.
pub struct Unlocked {
    pub master: MasterKey,
    /// The tag of the wrapping that let us in.
    pub via: String,
    /// That wrapping is flagged for rotation or expired — the caller holds
    /// the master now and should [`rotate_secret`] or [`drop_tag`] it.
    pub stale: bool,
}

/// Try `secret` against every `Secret` wrapping, fresh rows first so a
/// live key is preferred over a stale one. The AEAD tag is the
/// disambiguator when several rows sit in the same secret space (a
/// passphrase plus a recovery code, or rows merged from another instance).
pub fn present_secret(wrappings: &[Wrapping], secret: &[u8]) -> Result<Unlocked, UnlockError> {
    let now = unix_secs();
    let mut candidates: Vec<&Wrapping> = wrappings
        .iter()
        .filter(|w| w.kind == KekKind::Secret)
        .collect();
    candidates.sort_by_key(|w| w.stale(now)); // false (fresh) before true (stale)

    let mut broken: Option<EnvelopeError> = None;
    for w in candidates {
        match w.unwrap_secret(secret) {
            Ok(master) => {
                return Ok(Unlocked {
                    master,
                    via: w.tag.clone(),
                    stale: w.stale(now),
                })
            }
            // wrong secret for this row — the expected case with >1 row
            Err(EnvelopeError::Decrypt) => {}
            // a corrupt row must not block a good one; remember the reason
            Err(e) => broken = Some(e),
        }
    }
    match broken {
        Some(e) => Err(UnlockError::Envelope(e)),
        None => Err(UnlockError::NoMatch),
    }
}

/// Append a fresh `Secret` wrapping of `master` under `secret`, tagged
/// `tag`. The master is unchanged; this is additive.
pub fn add_secret(
    wrappings: &mut Vec<Wrapping>,
    master: &MasterKey,
    tag: &str,
    secret: &[u8],
) -> Result<(), UnlockError> {
    if wrappings.iter().any(|w| w.tag == tag) {
        return Err(UnlockError::DuplicateTag(tag.to_string()));
    }
    wrappings.push(Wrapping::seal_secret(tag, secret, master)?);
    Ok(())
}

/// Remove the wrapping tagged `tag`. Refuses to remove the last one —
/// losing every KEK is identity loss, reached through the recovery code,
/// not here.
pub fn drop_tag(wrappings: &mut Vec<Wrapping>, tag: &str) -> Result<Wrapping, UnlockError> {
    let idx = wrappings
        .iter()
        .position(|w| w.tag == tag)
        .ok_or_else(|| UnlockError::UnknownTag(tag.to_string()))?;
    if wrappings.len() == 1 {
        return Err(UnlockError::WouldOrphan);
    }
    Ok(wrappings.remove(idx))
}

/// Re-seal the `tag` wrapping of `master` under `new_secret` — fresh salt
/// and nonce, `rotate` cleared. A passphrase change.
pub fn rotate_secret(
    wrappings: &mut [Wrapping],
    master: &MasterKey,
    tag: &str,
    new_secret: &[u8],
) -> Result<(), UnlockError> {
    let slot = wrappings
        .iter_mut()
        .find(|w| w.tag == tag)
        .ok_or_else(|| UnlockError::UnknownTag(tag.to_string()))?;
    *slot = Wrapping::seal_secret(tag, new_secret, master)?;
    Ok(())
}

/// Mark a wrapping so the next login re-wraps or drops it.
pub fn flag_for_rotation(wrappings: &mut [Wrapping], tag: &str) -> Result<(), UnlockError> {
    let slot = wrappings
        .iter_mut()
        .find(|w| w.tag == tag)
        .ok_or_else(|| UnlockError::UnknownTag(tag.to_string()))?;
    slot.rotate = true;
    Ok(())
}

/// Post-login housekeeping: drop every stale wrapping **provided** at
/// least one fresh wrapping remains. Returns the tags dropped. An
/// all-stale set is left intact — an overdue key beats a locked-out user.
pub fn sweep_stale(wrappings: &mut Vec<Wrapping>) -> Vec<String> {
    let now = unix_secs();
    if !wrappings.iter().any(|w| !w.stale(now)) {
        return Vec::new();
    }
    let mut dropped = Vec::new();
    wrappings.retain(|w| {
        if w.stale(now) {
            dropped.push(w.tag.clone());
            false
        } else {
            true
        }
    });
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(master: &MasterKey, tag: &str, secret: &[u8]) -> Wrapping {
        Wrapping::seal_secret(tag, secret, master).unwrap()
    }

    #[test]
    fn present_finds_the_wrapping_that_holds_the_secret() {
        let m = MasterKey::random();
        let set = vec![
            wrap(&m, "passphrase", b"first-secret-xx"),
            wrap(&m, "recovery", b"second-secret-yy"),
        ];

        let a = present_secret(&set, b"first-secret-xx").unwrap();
        assert_eq!(a.via, "passphrase");
        assert!(!a.stale);

        let b = present_secret(&set, b"second-secret-yy").unwrap();
        assert_eq!(b.via, "recovery");
    }

    #[test]
    fn the_same_master_comes_back_through_any_wrapping() {
        let m = MasterKey::random();
        let set = vec![
            wrap(&m, "passphrase", b"first-secret-xx"),
            wrap(&m, "recovery", b"second-secret-yy"),
        ];
        let a = present_secret(&set, b"first-secret-xx").unwrap();
        let b = present_secret(&set, b"second-secret-yy").unwrap();
        // a blob sealed under one opens under the other
        let blob = a.master.seal(b"ledger", b"acct");
        assert_eq!(b.master.open(&blob, b"acct").unwrap(), b"ledger");
    }

    #[test]
    fn present_wrong_secret_is_no_match() {
        let m = MasterKey::random();
        let set = vec![wrap(&m, "passphrase", b"the-real-secret")];
        assert!(matches!(
            present_secret(&set, b"not-it"),
            Err(UnlockError::NoMatch)
        ));
    }

    #[test]
    fn present_prefers_a_fresh_wrapping_over_a_stale_one() {
        let m = MasterKey::random();
        let mut old = wrap(&m, "old-passphrase", b"shared-secret-x");
        old.rotate = true;
        let fresh = wrap(&m, "new-passphrase", b"shared-secret-x");
        let set = vec![old, fresh];

        let u = present_secret(&set, b"shared-secret-x").unwrap();
        assert_eq!(u.via, "new-passphrase");
        assert!(!u.stale);
    }

    #[test]
    fn present_reports_stale_when_only_a_stale_wrapping_matches() {
        let m = MasterKey::random();
        let mut w = wrap(&m, "passphrase", b"the-secret-value");
        w.rotate = true;
        let set = vec![w];

        let u = present_secret(&set, b"the-secret-value").unwrap();
        assert!(u.stale);
        assert_eq!(u.via, "passphrase");
    }

    #[test]
    fn add_secret_appends_and_rejects_a_duplicate_tag() {
        let m = MasterKey::random();
        let mut set = vec![wrap(&m, "passphrase", b"secret-number-one")];

        add_secret(&mut set, &m, "seed", b"twelve words here").unwrap();
        assert_eq!(set.len(), 2);
        assert_eq!(present_secret(&set, b"twelve words here").unwrap().via, "seed");

        assert!(matches!(
            add_secret(&mut set, &m, "seed", b"another"),
            Err(UnlockError::DuplicateTag(t)) if t == "seed"
        ));
    }

    #[test]
    fn drop_tag_removes_but_never_the_last() {
        let m = MasterKey::random();
        let mut set = vec![
            wrap(&m, "passphrase", b"secret-a-aaaaaa"),
            wrap(&m, "seed", b"secret-b-bbbbbb"),
        ];

        drop_tag(&mut set, "seed").unwrap();
        assert_eq!(set.len(), 1);
        assert!(present_secret(&set, b"secret-a-aaaaaa").is_ok());

        assert!(matches!(
            drop_tag(&mut set, "passphrase"),
            Err(UnlockError::WouldOrphan)
        ));
        assert!(matches!(
            drop_tag(&mut set, "nope"),
            Err(UnlockError::UnknownTag(t)) if t == "nope"
        ));
    }

    #[test]
    fn rotate_secret_swaps_the_accepted_secret() {
        let m = MasterKey::random();
        let mut set = vec![wrap(&m, "passphrase", b"old-passphrase-v")];

        rotate_secret(&mut set, &m, "passphrase", b"new-passphrase-v").unwrap();
        assert!(matches!(
            present_secret(&set, b"old-passphrase-v"),
            Err(UnlockError::NoMatch)
        ));
        assert_eq!(
            present_secret(&set, b"new-passphrase-v").unwrap().via,
            "passphrase"
        );

        assert!(matches!(
            rotate_secret(&mut set, &m, "ghost", b"x"),
            Err(UnlockError::UnknownTag(_))
        ));
    }

    #[test]
    fn flag_for_rotation_marks_a_row_and_makes_present_report_it_stale() {
        let m = MasterKey::random();
        let mut set = vec![wrap(&m, "passphrase", b"a-secret-value-x")];
        assert!(!present_secret(&set, b"a-secret-value-x").unwrap().stale);

        flag_for_rotation(&mut set, "passphrase").unwrap();
        assert!(present_secret(&set, b"a-secret-value-x").unwrap().stale);

        assert!(matches!(
            flag_for_rotation(&mut set, "ghost"),
            Err(UnlockError::UnknownTag(_))
        ));
    }

    #[test]
    fn sweep_stale_drops_stale_only_when_a_fresh_one_survives() {
        let m = MasterKey::random();
        let mut s1 = wrap(&m, "old-1", b"secret-1-xxxxxx");
        s1.rotate = true;
        let mut s2 = wrap(&m, "old-2", b"secret-2-xxxxxx");
        s2.not_after = Some(0); // long expired
        let fresh = wrap(&m, "current", b"secret-3-xxxxxx");
        let mut set = vec![s1, s2, fresh];

        let dropped = sweep_stale(&mut set);
        assert_eq!(dropped.len(), 2);
        assert!(dropped.contains(&"old-1".to_string()));
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].tag, "current");

        // an all-stale set is left intact
        let mut only = wrap(&m, "only", b"secret-only-xxx");
        only.rotate = true;
        let mut set2 = vec![only];
        assert!(sweep_stale(&mut set2).is_empty());
        assert_eq!(set2.len(), 1);
    }

    #[test]
    fn enumerate_exposes_metadata_not_key_material() {
        let m = MasterKey::random();
        let mut w = wrap(&m, "passphrase", b"a-secret-string");
        w.rotate = true;
        let info = enumerate(&[w]);

        assert_eq!(info[0].tag, "passphrase");
        assert_eq!(info[0].kind, KekKind::Secret);
        assert!(info[0].rotate && info[0].stale);

        let js = serde_json::to_string(&info).unwrap();
        for leak in ["wrapped", "nonce", "salt", "kdf"] {
            assert!(!js.contains(leak), "WrappingInfo leaked {leak}");
        }
    }
}
