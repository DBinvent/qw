//! External signer protocol (§7, FAQ §2 "Amber pattern"): a thin client
//! that must not hold the identity key — browser storage is XSS-exposed,
//! and Safari evicts data after roughly a week of non-use, meaning
//! silent permanent identity loss — composes an unsigned event and hands
//! it to an external signer (a phone holding the real key) via a QR code
//! or deep link: "web app composes and displays; phone or node signs via
//! QR/deep link" (FAQ).
//!
//! This module defines the request/response **wire format only**. The
//! actual transport — rendering a QR code, firing an Android intent,
//! handling an iOS universal link — is platform/UI code this crate
//! doesn't own; a `qw-signer:` URI is what both transports carry; a QR
//! code is just that URI rendered as an image, an intent is just that
//! URI dispatched by the OS.
//!
//! Inspired by the Amber/NIP-46 pattern, not wire-compatible with either
//! — this is QW's own minimal protocol, not an interop target.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};

use crate::events::{Event, UnsignedEvent};
use crate::identity::Identity;

pub const SCHEME: &str = "qw-signer";

#[derive(Debug, PartialEq)]
pub enum SignerProtocolError {
    Malformed(&'static str),
    RequestIdMismatch,
    VerificationFailed,
}

impl std::fmt::Display for SignerProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignerProtocolError::Malformed(why) => write!(f, "malformed qw-signer URI: {why}"),
            SignerProtocolError::RequestIdMismatch => {
                write!(f, "response request_id does not match the request")
            }
            SignerProtocolError::VerificationFailed => {
                write!(f, "assembled event failed verification")
            }
        }
    }
}

impl std::error::Error for SignerProtocolError {}

/// What the thin client sends to the signer: an unsigned event plus a
/// `request_id` to correlate the eventual response (a signer may be
/// mid-flight on more than one request) and an optional callback URI for
/// deep-link transports (`None` when the response instead comes back via
/// a second QR code the thin client scans, or manual paste).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignRequest {
    pub request_id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub kind: u16,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback: Option<String>,
}

impl SignRequest {
    pub fn new(
        request_id: impl Into<String>,
        unsigned: &UnsignedEvent,
        callback: Option<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            pubkey: unsigned.pubkey.clone(),
            created_at: unsigned.created_at,
            kind: unsigned.kind,
            tags: unsigned.tags.clone(),
            content: unsigned.content.clone(),
            callback,
        }
    }

    pub fn unsigned_event(&self) -> UnsignedEvent {
        UnsignedEvent::with_created_at(
            self.pubkey.clone(),
            self.kind,
            self.tags.clone(),
            self.content.clone(),
            self.created_at,
        )
    }

    /// Encode as a `qw-signer:sign?payload=<base64url json>` URI — the
    /// one string that serves as both a QR-code payload and a deep link.
    pub fn to_uri(&self) -> String {
        let json = serde_json::to_string(self).expect("SignRequest serializes");
        format!("{SCHEME}:sign?payload={}", URL_SAFE_NO_PAD.encode(json))
    }

    pub fn from_uri(uri: &str) -> Result<Self, SignerProtocolError> {
        let payload = uri
            .strip_prefix(&format!("{SCHEME}:sign?payload="))
            .ok_or(SignerProtocolError::Malformed("not a qw-signer sign URI"))?;
        let json = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| SignerProtocolError::Malformed("payload is not valid base64url"))?;
        serde_json::from_slice(&json)
            .map_err(|_| SignerProtocolError::Malformed("payload is not a valid SignRequest"))
    }
}

/// What the signer sends back: just the signature. The thin client
/// reconstructs everything else from the request it already composed —
/// nothing here needs the signer to echo back fields the client can't
/// already verify independently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignResponse {
    pub request_id: String,
    pub sig: String,
}

impl SignResponse {
    pub fn to_uri(&self) -> String {
        let json = serde_json::to_string(self).expect("SignResponse serializes");
        format!("{SCHEME}:signed?payload={}", URL_SAFE_NO_PAD.encode(json))
    }

    pub fn from_uri(uri: &str) -> Result<Self, SignerProtocolError> {
        let payload = uri
            .strip_prefix(&format!("{SCHEME}:signed?payload="))
            .ok_or(SignerProtocolError::Malformed("not a qw-signer signed URI"))?;
        let json = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| SignerProtocolError::Malformed("payload is not valid base64url"))?;
        serde_json::from_slice(&json)
            .map_err(|_| SignerProtocolError::Malformed("payload is not a valid SignResponse"))
    }
}

/// Signer-side: sign the request's event and produce the response to
/// send back. Panics (via `UnsignedEvent::sign`'s own guard) rather than
/// silently signing under the wrong identity if `identity` doesn't hold
/// `request.pubkey`'s key — the signer app is expected to check that
/// before ever getting here (e.g. "no matching key" in its own UI), not
/// rely on this function to fail gracefully.
pub fn sign(request: &SignRequest, identity: &Identity) -> SignResponse {
    let event = request.unsigned_event().sign(identity);
    SignResponse {
        request_id: request.request_id.clone(),
        sig: event.sig,
    }
}

