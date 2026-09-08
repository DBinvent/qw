# NIP-QW14: Broadcast propagation

`draft` — kinds `9100` (broadcast envelope), `9101` (hop rating)

## Abstract

NIP-QW06 propagates a **query** over multiple hops. NIP-QW11 publishes a
standing self-ad to **one** board. Neither covers what the FAQ §6 calls
"broadcasting": a message the originator wants *pushed outward* through the
web of trust — an open job proposal, a "wanted" demand, a fresh profile, a
news item, a review — fanning out hop by hop, store-and-forward, the way
FidoNet echomail moves a message through a topic.

This NIP defines that layer, and it is deliberately conservative, because
an unbounded push is a spam amplifier:

- The **envelope** (kind 9100) is signed once by the originator and
  forwarded byte-for-byte. Hops never rewrite it.
- Each relaying hop emits a **hop rating** (kind 9101): its own
  reputation reading of the originator, in the payload's domain,
  **signed and time-boxed**. A hop computes this once per
  `(originator, domain)` and reattaches the cached, still-valid rating to
  every envelope from that originator — it does not rescore per message.
  The chain of hop ratings is also the path record (FidoNet's
  `SEEN-BY` / `PATH`).
- Forwarding is gated by a **per-message-type policy**
  (`PropagationPolicy`) — hop count, age, size, fan-out, a **minimum hop
  score**, the hop-rating validity window, and a rate limit — each with
  its own default per type. A node applies the policy; nothing about it is
  on the wire (same as NIP-QW06's `ContactPolicy`: local, never
  published).
- A recipient re-derives a trust number from the hop ratings on the chain
  (weighting each by *their own* score of the signing hop, NIP-QW13 §4)
  and runs its NIP-QW13 §6 admission filter on the result. The record is
  always verifiable by anyone who holds it; the policy only decides whose
  node bothers to relay or surface it.

## 1. Kind 9100 — Broadcast envelope

Tags: `["broadcast", <type>]` (the echo selector), one `["t", <skill
tag>]` per routing tag, `["expiration", <unix>]` (NIP-40, so a plain relay
can drop it).

```json
{
  "type": "proposal",
  "body": { "...": "shape depends on type, see §3" },
  "expires_at": 1737000000
}
```

`type` is one of `proposal` / `demand` / `profile` / `news` / `review`
(§3). `expires_at` is a hard stop — a node MUST NOT relay or surface an
envelope past it, regardless of policy. The envelope's own `id`/`sig` are
NIP-01; the `id` is the **MSGID**: a node's dedup set is "envelope ids I
have already handled," and it forwards an id to a given peer at most once.

The envelope is **immutable**. What each hop thinks of the originator lives
in the kind 9101 events (§2). Who relayed it and how far — the *path* —
travels as unsigned hop metadata alongside the envelope (`["hop",
<pubkey>]` entries a node appends as it forwards): it is used only for
loop-avoidance and the hop count, and a *signed* path record is a
follow-up (§8), the same way NIP-QW06's `referral-hop` chain came after
the bare query.

## 2. Kind 9101 — Hop rating

Tags: `["p", <subject pubkey>]`, `["expiration", <valid_until>]`.

```json
{
  "subject_pubkey": "<the pubkey this rating is about>",
  "domain": "it/backend",
  "score": 1.15,
  "computed_at": 1736400000,
  "valid_until": 1736660000
}
```

A hop rating is **envelope-independent** — that is the whole design. It
says only "signer's NIP-QW13 reading of `subject_pubkey` in `domain`, good
until `valid_until`." A hop computes one per `(subject_pubkey, domain)`,
signs it once, and **re-attaches this same signed event to every broadcast
it relays from that originator** until it expires. A rating tied to one
envelope would have to be re-signed per envelope, which is exactly the cost
the caching exists to avoid.

- **`score`** — the `1.0`-centred multiplier, or the string
  `"unknown-risk"` when no verified path reaches the subject in `domain`.
- **`domain`** — `sector/domain` (`crate::events::tag_domain`), or `""`
  for a non-domain-scoped broadcast (`news`, a tagless `profile`).
- **`computed_at` / `valid_until`** — the cache window;
  `valid_until - computed_at` is the hop's `hop_rating_ttl` for the
  relayed type (§4). Past `valid_until` the rating is dead: a verifier
  ignores it, a relaying hop recomputes and re-signs before forwarding
  again.

Signed by the hop. Cheap to make, cheap to verify, and — cached for days —
cheap to *keep* attaching across a burst of broadcasts from one
originator.

