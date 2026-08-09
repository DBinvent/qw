# NIP-QW04: Dispute annotation

`draft` — kind `9030`

## Abstract

Per `qw-design-faq.md` §"How are disputes handled?": keep the record,
attach annotations — never hide. An annotation is always a reply *to* an
existing signed event; it never mutates or supersedes the original. This
closes the "unsigned completion left contracts in limbo" hole: instead of
needing every contract to reach a signed completion to be meaningful, an
unresolved one gets annotated and, absent that, times out to
`unsigned/expired` (§0.7, 30-day default) — "disputed, no audit" is
itself a valid terminal state, not an error state.

| Annotation | Signed by | Effect |
|---|---|---|
| Reply | Party being criticized | Visible alongside, no score effect |
| Audit request | Either party | Marks record "under review" |
| Audit opinion | Third-party auditor | Weight proportional to auditor's standing |

Design constraints from the FAQ, not enforced by this NIP's wire format
but binding on any client/scoring logic that consumes it:
- Auditors are drawn from the intersection of both parties' WoT, or
  accepted by both.
- Auditors stake reputation — the opinion attaches to the auditor's *own*
  record too (i.e. a client should also index kind-9030 `audit_opinion`
  events by their author, not only by their `e`-tag target).
- Auditors are paid in Quants (via a normal job-lifecycle/credit-issuance
  contract between the disputing party and the auditor — not a separate
  payment mechanism).

## Kind 9030 — Dispute annotation

Tags: `["e", <target event id>]` — the record being annotated (typically a
kind 9000-9003 job-lifecycle event, but any signed QW event is a valid
target).

`content` is tagged on `annotation_type`:

```json
{ "annotation_type": "reply", "body": "the delay was on my end, fixed now" }
```
```json
{ "annotation_type": "audit_request", "body": "milestone was never delivered" }
```
```json
{
  "annotation_type": "audit_opinion",
  "body": "reviewed both sides' evidence",
  "outcome": "favors_worker"
}
```

`outcome` (audit opinion only) is one of `favors_client`, `favors_worker`,
`split`, `inconclusive`.

No annotation is restricted at the protocol level to being signed only by
the "correct" party (a relay/client cannot verify who is "the party being
criticized" without replaying the whole contract graph) — that check, and
any resulting weighting, is §5 per-viewer scoring logic, not this NIP.
