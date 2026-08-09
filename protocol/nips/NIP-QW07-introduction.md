# NIP-QW07: Introduction

`draft` — kind `9060`

## Abstract

A **contact** operation, not a contract one — `abstract.md`'s "Basic Use
Cases" §Introduce is explicit that introducing a job is a `JobOffer`
(NIP-QW01), covered separately. This kind is how a contact-graph edge
actually comes to exist: `qw_node::contact::Contact` (§3) is populated
directly today, with nothing behind it — this NIP is the missing signed
mechanism.

## Kind 9060 — Introduction

Tags: `["p", <recipient pubkey>]`.

```json
{
  "subject_pubkey": "<hex pubkey being introduced>",
  "chain": [],
  "note": "we met at the meetup"
}
```

Two shapes share this kind:

- **Self-introduction**: `subject_pubkey == event.pubkey` — the signer
  introducing themself to `recipient_pubkey` (someone they found, e.g.
  via NIP-QW06 referral or the public gateway).
- **Mutual introduction**: `subject_pubkey` is a third party — one of the
  signer's own contacts — being introduced to `recipient_pubkey`, another
  of the signer's own contacts. `chain` carries the connections linking
  `subject_pubkey` to `recipient_pubkey`, oldest hop first, *not*
  including the signer's own hop (that's `event.pubkey`); empty for a
  direct, one-hop introduction.

Signed, and therefore attributable — the introducer's reputation is
behind it, the same way a NIP-QW06 relay hop's vouch is.

## Profile exchange

`abstract.md` also calls for exchanging profiles (skill tags plus
completed jobs/contracts as evidence) alongside an introduction, with
fields redactable for privacy or NDA. This NIP doesn't define a new
format for that exchange — it's `KIND_PROFILE_SKILL_TAGS` (NIP-QW03) for
the tags, and `crate::vc`'s SD-JWT selective disclosure (already built,
§2) for the evidence, presented directly between the two parties rather
than published. No new event kind needed.

## Accepting an introduction

Accepting one — adding `subject_pubkey` as a contact — is a **local
decision** by the recipient, not itself a signed protocol step. The
resulting edge asserts acquaintance, not competence: it makes
`subject_pubkey` reachable by the recipient's relayed queries at that hop
(NIP-QW06), but only completed, countersigned work (NIP-QW01/QW02)
carries trust in a domain (§5's per-viewer scoring). If a future revision
needs provenance for the edge itself (e.g. "X really did accept Y's
introduction"), that would be a second, distinct event kind — not part of
this one.
