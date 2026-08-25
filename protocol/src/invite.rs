//! Public invite links (NIP-QW07 "Public self-introduction") — the
//! network's front door, and the client half of it.
//!
//! A participant publishes `https://knownby.work/i/<npub>` wherever their
//! professional history already lives — a LinkedIn post, a talk slide, an
//! email signature. Anyone who follows it exchanges introductions with the
//! publisher and lands as a **hop-1 contact**, whether they were four hops
//! out in the contact graph or not in it at all. There is no invitation to
//! wait for and no cohort to be admitted to: §10 dropped invite-only
//! entirely.
//!
//! What the resulting edge does and does not mean is the whole design:
//!
//! - It makes the newcomer **reachable** — referral queries can route to
//!   them (NIP-QW06), offers can arrive.
//! - It carries **no vouch and no reputation**. Nobody who posts a link
//!   knows who will click it. Trust is computed only from completed,
//!   countersigned work (`crate::trust` walks `CreditIssuance` edges and
//!   nothing else), so a stranger at hop 1 with no contracts scores
//!   exactly what a stranger at hop 4 with no contracts scores: nothing.
//!   Collapsing distance changes who can *reach* you, never who is
//!   *trusted*.
//! - Both events carry [`VIA_PUBLIC_LINK`], which
//!   `crate::cascade::introduction_adjacency` skips — otherwise publishing
//!   an ad would make every reader who clicked it distance-1 from the
//!   publisher, and two flags against any one of them would cascade onto
//!   the publisher.

use bech32::{Bech32, Hrp};

use crate::events::kinds::{introduction, Introduction};
use crate::events::UnsignedEvent;

/// NIP-19 human-readable part for a public key.
const NPUB_HRP: &str = "npub";

/// Path prefix the landing site serves invite links under.
pub const INVITE_PATH_PREFIX: &str = "/i/";

#[derive(Debug, PartialEq, Eq)]
pub enum InviteError {
    /// Not 32 bytes of hex, and not a `npub1…` this crate can decode.
    NotAPubkey,
    /// Decoded, but the human-readable part was something other than
    /// `npub` — an `nsec1…` most importantly, which is a *secret* key and
    /// must never be treated as an invite target.
    WrongPrefix(String),
}

impl std::fmt::Display for InviteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InviteError::NotAPubkey => write!(f, "not a 32-byte hex pubkey or npub"),
            InviteError::WrongPrefix(hrp) => {
                write!(f, "expected an npub, got a '{hrp}'")
            }
        }
    }
}

impl std::error::Error for InviteError {}

/// Encode a 32-byte hex x-only pubkey as NIP-19 `npub1…`.
pub fn npub_encode(pubkey_hex: &str) -> Result<String, InviteError> {
    let bytes = hex::decode(pubkey_hex).map_err(|_| InviteError::NotAPubkey)?;
    if bytes.len() != 32 {
        return Err(InviteError::NotAPubkey);
    }
    let hrp = Hrp::parse(NPUB_HRP).expect("npub is a valid hrp");
    bech32::encode::<Bech32>(hrp, &bytes).map_err(|_| InviteError::NotAPubkey)
}

/// Decode a NIP-19 `npub1…` back to a 32-byte hex pubkey.
pub fn npub_decode(npub: &str) -> Result<String, InviteError> {
    let (hrp, bytes) = bech32::decode(npub).map_err(|_| InviteError::NotAPubkey)?;
    if hrp.as_str() != NPUB_HRP {
        return Err(InviteError::WrongPrefix(hrp.as_str().to_string()));
    }
    if bytes.len() != 32 {
        return Err(InviteError::NotAPubkey);
    }
    Ok(hex::encode(bytes))
}

/// Accept either form a link might carry — `npub1…` or bare hex — and
/// return the hex pubkey. Bare hex is accepted because it is what every
/// other API in this crate speaks; npub is what a human copies out of a
/// client.
pub fn parse_invite_target(s: &str) -> Result<String, InviteError> {
    let s = s.trim();
    if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(s.to_ascii_lowercase());
    }
    npub_decode(s)
}

/// Extract the target from a link path such as `/i/npub1…` (query string
/// and trailing slash tolerated). Returns `None` for any other path, so a
/// caller can fall through to ordinary asset serving.
pub fn parse_invite_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix(INVITE_PATH_PREFIX)?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let rest = rest.trim_end_matches('/');
    parse_invite_target(rest).ok()
}

/// The shareable link for a pubkey. `base` is the site origin, e.g.
/// `https://knownby.work`.
pub fn invite_url(base: &str, pubkey_hex: &str) -> Result<String, InviteError> {
    let npub = npub_encode(pubkey_hex)?;
    Ok(format!(
        "{}{INVITE_PATH_PREFIX}{npub}",
        base.trim_end_matches('/')
    ))
}

/// Step 1 of following a link: the newcomer's own self-introduction to
/// the publisher, marked as link-minted. Caller signs it with the
/// follower's identity.
pub fn follow_public_link(follower_pubkey_hex: &str, publisher_pubkey_hex: &str) -> UnsignedEvent {
    public_link_introduction(follower_pubkey_hex, publisher_pubkey_hex)
}

