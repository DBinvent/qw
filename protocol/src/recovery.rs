//! Controller key recovery (§2/§7, NIP-QW09): verifying that a quorum of
//! an account's trusted contacts actually countersigned a person-record
//! amendment, resolving the controller key from a chain of amendments at
//! any point in time, and — the device-subkey layer — classifying whether
//! an arbitrary signer was authoritative for an account when it signed.
//!
//! The check NIP-QW09 defines for verifying *any* QW event beyond its own
//! NIP-01 signature: the signer was, at the event's `created_at`, either
//! the controller ([`controller_at`]) **or** a device key the controller
//! had delegated and not yet revoked ([`device_authority`] returning
//! [`DeviceAuthority::Delegated`]). A signature under a revoked device key
//! *after* its `revoked_at` ([`DeviceAuthority::Revoked`]) is the
//! strongest available evidence the key is in hostile hands — surface it,
//! never silently drop it.
//!
//! What this module does *not* do: resolve competing/conflicting
//! amendments (two different amendments both claiming to revoke the same
//! key, e.g. an attacker racing the legitimate holder). Quorum membership
//! is the account holder's own configuration, not protocol-enforced, and
//! the FAQ's own answer to that race ("the legitimate holder can raise a
//! competing amendment") leaves resolution to the same per-viewer trust
//! judgment as everything else in this design (§0: "no global reputation
//! score, ever") — there is no universal tiebreaker to implement here.

use std::collections::HashSet;
use std::fmt;

use crate::events::kinds::{DeviceSubkey, PersonRecordAmendment, RecoveryPolicy, KIND_DEVICE_SUBKEY};
use crate::events::Event;
use crate::identity::verify_hex_schnorr;

#[derive(Debug, PartialEq)]
pub enum RecoveryError {
    InsufficientQuorum { valid: usize, required: u8 },
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecoveryError::InsufficientQuorum { valid, required } => {
                write!(
                    f,
                    "amendment has {valid} valid quorum signature(s), needs {required}"
                )
            }
        }
    }
}

impl std::error::Error for RecoveryError {}

/// Count how many of `amendment.quorum_sigs` are valid BIP-340 Schnorr
/// signatures by a pubkey in `policy.trusted_pubkeys`, over the
/// amendment's own payload hash — deduped by signer, with malformed or
/// untrusted entries simply not counted rather than erroring (a garbage
/// or unauthorized entry from an adversary should fail to reach quorum,
/// not abort verification). `Ok` iff that count meets
/// `policy.quorum_threshold`.
pub fn verify_amendment(
    amendment: &PersonRecordAmendment,
    policy: &RecoveryPolicy,
) -> Result<usize, RecoveryError> {
    let hash = PersonRecordAmendment::payload_hash(
        &amendment.account_id,
        &amendment.revoked_pubkey,
        &amendment.new_controller_pubkey,
        amendment.effective_at,
    );

    let mut counted_signers = HashSet::new();
    let mut valid = 0usize;
    for qs in &amendment.quorum_sigs {
        if !policy
            .trusted_pubkeys
            .iter()
            .any(|p| p == &qs.signer_pubkey)
        {
            continue;
        }
        if !counted_signers.insert(qs.signer_pubkey.clone()) {
            continue;
        }
        if verify_hex_schnorr(&qs.signer_pubkey, &qs.sig, &hash) {
            valid += 1;
        }
    }

    if valid >= policy.quorum_threshold as usize {
        Ok(valid)
    } else {
        Err(RecoveryError::InsufficientQuorum {
            valid,
            required: policy.quorum_threshold,
        })
    }
}

