//! Verifiable Credential for a completed piece of work, per §2: issuer =
//! counterparty, subject = worker, claim = `{hours, rate, ko, km,
//! skill_tags, timestamp}`. Selective disclosure via SD-JWT — the holder
//! (worker) can present only some fields of an issued credential and the
//! verifier can still check the presented ones against the issuer's
//! signature, without seeing the withheld ones.
//!
//! This is a minimal from-scratch SD-JWT (draft-ietf-oauth-sd-jwt-vc
//! shape: `<jws>~<disclosure>~...~`), not a JOSE-library integration. The
//! signature is BIP-340 Schnorr over `sha256(header.payload)` — reusing
//! the same primitive as Nostr event signing (`crate::identity`) rather
//! than adding a second (ECDSA/ES256K) signing stack for one credential
//! type. The JWT `alg` header is set to the non-standard `"BIP340"` to be
//! honest about that; swap in real ES256K if/when interop with external
//! SD-JWT wallets is needed.

use std::fmt;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use secp256k1::{rand::RngCore, schnorr};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::identity::{self, Identity};

const ALG: &str = "BIP340";
const VCT: &str = "qw-work-claim";

/// `Hours × Rate × ko × km` (abstract.md). `ko`/`km` are optional —
/// omittable to simplify negotiation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkClaim {
    pub hours: f64,
    pub rate: f64,
    pub ko: Option<f64>,
    pub km: Option<f64>,
    pub skill_tags: Vec<String>,
    pub timestamp: u64,
}

impl WorkClaim {
    /// Every field is individually selectively-disclosable — the doc's
    /// "SD-JWT... for MVP field-level hiding" requirement.
    fn disclosable_fields(&self) -> Vec<(&'static str, Value)> {
        let mut fields = vec![
            ("hours", json!(self.hours)),
            ("rate", json!(self.rate)),
            ("skill_tags", json!(self.skill_tags)),
            ("timestamp", json!(self.timestamp)),
        ];
        if let Some(ko) = self.ko {
            fields.push(("ko", json!(ko)));
        }
        if let Some(km) = self.km {
            fields.push(("km", json!(km)));
        }
        fields
    }
}

#[derive(Debug)]
pub enum VcError {
    Malformed(&'static str),
    InvalidIssuerDid,
    SignatureVerificationFailed,
    UndisclosedDigest,
}

impl fmt::Display for VcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VcError::Malformed(why) => write!(f, "malformed SD-JWT: {why}"),
            VcError::InvalidIssuerDid => write!(f, "issuer 'iss' is not a resolvable did:key"),
            VcError::SignatureVerificationFailed => {
                write!(f, "SD-JWT signature verification failed")
            }
            VcError::UndisclosedDigest => write!(
                f,
                "a disclosure's digest is not committed to in the payload"
            ),
        }
    }
}

impl std::error::Error for VcError {}

#[derive(Debug, Clone, PartialEq)]
struct Disclosure {
    salt: String,
    claim_name: String,
    claim_value: Value,
}

impl Disclosure {
    fn new(claim_name: &str, claim_value: Value) -> Self {
        let mut salt_bytes = [0u8; 16];
        secp256k1::rand::rng().fill_bytes(&mut salt_bytes);
        Self {
            salt: URL_SAFE_NO_PAD.encode(salt_bytes),
            claim_name: claim_name.to_string(),
            claim_value,
        }
    }

    fn encode(&self) -> String {
        let arr = json!([self.salt, self.claim_name, self.claim_value]).to_string();
        URL_SAFE_NO_PAD.encode(arr)
    }

    fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.encode().as_bytes());
        URL_SAFE_NO_PAD.encode(hasher.finalize())
    }

    fn decode(s: &str) -> Result<Self, VcError> {
        let raw = URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|_| VcError::Malformed("disclosure is not valid base64url"))?;
        let value: Value = serde_json::from_slice(&raw)
            .map_err(|_| VcError::Malformed("disclosure is not a JSON array"))?;
        let arr = value
            .as_array()
            .ok_or(VcError::Malformed("disclosure is not a JSON array"))?;
        let [salt, name, claim_value] = <[Value; 3]>::try_from(arr.clone())
            .map_err(|_| VcError::Malformed("disclosure array must have exactly 3 elements"))?;
        let salt = salt
            .as_str()
            .ok_or(VcError::Malformed("disclosure salt must be a string"))?
            .to_string();
        let claim_name = name
            .as_str()
            .ok_or(VcError::Malformed("disclosure claim name must be a string"))?
            .to_string();
        Ok(Self {
            salt,
            claim_name,
            claim_value,
        })
    }
}

