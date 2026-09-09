# NIP-QW15: Skill recognition, by a bureau

`draft` — kinds `9092` (recognition request), `9093` (recognition)

## Abstract

A **bureau** is a server-hosted convenience (§8, like NIP-QW10's
chain-calculation and NIP-QW08's rating bureau): it takes a subject's
Nostr pubkey, checks which skills their **countersigned QW work record**
already backs — and, opt-in, a parse of external profiles the bureau has
verified they control — and returns a **signed** statement of what it
found. Corroboration, not certification: it says what it checked and by
what method, never "this person is legit". The subject then decides which
tags to keep on their NIP-QW03 profile.

The bureau is never in the trust path. Its signature is a shortcut for a
reader who trusts *that bureau*; a reader who does not re-runs the same
check over the same public records. A client keeps a configurable list of
bureaus with a per-bureau weight, exactly as it weighs any other node
(`attest.md`).

## Kind 9092 — Recognition request

Tags: `["p", <bureau pubkey>]`. Signed by the **subject** — recognition is
always about yourself.

```json
{
  "skill_tags": ["it/backend/languages#rust"],
  "append_public_profile": false
}
```

`skill_tags` may be empty (ask the bureau to *suggest*). With
`append_public_profile` the bureau also parses the external profiles it
holds a verified binding for and folds those findings in.

## Kind 9093 — Recognition

Tags: `["p", <subject pubkey>]`, `["e", <request event id>]`, one
`["t", <tag>]` per endorsed skill. Signed by the **bureau**.

```json
{
  "subject_pubkey": "<hex>",
  "endorsements": [
    { "skill_tag": "it/backend/languages#rust",
      "corroborated_by": "2 credit-backed contracts",
      "tier": "qw_entitled" }
  ],
  "suggestions": [],
  "unverified": ["it/frontend#vue"],
  "unmapped": [],
  "checked_at": 1788918903,
  "expires_at": 1796694903
}
```

- **`endorsements`** / **`suggestions`** — a `skill_tag`, a plain-language
  `corroborated_by` note, and a `tier`: `qw_entitled` (a contract settled
  through a credit issuance, NIP-QW02) > `side_settled` (a closed contract
  with nothing on the ledger, NIP-QW01 kind 9006) > `profile` (an
  external-profile parse). Suggestions are proposals the subject has not
  listed; nothing enters a profile until the subject accepts it.
- **`unverified`** — listed tags the bureau found no evidence for.
- **`unmapped`** — profile findings that mapped to no taxonomy leaf.
- **`checked_at` / `expires_at`** — the reply speaks only to that instant;
  refresh by asking again.

### Verifying

`event.verify()`, `event.pubkey` equals the key the bureau advertises,
`["e"]` is the request id. Then the recognition is only as good as the
reader's own weight on that bureau.

## Not in this NIP

The bureau's *own* interfaces — how it verifies a profile binding, how it
runs auto-skill-build, its subscription billing — are operator concerns,
not wire format. `qw-server`'s `crate::attestation` is one implementation.
