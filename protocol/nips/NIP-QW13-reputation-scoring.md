# NIP-QW13: Reputation scoring & propagation

`draft` — **no new event kind.** A client-computation spec, like
[NIP-QW12](./NIP-QW12-ledger-sync.md): it defines how a conforming client
turns the signed records it already holds (offers, completions, ratings,
credit issuances, audit opinions) into a **per-viewer reputation score**,
how that score propagates along trust paths, and where offers, contracts,
visibility and broadcasting depend on it.

## Abstract

`abstract.md` §"Web of Trust" and the FAQ §5 fix the shape:

- **No global score, ever** — only N subjective readings of the same
  public record. This NIP standardises the *inputs and the mechanics*, not
  a canonical output. Two clients over the same events, with different
  config, legitimately disagree.
- **Domain-specific** — trust in `it/frontend` is independent of trust in
  `it/backend` (`abstract.md`).
- **Directional and local** — "each agent computes their own subjective
  score with their own weights and tolerances."
- **From completed work** — reputation "emerges from the job lifecycle
  itself"; a declared tag and a broker review add *reach*, not score
  (`todo-impl.md` §5, "Never enters the trust computation").

The score is a **normalised multiplier centred on `1.0`**:

| Score | Meaning |
|---|---|
| `> 1.0` | trusted in this domain — vouches and answers carry through, filters pass |
| `= 1.0` | neutral — a known contact with no domain evidence either way |
| `< 1.0` | distrusted — a failed job, an unfavourable audit, chronic over-issuance |
| `unknown-risk` | **not a number.** No verified path reaches them in this domain. A fresh key is unknown-risk, *not* `1.0` (`abstract.md` attack model). |

`unknown-risk` is distinct from a low number and is never silently mapped
to one: it is "I have no basis," and it fails every configured threshold.

## 1. Configuration surface (per viewer, per domain, editable)

Every knob below is the viewer's own and starts at a documented default.
A client MUST let a person edit them and MUST persist them beside the
identity (in QW's client the natural home is the sealed `SyncState`, next
to `AdmissionPolicy`).

```
ReputationConfig {
  // taxonomy: sector/domain[/area]#skill  (see /taxonomy.yaml)
  scope_inheritance: {
    parent_of:   0.0..1,   // a child-domain signal counts this much toward the parent
                           //   (java → backend). Default 0.5.
    child_of:    0.0..1,   // a parent-domain signal counts this much toward a child
                           //   (backend → java). Default 0.25.
    sibling:     0.0..1,   // area↔area within one domain. Default 0.0.
    cross_sector:0.0..1,   // it ↔ design, etc. Default 0.0.
  },

  per_domain: {
    "<sector/domain>": {
      tolerance:        f64,   // the admission threshold for THIS domain (NIP-QW06 §5).
                               //   e.g. frontend 0.8, backend 1.1.
      rating_weight:    0.0..1, // how much the 0–5 counterparty rating moves the signal
      completion_weight:0.0..1, // how much a pass/fail moves the signal
      quant_weight:     0.0..1, // how much the contract's Quant size moves the signal
      side_settled_factor: 0.0..1, // a contract settled off-system (NIP-QW01 kind 9006,
                               //   no credit issuance) has its whole per-contract
                               //   signal multiplied by this. Default 0.6 — below a
                               //   credit-backed contract, above a commit-analysis tag.
      recency_halflife: secs,   // exponential decay on contract age
      audit_weight:     0.0..1, // how much a trusted auditor's opinion moves it
      mf2_weight:       0.0..1, // "mutual of friends": how much a direct contact's
                               //   judgment of a third party I *also* have a read on
                               //   moves my score of the contact. §2b. Default 0.1
                               //   (small); the term is capped at ±0.3.
    }
  },

  path: {
    hop_decay:      0.0..1,   // multiplier per extra hop. Default 0.5 (current behaviour).
    max_hops:       1..6,     // Default 3.
    reverse:        0.0..1,   // the "contrarian" transform, §4. Default 0.0 (off).
    multipath:      "max" | "sum-capped" | "mean",  // §4. Default "max" (current behaviour).
  },

  balance: {
    net_position_weight: 0.0..1,  // how much the jobs-provided-vs-consumed balance
                                  //   pulls the overall score. Default small.
  },
}
```