/// An issued (or presented) work credential: the signed `header.payload`
/// plus whichever disclosures currently travel with it.
#[derive(Debug, Clone, PartialEq)]
pub struct SdJwtVc {
    header_b64: String,
    payload_b64: String,
    sig_b64: String,
    disclosures: Vec<Disclosure>,
}

/// Issue a `WorkClaim` credential: `issuer` (the counterparty) attests
/// about `subject_did` (the worker). Returns the full credential with
/// every field's disclosure attached — the holder narrows it down with
/// [`SdJwtVc::present`] before showing it to a third party.
pub fn issue(issuer: &Identity, subject_did: &str, claim: &WorkClaim) -> SdJwtVc {
    let disclosures: Vec<Disclosure> = claim
        .disclosable_fields()
        .into_iter()
        .map(|(name, value)| Disclosure::new(name, value))
        .collect();
    let sd_digests: Vec<String> = disclosures.iter().map(Disclosure::digest).collect();

    let header = json!({"alg": ALG, "typ": "vc+sd-jwt"});
    let payload = json!({
        "iss": issuer.did_key(),
        "sub": subject_did,
        "vct": VCT,
        "_sd": sd_digests,
        "_sd_alg": "sha-256",
    });

    let header_b64 = URL_SAFE_NO_PAD.encode(header.to_string());
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
    let sig = sign_over(issuer, &header_b64, &payload_b64);

    SdJwtVc {
        header_b64,
        payload_b64,
        sig_b64: URL_SAFE_NO_PAD.encode(sig.to_byte_array()),
        disclosures,
    }
}

fn signing_input_hash(header_b64: &str, payload_b64: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(header_b64.as_bytes());
    hasher.update(b".");
    hasher.update(payload_b64.as_bytes());
    hasher.finalize().into()
}

fn sign_over(issuer: &Identity, header_b64: &str, payload_b64: &str) -> schnorr::Signature {
    issuer.sign_schnorr(&signing_input_hash(header_b64, payload_b64))
}

impl SdJwtVc {
    /// Compact serialization: `header.payload.sig~disclosure~disclosure~...~`.
    pub fn to_compact(&self) -> String {
        let mut out = format!("{}.{}.{}", self.header_b64, self.payload_b64, self.sig_b64);
        for d in &self.disclosures {
            out.push('~');
            out.push_str(&d.encode());
        }
        out.push('~');
        out
    }

    pub fn from_compact(s: &str) -> Result<Self, VcError> {
        let mut parts = s.split('~');
        let jws = parts.next().ok_or(VcError::Malformed("empty input"))?;
        let mut jws_parts = jws.split('.');
        let (header_b64, payload_b64, sig_b64) = (
            jws_parts
                .next()
                .ok_or(VcError::Malformed("missing header"))?
                .to_string(),
            jws_parts
                .next()
                .ok_or(VcError::Malformed("missing payload"))?
                .to_string(),
            jws_parts
                .next()
                .ok_or(VcError::Malformed("missing signature"))?
                .to_string(),
        );
        if jws_parts.next().is_some() {
            return Err(VcError::Malformed("too many '.'-separated JWS segments"));
        }

        let disclosures = parts
            .filter(|s| !s.is_empty())
            .map(Disclosure::decode)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            header_b64,
            payload_b64,
            sig_b64,
            disclosures,
        })
    }

    /// Holder-side selective disclosure: keep only the named claims,
    /// dropping the rest before presenting to a verifier.
    pub fn present(&self, reveal: &[&str]) -> SdJwtVc {
        SdJwtVc {
            header_b64: self.header_b64.clone(),
            payload_b64: self.payload_b64.clone(),
            sig_b64: self.sig_b64.clone(),
            disclosures: self
                .disclosures
                .iter()
                .filter(|d| reveal.contains(&d.claim_name.as_str()))
                .cloned()
                .collect(),
        }
    }

    /// Verify the issuer's signature and that every attached disclosure is
    /// one the issuer actually committed to (its digest is in `_sd`).
    /// Returns the visible claim set: `iss`/`sub`/`vct` plus whichever
    /// fields are disclosed on `self`.
    pub fn verify(&self) -> Result<VerifiedVc, VcError> {
        let payload_json = URL_SAFE_NO_PAD
            .decode(&self.payload_b64)
            .map_err(|_| VcError::Malformed("payload is not valid base64url"))?;
        let payload: Value = serde_json::from_slice(&payload_json)
            .map_err(|_| VcError::Malformed("payload is not valid JSON"))?;
        let payload = payload
            .as_object()
            .ok_or(VcError::Malformed("payload is not a JSON object"))?;

        let iss = payload
            .get("iss")
            .and_then(Value::as_str)
            .ok_or(VcError::Malformed("missing 'iss'"))?;
        let issuer_pubkey = identity::resolve_did_key_to_nostr_pubkey(iss)
            .map_err(|_| VcError::InvalidIssuerDid)?;

        let sig_bytes: [u8; 64] = URL_SAFE_NO_PAD
            .decode(&self.sig_b64)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or(VcError::Malformed("signature is not 64 bytes"))?;
        let sig = schnorr::Signature::from_byte_array(sig_bytes);
        let hash = signing_input_hash(&self.header_b64, &self.payload_b64);
        sig.verify(&hash, &issuer_pubkey)
            .map_err(|_| VcError::SignatureVerificationFailed)?;

        let sd_digests: Vec<&str> = payload
            .get("_sd")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();

        let mut claims = Map::new();
        for (k, v) in payload {
            if k != "_sd" && k != "_sd_alg" {
                claims.insert(k.clone(), v.clone());
            }
        }
        for d in &self.disclosures {
            if !sd_digests.contains(&d.digest().as_str()) {
                return Err(VcError::UndisclosedDigest);
            }
            claims.insert(d.claim_name.clone(), d.claim_value.clone());
        }

        Ok(VerifiedVc { claims })
    }
}

