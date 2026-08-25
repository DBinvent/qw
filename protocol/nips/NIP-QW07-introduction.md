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

Three shapes share this kind — the first two below, plus the public
invite link further down, which is a self-introduction distributed as an
ad rather than sent to one person:

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

## Public self-introduction (invite link)

The third shape, and the network's front door: a participant publishes
their own self-introduction as a **link** and distributes it anywhere
people already are — a LinkedIn post, a talk slide, an email signature, a
job ad. Anyone who follows it exchanges introductions with the publisher
and lands as a hop-1 contact, whether they were four hops away in the
contact graph or not reachable from it at all.

```
https://knownby.work/i/<npub>[?r=<relay hint>]
```

The link carries only the publisher's pubkey and optional relay hints —
it is public by definition, so it holds nothing that is not already
publishable. Following it produces two ordinary kind-9060 events:

1. The newcomer signs a **self-introduction** to the publisher
   (`subject_pubkey == event.pubkey`, `chain: []`).
2. The publisher's client answers with its own self-introduction, making
   the edge mutual. That answer is automatic *because the publisher chose
   to publish the link* — it is the standing consent the ad represents,
   not a per-person decision.

Both carry `"via": "public-link"` in their content, and this marker is
load-bearing rather than informational:

- **It is not a vouch.** A mutual introduction normally means one party
  put their name behind another. Nobody who posts a link knows who will
  click it, so an edge minted this way asserts reachability and nothing
  else. Trust still comes only from completed, countersigned work
  (NIP-QW01/QW02), which is why collapsing distance here does not
  manufacture reputation: a stranger at hop 1 with no contracts scores
  exactly what a stranger at hop 4 with no contracts scores — nothing.
- **Cascade blocks must skip it.** NIP-QW05 measures cascade distance
  over *this* graph, the published introduction graph, because a node's
  `Contact` list is private. If public-link edges counted, publishing an
  ad would make every stranger who clicked it distance-1 from you, and
  two flags against any of them would cascade onto you — the opposite of
  cascade block's premise, which is that a real signing account stands
  behind each edge. A client evaluating a cascade therefore walks
  introductions **excluding** `via: "public-link"` edges.
- **Admission filters still apply.** §5's client-side pre-filters run on
  inbound introductions, offers and queries regardless of how the edge
  was created; an open front door is not an open inbox.

Nothing about this shape is invite-only, rate-limited or gated by
default. A publisher who wants those can rotate the link's pubkey or stop
publishing it; the protocol does not model a revocable invite token,
because a link that a stranger can use is exactly the point.

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