Defaults are chosen so that a client that ships them **reproduces today's
`qw_protocol::trust` behaviour** (`hop_decay 0.5`, single strongest path,
credit-magnitude signal) — the config only ever *widens* what a
participant can express, never changes an unconfigured client.

## 2. The per-contract signal

For one completed, **dual-indexed** contract (`crate::dual_index`:
`KIND_JOB_COMPLETION` from both parties on the same offer) in which the
subject is the person being scored:

```
raw_signal = 1.0
           + rating_weight     * ((counterparty_rating - 3) / 2)      //  0..5 → −1..+1
           + completion_weight * (delivered ? +δ : −δ)                //  pass/fail
           + quant_weight      * scaled(quant_value)                  //  bigger stakes count more
raw_signal *= 2 ^ ( -age / recency_halflife )                        //  recent work weighs heavily
```

- **`counterparty_rating`** is `JobCompletion.rating` read from **the
  other party's** completion, never the subject's own (a subject rating
  themselves five stars is the exact failure §5 exists to prevent). `3/5`
  is neutral, so an unrated but countersigned contract contributes only
  the completion and quant terms.
- **`delivered`** — a countersigned completion is "delivered." A contract
  that reached `unsigned/expired` (NIP-QW04 §0.7) or carries an
  unfavourable audit opinion (NIP-QW04, kind 9030 `audit_opinion`) is a
  fail, and its `raw_signal` drops below `1.0`. The user's worked example
  — *"the person who failed a job … score is 0.9"* — is this term.
- **Side-settled** — a contract that carries a NIP-QW01 kind 9006 (side
  settlement) and **no** matching kind 9010 credit issuance keeps its
  `rating` and `completion` terms, contributes **no** `quant_weight` term
  (there is no amount), and has its whole `raw_signal` multiplied by
  `side_settled_factor`. This is the evidence class between a credit-backed
  contract and a commit-analysis tag (NIP-QW03). A contract with both a
  9006 and a 9010 is scored from the 9010 (credit-backed) and the conflict
  is surfaced.
- **Audit opinions** are folded in at `audit_weight`, but **only from an
  auditor the viewer themselves scores `> tolerance`** in the relevant
  domain, and weighted by that auditor's own score (`abstract.md`: "weight
  in proportion to their standing"). An opinion from an auditor the viewer
  does not trust is ignored, not inverted.

A subject's **domain score** is the recency-weighted mean of the
`raw_signal`s of their contracts *resolved into that domain* by §3, plus
the second-order term below.

### 2b. Second-order judgment concordance ("mutual of friends", MF2)

A direct contact `A`'s **domain score** is nudged by how well `A`'s
judgments of third parties I also have a read on line up with mine — a
friend who vouches for someone I think is bad has shown me something about
their judgment, and a neutral acquaintance who independently distrusts the
same person has too.

The term applies **only to a direct contact** `A` (I have my own contract
history with `A`), and draws **only on third parties `B` I distrust** in
this domain — `my_domain_score(B, d) < 1.0`. For each such `B` that `A`
has also expressed a domain-`d` sentiment about:

```
my_distrust(B)  = 1 − my_domain_score(B, d)         // (0..1], how far below neutral
a_sentiment(B)  = +1  if A rated B favourably   (JobCompletion.rating ≥ 4 from A's
                                                  own completion, a favourable
                                                  review (NIP-QW14) or audit opinion
                                                  (NIP-QW04) A signed)
                  −1  if A rated B unfavourably (rating ≤ 2, unfavourable review /
                                                  opinion)
                   0  otherwise                  → B contributes nothing
concordance(B)  = −a_sentiment(B)                   // A liked whom I distrust → −1
                                                    // A distrusted whom I distrust → +1
dist(B)         = min hops me → B in the verified graph (≥ 1)

mf2(A, d) = clamp(
    mf2_weight * Σ_B [ concordance(B) * my_distrust(B) * hop_decay^(dist(B) − 1) ],
    −0.3, +0.3
)
domain_score(A, d) += mf2(A, d)
```