/// The claim set a verifier actually gets to see: always-visible fields
/// (`iss`, `sub`, `vct`) plus whichever `WorkClaim` fields were disclosed.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedVc {
    claims: Map<String, Value>,
}

impl VerifiedVc {
    pub fn get(&self, field: &str) -> Option<&Value> {
        self.claims.get(field)
    }

    pub fn issuer_did(&self) -> Option<&str> {
        self.get("iss").and_then(Value::as_str)
    }

    pub fn subject_did(&self) -> Option<&str> {
        self.get("sub").and_then(Value::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_claim() -> WorkClaim {
        WorkClaim {
            hours: 8.0,
            rate: 40.0,
            ko: Some(1.1),
            km: None,
            skill_tags: vec!["it/backend/languages#rust".to_string()],
            timestamp: 1_700_000_000,
        }
    }

    #[test]
    fn full_disclosure_round_trips_all_fields() {
        let issuer = Identity::generate();
        let subject = Identity::generate();
        let vc = issue(&issuer, &subject.did_key(), &sample_claim());

        let compact = vc.to_compact();
        let parsed = SdJwtVc::from_compact(&compact).unwrap();
        let verified = parsed.verify().unwrap();

        assert_eq!(verified.issuer_did(), Some(issuer.did_key().as_str()));
        assert_eq!(verified.subject_did(), Some(subject.did_key().as_str()));
        assert_eq!(verified.get("hours"), Some(&json!(8.0)));
        assert_eq!(verified.get("rate"), Some(&json!(40.0)));
        assert_eq!(verified.get("ko"), Some(&json!(1.1)));
        assert_eq!(verified.get("km"), None);
    }

    #[test]
    fn selective_presentation_hides_unrevealed_fields() {
        let issuer = Identity::generate();
        let subject = Identity::generate();
        let vc = issue(&issuer, &subject.did_key(), &sample_claim());

        let presented = vc.present(&["skill_tags"]);
        let verified = presented.verify().unwrap();

        assert_eq!(
            verified.get("skill_tags"),
            Some(&json!(["it/backend/languages#rust"]))
        );
        assert_eq!(
            verified.get("hours"),
            None,
            "hours was withheld and must not be reconstructable"
        );
        assert_eq!(verified.get("rate"), None);
    }

    #[test]
    fn forged_disclosure_is_rejected() {
        let issuer = Identity::generate();
        let subject = Identity::generate();
        let vc = issue(&issuer, &subject.did_key(), &sample_claim());

        let mut presented = vc.present(&["hours"]);
        // splice in a disclosure for a value the issuer never signed
        presented
            .disclosures
            .push(Disclosure::new("rate", json!(999999.0)));

        assert!(matches!(
            presented.verify(),
            Err(VcError::UndisclosedDigest)
        ));
    }

    #[test]
    fn tampered_payload_fails_signature_check() {
        let issuer = Identity::generate();
        let subject = Identity::generate();
        let vc = issue(&issuer, &subject.did_key(), &sample_claim());

        let mut compact = vc.to_compact();
        // corrupt one payload character without touching the '.' delimiters
        let bad_payload: String = vc.payload_b64.chars().rev().collect();
        compact = compact.replacen(&vc.payload_b64, &bad_payload, 1);

        let parsed = SdJwtVc::from_compact(&compact).unwrap();
        assert!(matches!(
            parsed.verify(),
            Err(VcError::SignatureVerificationFailed) | Err(VcError::Malformed(_))
        ));
    }
}
