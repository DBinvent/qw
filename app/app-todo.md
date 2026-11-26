# The client, against the protocol and the architecture

A map, not a backlog. Every gap below is tracked as an item in
`../todo-impl.md` — this exists to show *where* the client stands relative to
what the protocol already supports, which a linear plan cannot show. Nothing
here restates an item; it points at one.

Written 2026-08-26, against 157 passing tests.

**The finding: the protocol is far ahead of the client, and the architecture
is behind both.** 13 event kinds are specified across 12 NIPs and
implemented; the app exposes three commands and can sign one of the 13.
Almost nothing missing from the client is missing because it is unsolved.

---

## What the client can produce

Of 13 signable kinds, **one**: the introduction, via `follow`.

| Kind | | Client | Tracked as |
|---|---|---|---|
| 9060 | Introduction | **yes** | — |
| 10020 | Profile / skill tags (replaceable) | **yes** (`set_profile`) | — |
| 9000 | Job offer | no | §7 *OS deep links, external-signer delegation, and UI past identity/follow/sync* |
| 9004 | Counteroffer | no | ″ |
| 9001 | Accept | no | ″ |
| 9002 | Milestone | no | ″ |
| 9003 | Completion | no | ″ |
| 9010 | Credit issuance | no | ″ |
| 9050/9051 | Referral query / answer | **yes** (`Session::find_by_skill` + `sync_now` relay, 2026-09-05; the 9051 carries the responder's signed `10020` so a non-contact match is viewable, 2026-09-06) | — |
| 9030 | Dispute annotation | **yes** (`Session::annotate` — reply / audit request / audit opinion; `NegotiationView.disputes` + `under_review`, 2026-09-06) | — |
| 9040 | Cascade block flag | no | ″ |
| 9070/9071 | History request / response | no | ″ |
| 9080/9081 | Recovery policy / amendment | no | §7 *Routine device-key changes* |
| 9091 | Bulletin listing | no | §8 |

The §7 item covering most of that row block is one line — *"no contract
composition, no referral query, no trust display"* — carrying eight event
kinds and the entire job lifecycle behind it. That is the single largest
undersized item in the plan, and the reason this table exists.

## What the client can display

| Available in `qw-protocol` | Reachable from the app |
|---|---|
| `Contract::from_events` — full state machine over held events | via `negotiations` (head terms + state); no milestone/completion view yet |
| `find_trust_path`, `score_trust_path`, `net_position` | **yes** (`Session::{trust, net_position}`, 2026-09-05) — badged on contacts + negotiation counterparties |
| `evaluate_admission` — the offer-time gate | **yes** (`Session::{admission_policy, set_admission_policy}`, 2026-09-07) — a **Filter** tab configures min-reputation (the `assess_reputation` verified-credit-path score) and a position-limit multiplier on `counterparty_recent_volume`; `NegotiationView.passes_filter` flags a failing inbound proposal (shown, not hidden) |
| `pub_window` — offer-time liveness signal §2 requires | no |
| `earned_skills` — tags, contract counts, counterparty ratings | **yes**, badged |
| A stranger's `10020` profile — found via referral, no direct edge | **yes** (`Session::profile_of`; the 9051 answer carries it, 2026-09-06) — name + all declared tags on a `find_by_skill` result, not just the matched one |
| `skill_levels` / `skill_sources` / `links` on `10020` | **yes** (2026-09-08) — self-assessed level per skill, `self`/`commit-analysis` provenance, and a flat external-link list; `SkillView.{group, level, evidence}` drives grouped rows and a 3-state `unproven`/`algorithmic`/`approved_by_job` icon |

`Contract::from_events` takes a `&[Event]`; `EventStore::events()` is exactly
that slice. A contract list became pure wiring the day the store landed.

## Architecture

Not visible from any one file, and each makes a public claim untrue of the
running system:

| Gap | Tracked as |
|---|---|
| No *second* `MailboxTransport` — `HttpMailbox` is the only one; if every coordination server is down the mailbox path stops (the NIP-QW12 `HttpLedger`, added 2026-09-05, is replica-to-replica) | §8 *"Optional" is not true of the client yet* |
| ~~`rank_servers` never called~~ — wired 2026-09-05 (`Session::rank_servers`, called from `main.rs` + `src-tauri`); real trust ranking waits on servers advertising a pubkey | ″ |
| ~~No `Node` runs in the client~~ — since 2026-09-05 `Session` holds one: contact book from held introductions, earned-tag refresh, referral query origination + relay. Left: the UI, and per-contact policy tuning | §7 *Earned-skill routing* |
| Key is the account, and cannot be viewed or backed up from the app | §7 *Key backup* |
| No OS integration: invite links do not open the app, `qw-signer:` unhandled | §7 *OS deep links* |
| `EventStore` unbounded, nothing prunes | §7 *A local event store* |
| Android only; iOS shelved, desktop never run | §7 *Google Play*, `qw-bo/review.md` §3 |

## Protocol

Open at this layer, and only these three (the profile kind moved to the
replaceable range with an author `revision`, 2026-09-05):

| Gap | Tracked as |
|---|---|
| Controller DID vs. device key are one key in `identity.rs` (the kind-9082 record + `recovery::device_authority` resolver exist as of 2026-09-05; nothing produces or verifies against them yet) | §2 |
| External identity links (GitHub, LinkedIn) — a flat `links` list landed 2026-09-08; the open part is the NIP-39 `["i", …]` proof shape with a checkable back-link | §2 |
| Reviewed skills — the broker-signed badge axis. Self-signed `skill_levels` / `skill_sources` landed 2026-09-08 (the VC claim vocabulary); a real issuer + `method`/`examined_at` still open | §2 |

Everything else the client needs already exists here, tested.

---

## Order

1. **Profile editing** (§7) — nobody can be found without it, so nobody gets
   a first contract, so nothing is ever proven. Load-bearing precisely for
   people who have nothing else.
2. **Key backup** (§7) — small, and the consequence of its absence is total.
3. **Contract composition** (§7) — closes the loop; the first countersigned
   completion is what makes any *proven* badge reachable from the app.
4. **Contract list** — wiring, once (3) exists.
5. **Run a `Node`, and wire `rank_servers`** (§7, §8) — makes referral
   queries live and stops the client depending on one server. Same change in
   spirit, worth doing together.
6. **Deep links and external signer** (§7).

None of it needs a protocol change. The replaceable-kind move (1 built on
it) landed 2026-09-05; (5) is where "no central server" stops being
aspirational.
