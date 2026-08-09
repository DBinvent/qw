# NIP-QW10: Chain-calculation result

`draft` — kind `9090`

## Abstract

The wire format for §8's optional coordination server's first (and most
concretely specified) service: "traverses trust graph on request,
returns a signed path + score, spot-checkable by the client against raw
relay data" — `abstract.md`'s "Optional Coordination Server" is explicit
that the server "holds no keys and cannot forge signatures" and "every
result it returns is verifiable against the public record." This NIP is
what makes that concrete: the result is a normal signed event referencing
real event ids, not an opaque API response the client has to trust.

## Kind 9090 — Chain-calculation result

Tags: `["p", <requester pubkey>]`.

```json
{
  "target_pubkey": "<hex pubkey the requester asked about>",
  "hops": 2,
  "edge_event_ids": ["<hex event id>", "<hex event id>"],
  "score": 0.75
}
```

`edge_event_ids` are the real `CreditIssuance` (NIP-QW02) event ids
forming the path from requester to `target_pubkey`, in order — the same
shape `qw_protocol::trust::TrustPath` already produces locally. `score`
is whatever the server's own scoring function
(`qw_protocol::trust::score_trust_path` or the server operator's own)
computed over that path.

## Why this is a signed event, not just an HTTP response body

The server "must never be the only source of truth for a result it
returns" (`todo-impl.md` §8). Wrapping the answer as a normal QW event
gets that property for free:

- The requester can fetch `edge_event_ids` directly from relays and
  independently re-verify each one (`Event::verify`,
  `qw_protocol::contract::verify_credit_issuance`) — the server's `hops`
  and `score` fields are checkable, not asserted.
- The server's own pubkey is on the result, so a server that returns
  fabricated paths or fictitious event ids accrues visible, checkable
  reputation damage the same way any participant would — no special
  server-trust mechanism needed.
- The exact same verification code path (`crate::events::Event::verify`)
  applies whether the signer is a peer or a coordination server; nothing
  server-specific needed in the client.

## Request

There is no corresponding signed "chain-calculation request" kind — the
request is just a plain query (self pubkey, target pubkey, max hops,
optional skill-tag domain filter) over whatever transport the server
exposes (`qw-server`'s HTTP API, in this implementation). Only the
*answer* needs to be a durable, attributable, checkable artifact; the
question doesn't.
