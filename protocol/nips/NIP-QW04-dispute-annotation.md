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

## Carriage (client note, not wire format)

`qw_client_core::Session::annotate` builds and signs a 9030 from held
history (2026-09-06). Client-level choices, none a change to the 9030
*content*:

- **`["p", …]` on the annotation.** The wire format is `["e", <target>]`
  only, but the coordination mailbox files by `p` tag, so `annotate` adds
  a `p` tag for the *other* contract party (both parties, when the signer
  is a third-party auditor) purely so the annotation is deliverable — the
  same carriage note NIP-QW06 makes for a 9050. A relay a party
  subscribes to ignores it.
- **Where it attaches.** A client annotates a contract by its root offer
  id; the annotation targets that contract's current negotiation head
  unless the caller names another event the contract already involves (a
  milestone, a completion). Targets are held to the events
  `qw_protocol::contract::Contract::from_events` re-collects annotations
  for, so a signed annotation always surfaces on the contract view rather
  than becoming an orphan — threading a reply onto another annotation is
  therefore not yet expressible, a documented follow-up.
- **Signer rule, client-side.** `reply` and `audit_request` require the
  signer to be the client or the worker; `audit_opinion` does not (an
  auditor is by definition a third party) but still resolves a real
  target inside the contract. Any contract state is annotatable —
  annotating one stuck in limbo is the whole point.

`NegotiationView` carries the resulting rows (`disputes`, oldest first;
`under_review` = an `audit_request` with no `audit_opinion` answering it
yet). `AuditOpinion` events are **not** yet indexed by their author's
pubkey — the "an auditor stakes their own record" half of the design is
still to come.