### Verifying and folding a chain

A node holding envelope `E` (path `p₀ … pₙ`, `p₀` the originator, `pₙ`
this node), with the hop ratings `R` gathered along the way:

1. `E.verify()`, `E` not past `expires_at`, `age ≤ max_age`, `E` within
   `max_bytes` for the type.
2. Each `Rᵢ.verify()`, `now < Rᵢ.valid_until`, `Rᵢ.subject_pubkey == p₀`,
   `Rᵢ.domain` matches the payload's.
3. Relayer count `n` (the `pᵢ` after the originator) is `< max_hops` for
   the type — otherwise this node holds `E` but does not forward it.
4. Fold, NIP-QW13 §4 style: take the reading nearest the originator (the
   earliest relayer who left one) as the base, then for each *later*
   relayer `pᵢ` — excluding this node itself — multiply by
   `min(1.0, my_score(pᵢ)) * hop_decay`. A relayer this node does not
   trust, or a `"unknown-risk"` base, collapses the result to
   `"unknown-risk"`.
5. Gate on the folded number against this node's `min_hop_score` for the
   type; run the NIP-QW13 §6 admission filter before a human sees the
   payload.

A node that cannot reach step 4 with a number `≥` its own
`min_hop_score` **does not relay** `E` further — it may still surface it
locally, but it stops being a propagation path.

## 3. Message types and their bodies

| `type` | `body` shape | domain-scoped | notes |
|---|---|---|---|
| `proposal` | a `JobOffer` (NIP-QW01) with **no `["p"]`** — an open call, not addressed | yes, from `skill_tags` | "I have work in X" |
| `demand` | `{ skill_tags, terms, budget? }` | yes | "I want work in X" — the wanted side |
| `profile` | a `ProfileSkillTags` (NIP-QW03) | its tags' domains, or `""` | push a new/updated profile toward tag-similar contacts |
| `news` | `{ title, text, url? }` | `""` | an update; travels furthest and cheapest |
| `review` | `{ subject_pubkey, skill_tags, body, rating? }` | yes | a third-party opinion about someone's work in a domain — the reviewer stakes their own standing, same as an NIP-QW04 audit opinion |

`review` is not evidence in the NIP-QW13 §2 sense (it is not a
countersigned contract); it is a *reach* signal a recipient weighs by
their own score of the reviewer, exactly like a declared tag.

Wherever a body carries `skill_tags` (`proposal`, `demand`, `profile`,
`review`), the NIP-QW03 size limits apply unchanged — each tag `<
MAX_SKILL_TAG_LEN` characters, `< MAX_PROFILE_SKILLS` of them — and a node
rejects a body that breaks them, the same as it would a malformed
profile.

## 4. `PropagationPolicy` — per type, local, unpublished

```
PropagationPolicy {                 // one of these per `type`
  max_hops:            u8,          // cap on relayer count along the path
  max_age_secs:        u64,         // relay only while (now - E.created_at) < this
  max_bytes:           u32,         // reject / drop an envelope larger than this
  fanout:              u8,          // peers to forward each new envelope to (NIP-QW06 greedy select)
  min_hop_score:       f64,         // don't relay unless the folded chain score ≥ this
  hop_rating_ttl_secs: u64,         // how long this node's own hop rating stays valid
  rate_per_day:        u32,         // envelopes of this type this node will originate/relay per contact per day
}
```

Documented defaults — news wide and cheap, a proposal tight, a review
slow and demanding:

| type | max_hops | max_age | max_bytes | fanout | min_hop_score | hop_rating_ttl | rate/day |
|---|---|---|---|---|---|---|---|
| `proposal` | 3 | 7 d | 8 KiB | 3 | 1.0 | 3 d | 50 |
| `demand` | 4 | 14 d | 4 KiB | 3 | 0.9 | 3 d | 20 |
| `profile` | 3 | 30 d | 16 KiB | 2 | 1.0 | 7 d | 5 |
| `news` | 6 | 30 d | 32 KiB | 4 | 0.8 | 7 d | 10 |
| `review` | 4 | 90 d | 8 KiB | 3 | 1.1 | 3 d | 20 |

Every field is the node operator's to edit and is persisted the same way
`ReputationConfig` and `ContactPolicy` are (QW's client: the sealed
`SyncState`). Defaults reproduce a sane conservative flood; the config
only ever tightens or loosens per type.

## 5. Relay mechanics (a node holding envelope `E` from `from`, path `P`)