/// Step 2: the publisher's automatic answer, which makes the edge mutual.
/// Automatic *because the publisher chose to publish the link* — that is
/// the standing consent the ad represents, not a per-person decision. A
/// client that would rather confirm each one simply does not call this
/// until a human says so; nothing downstream can tell the difference.
pub fn answer_public_link(publisher_pubkey_hex: &str, follower_pubkey_hex: &str) -> UnsignedEvent {
    public_link_introduction(publisher_pubkey_hex, follower_pubkey_hex)
}

/// Both directions are the same shape: a self-introduction (the signer is
/// its own subject) carrying `via: "public-link"`.
fn public_link_introduction(signer_pubkey_hex: &str, recipient_pubkey_hex: &str) -> UnsignedEvent {
    introduction(
        signer_pubkey_hex,
        recipient_pubkey_hex,
        &Introduction::public_link(signer_pubkey_hex),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::kinds::KIND_INTRODUCTION;
    use crate::identity::Identity;

    #[test]
    fn npub_round_trips() {
        let id = Identity::generate();
        let hex = id.nostr_pubkey_hex();
        let npub = npub_encode(&hex).unwrap();
        assert!(npub.starts_with("npub1"));
        assert_eq!(npub_decode(&npub).unwrap(), hex);
    }

    #[test]
    fn npub_matches_nip19_test_vector() {
        // NIP-19's own example pair — proves this is the real encoding and
        // not merely self-consistent with `npub_decode`.
        let hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let npub = "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6";
        assert_eq!(npub_encode(hex).unwrap(), npub);
        assert_eq!(npub_decode(npub).unwrap(), hex);
    }

    #[test]
    fn an_nsec_is_never_accepted_as_a_target() {
        // A secret key is bech32 too, and a careless parser would happily
        // "decode" one. Following a link built from someone's nsec must
        // fail loudly rather than quietly introduce them to a key they
        // just leaked.
        let hrp = Hrp::parse("nsec").unwrap();
        let nsec = bech32::encode::<Bech32>(hrp, &[7u8; 32]).unwrap();
        assert_eq!(
            parse_invite_target(&nsec),
            Err(InviteError::WrongPrefix("nsec".to_string()))
        );
    }

    #[test]
    fn parses_both_link_forms_and_rejects_others() {
        let id = Identity::generate();
        let hex = id.nostr_pubkey_hex();
        let npub = npub_encode(&hex).unwrap();

        assert_eq!(parse_invite_path(&format!("/i/{npub}")).unwrap(), hex);
        assert_eq!(parse_invite_path(&format!("/i/{hex}")).unwrap(), hex);
        assert_eq!(parse_invite_path(&format!("/i/{npub}/")).unwrap(), hex);
        assert_eq!(
            parse_invite_path(&format!("/i/{npub}?utm=li")).unwrap(),
            hex
        );

        assert!(parse_invite_path("/i/").is_none());
        assert!(parse_invite_path("/i/not-a-key").is_none());
        assert!(parse_invite_path("/about").is_none());
        // A near-miss that would be a nasty silent failure: 63 hex chars.
        assert!(parse_invite_path(&format!("/i/{}", &hex[..63])).is_none());
    }

    #[test]
    fn invite_url_is_the_published_form() {
        let hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        assert_eq!(
            invite_url("https://knownby.work/", hex).unwrap(),
            "https://knownby.work/i/npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6"
        );
    }

    #[test]
    fn following_a_link_produces_a_mutual_marked_pair() {
        let follower = Identity::generate();
        let publisher = Identity::generate();

        let a = follow_public_link(&follower.nostr_pubkey_hex(), &publisher.nostr_pubkey_hex())
            .sign(&follower);
        let b = answer_public_link(&publisher.nostr_pubkey_hex(), &follower.nostr_pubkey_hex())
            .sign(&publisher);

        for (event, signer, recipient) in [(&a, &follower, &publisher), (&b, &publisher, &follower)]
        {
            assert!(event.verify().is_ok());
            assert_eq!(event.kind, KIND_INTRODUCTION);
            assert_eq!(
                event.first_tag_value("p"),
                Some(recipient.nostr_pubkey_hex().as_str())
            );
            let intro: Introduction = serde_json::from_str(&event.content).unwrap();
            // Self-introduction in both directions: neither party is
            // vouching for the other, they are each presenting themselves.
            assert_eq!(intro.subject_pubkey, signer.nostr_pubkey_hex());
            assert!(intro.chain.is_empty());
            assert!(intro.is_public_link());
        }
    }

    #[test]
    fn a_link_edge_grants_no_trust() {
        // The claim the README and NIP make out loud, pinned as a test:
        // trust walks CreditIssuance edges, so a link-minted contact is
        // worth nothing until work is completed and countersigned.
        let follower = Identity::generate();
        let publisher = Identity::generate();
        let events = vec![
            follow_public_link(&follower.nostr_pubkey_hex(), &publisher.nostr_pubkey_hex())
                .sign(&follower),
            answer_public_link(&publisher.nostr_pubkey_hex(), &follower.nostr_pubkey_hex())
                .sign(&publisher),
        ];

        assert!(crate::trust::find_trust_path(
            &events,
            &publisher.nostr_pubkey_hex(),
            &follower.nostr_pubkey_hex(),
            4,
            None,
        )
        .is_none());
    }
}