- `A`'s rating of `B` is read from **`A`'s own** completion / review /
  opinion, never `B`'s — the same guard as `counterparty_rating` above.
- The effect shrinks with `B`'s distance (`hop_decay`, shared with §4) and
  grows with how strongly I distrust `B` (`my_distrust`).
- It is a *nudge*: `mf2_weight` defaults small and the whole term is
  clamped to `±0.3`, so it can shade a score, not flip it.
- The mirror case — `A` disagreeing with me about someone I **trust** — is
  deliberately left out: rewarding "A agrees with my *likes*" invites a
  mutual-admiration loop, and the design only ever lets a second-order
  signal *discount*, never inflate on agreement alone (cf. §4 `reverse`).

**Worked example 1 (penalty).** I did a `backend` contract with `A` and
rated it well. `A` then did a `backend` contract with `B` and rated `B`
5/5. But I distrust `B` for `backend` — `my_domain_score(B, backend) =
0.6`, `B` is 2 hops out. With `mf2_weight = 0.1`, `hop_decay = 0.5`:
`concordance = −1`, `my_distrust = 0.4`, so `mf2(A) = 0.1 × (−1) × 0.4 ×
0.5 = −0.02`. `A`'s backend score drops by `0.02` — `A` liked someone I
don't rate, and it costs `A` a little.

**Worked example 2 (reward).** I have a `backend` contract with `A` and
rate it neutrally (`domain_score(A, backend) ≈ 1.0`). `A` did a contract
with `B` and rated `B` 1/5. I also distrust `B` (`0.6`, 1 hop). `mf2(A) =
0.1 × (+1) × 0.4 × 1.0 = +0.04`. `A`'s neutral score rises to `1.04` — `A`
independently reached the same read on `B` that I did.

## 3. Scope resolution (the taxonomy inheritance the user asked for)

Tags are `sector/domain[/area]#skill`. `crate::events::tag_domain` already
reduces a tag to `(sector, domain)`; this NIP layers a **configurable
inheritance factor** on top instead of the current binary `same_domain`.

When scoring for target domain `T` and a contract carries tag `C`:

| relation of `C` to `T` | factor applied to that contract's signal |
|---|---|
| exact (`same domain and area`) | `1.0` |
| `C` is in a **child** area of `T`'s domain | `scope_inheritance.parent_of` |
| `C` is in the **parent** domain, `T` is a child area | `scope_inheritance.child_of` |
| `C` and `T` are **sibling areas** of one domain | `scope_inheritance.sibling` |
| different sector | `scope_inheritance.cross_sector` |

**Worked example (user):** *"the person I trust (score > 1) on `java`
approved a job on `backend`; I configured `parent_of = 1.0`, so it's OK."*
`java` = `it/backend/languages#java`; the `backend` job resolves to
`(it, backend)`. `java`'s contract counts toward the `backend` domain
score at factor `1.0`. With the default `parent_of = 0.5` it would count
half.

## 4. Path propagation

To score a target `X` the viewer does not have a direct contract with,
walk verified `CreditIssuance` edges (the existing
`qw_protocol::trust::find_trust_path` graph), domain-filtered per §3, out
to `path.max_hops`.

