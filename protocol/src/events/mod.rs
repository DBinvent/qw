//! Minimal NIP-01-shaped Nostr event model: just enough to define, sign and
//! verify QW's custom event kinds without depending on a full relay-client
//! crate. `id` = sha256 of the canonical `[0, pubkey, created_at, kind,
//! tags, content]` array (NIP-01 §"Events and signatures"); `sig` = BIP-340
//! Schnorr signature over that id, by the key behind `pubkey`.

use std::time::{SystemTime, UNIX_EPOCH};

use secp256k1::{schnorr, XOnlyPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::identity::Identity;

pub mod kinds;
pub use kinds::*;

pub type Tag = Vec<String>;

#[derive(Debug, Clone, PartialEq)]
pub struct UnsignedEvent {
    pub pubkey: String,
    pub created_at: u64,
    pub kind: u16,
    pub tags: Vec<Tag>,
    pub content: String,
}

/// Field names already match NIP-01's JSON event object exactly (`id`,
/// `pubkey`, `created_at`, `kind`, `tags`, `content`, `sig`), so deriving
/// `Serialize`/`Deserialize` here produces the real wire format directly
/// — no separate DTO needed wherever an `Event` crosses an HTTP boundary
/// (e.g. `qw_server`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub kind: u16,
    pub tags: Vec<Tag>,
    pub content: String,
    pub sig: String,
}

#[derive(Debug)]
pub enum EventError {
    IdMismatch,
    InvalidPubkey,
    InvalidSignature,
    SignatureVerificationFailed,
}

impl std::fmt::Display for EventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventError::IdMismatch => write!(f, "event id does not match its fields"),
            EventError::InvalidPubkey => write!(f, "event pubkey is not a valid hex x-only key"),
            EventError::InvalidSignature => {
                write!(f, "event sig is not a valid hex schnorr signature")
            }
            EventError::SignatureVerificationFailed => {
                write!(f, "schnorr signature verification failed")
            }
        }
    }
}

impl std::error::Error for EventError {}

impl UnsignedEvent {
    pub fn new(
        pubkey: impl Into<String>,
        kind: u16,
        tags: Vec<Tag>,
        content: impl Into<String>,
    ) -> Self {
        Self::with_created_at(pubkey, kind, tags, content, now())
    }

    pub fn with_created_at(
        pubkey: impl Into<String>,
        kind: u16,
        tags: Vec<Tag>,
        content: impl Into<String>,
        created_at: u64,
    ) -> Self {
        Self {
            pubkey: pubkey.into(),
            created_at,
            kind,
            tags,
            content: content.into(),
        }
    }

    fn canonical_json(&self) -> String {
        json!([
            0,
            self.pubkey,
            self.created_at,
            self.kind,
            self.tags,
            self.content
        ])
        .to_string()
    }

    pub fn id_bytes(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.canonical_json().as_bytes());
        hasher.finalize().into()
    }

    pub fn id_hex(&self) -> String {
        hex::encode(self.id_bytes())
    }

    /// Sign with `identity`. Panics if `identity`'s Nostr pubkey doesn't
    /// match this event's `pubkey` field — signing someone else's event is
    /// always a caller bug, not a runtime condition to recover from.
    pub fn sign(&self, identity: &Identity) -> Event {
        assert_eq!(
            self.pubkey,
            identity.nostr_pubkey_hex(),
            "signing identity's pubkey does not match event.pubkey"
        );
        let id = self.id_bytes();
        let sig = identity.sign_schnorr(&id);
        Event {
            id: hex::encode(id),
            pubkey: self.pubkey.clone(),
            created_at: self.created_at,
            kind: self.kind,
            tags: self.tags.clone(),
            content: self.content.clone(),
            sig: hex::encode(sig.to_byte_array()),
        }
    }
}

impl Event {
    fn as_unsigned(&self) -> UnsignedEvent {
        UnsignedEvent {
            pubkey: self.pubkey.clone(),
            created_at: self.created_at,
            kind: self.kind,
            tags: self.tags.clone(),
            content: self.content.clone(),
        }
    }

    /// Recompute the id from `self`'s fields and verify the signature
    /// against `self.pubkey`. This is the check every relay/client should
    /// run before trusting a received event.
    pub fn verify(&self) -> Result<(), EventError> {
        if self.as_unsigned().id_hex() != self.id {
            return Err(EventError::IdMismatch);
        }

        let pubkey_bytes: [u8; 32] = hex::decode(&self.pubkey)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or(EventError::InvalidPubkey)?;
        let xonly =
            XOnlyPublicKey::from_byte_array(pubkey_bytes).map_err(|_| EventError::InvalidPubkey)?;

        let sig_bytes: [u8; 64] = hex::decode(&self.sig)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or(EventError::InvalidSignature)?;
        let sig = schnorr::Signature::from_byte_array(sig_bytes);

        let id_bytes: [u8; 32] = hex::decode(&self.id)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or(EventError::IdMismatch)?;

        sig.verify(&id_bytes, &xonly)
            .map_err(|_| EventError::SignatureVerificationFailed)
    }

