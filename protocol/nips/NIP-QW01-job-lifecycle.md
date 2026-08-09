# NIP-QW01: Job lifecycle

`draft` — kinds `9000` (offer), `9001` (accept), `9002` (milestone),
`9003` (completion), `9004` (counteroffer), `9005` (review request)

## Abstract

The steps of a contract prior to credit issuance (NIP-QW02). Per
`qw-design-faq.md` §"How does the contract lifecycle get signed?", none of
these are atomic — each tolerates one party being offline indefinitely,
which is the mobile reality (`todo-impl.md` §4).

| Step | Signed by | Atomic? |
|---|---|---|
| Offer | Client | No |
| Counteroffer (repeatable) | Either party | No |
| Accept | Worker | No |
| Milestone (optional) | Either party | No |
| Completion | Each separately | No |

## Kind 9000 — Job offer

Tags: `["p", <worker pubkey>]`, one `["t", <skill tag>]` per offered skill.

```json
{
  "skill_tags": ["it/backend/languages#rust"],
  "hours": 8.0,
  "rate": 40.0,
  "ko": 1.1,
  "km": null,
  "terms": "sprint 12 backend work"
}
```

`hours`/`rate`/`ko`/`km` mirror `abstract.md`'s `Quants = Hours × Rate ×
ko × km`: `ko` is the objective coefficient (equipment, conditions,
hazard), `km` the subjective motivation/quality coefficient; both may be
omitted (`null`) to simplify negotiation. `skill_tags` are taxonomy leaves
(`/taxonomy.yaml`); the `t` tags carrying them are what a relay filters on
for referral routing (§3) — the content field is the source of truth,
tags are a denormalized index of it.

## Kind 9004 — Job counteroffer (optional, repeatable)

Tags: `["p", <counterparty pubkey>]`, `["e", <superseded offer or
counteroffer event id>]`, one `["t", <skill tag>]` per skill.

Same content shape as kind 9000 — a counteroffer *is* a full replacement
set of terms, not a diff:

```json
{
  "skill_tags": ["it/backend/languages#rust"],
  "hours": 8.0,
  "rate": 55.0,
  "ko": null,
  "km": null,
  "terms": "sprint 12 backend work"
}
```

A counteroffer **neither accepts nor rejects** the terms it references —
it supersedes them and hands the proposal back. Either party may counter
repeatedly; only a signed Accept (kind 9001) ends the exchange. Each
version is signed by whoever proposed it, so the negotiation itself is
available as evidence later (e.g. for an auditor via NIP-QW04), but no
version prior to the one actually accepted carries any obligation.

Resolved (`todo-impl.md` §4): un-accepted offers/counteroffers are **not**
specially excluded from dual indexing — their `p` tag already makes them
visible via `qw_protocol::dual_index::records_referencing` like any other
kind, same as the rest of this NIP. What actually keeps them out of
`net_position` is simpler and needs no special-casing: that figure is
computed strictly from `CreditIssuance` events (NIP-QW02), which an
un-accepted negotiation never produces.

## Kind 9001 — Job accept

Tags: `["p", <client pubkey>]`, `["e", <accepted event id>]` — the
original offer if nobody countered, or the last counteroffer otherwise.

```json
{ "note": "starting Monday" }
```

`note` is optional.

## Kind 9002 — Job milestone (optional)

Tags: `["p", <counterparty pubkey>]`, `["e", <offer event id>]`.

```json
{ "description": "auth module done", "hours_delta": 3.0 }
```

Either party may post any number of these against one offer. `hours_delta`
is optional — a milestone can be a pure status note.

## Kind 9003 — Job completion

Tags: `["p", <counterparty pubkey>]`, `["e", <offer event id>]`.

```json
{ "rating": 5, "note": "delivered on time" }
```

Both fields optional. **Each party posts their own** completion event,
independently, both anchored to the same offer event id — this is the
"two tagged events, cross-referencing" dual-indexing case described in
`todo-impl.md` §2: a completion event's own id can't embed the sibling's
id (it doesn't exist yet when either side signs), so both sides instead
anchor to the one id that already exists — the offer — making "did both
sides complete?" a union query over that anchor
(`qw_protocol::dual_index::check_dual_index`), not a field either party
could omit or forge unilaterally.

A one-sided completion (only one party ever posts) is a valid, detectable
state — it is exactly what "unsigned/expired" (§0.7, 30-day default) and
the dispute annotations of NIP-QW04 exist to handle; it is not an error in
this NIP.

## Kind 9005 — Job review request (optional)

Tags: `["p", <counterparty pubkey>]`, `["e", <milestone or completion
event id under review>]`.

```json
{ "feedback": "looks close, one nit on the error handling" }
```

`feedback` is optional. A **pre-signature** negotiation step (`abstract.md`
"Basic Use Cases" §"Commit a contract", added 2026-08-07): either party
may request review of a delivered milestone or a completed job, with
optional feedback, before posting their own kind 9003 completion. Closer
in spirit to Counteroffer than to NIP-QW04's dispute annotations, which
apply only to already-signed records — this applies before one.

## State machine

`qw_protocol::contract` (§4) turns a set of these events plus a
`CreditIssuance` (NIP-QW02) into one `ContractState` — negotiation head,
accept status, milestones, dual-indexed completion, and the 30-day
`unsigned/expired` timeout (§0.7) — rather than each client re-deriving
that logic independently.