1. **Dedup.** If `E.id` is in the handled set, stop. (First arrival wins,
   as in NIP-QW06; multi-path reinforcement is a follow-up.)
2. **Admit.** `from` is a known contact; `E.verify()`; `E` within
   `max_bytes` / `max_age_secs` / `E.expires_at`. On failure, drop — do
   not store, do not forward.
3. **Surface.** Hold `E` locally regardless of what follows — a node that
   won't relay a broadcast can still show it to its own user.
4. **Score.** Look up this node's cached hop rating for
   `(E.originator, domain)`. If absent or past `valid_until`, compute the
   NIP-QW13 score now, sign a fresh kind 9101 with
   `valid_until = now + hop_rating_ttl_secs`, cache it.
5. **Gate.** With the path `P' = P ++ [self]`: if `|relayers(P')| ≥
   max_hops`, hold. Else fold the ratings (§2 step 4) — the ones gathered
   along `P` plus (4); if `< min_hop_score`, or `"unknown-risk"`, hold.
6. **Forward.** Pick up to `fanout` onward peers by NIP-QW06 greedy
   selection over the payload's tag (a tagless `news` goes to the first
   `fanout` contacts); skip `from` and everyone already in `P'`. To each,
   send `E` unchanged, the hop ratings so far ++ this node's, and `P'`.
   `E` is never rewritten.
7. **Rate-limit.** Count relays of this `type` per downstream contact per
   day against `rate_per_day`; a contact over budget is skipped this
   window, not dropped.

## 6. Where this meets the rest

- **NIP-QW06** — greedy fan-out selection (`qw_node::routing`) is reused
  verbatim for step 5; `ContactPolicy.relay_depth` / `accept_depth` still
  cap a given edge independently of the type policy, and the stricter of
  the two wins.
- **NIP-QW11** — a bulletin board is a *node with fan-out 0*: it admits
  and surfaces `proposal` / `demand` envelopes for browsing but never
  forwards. An operator can run a board that is also a relay by raising
  `fanout`.
- **NIP-QW13** — the chain fold (§2 step 4) *is* §4 path propagation with
  the hop ratings standing in for `CreditIssuance` edges; the admission
  filter (§2 step 5) *is* §6. `min_hop_score` is the per-type spelling of
  §6's "per-domain score floor on relay-forward," which §7 lists as the
  outstanding visibility item.
- **NIP-QW03** — `type: "profile"` is how a replaceable profile reaches
  someone who has never queried for it; the recipient still treats it as a
  claim, not evidence.

## 7. Current implementation vs this spec

| aspect | today | this spec |
|---|---|---|
| multi-hop push | none — NIP-QW06 pushes queries, NIP-QW11 is one hosted board | kind 9100 envelope, echomail fan-out |
| per-hop score | a relay re-signs a NIP-QW06 hop but attaches no score | kind 9101, signed, cached per `(originator, domain)` for `hop_rating_ttl` days |
| forward gate | `ContactPolicy` depth / categories / rate; **no score floor** | `min_hop_score` per type, folded chain score |
| per-type config | one `ContactPolicy` for all query traffic | one `PropagationPolicy` per message type, defaults tabled in §4 |
| news / reviews | not carried at all | first-class `type`s |

**Nothing here changes an existing wire format.** Two new kinds, both
additive; every gate is local computation over records a node already
holds plus the cheap signed ratings this NIP adds.

## 8. Staging (smallest useful first)

1. **Kinds 9100 / 9101 + builders + verify** (`qw_protocol::events::kinds`).
   *Done.*
2. **`PropagationPolicy` + `PropagationConfig` + the pure `evaluate_relay`
   / `fold_chain`**, defaulted to §4; the **hop-rating cache** keyed by
   `(subject, domain)`; `Node::originate_broadcast` / `receive_broadcast`
   wired to the contact book and greedy select (`qw_node::broadcast`).
   *Done.*
3. **`PropagationConfig` in the sealed `SyncState`**, beside
   `ReputationConfig` / `AdmissionPolicy`, with a client config screen.
4. **A signed path record** — replace the unsigned `["hop", …]` metadata
   with a per-hop signed chain (as NIP-QW06's `referral-hop` did), for
   loop-avoidance a recipient can verify.
5. **`news` / `review` payloads** and their surfaces in the client.
6. **Board-as-relay** — let a NIP-QW11 operator opt into `fanout > 0`; the
   coordination server today is a `fanout = 0` rendezvous
   (`qw_server::broadcast`: hold + serve, no relay decision).