/// The controller pubkey for `genesis_pubkey_hex` as of `at` (unix
/// seconds — normally an event's `created_at`). Walks a linear chain of
/// amendments from the genesis key (the account's `account_id`), applying
/// only those that: took effect at or before `at` (`effective_at <= at`,
/// so revocation is not retroactive — an old event still resolves against
/// the key that was current *then*), verify against their paired policy,
/// and chain from the currently-resolved key. An amendment that doesn't
/// chain is skipped, not an error. Does not resolve competing
/// amendments — see module docs.
pub fn controller_at(
    genesis_pubkey_hex: &str,
    amendments: &[(PersonRecordAmendment, RecoveryPolicy)],
    at: u64,
) -> String {
    let mut current = genesis_pubkey_hex.to_string();
    for (amendment, policy) in amendments {
        if amendment.effective_at > at {
            continue;
        }
        if amendment.revoked_pubkey != current {
            continue;
        }
        if verify_amendment(amendment, policy).is_ok() {
            current = amendment.new_controller_pubkey.clone();
        }
    }
    current
}

/// The current controller — [`controller_at`] with no time bound.
pub fn latest_valid_controller(
    genesis_pubkey_hex: &str,
    amendments: &[(PersonRecordAmendment, RecoveryPolicy)],
) -> String {
    controller_at(genesis_pubkey_hex, amendments, u64::MAX)
}

/// How a signer relates to an account's device-key set at a moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceAuthority {
    /// A device key the controller delegated (`valid_from <= at`) and had
    /// not revoked as of `at`.
    Delegated { label: String },
    /// A device key whose delegation was revoked at or before `at`. A
    /// signature here is an **alert**, not a plain rejection.
    Revoked { label: String, revoked_at: u64 },
    /// Not a delegated device key for this account. (It may still be the
    /// controller — check [`controller_at`] separately.)
    Unknown,
}