/// Requester-side: combine the original request with the signer's
/// response into a finished, self-verifying `Event`. Never trusts the
/// signer blindly — the assembled event still has to pass `Event::verify`
/// (id recomputed from the request's own fields, signature checked
/// against `request.pubkey`), so a broken or malicious signer returning
/// garbage is caught here, not silently accepted. Note what this
/// function's signature itself proves: it takes no `Identity` and no key
/// material at all — the requester genuinely cannot need the private key
/// to complete this exchange.
pub fn assemble_response(
    request: &SignRequest,
    response: &SignResponse,
) -> Result<Event, SignerProtocolError> {
    if request.request_id != response.request_id {
        return Err(SignerProtocolError::RequestIdMismatch);
    }
    let unsigned = request.unsigned_event();
    let event = Event {
        id: unsigned.id_hex(),
        pubkey: unsigned.pubkey,
        created_at: unsigned.created_at,
        kind: unsigned.kind,
        tags: unsigned.tags,
        content: unsigned.content,
        sig: response.sig.clone(),
    };
    event
        .verify()
        .map_err(|_| SignerProtocolError::VerificationFailed)?;
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::kinds::{profile_skill_tags, ProfileSkillTags};
    use crate::identity::Identity;

    #[test]
    fn request_round_trips_through_its_uri_encoding() {
        let phone = Identity::generate();
        let unsigned = profile_skill_tags(
            &phone.nostr_pubkey_hex(),
            &ProfileSkillTags {
                display_name: None,
                skill_tags: vec!["it/backend/languages#rust".to_string()],
            },
        );
        let request = SignRequest::new("req-1", &unsigned, Some("qw-app://signed".to_string()));

        let uri = request.to_uri();
        assert!(uri.starts_with("qw-signer:sign?payload="));
        let decoded = SignRequest::from_uri(&uri).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn full_exchange_produces_a_verifying_event() {
        let phone = Identity::generate();
        let unsigned = profile_skill_tags(
            &phone.nostr_pubkey_hex(),
            &ProfileSkillTags {
                display_name: Some("vk".to_string()),
                skill_tags: vec![],
            },
        );
        let request = SignRequest::new("req-1", &unsigned, None);

        // requester composes, encodes, hands off (QR/deep link) —
        // nothing here ever touches `phone`'s key
        let request_uri = request.to_uri();

        // signer side: decode, sign, encode response
        let received_request = SignRequest::from_uri(&request_uri).unwrap();
        let response = sign(&received_request, &phone);
        let response_uri = response.to_uri();

        // requester side: decode the response, assemble+verify
        let received_response = SignResponse::from_uri(&response_uri).unwrap();
        let event = assemble_response(&request, &received_response).unwrap();
        assert!(event.verify().is_ok());
        assert_eq!(event.pubkey, phone.nostr_pubkey_hex());
    }

    #[test]
    fn mismatched_request_id_is_rejected() {
        let phone = Identity::generate();
        let unsigned = profile_skill_tags(
            &phone.nostr_pubkey_hex(),
            &ProfileSkillTags {
                display_name: None,
                skill_tags: vec![],
            },
        );
        let request = SignRequest::new("req-1", &unsigned, None);
        let response = sign(&request, &phone);

        let mismatched = SignResponse {
            request_id: "some-other-request".to_string(),
            ..response
        };
        assert_eq!(
            assemble_response(&request, &mismatched),
            Err(SignerProtocolError::RequestIdMismatch)
        );
    }

    #[test]
    fn forged_signature_fails_assembly() {
        let phone = Identity::generate();
        let attacker = Identity::generate();
        let unsigned = profile_skill_tags(
            &phone.nostr_pubkey_hex(),
            &ProfileSkillTags {
                display_name: None,
                skill_tags: vec![],
            },
        );
        let request = SignRequest::new("req-1", &unsigned, None);

        // a compromised/malicious signer returns a signature that isn't
        // actually valid for phone's pubkey over this event
        let bogus_sig = attacker.sign_schnorr(&unsigned.id_bytes());
        let response = SignResponse {
            request_id: "req-1".to_string(),
            sig: hex::encode(bogus_sig.to_byte_array()),
        };

        assert_eq!(
            assemble_response(&request, &response),
            Err(SignerProtocolError::VerificationFailed)
        );
    }

    #[test]
    fn from_uri_rejects_garbage() {
        assert!(SignRequest::from_uri("not-a-uri-at-all").is_err());
        assert!(SignRequest::from_uri("qw-signer:sign?payload=not-valid-base64!!!").is_err());
        assert!(
            SignResponse::from_uri("qw-signer:sign?payload=abc").is_err(),
            "wrong action ('sign' vs 'signed') must be rejected"
        );
    }
}
