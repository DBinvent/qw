//! Controller key recovery (§2/§7, NIP-QW09): verifying that a quorum of
//! an account's trusted contacts actually countersigned a person-record
//! amendment, and resolving the current controller key from a chain of
//! amendments.
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

use crate::events::kinds::{PersonRecordAmendment, RecoveryPolicy};
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

/// Resolve the current controller pubkey by walking a linear chain of
/// amendments from `genesis_pubkey_hex` (the account's original
/// controller key — its `account_id`), applying only those that verify
/// against their paired policy *and* chain from the currently-resolved
/// key (`amendment.revoked_pubkey == current`). Amendments are consumed
/// in the given order; one that doesn't chain from the current key is
/// skipped, not treated as an error. Does not resolve competing
/// amendments — see module docs.
pub fn latest_valid_controller(
    genesis_pubkey_hex: &str,
    amendments: &[(PersonRecordAmendment, RecoveryPolicy)],
) -> String {
    let mut current = genesis_pubkey_hex.to_string();
    for (amendment, policy) in amendments {
        if amendment.revoked_pubkey != current {
            continue;
        }
        if verify_amendment(amendment, policy).is_ok() {
            current = amendment.new_controller_pubkey.clone();
        }
    }
    current
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
}