    pub fn tag_values<'a>(&'a self, key: &str) -> impl Iterator<Item = &'a str> + 'a {
        let key = key.to_string();
        self.tags
            .iter()
            .filter(move |t| t.first().map(|k| *k == key).unwrap_or(false))
            .filter_map(|t| t.get(1).map(String::as_str))
    }

    pub fn first_tag_value(&self, key: &str) -> Option<&str> {
        self.tag_values(key).next()
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before 1970")
        .as_secs()
}

/// Tag a record with the other party's pubkey — the basis of dual indexing
/// (`crate::dual_index`): "all records referencing pubkey X" is then a
/// plain `["p", X]` tag filter.
pub fn p_tag(pubkey_hex: impl Into<String>) -> Tag {
    vec!["p".to_string(), pubkey_hex.into()]
}

/// Reference another event by id (e.g. accept -> offer, milestone -> offer).
pub fn e_tag(event_id_hex: impl Into<String>) -> Tag {
    vec!["e".to_string(), event_id_hex.into()]
}

/// Reference another event by id with a NIP-10-style marker; used for the
/// dual-index cross-reference (`marker = "dual-index"`) so a client can
/// distinguish "my sibling record under the other party's pubkey" from an
/// ordinary lifecycle reference.
pub fn e_tag_marked(event_id_hex: impl Into<String>, marker: &str) -> Tag {
    vec![
        "e".to_string(),
        event_id_hex.into(),
        String::new(),
        marker.to_string(),
    ]
}

/// Skill tag for referral-query routing (`crate::events::kinds` content
/// still carries the full tag; this is what relays filter on).
pub fn t_tag(skill_tag: impl Into<String>) -> Tag {
    vec!["t".to_string(), skill_tag.into()]
}

/// Parse a taxonomy tag's `(sector, domain)` prefix, e.g.
/// `"it/backend/languages#rust"` -> `("it", "backend")`. Mirrors
/// `/taxonomy.yaml`'s own format rule: `sector/domain[/area]#skill`. Used
/// wherever two skill tags need a coarser same-domain comparison rather
/// than an exact match — greedy referral routing (`qw_node::routing`) and
/// the trust-graph walk's domain filter (`crate::trust`).
pub fn tag_domain(tag: &str) -> (&str, &str) {
    let path = tag.split('#').next().unwrap_or(tag);
    let mut parts = path.split('/');
    let sector = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    (sector, domain)
}

pub fn same_domain(a: &str, b: &str) -> bool {
    tag_domain(a) == tag_domain(b)
}

#[cfg(test)]
mod tag_domain_tests {
    use super::*;

    #[test]
    fn tag_domain_ignores_area_and_skill() {
        assert_eq!(tag_domain("it/backend/languages#rust"), ("it", "backend"));
        assert_eq!(tag_domain("it/frontend#react"), ("it", "frontend"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    #[test]
    fn sign_then_verify_round_trips() {
        let id = Identity::generate();
        let unsigned = UnsignedEvent::with_created_at(
            id.nostr_pubkey_hex(),
            KIND_PROFILE_SKILL_TAGS,
            vec![t_tag("it/backend/languages#rust")],
            "{}",
            1_700_000_000,
        );
        let event = unsigned.sign(&id);
        assert!(event.verify().is_ok());
    }

    #[test]
    fn event_serde_round_trips_with_nip01_field_names() {
        let id = Identity::generate();
        let unsigned = UnsignedEvent::with_created_at(
            id.nostr_pubkey_hex(),
            KIND_PROFILE_SKILL_TAGS,
            vec![t_tag("it/backend/languages#rust")],
            "{}",
            1_700_000_000,
        );
        let event = unsigned.sign(&id);

        let json = serde_json::to_value(&event).unwrap();
        for field in [
            "id",
            "pubkey",
            "created_at",
            "kind",
            "tags",
            "content",
            "sig",
        ] {
            assert!(
                json.get(field).is_some(),
                "missing NIP-01 field '{field}' in serialized Event"
            );
        }

        let decoded: Event = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, event);
        assert!(decoded.verify().is_ok());
    }

    #[test]
    fn tampered_content_fails_verification() {
        let id = Identity::generate();
        let unsigned = UnsignedEvent::with_created_at(
            id.nostr_pubkey_hex(),
            KIND_JOB_OFFER,
            vec![],
            "original",
            1,
        );
        let mut event = unsigned.sign(&id);
        event.content = "tampered".to_string();
        assert!(matches!(event.verify(), Err(EventError::IdMismatch)));
    }

    #[test]
    fn forged_signature_fails_verification() {
        let signer = Identity::generate();
        let attacker = Identity::generate();
        let unsigned = UnsignedEvent::with_created_at(
            signer.nostr_pubkey_hex(),
            KIND_JOB_OFFER,
            vec![],
            "content",
            1,
        );
        let mut event = unsigned.sign(&signer);
        // swap in a signature from a different key over the same id
        let forged = attacker.sign_schnorr(&event.id_bytes_from_hex());
        event.sig = hex::encode(forged.to_byte_array());
        assert!(matches!(
            event.verify(),
            Err(EventError::SignatureVerificationFailed)
        ));
    }

    impl Event {
        fn id_bytes_from_hex(&self) -> [u8; 32] {
            hex::decode(&self.id).unwrap().try_into().unwrap()
        }
    }
}
