# NIP-QW11: Bulletin listing

`draft` — kind `9091`

## Abstract

The "offline bulletin board" (`todo-impl.md` §8, added after the section
was otherwise complete): a **public, undirected** self-advertisement —
"I'm available for X" or "I need X" — meant to be discovered by someone
who doesn't know the poster's pubkey in advance, the way a Craigslist
post works. Neither side needs to be online at the same time, only the
server hosting the board needs to be reachable when each of them happens
to check it — this is `abstract.md`/FAQ §6's third discovery layer,
"Public gateway — indexable signed job postings with stable URLs," now
concrete.

This is deliberately different from two kinds that might look similar:

| Kind | Directed? | Standing or scoped? |
|---|---|---|
| `JobOffer` (NIP-QW01, kind 9000) | Yes — to one specific worker pubkey | One negotiation |
| `ProfileSkillTags` (NIP-QW03, kind 9020) | No | Standing self-description, no expiry |
| `BulletinListing` (this NIP) | **No** | **Time-scoped posting**, meant to be browsed |

## Kind 9091 — Bulletin listing

Tags: one `["t", <skill tag>]` per tag in `skill_tags`. **No `p` tag** —
a listing has no addressed counterparty; that omission is the whole
point.

```json
{
  "listing_type": "offering",
  "skill_tags": ["it/backend/languages#rust", "it/backend/languages#go"],
  "description": "Rust/Go contractor, available evenings",
  "expires_at": 1735689600
}
```

`listing_type` is `"offering"` or `"seeking"` — mirrors a classifieds
board's offered/wanted split. `expires_at` (unix seconds, optional) is
the poster's own requested expiry; a board should stop surfacing the
listing after that point, but nothing prevents the underlying event
from continuing to exist and being verifiable — expiry is a *display*
convention for board implementations, not a protocol-level deletion (see
`README.md`'s deletion-rights notice: nothing here is ever a real
deletion guarantee).

## Server-side browsing

Unlike `qw_server::vault` (retrieve by a *known* pubkey) or
`qw_server::rating_bureau` (a signed request about a *known* subject),
a board needs to answer "show me what's currently posted matching X" —
`qw_server::board` filters by `listing_type` and by skill-tag domain
(`qw_protocol::events::same_domain`, so a "rust" listing is found by a
"backend" browse, not only an exact-tag match), and excludes anything
past its `expires_at`.

## What this NIP does not fix

Per `todo-impl.md` §8's own note on this item: rate limits and
monetization for board usage are left to the server operator, not
specified here — same treatment as NIP-QW08's rating-bureau
subscription billing. A malicious or spammy poster is the board
operator's problem to solve (rate limiting, proof-of-work, a Quant
posting fee, ...), not something this wire format enforces.
