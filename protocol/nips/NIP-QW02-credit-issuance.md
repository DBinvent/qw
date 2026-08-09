# NIP-QW02: Credit issuance

`draft` — kind `9010`

## Abstract

The one step in the contract lifecycle requiring atomic dual-sign
(`qw-design-faq.md` §"How does the contract lifecycle get signed?"):
issuer (counterparty/payer) and subject (worker/payee) both sign the same
`payload_hash`, and either party may publish once both signatures exist.
The published event carries both signatures inline, so a verifier trusts
the *signatures*, not whichever party happened to publish
(`todo-impl.md` §4: "two-phase exchange... either party can publish once
both signatures are collected").

`issuer`/`subject` are the same roles as the VC schema (NIP-QW02's event
and `qw_protocol::vc`'s credential are two views of the same fact — the
event is the on-relay record; the VC is what's handed to a third party for
selective-disclosure verification).

## Kind 9010 — Credit issuance

Tags: `["p", <subject/worker pubkey>]`, `["e", <completion event id>]`.

```json
{
  "completion_event_id": "<hex event id of the kind 9003 this issuance settles>",
  "payload_hash": "<hex sha256 of the agreed terms, signed by both parties>",
  "amount": { "unit": "bucket", "index": 5 },
  "issuer_sig": "<hex BIP-340 schnorr sig, issuer over payload_hash>",
  "subject_sig": "<hex BIP-340 schnorr sig, subject over payload_hash>"
}
```

### `amount`

Q4 default (`todo-impl.md` §0): **ranged/bucketed**, full value opt-in.

```json
{ "unit": "bucket", "index": 5 }
```
or, only if the participant opts in to disclosing the exact figure:
```json
{ "unit": "exact", "quants": 320.0 }
```

Bucket index is a log-scale bucket over Quants; the exact bucket-edge table
is not yet fixed (open item — revisit alongside Q4 in `todo-impl.md` when
reputation-market data exists to size the buckets).

### `payload_hash`

`CreditIssuance::payload_hash(completion_event_id, amount) -> [u8; 32]` —
sha256 over `[completion_event_id, amount]`, i.e. everything about the
issuance except the two signatures. Both `issuer_sig` and `subject_sig`
must be valid BIP-340 signatures over this same hash.

### Verification

A verifier who was not a party to the exchange can still confirm consent
without trusting the publisher (`qw_protocol::contract::verify_credit_issuance`):

1. Recompute `payload_hash` from `completion_event_id` and `amount`.
2. Verify `issuer_sig` is a valid BIP-340 signature by the issuer's Nostr
   pubkey over `payload_hash`.
3. Verify `subject_sig` likewise for the subject's pubkey.
4. Verify the event's own `sig` (NIP-01) — proves whoever published it had
   the right to (in practice, either the issuer or subject, but the event
   signer need not equal either embedded signer for the two-phase exchange
   to be honored, since the embedded signatures are what carry consent).

The two-phase exchange itself — each party independently producing their
signature over `payload_hash`, and either publishing once both exist — is
`qw_protocol::contract`'s `sign_credit_issuance_payload` /
`assemble_credit_issuance` (§4). This NIP only fixes the wire shape of the
resulting event; how the two signatures actually reach each other (a
direct message, a shared draft, etc.) is a transport concern, not fixed
here.
