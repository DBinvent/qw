# NIP-QW05: Cascade block

`draft` — kinds `9040` (flag), `9041` (block record)

## Abstract

Sybil resistance without a central blocklist (§6, `qw-design-faq.md`
§"Trust, Sybil & Disputes"). Two kinds working together:

- **Flag** (9040): any WoT member accuses a pubkey.
- **Block record** (9041): a node that has locally decided to act on a
  flag (per its own policy) *re-publishes its own vouch* — "I also block
  X, sourced from Y." Cascade propagation is therefore a chain of these
  block records, not a lookup against one authoritative list; no party
  ever needs to enumerate every blocked account, only follow the vouch
  chain from a flag it already trusts.

Default trigger (`todo-impl.md` §0.5, locked): any WoT member may flag;
a block **auto-cascades** to accounts within relay-graph distance 1 of a
flagged signer once ≥2 independent flaggers (non-overlapping paths)
confirm; beyond distance 1, review is manual per-participant. This NIP
defines the wire format both kinds travel in; the distance/threshold
policy itself lives in client-side scoring logic (§5/§6), not the event
schema.

## Kind 9040 — Cascade block flag

Tags: `["p", <target pubkey>]`.

```json
{
  "reason": "signed 40 contracts in one hour, no prior history",
  "evidence_event_id": "<hex event id, optional>"
}
```

`evidence_event_id` is an optional pointer to a specific record (e.g. a
suspicious kind-9000 offer) supporting the flag; omit if the reason is
pattern-based rather than tied to one record.

## Kind 9041 — Cascade block record

Tags: `["p", <blocked pubkey>]`, `["e", <sourced-from event id>, "",
"cascade-source"]` — the `"cascade-source"` marker (rather than a plain
`e` tag) distinguishes "the flag/record I'm propagating" from an ordinary
lifecycle reference, so a client walking a cascade chain doesn't confuse
it with, say, a job-offer reference.

```json
{ "distance": 1 }
```

`distance` is this voucher's hop count from the *originally flagged
signer* at publish time (not from the immediate `sourced-from` event,
which may itself already be a distance-N block record) — it's what a
client's local policy checks against the distance-1 auto-cascade default
before deciding to also block-and-republish.

## Propagation walk

1. Node sees a kind-9040 flag (or a kind-9041 record) against pubkey X,
   `distance` (0 for a fresh flag) attached or implied.
2. Node applies its own local policy (§0.5 default: ≥2 independent
   flaggers, non-overlapping paths, distance ≤ 1) to decide whether to
   locally block X.
3. If it decides to block, it publishes its own kind-9041 with `p: X`,
   `e: <the flag or record that convinced it, marked "cascade-source">`,
   and `distance` = its own hop distance from the original flagged
   signer.
4. Anyone downstream can now see *this* node's vouch and apply their own
   policy to it in turn — the chain, not a central table, is the
   blocklist.

This NIP does not define how a client computes "independent, non-
overlapping paths" (§5's trust-graph traversal) — only the wire shape of
what gets published once that computation concludes.
