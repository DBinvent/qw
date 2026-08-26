# QW Implementation Plan

Source docs: `doc-link/abstract.md`, `doc-link/qw-design-faq.md`, `doc-link/qw-todo.md`
(symlinked to `../vkrinitsyn.github.io/qw`, not part of this repo).

Reviewed against source commit `6759eb1` ("qw: added usecases", 2026-08-07),
which added a "Basic Use Cases" section to `abstract.md` and matching Q&As to
`qw-design-faq.md`. New items from that section are marked **(added
2026-08-07)** below so it's clear which bullets predate the original plan.

This repo is currently empty (README + gitignore only). This plan turns the
design docs into a buildable sequence, starting from the docs' own
recommendation: prototype the referral-query mechanic first, because it's the
one piece that is platform-agnostic, solo-buildable in weeks, and is the
actual differentiator — everything else (VC schema, signing, trust scoring)
can be built underneath it incrementally.

---

## 0. Decisions to lock before/while coding

`qw-todo.md` leaves several questions open. Blocking on all of them before
writing code isn't necessary — most have a safe default that matches the
docs' own reasoning. Defaults below are **provisional**, chosen to unblock
implementation; revisit before any real-money-adjacent or multi-org rollout.

| # | Question | Default for implementation | Revisit when |
|---|---|---|---|
| 1–2 | Mixer vs. cascade block / pricing | **No mixer.** Already resolved in FAQ §5 — do not build one. | Only if a future requirement reintroduces unlinkability |
| 3 | Searchable directory architecture | MVP = local per-node index + greedy referral routing (FAQ "every node holds an index…"). No coordination-server shard in MVP. | When federated/global search demand is proven |
| 4 | Quant amount public vs. ranged | Default **ranged/bucketed** (e.g. log-scale buckets), full value opt-in per participant | If reputation-market data shows ranged buckets are too coarse to price work |
| 5 | Cascade block threshold / initiator | Default: any WoT member can *flag*; a block **auto-cascades** to accounts within relay-graph distance 1 of a flagged signer once ≥2 independent flaggers (distance-bounded, non-overlapping paths) confirm; beyond that, manual per-participant review | Before opening flagging to the public internet (spam-flag risk) |
| 6 | Cold start / network density | Launch wedge = **one existing open-source ecosystem** with contribution history already in commit logs (see §7) | After first cohort proves the referral loop |
| 7 | Dispute timeout default | **30 days** from counterparty signature timestamp before a contract flips to `unsigned/expired` (raised from the doc's 14-day suggestion for more offline/mobile tolerance) | Adjust from field data once contracts are flowing |
| 8 | Admission-filter defaults — min-reputation threshold and position-limit scaling (added 2026-08-07, see §5) | **No enforced default at launch.** Filters exist in the client but start unset (everything passes) until a participant configures them; the position limit's scaling formula against "counterparty's recent completed work" is left as a client-side implementation detail, not a protocol default | Once §5 produces a real per-viewer trust score to threshold against |

Also locked in from the FAQ (not open questions, just flagging as constraints
the implementation must respect):
- No blockchain, no token, **no Shadow Quant** — cut entirely, including from
  any future roadmap language in code/docs.
- No global reputation score, ever — only locally-computed, per-viewer trust.

---

## 1. Repo & stack scaffolding

- [x] Decide repo layout: monorepo (`/protocol`, `/node`, `/app`, `/server`) vs.
      separate repos per component. **Decided: monorepo.** Root `Cargo.toml`
      workspace, `/protocol` crate scaffolded; `/node`, `/app`, `/server` added
      as their sections start.
- [x] Pick language(s):
  - Protocol/schema + relay-facing logic: Rust (pairs with Tauri v2, and with
      `nostr-sdk`/`nostr-rs-relay` ecosystem) or TypeScript (`nostr-tools`) —
      pick one as the source of truth for event/VC schemas to avoid drift.
      **Decided: Rust** (`protocol/` crate, `qw-protocol`).
  - Client shell: **Tauri v2** (Rust core + web frontend) per the stack table
      in the FAQ.
- [x] Set up workspace, lint/format, CI skeleton (build + test on push).
      (`.github/workflows/ci.yml` — fmt check, clippy `-D warnings`, build,
      test, on push to `main` and on PRs.)
- [x] `.gitignore` already excludes `doc-link/` (symlink) — keep it that way;
      do not vendor the design docs into this repo.

---

## 2. Protocol layer: identity, events, credentials

This is the shared substrate everything else depends on.

- [x] **Identity**: implement `did:key` generation/resolution first (simplest,
      no network dependency); add `did:web` support once a stable relay/domain
      exists. Device key = controller DID's signing key.
      (`protocol/src/identity.rs` — secp256k1 keypair backs both the
      `did:key` and the Nostr x-only pubkey; `did:web` still open, per plan.)
- [x] **Custom Nostr event kinds** — define and document (as a NIP-shaped spec
      in `/protocol/nips/`) for:
  - Job offer / accept / milestone / completion (unsigned intermediate steps)
  - Countersigned credit issuance (the one step requiring atomic dual-sign)
  - Profile / skill tags
  - Dispute annotation (reply, audit request, audit opinion)
  - Cascade-block flag and block record

      (`protocol/nips/NIP-QW01..05-*.md` + `protocol/src/events/kinds.rs`,
      kinds 9000-9041.)
- [x] **Verifiable Credential schema**: issuer = counterparty, subject =
      worker, claim = `{hours, rate, ko, km, skill_tags, timestamp}`. Implement
      as W3C VC with SD-JWT selective disclosure (start with SD-JWT; defer
      BBS+ — heavier crypto, not needed for MVP field-level hiding).
      (`protocol/src/vc.rs` — custom minimal SD-JWT, not a JOSE-library
      integration; signed with the same BIP-340 Schnorr key as Nostr events
      rather than adding an ES256K stack. Swap in real JOSE if/when external
      wallet interop is needed.)
- [x] **Dual indexing**: publish every contract under both parties' pubkeys
      (two tagged events, cross-referencing each other's event ID) so "all
      records referencing A" is a plain relay tag filter — no self-report
      trust needed.
      (`protocol/src/dual_index.rs`. Both events anchor to the same prior
      event id rather than embedding each other's id directly — an event id
      is a hash of its own fields, so it can't reference a sibling that
      doesn't exist yet.)
- [x] **Publication window enforcement**: client-side check at offer time —
      "counterparty's last signature was published within window T (default:
      24h)" — surfaced as a plain pass/fail signal, not a manual audit step.
      (`protocol/src/pub_window.rs`.)
- [x] Unit tests: round-trip sign/verify, dual-index query returns both
      records, tampered/omitted record is detectable via union query.
      (24 tests across the crate, `cargo test -p qw-protocol`; `cargo clippy`
      and `cargo fmt --check` clean.)
- [x] **Additional event kinds from "Basic Use Cases" (added 2026-08-07)**:
  - **Counteroffer** — repeatable, either party; supersedes the terms of
    the offer it references and hands the proposal back; only a signed
    Accept ends the exchange. Kind 9004, reuses `JobOffer`'s content shape
    (`protocol/nips/NIP-QW01-job-lifecycle.md`). Slots into §4's flow.
  - **Introduction** — a signed, attributable act of introducing yourself
    or two of your own contacts to each other; accepting one adds an edge
    to the recipient's contact graph (acquaintance, not competence). Kind
    9060 (`protocol/nips/NIP-QW07-introduction.md`). This is the protocol
    mechanism for building a contact list — §3's `node/src/contact.rs`
    still only constructs `Contact`s directly in tests/demos; wiring
    accepted introductions into it is unbuilt follow-up, not this bullet.
  - **History request / response** — request and receive a signed,
    filtered pointer into a contact's work history, scoped by skill tag
    and time window. Kinds 9070/9071
    (`protocol/nips/NIP-QW08-history-request.md`) — a signed list of
    already-dual-indexed record ids, not a re-attestation; the requester
    independently verifies each one, consistent with `crate::vc` staying
    the single-job-claim format rather than being extended for this.
  - **Person record amendment** (controller key rotation/recovery) — a
    quorum of the account's trusted contacts countersigns a replacement
    controller key as continuation of the same account, revoking the
    prior key from a stated, **non-retroactive** timestamp. Kinds
    9080/9081 (recovery policy / amendment,
    `protocol/nips/NIP-QW09-person-record-amendment.md`) plus
    `protocol/src/recovery.rs` for quorum-signature verification and
    linear-chain resolution (`verify_amendment`,
    `latest_valid_controller`). Deliberately does **not** resolve
    competing/conflicting amendments — left subjective, per module docs.
    See §7 for the recovery flow this backs.
      (All four: `protocol/src/events/kinds.rs`, kinds 9004/9060/9070-71/
      9080-81; 12 new tests across `events::kinds` and `recovery` (39 total
      in the crate, up from 27), all passing; `cargo clippy`/`cargo fmt
      --check` clean.)
- [ ] **Controller DID vs. device key hierarchy**: `protocol/src/identity.rs`
      currently treats them as one and the same key (1:1) — a deliberate
      MVP simplification matching the plan's original "device key =
      controller DID's signing key" wording. The FAQ's key-loss answer
      (added 2026-08-07) clarifies the intended design has device keys
      added/removed *beneath* a stable controller DID, with only the
      controller itself ever needing the quorum amendment above — routine
      device swaps (new phone) shouldn't require quorum sign-off. NIP-QW09
      introduces `account_id` (the genesis controller pubkey) as the
      permanent anchor amendments chain from, which is the piece that was
      actually blocking this — device-key delegation itself is still
      unbuilt. Revisit before §7's multi-device support lands.

---

## 3. Referral-query prototype (first working milestone)

Per `qw-todo.md` recommendation — build this before the full job lifecycle,
since it's demoable standalone and is the differentiator.

- [x] Minimal Nostr relay (self-hosted, or existing open relay for dev) + one
      custom event kind for "skill query" and "skill answer".
      Event kinds done (`KIND_SKILL_QUERY`/`KIND_SKILL_ANSWER` = 9050/9051,
      `protocol/nips/NIP-QW06-referral-query.md`). The relay itself is not
      built — `node/src/network.rs` delivers events between `Node`s
      in-process instead, standing in for "whatever relay the node's
      contacts are reachable through" for this milestone's demo. Wiring to
      a real (self-hosted or existing) relay is still open, needed before
      this runs across separate machines.
- [x] Per-contact relay policy: `relay_depth`, `accept_depth`, `categories`,
      `rate_limit`, `share_tags` — stored locally, not published.
      (`node/src/contact.rs`. The FAQ's table is a summary, not a spec — one
      consistent reading of `relay_depth`/`accept_depth` as outbound/inbound
      hop-depth caps is documented in NIP-QW06 and in the module itself.)
- [x] **Greedy/small-world routing**: each node caches direct contacts' skill
      tags; forward a query only to tag-similar contacts, not flood. This is
      called out in the FAQ as "the single most important change to the
      referral design" — implement it from the start, not flooding-then-fix.
      (`node/src/routing.rs` — exact-tag then same-domain ranking, fanout
      capped at 3 per the FAQ's "fanout 2-3 instead of 50.")
- [x] TTL-bounded propagation (default depth 3) with path-vouching: each hop
      attaches its own signature to the forwarded query so the receiver sees
      "2 hops via Anna" rather than an anonymous ping.
      (`node/src/node.rs` + kind 9050's per-hop signing. Identity is
      revealed only to hop 1, matching the FAQ's "who sees the query"
      answer — see NIP-QW06 for how the chain avoids exposing the
      requester past that point.)
- [x] CLI or minimal UI to fire a query, watch it propagate across ≥3 local
      test nodes, and collect deduped-by-pubkey responses with path count.
      (`node/examples/referral_demo.rs`, `cargo run -p qw-node --example
      referral_demo -- [n] [skill_tag] [max_hops]`. Path *count* — multiple
      independent paths reinforcing one responder — is not implemented;
      current dedup is first-arrival-wins per node, documented as follow-up
      work in NIP-QW06's scope note.)
- [x] Demo target: a query for a skill tag reaches a match in ~log(N) hops
      across a simulated contact graph of a few hundred synthetic nodes.
      Works on a 300+-node synthetic Watts-Strogatz-style graph
      (`node/src/graph.rs`); not run over many trials or measured
      statistically — a working demonstration, not a benchmark result.
- [ ] **Reputation-aware querying** ("Find a contributor by skill and
      reputation", added 2026-08-07): query by skill tag *and* a
      reputation threshold, results ranked by the querier's own trust
      computation, with matches reached via multiple independent paths
      aggregated as a stronger signal than a single path. Blocked on §5's
      per-viewer scoring existing to threshold/rank against. This is the
      same multi-path signal NIP-QW06 already flagged as a documented
      follow-up (current prototype dedups to one, first-arrival path per
      responder) — now backed by a named use case instead of just a
      footnote, but still sequenced after §5.

---

## 4. Job lifecycle & signing

- [x] Implement the flow: Offer (client-signed) → **Counteroffer**
      (repeatable, either party — added 2026-08-07; neither accepts nor
      rejects, it supersedes the referenced offer's terms and hands the
      proposal back; only a signed Accept ends the exchange) → Accept
      (worker-signed) → Milestones (optional, either party) → Completion
      (each party signs separately) → Credit issuance (atomic countersign).
      (`protocol/src/contract.rs::Contract::from_events` — walks the
      negotiation chain via `negotiation_head`, resolves accept/milestones/
      completion/credit-issuance/disputes into one `ContractState`.)
- [x] Negotiation history (added 2026-08-07): each Offer/Counteroffer
      version is signed by whoever proposed it, so the negotiation itself
      is available as evidence later (e.g. for an auditor). But no version
      prior to the accepted one carries any obligation, and a negotiation
      that never reaches Accept leaves nothing in either party's permanent
      history.
      **Resolved**: un-accepted offers/counteroffers are dual-indexed like
      any other kind (their `p` tag already makes them so) — nothing
      special-cases them out. `net_position` simply never reads them: it's
      computed strictly from `CreditIssuance` events, which an un-accepted
      negotiation never produces. See NIP-QW01.
- [x] Request-review step (added 2026-08-07, "Commit a contract"): either
      party may request review of a completed job or a delivered
      milestone, with optional feedback, before the countersigned
      Completion. Distinct from §2/§6's after-the-fact dispute annotations,
      which apply to already-signed records — this is a pre-signature
      negotiation step, closer in spirit to Counteroffer than to a dispute.
      (Kind 9005, `job_review_request` in `protocol/src/events/kinds.rs`.)
- [x] Only the final step requires atomicity — implement as a two-phase
      exchange (both sign the same payload hash; either party can publish
      once both signatures are collected) rather than needing a coordination
      service.
      (`protocol/src/contract.rs::sign_credit_issuance_payload` +
      `assemble_credit_issuance` + `verify_credit_issuance`. Caught and
      fixed a real gap while testing: the contract-state view was only
      checking `payload_hash` internal consistency, which a forged
      `CreditIssuance` event could satisfy trivially — it now runs full
      dual-signature verification against the contract's actual
      issuer/subject pubkeys before reporting `CreditIssued`.)
- [x] Offline tolerance: every step except final countersign must survive one
      party being offline indefinitely (mobile reality) — write this as an
      explicit test (kill network between steps, resume later).
      (`protocol/src/contract.rs` test
      `offline_tolerance_every_step_composes_from_purely_local_data` —
      steps built with month-wide gaps between `created_at` timestamps,
      each still verifies independently; no step needs the counterparty or
      a network reachable at the moment it's created.)
- [x] Wire in the dispute annotations from §2 (reply / audit request / audit
      opinion) as attachable-after-the-fact records, never mutating the
      original signed contract.
      (`Contract::disputes` — aggregates any `KIND_DISPUTE_ANNOTATION`
      referencing the offer, negotiation head, milestones, or either
      party's completion.)
- [x] Implement `unsigned/expired` terminal state at the 30-day default
      timeout (§0.7).
      (`ContractState::Expired` — covers both a stale negotiation with no
      Accept and a one-sided completion nobody countersigned, each timed
      from the relevant event's own `created_at`.)

---

## 5. Trust graph & net_position

- [x] Local graph walk: given a target pubkey, traverse signed contract
      records outward from "self" up to N hops, domain-filtered by skill tag.
      No global index — this runs against relays the node already has access
      to (own + friends').
      (`protocol/src/trust.rs::find_trust_path` — BFS over *verified*
      `CreditIssuance` edges only, undirected for reachability; domain
      filter resolves `credit_issuance → completion → offer → skill_tags`
      and excludes an edge whose domain can't be confirmed rather than
      assuming a match. Only ever reads the `events` slice the caller
      already has — no fetch, no assumption of a complete graph.)
- [x] Per-viewer subjective scoring: configurable weights/tolerances, not a
      shared algorithm output — implement as a pluggable scoring function
      over the same traversal result, so two nodes can disagree.
      (`ScoringWeights` + `score_trust_path`. Caught a real design bug
      while testing: an early version summed *every* edge along the path,
      which let a long path of unrelated transactions outscore a short,
      direct one — the opposite of "decay with distance." Fixed to score
      only the closing edge's value, discounted by `hop_decay^(hops-1)`;
      earlier edges affect the score only via that exponent.)
- [ ] **Calculator profile** (added 2026-08-10, from conversation): attach
      an explicit, referenceable "who computed this and under what
      parameters" profile to a computed score, not just the raw number
      `ScoringWeights`/`score_trust_path` produce today. A profile would
      record at minimum (viewer pubkey, weights used, timestamp), signed
      by whoever ran the calculation (self, or the broker from §8) — so a
      score can be:
      - **Compared**: two scores are only meaningfully comparable if the
        reader knows they came from the same (or an equivalent) profile —
        `ScoringWeights` today is purely local/ephemeral, nothing external
        can tell one score's recipe from another's.
      - **Transponded**: forwarded to a third party who wasn't the
        original viewer, who can then judge how much to trust the score
        *given* who calculated it and for whom, instead of treating it as
        an opaque, context-free number.
      Same tension as the broker-signed-score item in §8, and needs the
      same decision alongside it: a shareable calculator profile is
      exactly what a portable/transponded score needs, but standardized,
      reused profiles are also exactly what could turn "locally-computed,
      per-viewer trust" (§0) into a de facto global score. Resolve both
      together, not independently.
- [x] `net_position` as a pure derived query: `Σ(delivered) − Σ(issued)` from
      dual-indexed public records — no separate balance store, ever.
      (`net_position` / `net_position_with` (bilateral) — only counts
      `CreditIssuance` events that pass `contract::verify_credit_issuance`
      against the pubkeys the event itself claims, not a bare `p`-tag
      match, for the same reason §4's `derive_state` needed the same fix.)
- [x] New-account handling: unknown-risk (not neutral/zero) — encode as a
      distinct UI/scoring state, not "trust score 0".
      (`ReputationState::UnknownRisk` vs `Scored(f64)`, produced by
      `assess_reputation`; unreachable-within-`max_hops` is `None`/
      `UnknownRisk`, never a `Scored(0.0)`.)
- [x] **Admission filters** ("Basic Use Cases" / FAQ "What stops unwanted
      requests from reaching a participant at all?", added 2026-08-07): two
      local, client-side pre-filters applied before an inbound request
      (introduction, job offer, referral query) ever surfaces to a human —
      minimum reputation for the relevant domain, and a position-limit
      (exposure ceiling) on Quants given vs. taken with that counterparty,
      scaling with how much work that counterparty has recently completed.
      Both are private, per-participant config, never protocol-mandated; a
      declined request returns no reason, so thresholds can't be probed and
      tuned around. Reuses `net_position` and the per-viewer scoring
      function above as a pre-request gate, not just a display value —
      "the same market-pricing mechanism as record acceptance, moved
      earlier in the lifecycle." Default thresholds: open, see §0.8.
      (`AdmissionPolicy` / `AdmissionDecision` / `evaluate_admission`, plus
      `counterparty_recent_volume` as the building block for a client's own
      scaling formula — the formula itself stays unimplemented, per §0.8.)

(All of §5: `protocol/src/trust.rs`, 11 tests, plus one `QuantAmount::approx_value`
helper and a `same_domain`/`tag_domain` refactor — moved from `qw_node::routing`
into `protocol::events` since `protocol::trust` needed the same taxonomy-domain
matching and `protocol` is the lower crate; `qw_node::routing` now re-exports it.
79 tests total across the workspace, `cargo clippy`/`cargo fmt --check` clean.)

---

## 6. Sybil resistance: cascade block

"Submit an appeal" in the new "Basic Use Cases" section (added 2026-08-07)
is the same mechanism as the audit-request/audit-opinion dispute
annotations already speced in NIP-QW04 (§2) — confirms the existing
design, no new event kind needed.

`qw-design-faq.md` §5 gained a worked bot-farm example (added 2026-08-10,
from conversation, "Can a bot-farm operator just pay themselves a huge
balance?") — a million-bot farm can inflate its own internal balance for
free, but that balance is only redeemable inside the farm; borrowing
reputation against a real outside counterparty is what exposes a signer to
being flagged, and cascades to the whole farm in one shot. Same conclusion
as this section, illustrated concretely — confirms the existing design
again, no new mechanism needed.

- [x] Implement flagging (any participant, any signed record) and the
      default cascade rule from §0.5.
      (`protocol/src/cascade.rs::evaluate_flags` — flagging itself was
      already built in §2 (`KIND_CASCADE_BLOCK_FLAG`, NIP-QW05); this adds
      the decision logic: ≥2 distinct flaggers triggers a direct block,
      auto-cascading to distance-1 neighbors. One real design question
      resolved along the way — "relay-graph distance" can't mean a node's
      private `Contact` list (§3, never published), so cascade distance is
      measured over the *published* Introduction graph (NIP-QW07)
      instead, the only contact-adjacent graph that's actually public.
      "Independent flaggers" is simplified to distinct signer pubkeys —
      true non-overlapping-path verification is out of scope, same
      deferral NIP-QW06 already made for multi-path reinforcement.)
- [x] **Exclude public-link introductions from the cascade walk** (opened and
      closed 2026-08-25, by §10 dropping invite-only). Without it an ad
      campaign was a liability: `evaluate_flags` cascades to distance-1
      neighbours over the introduction graph, so two flags against any
      stranger who followed a published link would land on the publisher.
      (`events::kinds::VIA_PUBLIC_LINK` + `Introduction::via`,
      `Introduction::public_link()`, `is_public_link()`;
      `cascade::introduction_adjacency` skips those edges, which drops them
      from the BFS entirely — so a public-link edge is not a *bridge*
      either, not just an excluded endpoint. Three tests:
      a public-link neighbour is not cascaded while a vouched one still is,
      a cascade does not travel *through* such an edge at distance 2, and
      the field round-trips while an ordinary introduction still serializes
      without it — `via` is `skip_serializing_if`, so existing events keep
      their ids and older clients read absent-as-vouched. Both cascade tests
      were mutation-checked: removing the skip fails them.)
      Spec: NIP-QW07 "Public self-introduction", NIP-QW05's distance
      paragraph.
- [x] Cascade propagation: when a node's local policy accepts a block signal,
      it locally blocks and *re-publishes* the flag with its own vouch —
      implement as a signed "I also block X, sourced from Y" event, so
      cascade is social propagation, not a central blocklist.
      (`BlockReason::Vouched` — a `CascadeBlockRecord` already visible in
      `events` is itself adopted as grounds to (re-)block, so a chain of
      published vouches *is* the propagation; `evaluate_flags` decides,
      the caller publishes via the existing `cascade_block_record`
      builder with whichever evidence event it chose.)
- [x] Test: synthetic bot-farm graph behind a small number of real signers —
      confirm blocking the real signers collapses reachability to the whole
      farm without a central authority needing to enumerate every bot.
      (`bot_farm_reachability_collapses_by_blocking_the_real_signers` — 2
      signers each introduced to 10 sockpuppets; flagging just the 2
      signers cascades to block all 22 accounts, none of the 20 bots
      individually flagged.)

(All of §6: `protocol/src/cascade.rs`, 7 tests. 86 tests total across the
workspace, `cargo clippy`/`cargo fmt --check` clean.)

---

## 7. Mobile client (Tauri v2)

No Node/npm, `cargo-tauri`, or Android/iOS SDKs are available in the
implementation environment — the actual Tauri app, external-signer OS
integration (Android intents, iOS universal links), and any UI can't be
scaffolded, built, or tested here. What follows is scoped to what's
genuinely testable without that tooling: the signing *protocol* a thin
client and an external signer speak to each other, as plain Rust. The app
shell itself is still entirely unbuilt.

- [x] Thin-client signing model: phone holds key, signs payloads, syncs on
      relay wake — no local DHT/relay portion on mobile (matches the
      "zero-arc"/Signal-model reasoning in the FAQ).
      Already satisfied by the architecture as built, nothing new needed:
      no crate in this workspace runs a persistent relay or DHT anywhere —
      `qw_node::network::Network` is explicitly an in-memory stand-in for
      §3's demo/tests, not something intended to run on-device, and every
      protocol-layer function (`contract`, `trust`, `cascade`, ...) is a
      pure read over whatever events the caller already has, never a
      long-running service.
- [x] External signer integration (Amber-style intent/deep-link pattern on
      Android; equivalent flow for iOS/web) — do **not** store the identity
      key in browser storage or WebView local storage (Safari eviction +
      XSS exposure called out explicitly in the FAQ).
      **Protocol only** (`protocol/src/signer.rs`, 5 tests): `SignRequest`/
      `SignResponse` encode to/from a `qw-signer:` URI — one string usable
      as both a QR-code payload and a deep link, inspired by Amber/NIP-46
      but not wire-compatible with either. The requester-side function
      (`assemble_response`) takes no `Identity` and no key material at
      all — provably cannot need the private key to complete the
      exchange — and never trusts the signer blindly: the assembled event
      still has to pass `Event::verify` before being accepted. Actual
      Android intent / iOS universal link wiring is unbuilt (needs
      tooling this environment doesn't have).
- [x] **Mailbox sync logic** (added 2026-08-25, the client half of §8's
      store-and-forward cache): cursor tracking, publish-and-retry, and what
      a client is willing to believe from a server.
      (`node/src/sync.rs`, 10 tests. Transport-agnostic — the crate has no
      HTTP client and no async runtime, so a `MailboxTransport` trait is
      supplied by the app (`reqwest`) or a test (a `HashMap`), same split
      `network::Network` already draws. Three properties the tests pin, each
      mutation-checked by breaking the code and watching them fail:
      **everything delivered is verified locally** and events addressed to
      someone else are dropped — a hostile cache can withhold mail but not
      inject any; **the cursor only advances on a successful fetch**, so a
      failed poll re-asks the same window instead of stepping over it; and
      **`since` is inclusive with dedupe by event id**, because
      `since = cursor + 1` silently loses every event sharing that second.
      Publishing is per-event across ranked servers: one acceptance is
      delivery, a full mailbox falls through to the next server without
      being counted an error, and anything nobody took stays queued.)
- [x] **Client core + Tauri shell scaffold** (added 2026-08-25).
      Split along what can be built anywhere: `app/core` (`qw-client-core`,
      6 tests) holds the on-disk identity (`0600`, and a corrupt key file is
      an error rather than a silently fresh identity), the HTTP
      `MailboxTransport` mapping 201/200/507 onto Accepted/AlreadyHeld/
      MailboxFull, and `follow_invite` accepting every form a person pastes
      (full URL, bare npub, hex, with tracking params). Tested against a real
      axum server on a real socket, including a full mailbox keeping the
      event queued and an unreachable server being an error rather than a
      silently empty inbox.
      `app/src-tauri` is the window — three commands over that core — and is
      **not** a workspace member: Tauri cannot compile without webkit2gtk,
      so a member would break `cargo test --workspace` for anyone without
      those packages. **Never compiled here**, and `app/README.md` says so;
      `app/install.sh --desktop` installs what it needs.
      *(Updated 2026-08-25: with webkit2gtk + tauri-cli installed it does
      compile — `cargo build`/`clippy` clean. Still not run: needs a
      display.)*
      Still open in §7: OS deep links (clicking a `/i/<npub>` link does not
      open the app — it must be pasted), external-signer delegation, and any
      UI beyond identity/follow/sync.
- [ ] **Android build** — the toolchain, not the code. `[lib]` already emits
      `cdylib`/`staticlib` and `run()` carries `mobile_entry_point`, so the
      Rust side needs nothing; `cargo tauri android init` stops on a missing
      JDK (verified 2026-08-26, that exact error). `/tmp/install-android-
      deps.sh` installs JDK + SDK + NDK + the four rustup targets — JDK as
      root, SDK as the invoking user, since `android build` writes into the
      SDK. Open when someone runs it: whether `gen/android/` is committed or
      regenerated, and app signing (nothing is signed or published today).
      `app/ui/index.html` was made phone-shaped in advance (device-width
      viewport, safe-area insets, 44px targets, 16px inputs, clipboard
      fallback for a non-secure WebView) — unverified on a device.
- [x] **Public joining instructions** (added 2026-08-26): `knownby.work/join`
      (`landing/app/join/page.tsx`), linked from the "How to join" card on
      the home page, the nav, the footer and `/i/<npub>`. Four steps —
      install the app, first launch generates the key, open or post an
      invite link, publish skill tags — plus a per-platform status table and
      "what exists today". Written so the "no download yet" line comes
      *before* the steps: a reader who follows four steps and then finds
      there is nothing to install has been misled by omission.
- [ ] Web app path: compose/display only; signing delegated via QR or deep
      link to the external signer.
      The delegation protocol it would use is done (above); the actual web
      app — composing events, displaying them, rendering/scanning the
      QR — is unbuilt (needs a frontend, which needs Node/npm).
- [x] **Controller key recovery**, concretized by the FAQ's "What happens
      when a signing key is lost or compromised?" (added 2026-08-07):
      implements §2's person-record-amendment event kind — a quorum of the
      account's trusted contacts (size/membership set in advance by the
      account holder; no protocol-wide default) countersigns a replacement
      controller key as continuation of the same account, revoking the
      prior key from a stated timestamp. Revocation is **not** retroactive
      (signatures before the timestamp stay valid, so one lost phone
      doesn't evaporate every past contract); a signature under a revoked
      key afterward is surfaced as an **alert**, never silently dropped —
      it's the strongest available evidence the key is in hostile hands.
      Design and implement before any real usage, since key loss = identity
      loss with no fallback otherwise.
      Already fully built in §2 (`protocol/src/recovery.rs` +
      NIP-QW09) — nothing further needed here.
- [ ] Routine device-key changes (new phone) do **not** go through the
      quorum amendment above — under the controller/device-key hierarchy
      flagged in §2, device keys are added/removed beneath the controller
      directly. Amendment is only for the controller key itself.
      Still open — the device-key hierarchy itself isn't built (§2's
      `identity.rs` still treats controller and device as one key).

(§7 protocol pieces: `protocol/src/signer.rs`, 5 tests. 91 tests total
across the workspace, `cargo clippy`/`cargo fmt --check` clean. The app
shell, platform signer integration, and web UI remain entirely open,
blocked on tooling not present here.)

---

## 8. Optional coordination server

Build only after the peer-to-peer core works standalone — this is an
efficiency/monetization layer, not a dependency.

**Promoted to current priority alongside §7 (2026-08-25).** The precondition
above is satisfied: §1–§6 are done and the core composes offline already
(`contract.rs::offline_tolerance_every_step_composes_from_purely_local_data`
builds a full contract with month-wide gaps and no network). What is missing
is carriage — two mobile clients that are never awake at the same time have
nothing between them. That is message caching, not coordination, and the
distinction is what keeps this section honest:

- **Optional stays literal.** Every existing item here already obeys it —
  chain-calculation results are re-derivable by the client, vault only ever
  returns events that verify on their own, and `node/src/server_registry.rs`
  ranks *multiple* servers by ordinary trust score so none is hard-coded as
  authoritative. A cache that a client cannot do without would break the
  claim the landing page makes ("no central server"), so the client must
  still work — degraded, not broken — against direct relays alone.
- **Never the only copy.** Same rule as chain-calculation: a cached event is
  a convenience copy of something the author can re-publish, never the sole
  record. Losing the server loses latency, not history.

- [x] **Store-and-forward message cache** (the priority item): hold signed
      events addressed to a pubkey until that client next connects, then
      deliver and expire. Distinct from the vault below, which is *backup of
      your own records*; this is *delivery of someone else's message to you*.
      Reuses what exists rather than inventing: acceptance is
      `Event::verify()` (already the vault's only admission rule), addressing
      is the existing `p` tag, and retrieval is
      `qw_protocol::dual_index::all_records_about`. Open questions to settle
      before coding: retention window, whether a cache may hold events it
      cannot read (encrypted DMs — the NIP-QW06 requester's private ask to
      hop 1 is exactly such a message), and per-pubkey quota so one account
      cannot fill it.
      (`qw-bo/server/src/mailbox.rs`, 10 tests. The three open questions are
      settled in its module doc: **30-day retention**, matching §0.7's
      dispute timeout so a step cannot expire while its contract is live;
      **cursor, not acks** — `GET /mailbox?pubkey=&since=` returns and
      deletes nothing, because delete-on-read loses a message whenever a
      mobile fetch dies mid-flight and would need an auth scheme this server
      is forbidden to require; **per-recipient quota of 500, full rejects**
      rather than evicting, so a flooder cannot flush real mail out of
      someone's mailbox. Content is never parsed — a test pushes an opaque
      ciphertext through untouched. Admission is `Event::verify()` plus a `p`
      tag, nothing else.)

- [x] Chain-calculation service: traverses trust graph on request, returns a
      signed path + score, spot-checkable by the client against raw relay
      data (server must never be the only source of truth for a result it
      returns).
      (`server/src/chain_calculation.rs` + new kind 9090,
      `protocol/nips/NIP-QW10-chain-calculation-result.md`. The HTTP layer
      wraps `qw_protocol::trust::find_trust_path`/`score_trust_path` — all
      the actual graph logic already lived in and was tested by §5; this
      is a thin axum wrapper that signs the result as a normal NIP-QW10
      event, so the exact same `Event::verify()` a client already uses
      everywhere else applies here too, no server-specific trust
      mechanism needed.)
- [x] Rating-bureau service: filtered, re-signed history aggregation
      (subscription, priced in Quants).
      (`server/src/rating_bureau.rs` — reuses NIP-QW08's
      `HistoryRequest`/`HistoryResponse` shape exactly, server as
      responder instead of a peer. Billing/subscription itself
      (the "priced in Quants" part) is not implemented — that's business
      logic layered on top of this endpoint, not aggregation logic.)
- [x] Vault/neighbor storage: signed-record backup for participants without
      always-on nodes.
      (`server/src/vault.rs` — accepts a signed event only if it verifies,
      retrieves by pubkey via a new shared `qw_protocol::dual_index::
      all_records_about` helper, factored out once vault and rating-bureau
      both needed the same "self-authored ∪ referenced" query.)
- [ ] Community insurance pool: explicitly last — depends on transaction
      volume existing first to fund the pool meaningfully.
      Deliberately unbuilt, matching the doc's own sequencing.
- [ ] **Broker-signed score** (added 2026-08-10, from conversation, not yet
      in the source docs): a coordination-server operator ("broker")
      computes a score for a subject and signs it, so the subject can hold
      and present it as a portable attestation elsewhere — distinct from
      the chain-calculation service above, whose signed *path* the client
      independently re-derives/spot-checks against raw relay data (the
      server isn't trusted for the result, only for convenience). A
      broker-signed score as portable evidence implies the *recipient*
      trusts the broker's methodology instead of re-deriving it themselves
      — closer to a credit-bureau attestation than to chain-calculation.
      **Needs a decision before building**: §0 locks in "no global
      reputation score, ever — only locally-computed, per-viewer trust" —
      confirm this stays strictly per-requester/scoped (broker computes
      *for* a specific asking party for use only, like rating-bureau in
      this same section, not a single portable number the subject reuses
      everywhere, which is functionally close to a global score by another
      name) before adding it to the plan for real. Pairs with §5's
      **Calculator profile** item — a broker-signed score needs exactly
      that profile (who calculated it, for whom, under what weights)
      attached to be interpretable/comparable by whoever it's presented
      to; resolve the two together.
- [x] Multi-server support in the client from day one (routing to whichever
      server a node's own trust config ranks best/cheapest) — avoids
      hard-coding a single server as authoritative.
      (`node/src/server_registry.rs::rank_servers` — a server is scored
      the same way any participant is, via §5's `qw_protocol::trust`; an
      unknown-risk server always sorts after any scored one, ties broken
      by fee. Returns a ranking, never a single hard-coded pick.)
- [x] Offline bulletin board where users can advertise themselves and their
      services. Usage is a subject for limits and monetization.
      Clarified: "offline" means the two sides don't need to be online at
      the same time (async, store-and-forward via the server) — not
      literal no-network/sneakernet distribution. Maps directly onto
      `abstract.md`/FAQ §6's third discovery layer, "Public gateway —
      indexable signed job postings with stable URLs."
      (New kind 9091 `BulletinListing` — **undirected**, unlike
      `JobOffer`'s addressed-to-one-worker shape or
      `ProfileSkillTags`'s standing/unscoped shape — `offering`/`seeking`,
      skill tags, description, optional expiry
      (`protocol/nips/NIP-QW11-bulletin-listing.md`). `server/src/board.rs`:
      post a verified listing, browse/filter by domain-aware skill tag and
      listing type, excluding expired — the genuinely new capability
      here, since `vault` and `rating_bureau` both require already
      knowing whose records you want, and a board has to be browsable
      without that. Rate limits/monetization explicitly left to the
      operator, not fixed by the wire format, same treatment as the
      rating-bureau's subscription billing. 6 tests.)

(§8 buildable pieces: `server/` crate scaffolded (axum), 16 tests;
`node/src/server_registry.rs`, 4 tests; `protocol/src/dual_index.rs`'s new
`all_records_about` helper, 1 test; kinds 9090-9091 + NIP-QW10/QW11. 124
tests total across the workspace, `cargo clippy`/`cargo fmt --check`
clean.)

---

## 9. Legal / compliance track (parallel, non-blocking for prototype)

- [x] Rewrite any public-facing copy to state the co-authorship boundary
      explicitly (declared open-source project + no counterparty control) —
      do this before any external pitch materials reference the abstract.
      Scoped to this repo, not `doc-link`'s source docs (a separate,
      user-owned repo not touched here) — `abstract.md` itself already
      states the boundary explicitly (checked: the FAQ's flagged phrases
      "removes the barter classification" and "private with no profit"
      are no longer present there). `README.md` now states the same
      boundary for this repo directly, verbatim-matched
      (`qw_protocol::legal::CO_AUTHORSHIP_BOUNDARY_NOTICE`) so the two
      copies can't silently drift.
- [x] Do not build Shadow Quant or any cross-token/mirrored-ledger feature —
      keep it off the roadmap, not just unbuilt.
      Verified: grep across all `.rs`/`.md`/`.yaml`/`.toml` in this repo
      (excluding `doc-link`) turns up only this plan's own "do not build
      this" statements — nothing implements or roadmaps it. Also stated
      explicitly in the new `README.md`.
- [ ] Get a written tax attorney opinion before any investor data room or
      public launch beyond a closed test cohort.
      **Re-read this after 2026-08-25:** invite-only was dropped (§10), so
      "beyond a closed test cohort" no longer describes an optional later
      stage — there is no closed cohort at any point, and §10 is a public
      launch on day one. This item therefore gates §10 outright rather than
      gating a step after it.
      **Not something this repository can complete** — a human/business
      action (retaining and paying an attorney), not engineering work.
      Left unchecked deliberately; `README.md` and
      `qw_protocol::legal::CO_AUTHORSHIP_BOUNDARY_NOTICE` both say plainly
      that this hasn't happened yet, so the gap stays visible rather than
      silently assumed closed.
- [x] Deletion-rights handling: since Nostr deletion is advisory only,
      document this limitation for users explicitly at record-creation time;
      treat ATProto migration as the fallback if a jurisdiction requires
      enforced deletion, not as a v1 requirement.
      (`qw_protocol::legal::DELETION_RIGHTS_DISCLOSURE` + `README.md`,
      verbatim-matched. The disclosure *text* is ready for a future UI to
      show at record-creation time, per the requirement; actually wiring
      it into a "show before first signed record" UI flow depends on §7's
      still-unbuilt app shell.)

(§9 text/verification pieces: `protocol/src/legal.rs`, 2 tests, plus
`README.md`. 93 tests total across the workspace, `cargo clippy`/`cargo
fmt --check` clean. The one item that can't be checked off from inside
this repository — the tax attorney opinion — is called out rather than
left ambiguously unchecked.)

---

## 10. Launch wedge

**No invite-only stage (decided 2026-08-25).** The launch is open: anyone
who follows a published invite link is in. That removes the closed-cohort
gate entirely — from this section, from §9's sequencing, and from the way
the pilot is described anywhere else. What replaces it is distribution:

- [ ] **Public invite links as the entry point** — NIP-QW07's third shape.
      A participant publishes `https://knownby.work/i/<npub>` and puts it
      wherever their professional history already lives: LinkedIn posts and
      profiles first (that is where "who I worked with" is already the
      subject), then conference talks, email signatures, README badges, job
      ads. Following the link exchanges introductions and makes the follower
      a hop-1 contact — someone four hops out, or not connected at all,
      arrives as a direct contact instead of waiting for a chain of
      introductions that a cold network cannot produce.
      Needs: the `/i/<npub>` route on the landing site (deep-links to the
      client, falls back to install instructions), `via: "public-link"` on
      both generated 9060 events, and the cascade walk in
      `protocol/src/cascade.rs::evaluate_flags` taught to skip those edges —
      without that last part an ad campaign becomes a cascade-block
      liability, see NIP-QW05.
- [ ] Optionally seed density first with **one** open-source ecosystem where
      contribution history already exists in commit logs (per the docs'
      cold-start mitigation) — active repo(s) with multiple maintainers and
      existing informal reciprocity norms. This is now one channel among
      several rather than the gate: `bootstrap_from_git` turns its commit
      history into candidate skill tags and introductions, so a repo's
      contributors arrive with a graph instead of an empty profile.
      **Not something this repository can decide** — same category as §9's
      tax attorney opinion: it needs real knowledge of which communities
      have an actual willing cohort, which is the user's own call, not an
      engineering one. Left unchecked deliberately. The tooling below is
      ecosystem-agnostic and ready the moment a candidate is picked.
- [x] Bootstrap trust graph from real historical collaboration (e.g. seed
      initial skill tags / relationships from git co-authorship) rather than
      asking a cold network to self-report from zero.
      (`node/src/bootstrap.rs`, 4 tests + `node/examples/bootstrap_from_git.rs`.
      Produces *suggestions* only — candidate skill tags from file
      extensions touched, candidate `Introduction` pairs from
      `Co-authored-by:` trailers — never fabricates or publishes on
      anyone's behalf, since git identifies people by name/email, not a
      QW key, and this tool holds no one's signing key; a real
      contributor reviews and signs their own suggestions once they join.
      Smoke-tested against two real repos (this one, and the separate
      `vkrinitsyn.github.io` doc-source repo, read-only) — that surfaced
      and fixed a real bug: the same person had committed under two name
      spellings ("Vladimir Krinitsyn" vs "vkrinitsyn") with one email, and
      the tool was treating them as different contributors. Contributor
      identity is now by email only, case-insensitively.)
- [x] Success metric for the pilot: N signed contracts completed end-to-end
      (offer → countersigned credit) inside the cohort, not signups.
      (`protocol/src/pilot.rs::completed_contracts_in_cohort` — counts
      verified `CreditIssuance` events where both parties are in the
      cohort; reuses `contract::verify_credit_issuance` so a forged or
      out-of-cohort event doesn't inflate the count, 3 tests.)

(§10 buildable pieces: `node/src/bootstrap.rs` + `node/examples/
bootstrap_from_git.rs` + `protocol/src/pilot.rs`, 7 new tests. 100 tests
total across the workspace, `cargo clippy`/`cargo fmt --check` clean.)

---

## Suggested build order (summary)

1. §1 scaffolding → §2 protocol layer (identity, event kinds, VC schema) — **done**
2. §3 referral-query prototype — first demoable milestone — **done**
3. §4 job lifecycle + §5 trust graph — makes the demo end-to-end — **done**
4. §6 cascade block — hardens it against the known attack model — **done**
5. **§7 client + §8 coordination server — the current priority, together.**
   Reordered 2026-08-25. The client is what makes any of this usable off a
   dev machine, and a store-and-forward cache is what lets two clients that
   are never online at the same moment exchange anything at all — §7's own
   thin-client model ("syncs on relay wake") assumes something is holding
   events until that wake. §8's original "only once organic usage justifies
   it" was written about the *monetizable* services (rating bureau, broker
   scores); it does not apply to plain message carriage, and its
   preconditions are met either way — the peer-to-peer core §1–§6 works
   standalone today.
6. §10 pilot cohort launch
7. §9 legal track — last

**Sequencing caveat, recorded rather than smoothed over:** §9 describes
itself as running in parallel from the start and *gating external launch*,
and its own first item is "get a written tax attorney opinion before any
investor data room or public launch beyond a closed test cohort". With
invite-only dropped (§10, 2026-08-25) there is no closed cohort to shelter
under, so §10 *is* the public launch: running it before §9 means launching
ahead of that opinion, deliberately. That is a business risk call, not an
engineering one — the order above records it rather than resolving it.