/// Classify `device_pubkey_hex` against an account's kind-9082 events as
/// of `at`. `subkey_events` are the raw 9082 events held in the ledger;
/// `amendments` is the account's quorum-verified controller chain. A 9082
/// event counts only if its own NIP-01 signature verifies **and** its
/// publisher was the controller as of *its own* `created_at` — a device
/// subkey is real only if the controller delegated it.
///
/// Later records supersede earlier ones by their own timestamp: the most
/// recent delegation (`valid_from`) and the most recent revocation
/// (`revoked_at`), each `<= at`, are compared — a re-delegation after a
/// revocation restores authority.
pub fn device_authority(
    account_id: &str,
    amendments: &[(PersonRecordAmendment, RecoveryPolicy)],
    subkey_events: &[Event],
    device_pubkey_hex: &str,
    at: u64,
) -> DeviceAuthority {
    // The most recent (timestamp, label) for each, considering only records
    // whose moment is already `<= at`.
    let mut latest_delegation: Option<(u64, String)> = None; // by valid_from
    let mut latest_revocation: Option<(u64, String)> = None; // by revoked_at
    let keep_later = |slot: &mut Option<(u64, String)>, t: u64, label: &str| {
        if slot.as_ref().is_none_or(|(cur, _)| t >= *cur) {
            *slot = Some((t, label.to_string()));
        }
    };

    for e in subkey_events {
        if e.kind != KIND_DEVICE_SUBKEY {
            continue;
        }
        if e.first_tag_value("account") != Some(account_id) {
            continue;
        }
        if e.verify().is_err() {
            continue;
        }
        if e.pubkey != controller_at(account_id, amendments, e.created_at) {
            continue; // not controller-signed when it was published
        }
        let Ok(sk) = serde_json::from_str::<DeviceSubkey>(&e.content) else {
            continue;
        };
        if sk.device_pubkey != device_pubkey_hex {
            continue;
        }
        match sk.revoked_at {
            None if sk.valid_from <= at => {
                keep_later(&mut latest_delegation, sk.valid_from, &sk.label)
            }
            Some(r) if r <= at => keep_later(&mut latest_revocation, r, &sk.label),
            _ => {} // a delegation not yet in force, or a future revocation
        }
    }

    match (latest_delegation, latest_revocation) {
        (Some((vf, label)), Some((rev, _))) if vf >= rev => DeviceAuthority::Delegated { label },
        (_, Some((revoked_at, label))) => DeviceAuthority::Revoked { label, revoked_at },
        (Some((_, label)), None) => DeviceAuthority::Delegated { label },
        (None, None) => DeviceAuthority::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::kinds::QuorumSig;
    use crate::identity::Identity;

    fn sign_quorum(signer: &Identity, hash: &[u8; 32]) -> QuorumSig {
        let sig = signer.sign_schnorr(hash);
        QuorumSig {
            signer_pubkey: signer.nostr_pubkey_hex(),
            sig: hex::encode(sig.to_byte_array()),
        }
    }

    fn amendment_with(
        account: &Identity,
        new_controller: &Identity,
        effective_at: u64,
        quorum_sigs: Vec<QuorumSig>,
    ) -> PersonRecordAmendment {
        PersonRecordAmendment {
            account_id: account.nostr_pubkey_hex(),
            revoked_pubkey: account.nostr_pubkey_hex(),
            new_controller_pubkey: new_controller.nostr_pubkey_hex(),
            effective_at,
            quorum_sigs,
        }
    }

    #[test]
    fn quorum_met_verifies() {
        let account = Identity::generate();
        let new_controller = Identity::generate();
        let (friend_a, friend_b, friend_c) = (
            Identity::generate(),
            Identity::generate(),
            Identity::generate(),
        );
        let policy = RecoveryPolicy {
            quorum_threshold: 2,
            trusted_pubkeys: vec![
                friend_a.nostr_pubkey_hex(),
                friend_b.nostr_pubkey_hex(),
                friend_c.nostr_pubkey_hex(),
            ],
        };

        let hash = PersonRecordAmendment::payload_hash(
            &account.nostr_pubkey_hex(),
            &account.nostr_pubkey_hex(),
            &new_controller.nostr_pubkey_hex(),
            1000,
        );
        let amendment = amendment_with(
            &account,
            &new_controller,
            1000,
            vec![sign_quorum(&friend_a, &hash), sign_quorum(&friend_b, &hash)],
        );

        assert_eq!(verify_amendment(&amendment, &policy), Ok(2));
    }

    #[test]
    fn quorum_not_met_fails() {
        let account = Identity::generate();
        let new_controller = Identity::generate();
        let friend_a = Identity::generate();
        let policy = RecoveryPolicy {
            quorum_threshold: 2,
            trusted_pubkeys: vec![friend_a.nostr_pubkey_hex()],
        };

        let hash = PersonRecordAmendment::payload_hash(
            &account.nostr_pubkey_hex(),
            &account.nostr_pubkey_hex(),
            &new_controller.nostr_pubkey_hex(),
            1000,
        );
        let amendment = amendment_with(
            &account,
            &new_controller,
            1000,
            vec![sign_quorum(&friend_a, &hash)],
        );

        assert_eq!(
            verify_amendment(&amendment, &policy),
            Err(RecoveryError::InsufficientQuorum {
                valid: 1,
                required: 2
            })
        );
    }

    #[test]
    fn untrusted_signer_does_not_count() {
        let account = Identity::generate();
        let new_controller = Identity::generate();
        let (friend_a, stranger) = (Identity::generate(), Identity::generate());
        let policy = RecoveryPolicy {
            quorum_threshold: 2,
            trusted_pubkeys: vec![friend_a.nostr_pubkey_hex()],
        };

        let hash = PersonRecordAmendment::payload_hash(
            &account.nostr_pubkey_hex(),
            &account.nostr_pubkey_hex(),
            &new_controller.nostr_pubkey_hex(),
            1000,
        );
        // stranger's signature is cryptographically valid but they're not in the policy
        let amendment = amendment_with(
            &account,
            &new_controller,
            1000,
            vec![sign_quorum(&friend_a, &hash), sign_quorum(&stranger, &hash)],
        );

        assert_eq!(
            verify_amendment(&amendment, &policy),
            Err(RecoveryError::InsufficientQuorum {
                valid: 1,
                required: 2
            })
        );
    }

    #[test]
    fn duplicate_signer_counted_once() {
        let account = Identity::generate();
        let new_controller = Identity::generate();
        let friend_a = Identity::generate();
        let policy = RecoveryPolicy {
            quorum_threshold: 2,
            trusted_pubkeys: vec![friend_a.nostr_pubkey_hex()],
        };

        let hash = PersonRecordAmendment::payload_hash(
            &account.nostr_pubkey_hex(),
            &account.nostr_pubkey_hex(),
            &new_controller.nostr_pubkey_hex(),
            1000,
        );
        let sig = sign_quorum(&friend_a, &hash);
        let amendment = amendment_with(&account, &new_controller, 1000, vec![sig.clone(), sig]);

        assert_eq!(
            verify_amendment(&amendment, &policy),
            Err(RecoveryError::InsufficientQuorum {
                valid: 1,
                required: 2
            })
        );
    }

    #[test]
    fn forged_signature_does_not_count() {
        let account = Identity::generate();
        let new_controller = Identity::generate();
        let friend_a = Identity::generate();
        let policy = RecoveryPolicy {
            quorum_threshold: 1,
            trusted_pubkeys: vec![friend_a.nostr_pubkey_hex()],
        };

        // signed over the wrong hash (e.g. a different effective_at)
        let wrong_hash = PersonRecordAmendment::payload_hash(
            &account.nostr_pubkey_hex(),
            &account.nostr_pubkey_hex(),
            &new_controller.nostr_pubkey_hex(),
            9999,
        );
        let amendment = amendment_with(
            &account,
            &new_controller,
            1000,
            vec![sign_quorum(&friend_a, &wrong_hash)],
        );

        assert_eq!(
            verify_amendment(&amendment, &policy),
            Err(RecoveryError::InsufficientQuorum {
                valid: 0,
                required: 1
            })
        );
    }

    #[test]
    fn latest_valid_controller_walks_a_chain() {
        let genesis = Identity::generate();
        let controller_b = Identity::generate();
        let controller_c = Identity::generate();
        let friend = Identity::generate();
        let policy = RecoveryPolicy {
            quorum_threshold: 1,
            trusted_pubkeys: vec![friend.nostr_pubkey_hex()],
        };

        let hash1 = PersonRecordAmendment::payload_hash(
            &genesis.nostr_pubkey_hex(),
            &genesis.nostr_pubkey_hex(),
            &controller_b.nostr_pubkey_hex(),
            1000,
        );
        let amendment1 = amendment_with(
            &genesis,
            &controller_b,
            1000,
            vec![sign_quorum(&friend, &hash1)],
        );

        let hash2 = PersonRecordAmendment::payload_hash(
            &genesis.nostr_pubkey_hex(),
            &controller_b.nostr_pubkey_hex(),
            &controller_c.nostr_pubkey_hex(),
            2000,
        );
        let amendment2 = PersonRecordAmendment {
            account_id: genesis.nostr_pubkey_hex(),
            revoked_pubkey: controller_b.nostr_pubkey_hex(),
            new_controller_pubkey: controller_c.nostr_pubkey_hex(),
            effective_at: 2000,
            quorum_sigs: vec![sign_quorum(&friend, &hash2)],
        };

        let result = latest_valid_controller(
            &genesis.nostr_pubkey_hex(),
            &[(amendment1, policy.clone()), (amendment2, policy)],
        );
        assert_eq!(result, controller_c.nostr_pubkey_hex());
    }

    #[test]
    fn non_chaining_amendment_is_skipped() {
        let genesis = Identity::generate();
        let attacker_target = Identity::generate();
        let friend = Identity::generate();
        let policy = RecoveryPolicy {
            quorum_threshold: 1,
            trusted_pubkeys: vec![friend.nostr_pubkey_hex()],
        };

        // revokes a key that was never the current controller
        let some_other_key = Identity::generate();
        let hash = PersonRecordAmendment::payload_hash(
            &genesis.nostr_pubkey_hex(),
            &some_other_key.nostr_pubkey_hex(),
            &attacker_target.nostr_pubkey_hex(),
            1000,
        );
        let bogus = PersonRecordAmendment {
            account_id: genesis.nostr_pubkey_hex(),
            revoked_pubkey: some_other_key.nostr_pubkey_hex(),
            new_controller_pubkey: attacker_target.nostr_pubkey_hex(),
            effective_at: 1000,
            quorum_sigs: vec![sign_quorum(&friend, &hash)],
        };

        let result = latest_valid_controller(&genesis.nostr_pubkey_hex(), &[(bogus, policy)]);
        assert_eq!(
            result,
            genesis.nostr_pubkey_hex(),
            "amendment revoking a non-current key must not change resolution"
        );
    }

    // --- device subkeys (NIP-QW09 §"Device subkeys") ------------------

    use crate::events::kinds::{device_subkey, DeviceSubkey};
    use crate::events::Event;

    fn subkey_event(
        controller: &Identity,
        account_id: &str,
        device_pubkey: &str,
        label: &str,
        valid_from: u64,
        revoked_at: Option<u64>,
        created_at: u64,
    ) -> Event {
        let mut unsigned = device_subkey(
            &controller.nostr_pubkey_hex(),
            account_id,
            &DeviceSubkey {
                device_pubkey: device_pubkey.to_string(),
                label: label.to_string(),
                valid_from,
                revoked_at,
            },
        );
        unsigned.created_at = created_at;
        unsigned.sign(controller)
    }

    #[test]
    fn controller_at_is_time_bounded() {
        let genesis = Identity::generate();
        let controller_b = Identity::generate();
        let friend = Identity::generate();
        let policy = RecoveryPolicy {
            quorum_threshold: 1,
            trusted_pubkeys: vec![friend.nostr_pubkey_hex()],
        };
        let hash = PersonRecordAmendment::payload_hash(
            &genesis.nostr_pubkey_hex(),
            &genesis.nostr_pubkey_hex(),
            &controller_b.nostr_pubkey_hex(),
            5_000,
        );
        let amendment = amendment_with(&genesis, &controller_b, 5_000, vec![sign_quorum(&friend, &hash)]);
        let chain = [(amendment, policy)];

        // before effective_at the old key is still the controller
        assert_eq!(
            controller_at(&genesis.nostr_pubkey_hex(), &chain, 4_999),
            genesis.nostr_pubkey_hex()
        );
        assert_eq!(
            controller_at(&genesis.nostr_pubkey_hex(), &chain, 5_000),
            controller_b.nostr_pubkey_hex()
        );
        // and latest_valid_controller is the no-bound case
        assert_eq!(
            latest_valid_controller(&genesis.nostr_pubkey_hex(), &chain),
            controller_b.nostr_pubkey_hex()
        );
    }

    #[test]
    fn a_delegated_device_is_authorized_and_revocation_is_not_retroactive() {
        let controller = Identity::generate();
        let device = Identity::generate();
        let account = controller.nostr_pubkey_hex();
        let events = vec![
            subkey_event(&controller, &account, &device.nostr_pubkey_hex(), "phone", 100, None, 100),
            subkey_event(&controller, &account, &device.nostr_pubkey_hex(), "phone", 100, Some(500), 500),
        ];

        // in force between valid_from and revoked_at
        assert_eq!(
            device_authority(&account, &[], &events, &device.nostr_pubkey_hex(), 300),
            DeviceAuthority::Delegated { label: "phone".into() }
        );
        // a signature *before* revocation stays valid (not retroactive)
        assert_eq!(
            device_authority(&account, &[], &events, &device.nostr_pubkey_hex(), 499),
            DeviceAuthority::Delegated { label: "phone".into() }
        );
        // at or after revoked_at it is an alert, not a plain rejection
        assert_eq!(
            device_authority(&account, &[], &events, &device.nostr_pubkey_hex(), 500),
            DeviceAuthority::Revoked { label: "phone".into(), revoked_at: 500 }
        );
        // before valid_from: nothing yet
        assert_eq!(
            device_authority(&account, &[], &events, &device.nostr_pubkey_hex(), 99),
            DeviceAuthority::Unknown
        );
        // a stranger key is unknown
        assert_eq!(
            device_authority(&account, &[], &events, &Identity::generate().nostr_pubkey_hex(), 300),
            DeviceAuthority::Unknown
        );
    }

    #[test]
    fn a_9082_not_signed_by_the_controller_is_ignored() {
        let controller = Identity::generate();
        let impostor = Identity::generate();
        let device = Identity::generate();
        let account = controller.nostr_pubkey_hex();

        // impostor tries to delegate a device key for someone else's account
        let forged = subkey_event(&impostor, &account, &device.nostr_pubkey_hex(), "mine now", 1, None, 1);
        assert_eq!(
            device_authority(&account, &[], &[forged], &device.nostr_pubkey_hex(), 100),
            DeviceAuthority::Unknown,
            "only the resolved controller can delegate"
        );
    }

    #[test]
    fn a_re_delegation_after_revocation_restores_authority() {
        let controller = Identity::generate();
        let device = Identity::generate();
        let account = controller.nostr_pubkey_hex();
        let dp = device.nostr_pubkey_hex();
        let events = vec![
            subkey_event(&controller, &account, &dp, "box", 100, None, 100),
            subkey_event(&controller, &account, &dp, "box", 100, Some(500), 500),
            subkey_event(&controller, &account, &dp, "box", 800, None, 800), // re-delegated
        ];
        assert_eq!(
            device_authority(&account, &[], &events, &dp, 600),
            DeviceAuthority::Revoked { label: "box".into(), revoked_at: 500 }
        );
        assert_eq!(
            device_authority(&account, &[], &events, &dp, 900),
            DeviceAuthority::Delegated { label: "box".into() },
            "the later delegation wins"
        );
    }

    #[test]
    fn device_authority_resolves_the_controller_as_of_each_9082() {
        // The controller rotated at t=5000; a device delegated at t=6000
        // must be signed by the *new* controller, not the genesis key.
        let genesis = Identity::generate();
        let controller_b = Identity::generate();
        let friend = Identity::generate();
        let device = Identity::generate();
        let account = genesis.nostr_pubkey_hex();
        let policy = RecoveryPolicy {
            quorum_threshold: 1,
            trusted_pubkeys: vec![friend.nostr_pubkey_hex()],
        };
        let hash = PersonRecordAmendment::payload_hash(&account, &account, &controller_b.nostr_pubkey_hex(), 5_000);
        let chain = [(amendment_with(&genesis, &controller_b, 5_000, vec![sign_quorum(&friend, &hash)]), policy)];

        let by_new = subkey_event(&controller_b, &account, &device.nostr_pubkey_hex(), "d", 6_000, None, 6_000);
        let by_old = subkey_event(&genesis, &account, &device.nostr_pubkey_hex(), "d", 6_000, None, 6_000);

        assert_eq!(
            device_authority(&account, &chain, &[by_new], &device.nostr_pubkey_hex(), 7_000),
            DeviceAuthority::Delegated { label: "d".into() }
        );
        assert_eq!(
            device_authority(&account, &chain, &[by_old], &device.nostr_pubkey_hex(), 7_000),
            DeviceAuthority::Unknown,
            "the genesis key was no longer the controller at t=6000"
        );
    }
}
