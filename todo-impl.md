# QW Implementation Plan

Source docs: `doc-link/abstract.md`, `doc-link/qw-design-faq.md`, `doc-link/qw-todo.md`
(symlinked to `../vkrinitsyn.github.io/qw`, not part of this repo).

Reviewed against source commit `6759eb1` ("qw: added usecases", 2026-08-07),
which added a "Basic Use Cases" section to `abstract.md` and matching Q&As to
`qw-design-faq.md`. New items from that section are marked **(added
2026-08-07)** below so it's clear which bullets predate the original plan.

**What is left.** Completed items were removed on 2026-08-26 rather than
left to accumulate: `git log`, the NIPs and the test suite record what was
built, and a plan that is nine-tenths ticked boxes stops being read. Section
numbers are the original ones — they are referenced from the NIPs and from
code comments — so the gaps below (§1, §4, §6) are sections with nothing
left open in them, not mistakes.

Two items here are marked as not completable from inside this repository
(§9's attorney opinion, §10's ecosystem choice). They stay unchecked on
purpose; silently dropping them would read as if they were done.

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

## 2. Protocol layer: identity, events, credentials

This is the shared substrate everything else depends on.

- [ ] **Controller DID vs. device key hierarchy**: `protocol/src/identity.rs`
      currently treats them as one and the same key (1:1) — a deliberate
      MVP simplification matching the plan's original "device key =
      controller DID's signing key" wording. The FAQ's key-loss answer
      (added 2026-08-07) clarifies the intended design has device keys
      added/removed *beneath* a stable controller DID, with only the
      controller itself ever needing the quorum amendment (NIP-QW09) —
      routine
      device swaps (new phone) shouldn't require quorum sign-off. NIP-QW09
      introduces `account_id` (the genesis controller pubkey) as the
      permanent anchor amendments chain from, which is the piece that was
      actually blocking this — device-key delegation itself is still
      unbuilt. Revisit before §7's multi-device support lands.

---

## 3. Referral-query prototype (first working milestone)

Per `qw-todo.md` recommendation — build this before the full job lifecycle,
since it's demoable standalone and is the differentiator.

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

## 5. Trust graph & net_position

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

---

## 7. Mobile client (Tauri v2)

The tooling gap that scoped this section is closed (2026-08-26): the
Android SDK, NDK and a JDK are installed, and the shell builds. What is left
is everything that needed a device rather than a compiler — OS integration,
the platform signer, and any evidence at all that the thing works when a
person opens it.

- [ ] **Run the Android build on a device.** The build itself is no longer
      the open part (2026-08-26): the toolchain installs, `cargo tauri
      android init` generates `gen/android/` (gitignored — ~300 MB, and
      `app/android-signing.patch` carries the one hand-edit worth keeping),
      release signing is wired against a keystore held outside the repo, and
      a signed arm64 APK is published and linked from `knownby.work/join`.
      Nobody has launched it. `app/ui/index.html` was made phone-shaped in
      advance — device-width viewport, safe-area insets, 44px targets, 16px
      inputs, a clipboard fallback for a WebView that is not a secure
      context — and every one of those is a guess until a phone renders it.
      Until then "buildable" is the whole claim. Also open: the other three
      ABIs (the published APK is arm64-v8a only), and a Play listing, which
      needs the upload key registered with Play App Signing.
- [ ] **OS deep links, external-signer delegation, and UI past
      identity/follow/sync** — the residue left behind when the client shell
      landed, recorded here rather than inside a finished item. Clicking
      `knownby.work/i/<npub>` does not open the app; the link has to be
      pasted, which is exactly the OS integration this section could never
      test. The `qw-signer:` URI protocol exists (`protocol/src/signer.rs`)
      but nothing on either platform speaks it, so the key still sits in the
      app's data directory at `0600`. And the shell shows an invite link,
      follows one, and syncs — no contract composition, no referral query,
      no trust display, though all three exist in `qw-protocol`/`qw-node`
      with tests and no UI.
- [ ] Web app path: compose/display only; signing delegated via QR or deep
      link to the external signer.
      The delegation protocol it would use is done
      (`protocol/src/signer.rs`, `qw-signer:` URIs); the actual web
      app — composing events, displaying them, rendering/scanning the
      QR — is unbuilt (needs a frontend, which needs Node/npm).
- [ ] Routine device-key changes (new phone) do **not** go through the
      quorum amendment (NIP-QW09) — under the controller/device-key hierarchy
      flagged in §2, device keys are added/removed beneath the controller
      directly. Amendment is only for the controller key itself.
      Still open — the device-key hierarchy itself isn't built (§2's
      `identity.rs` still treats controller and device as one key).

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
  chain-calculation results (NIP-QW10) are re-derivable by the client,
  vault only ever
  returns events that verify on their own, and `node/src/server_registry.rs`
  ranks *multiple* servers by ordinary trust score so none is hard-coded as
  authoritative. A cache that a client cannot do without would break the
  claim the landing page makes ("no central server"), so the client must
  still work — degraded, not broken — against direct relays alone.
- **Never the only copy.** Same rule as chain-calculation: a cached event is
  a convenience copy of something the author can re-publish, never the sole
  record. Losing the server loses latency, not history.


- [ ] Community insurance pool: explicitly last — depends on transaction
      volume existing first to fund the pool meaningfully.
      Deliberately unbuilt, matching the doc's own sequencing.
- [ ] **Broker-signed score** (added 2026-08-10, from conversation, not yet
      in the source docs): a coordination-server operator ("broker")
      computes a score for a subject and signs it, so the subject can hold
      and present it as a portable attestation elsewhere — distinct from
      the chain-calculation service (NIP-QW10, kind 9090), whose signed
      *path* the client
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

---

## 9. Legal / compliance track (parallel, non-blocking for prototype)

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
      ecosystem-agnostic (`node/src/bootstrap.rs`,
      `node/examples/bootstrap_from_git.rs`) and ready the moment a
      candidate is picked.

---

## Remaining build order

§1–§6 are complete; what follows is the order for what is not.

1. **§7 client + §8 coordination server — the current priority, together.**
   Reordered 2026-08-25. The client is what makes any of this usable off a
   dev machine, and a store-and-forward cache is what lets two clients that
   are never online at the same moment exchange anything at all — §7's own
   thin-client model ("syncs on relay wake") assumes something is holding
   events until that wake. §8's original "only once organic usage justifies
   it" was written about the *monetizable* services (rating bureau, broker
   scores); it does not apply to plain message carriage, and its
   preconditions are met either way — the peer-to-peer core §1–§6 works
   standalone today.
2. §10 pilot cohort launch
3. §9 legal track — last

**Sequencing caveat, recorded rather than smoothed over:** §9 describes
itself as running in parallel from the start and *gating external launch*,
and its own first item is "get a written tax attorney opinion before any
investor data room or public launch beyond a closed test cohort". With
invite-only dropped (§10, 2026-08-25) there is no closed cohort to shelter
under, so §10 *is* the public launch: running it before §9 means launching
ahead of that opinion, deliberately. That is a business risk call, not an
engineering one — the order above records it rather than resolving it.

