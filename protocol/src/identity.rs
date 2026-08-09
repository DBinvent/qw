//! Identity: `did:key` generation/resolution over the same secp256k1 keypair
//! used to sign Nostr events. Per the design docs, the device key *is* the
//! controller DID's signing key — there is no separate identity key and
//! device key to keep in sync.
//!
//! `did:key` encodes the full compressed (33-byte) public key under the
//! `secp256k1-pub` multicodec (0xe7); the same keypair's x-only (32-byte)
//! public key is what goes in the Nostr event `pubkey` field. Both are
//! derived from one secret key.

use std::fmt;

use secp256k1::{schnorr, Keypair, PublicKey, XOnlyPublicKey, SECP256K1};

/// Multicodec varint prefix for `secp256k1-pub` (code 0xe7).
const SECP256K1_MULTICODEC: [u8; 2] = [0xe7, 0x01];
const DID_KEY_PREFIX: &str = "did:key:";

#[derive(Debug)]
pub enum IdentityError {
    InvalidSecretKey,
    InvalidDidKey(&'static str),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityError::InvalidSecretKey => write!(f, "invalid secp256k1 secret key"),
            IdentityError::InvalidDidKey(why) => write!(f, "invalid did:key: {why}"),
        }
    }
}

impl std::error::Error for IdentityError {}

/// A controller identity: one secp256k1 keypair backing both a `did:key`
/// and the Nostr signing key used for every event kind in `crate::events`.
pub struct Identity {
    keypair: Keypair,
}

impl Identity {
    /// Generate a fresh random identity.
    pub fn generate() -> Self {
        let keypair = Keypair::new_global(&mut secp256k1::rand::rng());
        Self { keypair }
    }

    /// Restore an identity from a raw 32-byte secret key.
    pub fn from_secret_bytes(bytes: [u8; 32]) -> Result<Self, IdentityError> {
        let keypair = Keypair::from_seckey_byte_array(SECP256K1, bytes)
            .map_err(|_| IdentityError::InvalidSecretKey)?;
        Ok(Self { keypair })
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.keypair.secret_bytes()
    }

    /// Full compressed public key — what gets encoded into `did:key`.
    pub fn public_key(&self) -> PublicKey {
        PublicKey::from_keypair(&self.keypair)
    }

    /// x-only public key used as the `pubkey` field of Nostr events.
    pub fn nostr_pubkey(&self) -> XOnlyPublicKey {
        self.keypair.x_only_public_key().0
    }

    /// Hex-encoded x-only public key (NIP-01 `pubkey` string form).
    pub fn nostr_pubkey_hex(&self) -> String {
        self.nostr_pubkey().to_string()
    }

    /// This identity's `did:key` controller identifier.
    pub fn did_key(&self) -> String {
        encode_did_key(&self.public_key())
    }

    /// BIP-340 Schnorr-sign a 32-byte message (a Nostr event id, or any
    /// other 32-byte digest such as a job-lifecycle payload hash).
    pub fn sign_schnorr(&self, msg_hash: &[u8; 32]) -> schnorr::Signature {
        self.keypair.sign_schnorr(msg_hash)
    }
}

/// Encode a secp256k1 public key as a `did:key` string.
pub fn encode_did_key(pk: &PublicKey) -> String {
    let mut bytes = Vec::with_capacity(SECP256K1_MULTICODEC.len() + 33);
    bytes.extend_from_slice(&SECP256K1_MULTICODEC);
    bytes.extend_from_slice(&pk.serialize());
    format!(
        "{DID_KEY_PREFIX}{}",
        multibase::encode(multibase::Base::Base58Btc, bytes)
    )
}

/// Resolve a `did:key` string back to the secp256k1 public key it encodes.
pub fn resolve_did_key(did: &str) -> Result<PublicKey, IdentityError> {
    let method_id = did
        .strip_prefix(DID_KEY_PREFIX)
        .ok_or(IdentityError::InvalidDidKey("missing 'did:key:' prefix"))?;
    let (base, bytes) = multibase::decode(method_id)
        .map_err(|_| IdentityError::InvalidDidKey("invalid multibase encoding"))?;
    if base != multibase::Base::Base58Btc {
        return Err(IdentityError::InvalidDidKey(
            "expected base58btc ('z') encoding",
        ));
    }
    if bytes.len() != SECP256K1_MULTICODEC.len() + 33 || bytes[..2] != SECP256K1_MULTICODEC {
        return Err(IdentityError::InvalidDidKey(
            "not a secp256k1-pub did:key (unexpected multicodec prefix or length)",
        ));
    }
    PublicKey::from_slice(&bytes[2..])
        .map_err(|_| IdentityError::InvalidDidKey("public key bytes do not parse"))
}

/// Resolve a `did:key` directly to the x-only pubkey used in Nostr events.
pub fn resolve_did_key_to_nostr_pubkey(did: &str) -> Result<XOnlyPublicKey, IdentityError> {
    Ok(resolve_did_key(did)?.x_only_public_key().0)
}

/// Verify a hex-encoded BIP-340 signature by a hex-encoded x-only pubkey
/// over `hash`. Returns `false` (never panics or errors) on any malformed
/// input — for callers that only care "does this count" and want a
/// garbled entry to simply not count, not abort a larger computation
/// (e.g. `crate::recovery`'s quorum counting, `crate::contract`'s
/// credit-issuance dual-signature check).
pub fn verify_hex_schnorr(pubkey_hex: &str, sig_hex: &str, hash: &[u8; 32]) -> bool {
    let Some(pk_bytes): Option<[u8; 32]> =
        hex::decode(pubkey_hex).ok().and_then(|v| v.try_into().ok())
    else {
        return false;
    };
    let Ok(xonly) = XOnlyPublicKey::from_byte_array(pk_bytes) else {
        return false;
    };
    let Some(sig_bytes): Option<[u8; 64]> =
        hex::decode(sig_hex).ok().and_then(|v| v.try_into().ok())
    else {
        return false;
    };
    schnorr::Signature::from_byte_array(sig_bytes)
        .verify(hash, &xonly)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn did_key_round_trips() {
        let id = Identity::generate();
        let did = id.did_key();
        assert!(did.starts_with("did:key:z"));

        let resolved = resolve_did_key(&did).expect("should resolve");
        assert_eq!(resolved, id.public_key());
    }

    #[test]
    fn did_key_matches_nostr_pubkey() {
        let id = Identity::generate();
        let did = id.did_key();

        let resolved_nostr_pk = resolve_did_key_to_nostr_pubkey(&did).expect("should resolve");
        assert_eq!(resolved_nostr_pk, id.nostr_pubkey());
    }

    #[test]
    fn from_secret_bytes_is_deterministic() {
        let bytes = [7u8; 32];
        let a = Identity::from_secret_bytes(bytes).unwrap();
        let b = Identity::from_secret_bytes(bytes).unwrap();
        assert_eq!(a.did_key(), b.did_key());
        assert_eq!(a.nostr_pubkey_hex(), b.nostr_pubkey_hex());
    }

    #[test]
    fn rejects_malformed_did() {
        assert!(resolve_did_key("did:key:zInvalidGarbage").is_err());
        assert!(resolve_did_key("not-a-did").is_err());
    }

    #[test]
    fn sign_schnorr_verifies_against_nostr_pubkey() {
        let id = Identity::generate();
        let msg = [42u8; 32];
        let sig = id.sign_schnorr(&msg);
        assert!(sig.verify(&msg, &id.nostr_pubkey()).is_ok());
    }
}
