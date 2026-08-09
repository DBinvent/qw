# NIP-QW06: Referral query

`draft` — kinds `9050` (skill query), `9051` (skill answer)

## Abstract

TTL-bounded referral propagation through contacts with per-contact relay
policies — the search mechanism itself (`qw-design-faq.md` §6 "Discovery
& Referrals"; `todo-impl.md` §3). Discovery becomes routing, not search:
no index to build, host, or shard; each edge caches its direct contacts'
skill tags; trust is carried by the path itself.

Flooding does not scale (Gnutella failure: 50 contacts × 3 hops = up to
125,000 messages per question). Instead: **greedy routing** — since each
node already caches contacts' skill tags, it forwards selectively toward
tag-similar contacts, reaching a match in ~logarithmic hops with fanout
2-3 instead of 50. The FAQ calls this "the single most important change
to the referral design."

## Privacy: who sees the query?

Per the FAQ's answer to "Who sees the query?": **identity is revealed
only to hop 1.** Each relay attaches its own vouch as the query moves;
a receiver two hops out sees "someone my contact Anna trusts, two hops
out, asking about Rust work" — Anna's identity (hop 1), not the original
requester's.

This NIP implements that by construction, not by convention: a kind-9050
event is **always signed by the relaying node, about itself** — it never
contains a reference to the requester's private ask to hop 1. The
requester's actual request to hop 1 is a private, direct exchange (out of
propagation scope for this NIP — an application-level DM, not itself
broadcast). Hop 1's own first forward is the chain's head:

- Hop 1's forward: no `e` tag back to anything (nothing upstream is
  observable).
- Hop 2's forward: `e` tag (marked `"referral-hop"`) to hop 1's forward
  event.
- Hop N's forward: `e` tag to hop (N-1)'s forward event.

Walking this chain backward from any event terminates at hop 1 — signed,
visible, legitimately vouching — and never reaches the requester.

## Kind 9050 — Skill query (per-hop forward)

Tags: `["t", <skill tag>]`; `["e", <prior hop's forward event id>, "",
"referral-hop"]` on every hop except hop 1's chain-head forward, which
omits the `e` tag entirely.

```json
{
  "query_id": "b7e9...",
  "skill_tag": "it/backend/languages#rust",
  "hops_from_origin": 1,
  "max_hops": 3
}
```

- `query_id`: a fresh random correlator chosen by the requester, carried
  unchanged by every hop. This — not any event id — is what ties one
  logical query's forwards together, since (per the privacy model above)
  there is no single genesis event every hop can point back to.
- `hops_from_origin`: how far this event has *already* traveled before
  being (re-)signed. Hop 1's own forward is `0`.
- `max_hops`: set once by the requester, unchanged thereafter, so any
  relay can compute its own remaining budget (`max_hops -
  hops_from_origin`) without a coordinator. §3 default: 3.

A relay applies its own per-contact policy (`qw_node::contact::ContactPolicy`)
on top of the query's own budget — see "Per-contact policy" below.

## Kind 9051 — Skill answer

Tags: `["p", <upstream hop pubkey>]`, `["e", <matched query event id>,
"", "referral-hop"]`.

```json
{
  "query_id": "b7e9...",
  "responder_pubkey": "<hex pubkey of the node that actually has the matching skill>",
  "matched_skill_tag": "it/backend/languages#rust",
  "hops": 2
}
```

An answer is addressed to the **immediate upstream hop** (whoever's
forward event it matched on), never to the requester directly — the
responder, by design, cannot see who the requester is. Delivery back to
the requester happens hop by hop along the same relay chain: each hop
re-signs its own leg (the event's `pubkey`/`sig` change every hop, same
as a query forward) while `responder_pubkey` in the content stays fixed
to whoever originally matched — it, not the event's own `pubkey`, is
what a receiver (ultimately the requester) reads to learn who to
contract with.

`hops` is the path length from hop 1 to the responder, for the "2 hops
via Anna" display and for `qw_node`'s deduped-by-pubkey, path-count
answer collection at the requester.

## Per-contact policy (not a wire format — local node state)

`qw-design-faq.md` §6 "What permission model?":

| Setting | Range | Meaning |
|---|---|---|
| `relay_depth` | 0–3 | How far I pass queries onward |
| `accept_depth` | 0–3 | How far away a requester may be |
| `categories` | tag set | Only relay/answer in these areas |
| `rate_limit` | N/day | Ceiling per contact |
| `share_tags` | bool | May this contact cache my skill tags |

This table is intentionally an FAQ-level summary, not a spec; `qw_node`'s
implementation (`qw_node::contact::ContactPolicy`) fixes one consistent
reading, documented there:

- `accept_depth`, checked against `hops_from_origin` **as received**: I
  refuse to process/relay a query from this upstream contact if it has
  already traveled further than this before reaching them.
- `relay_depth`, checked against `hops_from_origin` **after** I'd
  increment it: I won't extend a query to this downstream contact past
  this depth, regardless of the query's own `max_hops`.
- `categories`: allowlist by tag domain (`sector/domain` prefix, e.g.
  `it/backend`); empty = unrestricted.
- `rate_limit`: queries per day accepted from this contact.
- `share_tags`: gates whether this contact's cached-tag entry gets
  populated at all (§2 privacy — a contact I don't trust with my tags
  simply never gets a cache entry, so it can never be a routing target
  for queries needing tag similarity to *me*).

## Scope note (MVP)

The FAQ also notes "dedup by pubkey but keep path count — multiple
independent paths is stronger signal." The current `qw_node` simulation
dedups propagation per `(query_id, node)` — first arrival wins — which
bounds message counts but means only the shortest path to a given
responder is recorded, not a count of independent paths that also reached
them. Multi-path reinforcement is a documented follow-up, not implemented
in this prototype.
