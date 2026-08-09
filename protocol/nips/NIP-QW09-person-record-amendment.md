# NIP-QW09: Person record amendment

`draft` — kinds `9080` (recovery policy), `9081` (amendment)

## Abstract

What happens when a signing key is lost or compromised (FAQ, added
2026-08-07): **the account survives the key.** An amendment publishes a
replacement controller key as continuation of the same account and
revokes the prior key from a stated timestamp. Reputation attaches to the
account's history, not the key material, so nothing accumulated is lost.

This is the mechanism `todo-impl.md` §7's "key backup/recovery" bullet
pointed at without a concrete design; it's now concrete. It is **not**
for routine device changes — under the controller/device-key hierarchy
(flagged in `todo-impl.md` §2, not yet built), device keys are meant to
be added/removed beneath a stable controller directly. Amendment is only
for the controller key itself.

## Why a separate `account_id`

`did:key` (§2's identity module) encodes the compressed public key
directly in the identifier — it cannot rotate by construction; a new key
means a new `did:key`. To let an account survive a key change, this NIP
introduces `account_id`: the account's **genesis** controller pubkey,
which never changes and is what "the same account" means going forward.
`did_key()`-style identifiers derived from whichever key is *currently*
active are still useful for the identity layer's own purposes (signing,
event `pubkey`), but continuity/reputation attach to `account_id`, not to
any one key's `did:key`.

## Kind 9080 — Recovery policy

Tags: none (no single counterparty — this is the account holder's own
advance configuration, not a record about anyone else).

```json
{
  "quorum_threshold": 2,
  "trusted_pubkeys": ["<hex pubkey>", "<hex pubkey>", "<hex pubkey>"]
}
```

Published (and republished to change it) by the controller itself, while
still in control. **Not itself quorum-protected** — whoever holds the
current controller key can rewrite this at will, which is inherent to
"quorum size and membership are the account holder's own configuration."
A verifier resolving an amendment needs the policy that was in force
*before* the compromise, which is a data-availability/timing concern for
the verifier, not something this NIP's wire format can enforce alone.

## Kind 9081 — Person record amendment

Tags: `["account", <account_id hex pubkey>]`.

```json
{
  "account_id": "<hex pubkey, the account's genesis controller key>",
  "revoked_pubkey": "<hex pubkey being revoked>",
  "new_controller_pubkey": "<hex pubkey, the replacement>",
  "effective_at": 1700000000,
  "quorum_sigs": [
    { "signer_pubkey": "<hex pubkey>", "sig": "<hex BIP-340 schnorr sig>" },
    { "signer_pubkey": "<hex pubkey>", "sig": "<hex BIP-340 schnorr sig>" }
  ]
}
```

`quorum_sigs` are independent signatures by members of `trusted_pubkeys`
(kind 9080) over `PersonRecordAmendment::payload_hash(account_id,
revoked_pubkey, new_controller_pubkey, effective_at)` — everything about
the amendment except the signatures themselves. The event's own NIP-01
`pubkey`/`sig` only prove who *published* it; consent is carried entirely
by `quorum_sigs`, verified independently of the publisher
(`qw_protocol::recovery::verify_amendment`). That's what makes a stolen
key insufficient on its own: an attacker holding `revoked_pubkey` still
has to convince enough of the account's own trusted contacts to
countersign.

Two properties that matter more than the mechanism itself:

| Property | Why |
|---|---|
| Revocation is **not** retroactive | Signatures from `revoked_pubkey` before `effective_at` stay valid — otherwise one lost phone evaporates every past contract |
| A signature under a revoked key *after* `effective_at` is an **alert**, not a silent rejection | It's the strongest available evidence the key is in hostile hands — surface it |

## Verification and resolution

`qw_protocol::recovery`:

- `verify_amendment(amendment, policy) -> Result<usize, RecoveryError>` —
  counts valid, policy-trusted, deduped-by-signer countersignatures;
  `Ok` iff the count meets `policy.quorum_threshold`.
- `latest_valid_controller(genesis_pubkey_hex, amendments) -> String` —
  walks a **linear** chain of amendments (each must verify and must
  revoke whichever key is currently resolved) to find the current
  controller. Does not resolve **competing** amendments — see the module
  doc comment. Two different amendments both claiming to revoke the same
  key (e.g. an attacker racing the legitimate holder) is a genuine
  dispute, and per §0 ("no global reputation score, ever — only
  locally-computed, per-viewer trust") there is no universal tiebreaker
  to compute here; the FAQ's own answer is social ("the legitimate holder
  can raise a competing amendment"), which resolves the same way every
  other trust question in this design does — per viewer, over time.
