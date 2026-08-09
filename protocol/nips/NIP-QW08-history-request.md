# NIP-QW08: History request / response

`draft` — kinds `9070` (request), `9071` (response)

## Abstract

`abstract.md`'s "Basic Use Cases" §Introduce: "Accept a history request to
receive a signed, filtered work history from a contact. The history is
scoped by skill tag and time window, and the recipient may verify the
signature and check for omissions."

This is a discrete request/response *interaction* between two contacts,
distinct from `qw_protocol::dual_index`'s public relay-tag queries: it's
for the case where the requester wants a contact to actively curate and
hand over a scoped view, not (only) independently query the raw relay
data themselves.

## Kind 9070 — History request

Tags: `["p", <contact pubkey>]`.

```json
{
  "skill_tags": ["it/backend/languages#rust"],
  "since": 1700000000,
  "until": null
}
```

`skill_tags` empty means all domains; `since`/`until` are unix seconds,
inclusive, `null` = unbounded on that side.

## Kind 9071 — History response

Tags: `["p", <requester pubkey>]`, `["e", <request event id>]`.

```json
{ "record_event_ids": ["<hex event id>", "<hex event id>"] }
```

The response is a **signed pointer**, not a re-attestation: it names
which of the responder's own already-signed, already-dual-indexed records
(job completions, credit issuances) fall within the requested scope. It
doesn't restate their content — the requester independently fetches and
verifies each referenced id (`Event::verify`, `qw_protocol::dual_index`).

"The recipient may verify the signature and check for omissions" (per
`abstract.md`) means checking `record_event_ids` against whatever the
requester can independently see through other means (their own relay
access, other contacts, a prior direct query) — this event does not
itself prove completeness. A responder can always choose to omit an
in-scope record without saying so; this NIP gives the requester a signed
claim to check, not a completeness guarantee.

## Relationship to the VC schema

`crate::vc`'s SD-JWT credential (§2) is a single job's claim with
field-level selective disclosure, issued at credit-issuance time. This
NIP is a different shape: an aggregate *pointer* into many already-public
records, assembled on request. They compose — a `HistoryResponse` might
point at the same `CreditIssuance` events whose VCs a recipient later
requests directly from their respective issuers — but this NIP doesn't
replace or extend the VC format.
