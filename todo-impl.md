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

- [ ] Pick **one** open-source ecosystem where contribution history already
      exists in commit logs (per the docs' cold-start mitigation) — concrete
      candidate criteria: active repo(s) with multiple maintainers, existing
      informal reciprocity norms, willing pilot cohort of ~10–20 people who
      already know each other's work.
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

1. §1 scaffolding → §2 protocol layer (identity, event kinds, VC schema)
2. §3 referral-query prototype — first demoable milestone
3. §4 job lifecycle + §5 trust graph — makes the demo end-to-end
4. §6 cascade block — hardens it against the known attack model
5. §7 mobile client — makes it usable outside a dev machine
6. §9 legal track — run in parallel starting now, gates external launch
7. §10 pilot cohort launch
8. §8 coordination server — only once organic usage justifies it