**Per-path score.** For a path `me → A → … → X`, start from `X`'s own
domain score (§2, computed from `X`'s contracts as the closing party) and
fold in each intermediary:

```
adjusted = endpoint_domain_score
for each intermediary I on the path (nearest-last):
    t = my_domain_score(I)                       // how much I trust the voucher
    if reverse == 0:
        adjusted *= min(1.0, t) * hop_decay      // classic: a weak voucher can only lower it
    else:
        adjusted  = (1 - t) * reverse + adjusted // "contrarian": a voucher I distrust
                                                 //   discounts their signal instead of trusting it
```

**Worked example (user):** endpoint raw from a failed job `= 0.9`; the
single intermediary's score for backend `= 0.8` (below my backend
tolerance — I don't trust their backend judgement); `reverse = 0.95`.
`adjusted = (1 − 0.8) × 0.95 + 0.9 = 0.19 + 0.9 = 1.09`. The distrusted
voucher's negative relay is *discounted*, nudging the endpoint back above
neutral. `reverse` is off by default and is a deliberately unusual knob —
"an auditor whose judgement I rate poorly saying 'bad' is weak evidence of
'bad'."

**Multi-path aggregation** (`abstract.md` / FAQ §6: *"several independent
paths to the same contributor is a stronger signal than one"*). Collect
every node-disjoint path within `max_hops`; combine per `path.multipath`:

| mode | combine |
|---|---|
| `max` | strongest single path (today's behaviour) |
| `mean` | average of the per-path adjusted scores |
| `sum-capped` | `1 + Σ(path_score − 1)`, capped at a configured ceiling — independent corroboration compounds, but bounded |

## 5. Overall person-to-person reputation

```
overall(X) = weighted_mean( domain_score(X, d) for d in domains X has evidence in )
           + balance.net_position_weight * squash( net_position_with(me, X) )
```

`net_position_with` is `Σ(X delivered to me) − Σ(I issued to X)` over
verified `CreditIssuance` (`abstract.md` §"Balance Mechanics", already in
`qw_protocol::trust::net_position_with`). `squash` keeps a large balance
from dominating a good work record. This term is what the user means by
*"the balance of jobs provided vs jobs consumed makes the overall
person-to-person reputation score."*

`overall` is still **per-viewer** and still **never published** — it is
recomputed on demand from held events, exactly like `net_position`.

## 6. Where offers and contracts depend on score

### An offer is an invitation to discuss

A `JobOffer` (NIP-QW01, kind 9000) is **not a commitment** — it is the
opening move of a negotiation. A counteroffer supersedes its terms and
hands it back; only the worker's signed Accept ends the exchange; a
negotiation that never reaches Accept leaves nothing in either history
(FAQ §4). So "sending an offer" is closer to "asking to talk about a
contract" than to "making a binding bid."

Because an offer costs the sender only a signature, the score is what
keeps unwanted ones from ever surfacing:

### Admission pre-filter (inbound)

Two local filters, per `abstract.md` §"Basic Use Cases" / FAQ §5,
evaluated **before a human sees the offer**:

- **Minimum reputation** — the sender's `domain_score` (§2–§4) for the
  offer's skill area is compared against that domain's `tolerance` (§1).
  `unknown-risk` fails. The user's *"tolerance 0.8 for frontend, 1.1 for
  backend"* is exactly `per_domain[d].tolerance`.
- **Position limit** — `|net_position_with(me, sender)|` against a ceiling
  that **scales with the sender's recently completed work**
  (`counterparty_recent_volume`).

A declined offer returns **no reason** (thresholds stay private so they
cannot be probed and tuned around). The recipient may always override and
admit it.

### Visibility & broadcasting (outbound, about you)

`abstract.md`: *"an unknown-risk fresh key simply falls below more and
more counterparties' filters"* — score gates how far you carry:

| surface | score gate |
|---|---|
| **Referral query reaches you** (NIP-QW06, kind 9050) | a relay forwards toward you only if you clear its `accept_depth` **and** the relay operator's own score for you in the query's domain ≥ their floor. Today only `accept_depth` / `categories` / `rate_limit` exist (`ContactPolicy`); a per-domain score floor is the addition. |
| **You appear in a referral answer** (NIP-QW06, kind 9051) | the requester ranks answers by *their own* `domain_score` of each responder (§4 multi-path); low / unknown-risk responders sort last or are dropped by the requester's `tolerance`. |
| **Your bulletin listing is surfaced** (NIP-QW11, kind 9091) | a board or a browsing client MAY hide listings whose poster the viewer scores below `tolerance` for the listing's domain — the "public gateway" layer still filters per-viewer, it just does it at browse time. |
| **A coordination server includes you** in a chain-calculation result (NIP-QW10, kind 9090) | the server returns the path + its own `score`; the client re-derives §4 locally and applies its own `tolerance` — the server never gates, it only computes. |
| **Your broadcast is relayed onward** (NIP-QW14, kind 9100) | a hop forwards only if the folded chain of signed hop ratings (kind 9101) for you in the payload's domain clears that hop's `min_hop_score` for the message type. This is the "per-domain score floor on relay-forward" above, spelled per type; the hop rating is cached and re-signed only every `hop_rating_ttl` days. |

None of this is a protocol-level block. The record is always
verifiable by anyone who has it; score only decides **whose client
bothers to relay, rank, or show it.**

## 7. Current implementation vs. this spec

`qw_protocol::trust` today (`find_trust_path`, `score_trust_path`,
`assess_reputation`, `evaluate_admission`) and `AdmissionPolicy`:

| aspect | current | this spec |
|---|---|---|
| output | raw Quant magnitude of the closing edge × `hop_decay^(hops−1)`, `0..∞`, no neutral point | `1.0`-centred multiplier; `unknown-risk` is a distinct non-number |
| per-contract signal | **credit amount only** | `(counterparty rating, pass/fail, quant size, recency, trusted-auditor opinion)` — `rating` and audit opinions do **not** feed the score today |
| settlement | credit issuance or nothing; a contract with no 9010 does not form a trust edge at all | a NIP-QW01 kind 9006 side settlement makes the contract count without a 9010, scored at `side_settled_factor` with no quant term |
| domain match | binary `same_domain` (sector+domain prefix) | configurable inheritance matrix (`parent_of` / `child_of` / `sibling` / `cross_sector`) |
| per-domain config | none — one global `min_reputation` in `AdmissionPolicy` | `tolerance` + weights per `(sector/domain)` |
| paths | **single shortest** path | all node-disjoint paths, `multipath` = `max` \| `mean` \| `sum-capped` |
| intermediary effect | none — the score is the closing edge, decayed by hop count | each voucher's own score modulates the endpoint; optional `reverse` "contrarian" transform |
| second-order concordance (MF2) | none — a contact's opinion of a third party never touches my score of the contact | §2b: a direct contact's judgment of someone I distrust nudges their domain score, `±0.3` clamp, `mf2_weight` |
| balance term | separate `net_position` display, not folded into the score | `net_position` folded into `overall` at `net_position_weight` |
| config surface | `AdmissionPolicy { min_reputation, position_limit }` (both `Option<f64>`), persisted in `SyncState` | full `ReputationConfig`, per-domain, persisted the same way |
| visibility / broadcast | `ContactPolicy` (`relay_depth` / `accept_depth` / `categories` / `rate_limit` / `share_tags`); **no score floor** anywhere in routing, ranking, or bulletin browsing | per-domain score floors on relay-forward, answer-rank, and bulletin surfacing |
| audit → score | `NegotiationView.disputes` shows annotations; **no effect on any score** | trusted-auditor opinions move `raw_signal` at `audit_weight` |

**Nothing here changes a wire format.** Every input already exists as a
signed record; this is entirely client computation and client config.

## 8. Staging (smallest useful first)

1. **`1.0`-centred score + rating in the signal.** Re-base
   `score_trust_path` around a neutral point and fold in
   `JobCompletion.rating` and pass/fail. Keep single-path, `hop_decay`.
   This alone makes `min_reputation` mean something a person can reason
   about (the Filter tab already exposes it).
2. **`ReputationConfig` + per-domain `tolerance`.** Replace the single
   `min_reputation` with a per-`(sector/domain)` map; persist in
   `SyncState` beside `AdmissionPolicy`; a config screen in the client.
3. **Scope inheritance.** The `parent_of` / `child_of` factors in
   `verified_edges` / the per-contract resolution.
4. **Multi-path aggregation.** Enumerate disjoint paths; `multipath` mode.
5. **Audit opinions into the signal**, gated by the viewer's own score of
   the auditor.
6. **Score floors in routing / ranking / bulletin browsing** — the
   visibility half of §6, once the score is stable enough to gate on.
7. **MF2 (§2b)** — the second-order concordance nudge. Needs (5)'s
   opinion/review inputs and a stable per-domain read on third parties to
   draw on; opt-in via `mf2_weight`, clamped.
8. **`reverse` / contrarian transform** — last, opt-in, for the
   participant who explicitly wants it.

Until (1) ships, a client is at the defaults, which are defined to be
exactly today's behaviour.
