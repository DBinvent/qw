# QW Implementation Plan

Source docs: `doc-link/abstract.md`, `doc-link/qw-design-faq.md`, `doc-link/qw-todo.md`
(symlinked to `../vkrinitsyn.github.io/qw`, not part of this repo).

Reviewed against source commit `6759eb1` ("qw: added usecases", 2026-08-07),
which added a "Basic Use Cases" section to `abstract.md` and matching Q&As to
`qw-design-faq.md`. New items from that section are marked **(added
2026-08-07)** below so it's clear which bullets predate the original plan.

**What is left.** Completed items are removed rather than left to
accumulate (last pass 2026-09-05: a `Node` running in the client
(contact book + referral query, `Session::find_by_skill`) + a per-viewer
trust display (`Session::{trust, net_position}`), the NIP-QW12
ledger-sync core (`qw_node::ledger` + `HttpLedger` + single-user
`/ledger/*`) + `rank_servers` wiring + device-subkey resolver
(`recovery::device_authority`, kind 9082), the replaceable profile kind
(`10020`) + author `revision`, multi-user outbox persistence, KEK
management + key page, account recovery (registration recovery code +
seed export), `/auth/*` rate-limiting; 2026-09-04: profile editing,
contract negotiation, the `KeyStore`/`HistoryStore`/`Session` split,
single-user `qw-web`, and most of the multi-user host) —
`git log`, the NIPs and the test suite
record what was built, and a plan that is nine-tenths ticked boxes stops
being read. Section numbers are the original ones — referenced from the
NIPs and from code comments — so the gaps below (§1, §4, §6) are sections
with nothing left open in them, not mistakes. **The `## Priority` list at
the end is the current order**; the section items are the detail behind
it.

Client-side gaps are analysed separately in `app/app-todo.md` — what the
app can do against what the protocol already supports and what the
architecture says should exist. The summary of it: almost nothing missing
from the client is missing because it is unsolved.

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
| 8 | Admission-filter defaults — min-reputation threshold and position-limit scaling (added 2026-08-07, see §5) | **No enforced default at launch.** Filters exist in the client and start unset (everything passes) until a participant configures them. **Wired into the client 2026-09-07**: `Session::{admission_policy, set_admission_policy}` persisted in `SyncState`, a *Filter* tab, and `NegotiationView.passes_filter` flagging (not hiding) a failing inbound proposal. The two checks are the abstract.md §"Basic Use Cases" computations: min-reputation is `trust::assess_reputation` (shortest verified `CreditIssuance` path, `score_trust_path` = closing-edge value × `hop_decay^(hops-1)`, unknown-risk when no path); the position limit is a **multiplier** the client applies to `trust::counterparty_recent_volume` over a 90-day window (`Session::RECENT_VOLUME_WINDOW_SECS`), so the effective ceiling "scales with how much work that counterparty has recently completed". | Multi-path aggregation ("several independent paths ... a stronger signal") is still single-shortest-path only |

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

      **The 9082 record and its resolver are built (2026-09-05).**
      `KIND_DEVICE_SUBKEY = 9082` + `DeviceSubkey { device_pubkey, label,
      valid_from, revoked_at? }` + `device_subkey()` builder
      (`qw_protocol::events::kinds`), and in `qw_protocol::recovery`:
      `controller_at(genesis, amendments, at)` (time-bounded controller
      resolution; `latest_valid_controller` is now the no-bound case) and
      `device_authority(account_id, amendments, subkey_events, signer, at)
      -> DeviceAuthority::{Delegated, Revoked, Unknown}` — a 9082 counts
      only if its own signature verifies *and* its publisher was the
      controller at its `created_at`; revocation is not retroactive; a
      re-delegation after a revocation restores authority; a signature
      under a revoked key is `Revoked` (an alert), never a silent drop.

      **Still open:** `identity.rs` itself still has controller == device
      (1:1). Nothing yet *produces* 9082 events, and no live verification
      path calls `device_authority` — it is a standalone helper, like
      `verify_amendment` was before it. The re-sign-forward re-attestation
      envelope (NIP-QW12 §"Two hazards…") is also still just proposed.
- [ ] **Reviewed skills: a third evidence tier between mentioned and
      proven** (added 2026-08-26, from conversation). Today a skill tag is
      in one of two states, and the gap between them is most of a person's
      first year on the network:

      | axis | who asserts it | what it costs | scores? |
      |---|---|---|---|
      | *(the tag itself)* | you, in your profile | nothing | no |
      | **reviewed** | a broker, citing a proved external account and a stated method | the broker's own standing | no |
      | **contract-approved** | a counterparty, countersigning completed work | somebody else's signature | **yes** |

      **Not a ladder — two independent axes on one skill** (corrected
      2026-08-26 from the UI question, which is what exposed it). A skill can
      be reviewed and never contracted, contracted and never reviewed, both,
      or neither. Modelling it as tiers implies a skill is in exactly one
      state and pushes the UI toward three separate lists, which is wrong on
      a phone and wrong conceptually: the tag is the row, and the evidence
      about it is decoration on that row. One list, up to two badges.

      Before anyone negotiates with you they have to find you and form a
      first impression, and "proven" is empty until a first contract exists
      — which needs the impression first. `bootstrap.rs` already derives
      candidate skill tags from real commit history; what it deliberately
      will not do is sign them ("holds no one's signing key", so the subject
      signs their own suggestions and the result is still *mentioned*).
      Reviewed is that same analysis, signed by whoever ran it.

      Shape: an ordinary Verifiable Credential, which `protocol/src/vc.rs`
      already implements — issuer = the reviewer, subject = the pubkey,
      claim = `{skill_tag, level, source, method, examined_at}`. Nothing new
      cryptographically; a new kind and a claim schema.

      **The rules that keep it from becoming an authority:**

      - **Never enters the trust computation.** `score_trust_path` reads
        countersigned work and nothing else. A review is for reach and for a
        reader's own judgement, exactly like external links and declared
        tags. The moment a review moves a score, buying reviews beats doing
        work.
      - **The role is open.** knownby.work would be the first reviewer, not
        the only one — any pubkey may issue these, and a reader weighs the
        issuer. A tier only knownby.work can grant is a central authority in
        everything but name, which is what §0 rules out.
      - **State the method or it is a badge.** "Reviewed by GitHub" is
        meaningless without which account, what was examined, against what
        threshold, and when. Unexplained marks get gamed and then trusted
        anyway. Same requirement as §5's calculator profile, and worth
        resolving with it.
      - **Dated, and stale by default.** A 2024 review of a GitHub account
        says nothing about 2026. A completion is a fact about a moment; a
        skill review is a snapshot of something ongoing, so it needs an
        `examined_at` a reader can discount and probably an expiry.
      - **Depends on ownership proof.** Reviewing `github.com/x` means
        nothing unless x is provably this npub — so the external-links item
        above lands first, or the tier attests to a stranger's repository.

      Distinct from §8's **broker-signed score**, and the difference is the
      point: a score says "this person is worth N", which is a portable
      global reputation by another name and is why that item is still
      blocked. This says "I examined X on this date by this method and found
      Y" — checkable, attributable, and discardable. The narrow version is
      safe where the general one is not.

      **Groundwork landed 2026-09-08:** kind 10020 now carries a
      self-assessed `skill_levels` map and a `skill_sources` map
      (`self` / `commit-analysis`), and a client renders a three-state
      `evidence` class per skill (`unproven` / `algorithmic` /
      `approved_by_job`). That is the *self-signed* half — the `{skill_tag,
      level, source}` vocabulary a broker-issued VC will reuse, with
      `method` / `examined_at` and a real issuer still to add. Levels never
      enter `score_trust_path`.

- [ ] **External identity links in the profile** (added 2026-08-26, from
      conversation). Accept and expect a list of links to the places a
      person's work already lives — GitHub, LinkedIn, GitLab, a homepage —
      carried in the profile event (NIP-QW03, kind 10020) beside
      `skill_tags`, and offered as part of a self-introduction rather than
      as a separate lookup. Today a QW identity is a bare key: correct, and
      unrecognisable
      to someone who knows the person by their GitHub handle.

      **Partly landed 2026-09-08:** kind 10020 now carries a `links` list
      (`{network, url}`, http(s) only, de-duped, also emitted as
      `["r", url]` tags), surfaced in the profile editor and on the
      Identity view, and it explicitly renders as *not evidence*. What is
      still open is the **NIP-39 `["i", "github:handle", "<proof url>"]`
      shape with a checkable back-link** — the current list is a bare
      claim with no ownership proof and no `/gh/<handle>` short-link
      resolver.

      **Use Nostr's NIP-39 shape, do not invent one.** `["i",
      "github:vkrinitsyn", "<proof url>"]` tags on the profile event. QW is
      already Nostr-kinded, so every client that renders NIP-39 gets this
      for free, and the alternative is a QW-only dialect nothing else reads.

      **The two directions asked for are one mechanism.** "qw address → LN"
      is the claim, in the profile. "LN/GitHub → qw short link" is the
      proof: a gist, a pinned repo, a profile bio containing the npub. A
      claim with a matching back-link is checkable by anyone; a claim
      without one is a bare assertion. Same object, read from either end.

      **Links are claims and must never be trust.** §0 locks in
      locally-computed, per-viewer trust from countersigned work only, and
      NIP-QW05's rule that a public invite edge vouches for no one is the
      same rule in a different coat. `score_trust_path` must not read these
      tags, an unproven link must render as unproven, and "verified GitHub"
      must never become a reputation input — otherwise the cheapest way to
      look trustworthy is to own accounts, which is precisely the sybil
      shape §6 exists to defeat.

      Open, and worth deciding before writing the event:

      **Short invite links, keyed on a handle the person already owns**
      (decided 2026-08-26). `knownby.work/i/npub1…` is 76 characters, which
      is unusable in a bio — and a bio is exactly where the back-link
      belongs. The resolution is to borrow a namespace rather than create
      one:

          knownby.work/gh/vkrinitsyn  ->  302  ->  knownby.work/i/npub1…

      `knownby.work/@vk` was the obvious alternative and is the wrong one:
      it makes this project allocate names, which is a naming authority,
      first-come-first-served, and something to squat and to arbitrate.
      `/gh/<handle>` allocates nothing. GitHub already decides who owns that
      handle, so the mapping is self-certifying and there is no registry to
      keep.

      Resolution needs no index: the Worker (`landing/src/worker.ts`, which
      already runs first for `/i/*`) fetches `api.github.com/users/<handle>`
      and reads the npub out of the bio or blog field. Server-side on
      purpose — the same fetch from a phone would tell GitHub who is looking
      at whom, which is the privacy cost noted above; from the Worker,
      GitHub sees Cloudflare.

      Two rules this must not break:

      - **The short form redirects, it never replaces.** `/i/<npub>` carries
        the key, so it survives this site disappearing and anyone can
        re-host it; `/gh/vk` carries nothing but a promise. The canonical
        link stays the npub one, and the npub is what the client stores
        after following a short link.
      - **A renamed handle breaks the link rather than following it.**
        GitHub frees and re-registers handles. Re-verify on every resolve;
        if the bio no longer carries that npub, 404. Silently redirecting to
        whoever took the name over is the one real hazard in handle-based
        addressing, and it fails closed here.

      `/gh/` is **built** (`landing/src/worker.ts`, 2026-08-26): bio and
      website fields both searched, 404 when no key is published, and 503 —
      never 404 — when GitHub itself is unreachable, because a rate limit is
      not evidence that nobody claims the handle.

      **`/ln/`, `/fb/` and `/qw/<hash>` are wanted too** (2026-08-26), and
      none of them can work the way `/gh/` does. LinkedIn and Facebook have
      no fetchable public artifact and block automated reads, so nothing on
      their side can certify the mapping. Accepted anyway, with the
      difference made explicit rather than papered over: for these,
      resolution answers *"who claims this handle"*, not *"who owns it"*.

      That flips the dependency. `/gh/` needs nothing but GitHub; these need
      an index of published profiles claiming `linkedin:`/`facebook:`, which
      means the external-links half of this item shipped **and** a §8 server
      indexing it. Neither exists yet, so these routes stay unbuilt — a
      short link that resolves to a guess is worse than no short link.

      Rules for when they are built, all of them fail-closed:

      - Exactly one profile claims the handle -> redirect, and the invite
        page must show that the route was an **unverified** claim. Silence
        here would make an unprovable claim look like `/gh/`'s proven one.
      - More than one claims it -> a disambiguation page listing them.
        Never pick. With no proof there is no basis to prefer a claimant,
        and first-published is a land grab dressed as a rule.
      - None -> 404.

      **`/qw/<hash>` is the fallback for someone with neither account**, and
      it is the only one of the four that needs no other platform. The hash
      is *derived*, not allocated: `base32(sha256(pubkey))` truncated to
      ~12 characters. Nobody can choose theirs, so there is nothing to
      squat, and the lookup table is rebuildable by anyone holding the set
      of known keys rather than being a registry we own. Collisions get the
      same disambiguation page, and the truncation length is the knob.

      "A backup profile on us" is the §8 storage half: the coordination
      server holds the profile so the link resolves for someone with no
      GitHub presence. Ordinary §8 terms apply and are not negotiable here
      — optional, non-authoritative, and the client works without it. The
      canonical link is still `/i/<npub>`; every short form redirects to
      it.

      Neither the claim nor the short link touches trust: resolving
      `/gh/x` says a key claims that account and the account links back,
      and says nothing whatever about whether to work with them.
      - **Verifying a proof leaks the reader.** Fetching a gist to check a
        claim tells GitHub who is looking at whom, from a phone, which is a
        privacy cost the client cannot pay silently. Either verify on
        explicit request, or delegate to a §8 server and accept that the
        answer is then only as good as that server — an ordinary §8
        optional-service tradeoff, not a new one.
      - **Not every platform can be proved.** GitHub has gists; LinkedIn has
        no fetchable public artifact and actively blocks automated reads, so
        an `linkedin:` claim is unverifiable in practice and has to be
        displayed as such rather than quietly treated like the others.
      - **Open set or allowlist.** An open set means arbitrary strings in a
        signed public record; an allowlist means a protocol change per
        platform. NIP-39 chose the open set.

      The distribution half is §10's public-invite-link item: a back-link in
      a GitHub README or a LinkedIn profile is the same "post it where your
      work history already lives" channel, with the proof as a side effect.

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

- [ ] **Configurable reputation model — spec'd 2026-09-07 as
      [NIP-QW13](protocol/nips/NIP-QW13-reputation-scoring.md).** From
      conversation: the score should be a per-viewer, per-domain,
      fully-configurable computation over held records — a `1.0`-centred
      multiplier folding in the counterparty *rating* and pass/fail (not
      just the credit magnitude `score_trust_path` uses today), a
      taxonomy scope-inheritance matrix (`java` counting toward `backend`
      at a configured fraction) in place of the binary `same_domain`,
      per-domain `tolerance` thresholds, multi-path aggregation, an
      optional "contrarian" transitive transform, and a `net_position`
      balance term in the overall figure. NIP-QW13 §7 is the current-vs-
      spec gap table; §8 stages it (re-base the score + add rating first).
      Visibility/broadcasting score-gating (referral forward, answer rank,
      bulletin browse) is §6 of that NIP — none of it gates on score
      today.

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

## 7. Mobile client (Tauri v2) — and the hybrid web deployment

The tooling gap that scoped this section is closed and so is the one after
it: as of 2026-08-26 the Android build is signed, published, installed and
run on a device. What is left is not "does it work" but reach — the OS
integration that a sideloaded APK cannot have, the platform signer, the
other platforms, and a distribution story that is not "trust this file".

**Hybrid deployment (built 2026-09-02..04).** The same client runs three
ways off one codebase: the Tauri app; a **single-user `qw-web`** server
(open source, on the user's own box, key never leaves it); and a
**multi-user `qw-web`** host where a passphrase unlocks an in-memory
session for its lifetime, signs server-side, and shows an explicit
disclaimer — no memory-dump protection, no uptime guarantee. `qw-web` is a
*client* host, strictly separate from `qw-bo` (the coordination server).
It is also the review vehicle — the same core, far easier to run in CI and
read than a Tauri app needing a display + Android SDK. Multi-user is live
at `qw.knownby.work`; what remains is in the item below and the `##
Priority` list.

- [ ] **Google Play — the current distribution target** (2026-08-26). A
      sideloaded arm64 APK from `app.knownby.work`, signed by a key
      generated on the build host, is what reaches people now; that key
      asserts an identity and establishes none, and nothing links the binary
      to a commit. The order that pays off: signed release tags, the
      certificate fingerprint published beside the download, CI-built
      releases with provenance attestation, then Play App Signing and the
      remaining three ABIs. **CI-built, attested releases are the
      engineering item** — the rest is paperwork that only matters once that
      exists. Costs and the gating rules (Play's 12-tester/14-day clock for
      personal accounts) are in the private back-office notes, not here.
      **iOS is shelved** as of the same date: it needs hardware that does
      not exist here, and shipping a second platform before the first is on
      a store is how both end up half-done.

- [ ] **Contract lifecycle past Accept — client UI.** The client composes
      and negotiates offers (kinds 9000 / 9004 / 9001, built 2026-09-02),
      but stops at Accept. Still no UI for milestones (9002), review
      requests (9005), completion (9003) or the two-phase credit issuance
      (9010) — all defined in `qw_protocol` with tests. The first
      countersigned completion is what makes any *proven* skill badge
      reachable from the app, so this closes the loop.

- [ ] **Multi-user `qw-web` — encrypted account store + layered unlock**
      (built 2026-09-03..04, live; hardening + factors remain). Full design
      in `app/server/multi-user.md`. One per-account **master key**, random,
      generated once, **never at rest in the clear and never leaves server
      RAM** — it encrypts the account blob (identity key + `HistoryStore`)
      directly and never changes. Around it an ordered **KEK set**: each
      row an independent wrapping of that same master, so adding / dropping
      / rotating a factor is O(1) metadata with no blob re-encryption and
      no identity change. Sources, weakest-standalone first: (1) a
      user secret — passphrase / raw key / seed phrase, `Argon2id` in the
      browser, the secret never transits; (2) a hardware key fob (FIDO2 /
      YubiKey); (3) a user-attached KMS (AWS KMS etc.) — wrap/unwrap is a
      `Decrypt` against a CMK in the *user's* account, server holds only
      the wrapped master + ARN; (4) an authenticator / approval app; (5)
      the qw mobile app, biometric-gated, its wrapping key a revocable
      device subkey (NIP-QW09 kind 9082) on a phone that is already a
      ledger replica (NIP-QW12). Every wrapping row is dated
      (`created_at`, `not_after?`) and carries a `rotate` flag; a flagged
      row is refused and re-wrapped on next login by another KEK. Losing
      every KEK = identity loss → a one-time recovery-code wrapping +
      seed export. Directory tier (nickname) stays under an operator
      `server_key` with a blind index, separate from the master. Work is
      entirely in the `qw-web` crate — an encrypted `KeyStore` /
      `HistoryStore` over the RAM master plus an `unlock` module for the
      KEK set; `Session` and both traits are unchanged. `qw.knownby.work`
      is **public** — the host carries its own nickname/passphrase auth,
      no Cloudflare Access (`--single` is the no-shared-surface option for
      a personal box). Pairs with the ledger-sync item below (the
      encrypted `HistoryStore` is the same seam a `LedgerTransport` merges
      into).

      **Built 2026-09-03/04, live at `qw.knownby.work` (`:8112`,
      public, no Access).** `qw-web` crate: `envelope.rs` (`MasterKey`,
      `Wrapping`, `KekKind`), `accounts.rs` + `accounts.schema.yaml`
      (directory tier in Postgres via `marg` + `schema_guard_tokio` +
      `sqlx`), `account_session.rs` (`EncHistoryStore` + `open_account` /
      `seal_new_account`), `unlock.rs` (KEK set ops), `multiuser.rs`
      (`MuWeb`: `/auth/register|login|logout` + the `/api/<cmd>`
      routes behind `with_session`, token session table, sweeper).
      `app/ui/index.html` shows a sign-in panel on the first 401. Deploy:
      `../qw-bo/qw.sh web` (default mode; `--single` for the key-on-disk
      client) + `deploy/qw-web-mu.service`. Rate-limiting on
      `/auth/register` + `/auth/login` is built (`src/ratelimit.rs`: 5/hour
      per IP on register, 20/5min per IP + 10/5min per account on login).
      Workspace 274 green + 2 Postgres round-trips. Full record:
      `app/server/multi-user.md` and git. **2026-09-05:** outbox
      persistence (the blob carries the `SyncState`), KEK management + key
      page (`POST /api/kek_{list,add,drop,rotate,flag,recovery}`, `GET
      /session`), and account recovery (`register` mints a recovery-code
      KEK and returns it once; `POST /api/seed_export` gives the portable
      `identity.key` hex). The must-finish list is clear; email-verify and
      the delta blob format are parked as low-priority (see the `##
      Priority` list).

- [ ] **Multi-replica ledger sync — NIP-QW12** (spec'd 2026-09-02, the
      anti-entropy core **built 2026-09-05**). One identity's ledger may
      live on a phone and one or more `qw-web` boxes at once; they converge
      by anti-entropy, not through the `qw-bo` mailbox (different trust
      model, lifetime and direction — the two share no code). The ledger is
      a grow-only set of content-addressed, self-verifying events, merge =
      union, every view a pure fold — so a write from a stale replica adds
      one element and cannot corrupt a profile or stall an algorithm.

      **Built:** `qw_node::ledger` — `LedgerSync` (the anti-entropy round),
      `LedgerTransport` trait (sibling to `qw_node::sync`), `LedgerCoverage`
      (the **exact held-id set**; a per-author high-water cannot express "I
      have everything after T but lack something before it", which is what
      two devices that both authored produce — a negentropy digest is the
      NIP's sanctioned later optimisation), `PullResponse`. Verify-on-ingest;
      dedup by id (a same-second sibling has a different id, so the
      inclusive-window off-by-one doesn't arise); a broken peer never stops
      the others; 3+ replicas converge in one round via the middle.
      `qw_client_core::Session::ledger_round(transport, peers)` merges
      pulled events into the `HistoryStore` and folds the sync snapshot.

      **Still open:** a real `LedgerTransport` impl (HTTP replica-to-replica
      + `qw-web` `/ledger/*` endpoints) — belongs with the "run a `Node` in
      the client" item — and the rotated-key-outbox re-attestation envelope
      (§2 / NIP-QW09, still proposed).

- [ ] **Earned-skill routing** — protocol, node and client halves are
      **built** (2026-08-26); a `Node` now runs in the client
      (**2026-09-05**).

      The bug: routing matched `cached_skill_tags` only, so someone with ten
      countersigned Rust contracts and no `rust` tag published was
      unreachable by a Rust query — the participant with the most evidence
      was the hardest to find. Worse, `relay_for` self-matched on declared
      tags too, so such a node stayed *silent* even when the query arrived.

      Built: `qw_protocol::trust::earned_skill_tags`;
      `Contact::earned_skill_tags`; `routing::select_forward_targets_ranked`
      with `MatchSource`; `Node::refresh_earned_skill_tags`; the self-match
      accepting either source.

      **2026-09-05:** `qw_client_core::Session` holds a `Node`. On every
      history change ([`refresh_node`]) it rebuilds the contact book from
      held kind-9060 introductions, sets its own declared tags, and calls
      `refresh_earned_skill_tags` over the whole ledger.
      `Session::find_by_skill` originates a NIP-QW06 query (the session is
      its own hop 1); `sync_now` routes inbound 9050 / 9051 through the
      node and queues the forwards / answers it produces; a query's
      answers land in `Session::referral_results`. `Node` now `p`-tags its
      9050 forwards so the coordination mailbox can carry them, and adds
      `Node::note_contact` (upsert cached tags without resetting a
      contact's rate window). `qw-web` (`/api/{contacts,find_by_skill,
      referral_results}`, single- and multi-user) and the Tauri commands
      are wired.

      Still open: the fully-hidden-requester path (an encrypted DM to hop
      1 so hop 2 does not learn who is asking) — until then a query
      reveals the requester to their own contacts.

      Two properties not to regress. **Reach, not trust** — a match found
      this way still scores through `score_trust_path` on completed work
      alone. And **recomputed, never accumulated** — a history that stops
      being visible takes its tag away again, or the set is a cache
      pretending to be evidence.

- [ ] **A local event store — built 2026-08-26**, remaining work is what to
      do with it. `MailboxSync::poll` handed back `delivered` and the shell
      counted it and dropped it, so the client was amnesiac in a way that
      silently disabled most of the protocol: trust paths, contract lists
      and earned skills are all functions over held history, and the history
      was thrown away once per sync.

      `qw_client_core::EventStore` is append-only JSON Lines at `0600` beside
      the key, verified on append *and* on load. A mailbox is untrusted
      infrastructure (§8: it may withhold, never inject) and that guarantee
      only holds if the thing writing to disk enforces it — so an event that
      fails `verify` is refused, an edited line is skipped and *counted*
      (`rejected()`) rather than swallowed, and a half-written trailing line
      after a crash costs one event instead of the file. 5 tests.

      Deliberately unbounded: nothing prunes. What may be forgotten is a
      protocol question — records are evidence others may ask for — and not
      one to answer accidentally inside a cache. Revisit when a real history
      makes it a problem, with §8's vault as the other half of the answer.

      Unblocked by it and built: contract composition, driving a `Node` so
      earned-skill routing runs for real, a referral query from the client,
      and a per-viewer trust display (`Session::{trust, net_position}`,
      `ContactView` trust fields — all 2026-09-05).

- [ ] **NDA-covered work: decide whether silence needs a marker** (added
      2026-08-26, from conversation). Mostly already answered, recorded so
      it is not re-derived:

      - **Redaction is built.** `protocol/src/vc.rs` is an SD-JWT where
        `hours`, `rate`, `ko`, `km`, `skill_tags` and `timestamp` are each
        individually withholdable, and NIP-QW02 already buckets amounts with
        the exact figure opt-in. An NDA can hide what the work was and what
        it was worth.
      - **The issuer cannot be hidden, and that is correct.** The
        counterparty is the signature, not a disclosable field, so redaction
        can never anonymize who vouched. That is what keeps a redacted
        record useful: a vouch you cannot attribute cannot be weighted by
        anyone's trust graph, so an anonymized record would be worth zero to
        a stranger anyway. Redact the work, never the witness.
      - **No adverse inference exists to fix.** §2's rule is that omission
        is provable by production, not by gap analysis — the protocol
        already declines to read anything into an absence, which is what
        makes "I cannot publish this one" cost nothing.

      The open part is narrow: there is no way to positively state "a
      contract exists here, withheld". Before building one, two things.

      A marker leaks the existence of a relationship, which is exactly what
      many NDAs forbid — silence may already be the correct answer, and
      adding it may be strictly worse than not having it.

      And **an NDA is one example of an obligation to withhold, not a
      category the protocol should know about.** No enumerated reasons, no
      typed `reason` field: the moment the protocol lists which excuses
      exist, it is ruling on which are valid, which is the adjudication role
      §0 keeps it out of. Withholding is always permitted, any explanation
      is free-form and mostly out-of-band, and whether to accept it is the
      reader's judgment — per-viewer, like trust itself. Others have full
      right to decline the explanation, and the protocol's job is to leave
      them able to.

- [ ] **Key backup, and a way to see the key at all** (added 2026-08-26,
      from the client/architecture gap analysis). The key *is* the account:
      §2 has no server that could reissue one, `app/README.md` says losing it
      loses every record signed with it, and `/join` tells people to back it
      up. The app offers no way to view it, copy it, or write it down — so
      the one instruction the product gives about the one unrecoverable thing
      it holds cannot be followed from inside it.

      Small, and the consequence of not having it is total, which is why it
      outranks most of §7 despite being the least interesting item in it.
      Needs a deliberate reveal (not on the main screen), the `0600` file
      already written by `Vault`, and wording that does not imply anyone can
      help if it is lost.

      **Half done 2026-09-05:** the multi-user `qw-web` key page has the
      reveal — `POST /api/seed_export` over `Session::identity_secret_hex()`
      (the 32-byte secret as `identity.key`-style hex), behind an explicit
      "Show identity key" disclosure with loss-is-final wording. What is
      left is the **Tauri** shell: the same reveal against the on-disk
      `Vault`, and the deep-link / external-signer work below.

- [ ] **OS deep links, external-signer delegation, and UI past
      identity/follow/sync** — the residue left behind when the client shell
      landed, recorded here rather than inside a finished item. Clicking
      `knownby.work/i/<npub>` does not open the app; the link has to be
      pasted, which is exactly the OS integration this section could never
      test. The `qw-signer:` URI protocol exists (`protocol/src/signer.rs`)
      but nothing on either platform speaks it, so the key still sits in the
      app's data directory at `0600`. The shell now also composes contract
      proposals and negotiates them, runs a referral query, and shows a
      per-viewer trust read on every contact and negotiation counterparty
      (`Session::{find_by_skill, contacts, referral_results, trust,
      net_position}`, `qw-web` + Tauri + `app/ui/index.html`, 2026-09-05).
      What is left in this item is the OS deep link and the external
      signer.
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
      The signed record (kind 9082) and its resolver
      (`recovery::device_authority`) exist as of 2026-09-05; still open is
      the `identity.rs` hierarchy that produces and signs them, and a live
      verification path — see the §2 item.

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


- [ ] **"Optional" is not true of the client yet** (added 2026-08-26, from
      the client/architecture gap analysis). This section's own first
      principle is that a client must still work — degraded, not broken —
      against direct relays alone, because otherwise the landing page's "no
      central server" is false.

      **`rank_servers` is wired (2026-09-05).** `Session::rank_servers(
      &[ServerCandidate])` re-orders the server list by this identity's own
      trust view (`qw_node::server_registry`), and `main.rs` + `src-tauri`
      call it at setup. A blank server `pubkey` still scores as
      unknown-risk (fee-order) until servers advertise one — but the list
      is a ranked candidate set now, not one hard-coded authoritative URL,
      and `MailboxSync` already fails over across it.

      **Still open — a genuine second transport / relay path.**
      `HttpMailbox` is still the only `MailboxTransport`; if every
      coordination server is unreachable the mailbox path stops (the
      NIP-QW12 `HttpLedger` added 2026-09-05 is replica-to-replica, a
      different job). A direct-relay `MailboxTransport` (or accepting that
      "degraded" means "ledger sync between your own replicas keeps
      working, mailbox delivery to strangers pauses") is the remaining
      design call. Pairs with "run a `Node` in the client".

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

## Priority (2026-09-05)

§1–§6 are complete; the client (§7) and the coordination server (§8) are the
work. Multi-user `qw-web` is deployed at `qw.knownby.work` and, as of
2026-09-05, has no open items on the must-finish list — outbox persistence,
KEK management + key page, and account recovery all landed. What is left is
the standing client / protocol gaps, roughly in order.

*(Landed 2026-09-05, multi-user `qw-web`:*
- ***Outbox persistence** — `qw_client_core::{SyncState, Session::sync_state,
  Session::restore_sync_state}` + `MailboxSync::cursor_snapshot`; the
  multi-user blob folds the outbox ids and poll cursors into the same
  sealed payload, so an authored-but-unsent event and a warm cursor
  survive a host restart.*
- ***KEK management + key page** — `POST /api/kek_{list,add,drop,rotate,
  flag,recovery}` behind the session cookie, over `unlock::*`; a
  fetch→mutate→`set_wrappings` op serialised under the per-session lock
  like `with_session`. `AccountStore::by_account_id` added. The shared
  `app/ui/index.html` gains a "Keys" section — add / remove a passphrase,
  (re)generate a recovery code, reveal the identity key — shown once a new
  `GET /session` probe confirms a login host.*
- ***Account recovery** — `register` now mints a recovery-code KEK and
  returns it once (the UI reveals it straight after sign-up); `POST
  /api/seed_export` hands back the 32-byte identity secret as hex
  (`Session::identity_secret_hex`), the portable `identity.key` spelling,
  so the identity survives total loss of the host blob.)*
- ***Multi-replica ledger sync (NIP-QW12) + device subkeys (NIP-QW09 kind
  9082)*** — the transport-agnostic cores: `qw_node::ledger`
  (`LedgerSync` + `LedgerTransport` + exact-id-set `LedgerCoverage`),
  `Session::ledger_round`, and `qw_protocol::recovery::{controller_at,
  device_authority}` over `KIND_DEVICE_SUBKEY = 9082`.*
- ***HTTP `LedgerTransport` + `rank_servers` wiring*** — `qw_client_core::
  HttpLedger` (a real `LedgerTransport` over `/ledger/pull` + `/ledger/push`);
  those two routes on the single-user `qw-web` host, so the operator's
  phone can sync to their own box; `Session::rank_servers(&[ServerCandidate])`
  + `Session::{servers, held_events}`, called from `main.rs` and
  `src-tauri` at setup.*
- ***Node in the client*** — `Session` holds a `Node`, rebuilds its
  contact book from held introductions and refreshes earned tags on every
  history change; `Session::find_by_skill` originates a NIP-QW06 referral
  query (self as hop 1), `sync_now` relays inbound 9050/9051, answers land
  in `Session::referral_results`. `qw-web` + Tauri commands
  (`contacts` / `find_by_skill` / `referral_results`).
- ***Per-viewer trust display*** — `Session::{trust, net_position}` +
  `TrustView`; `ContactView` gained `trust_hops` / `trust_score` /
  `net_position`; `qw-web` `/api/{trust,net_position}` + Tauri commands;
  the UI shows it on every contact and negotiation counterparty. Never a
  global number — a shortest verified `CreditIssuance` path (or
  unknown-risk, not zero) computed from held records, with the path's
  edge ids for spot-checking (§8).
- ***Profile visible past hop 1*** ("profile adv" — open to view by anyone
  the referral network connects you to, still not a broadcast) — a
  kind-9051 skill answer now carries the responder's own signed profile
  (kind `10020`) as an optional `profile` field (`SkillAnswer.profile`,
  `Node::set_own_profile`). `Session::absorb_answer` re-verifies it
  (`kind == 10020`, `pubkey == responder_pubkey`, signature) and merges it
  into history; `FinalAnswerView` gained `display_name` /
  `declared_skill_tags`, and `Session::profile_of(pubkey)` +
  `/api/profile_of` (both hosts) + a Tauri command expose any held
  profile. So a requester who matched a stranger through a vouched path
  sees their whole self-description, not just the one matched tag.
  NIP-QW06 "`profile` — the responder's self-description on the answer",
  NIP-QW03 "Visibility past a direct contact".
- ***Dispute annotation / audit (NIP-QW04, kind 9030)*** — `Session::annotate`
  + `qw_client_core::negotiation::{annotate, AnnotateArgs}`: sign a
  **reply**, an **audit request**, or a third-party **audit opinion**
  against a contract, never mutating the record. `reply`/`audit_request`
  are party-only, `audit_opinion` is third-party-only and needs an
  outcome; a `["p", …]` carriage tag makes it mailbox-deliverable to the
  other party (both, for an auditor). `NegotiationView` gained `disputes`
  (rows, oldest first) and `under_review` (an open audit request with no
  opinion yet). `/api/annotate` on both hosts + a Tauri command; the UI
  shows the thread and an "annotate" control on each negotiation. Still
  to come: indexing `audit_opinion` by its author (the auditor's own
  staked record), and threading a reply onto another annotation.
- ***Coordination-server list — per-account + public host default***
  (2026-09-07 / -09-08) — `SyncState.servers`; `Session::set_servers`
  validates (`qw_client_core::clean_server_list`) and persists it. **Now
  persisted everywhere**: `EventStore` grew a `sync-state.json` sidecar
  (`HistoryStore::loaded_sync_state`, replayed in `Session::with_identity`),
  so single-user web + the phone keep the server list, outbox, cursors and
  admission policy across a restart — not only the multi-user blob. A
  **`host_config` Postgres row** (seeded from `QW_SERVERS`) is served on an
  unauthenticated, CORS-open `GET /servers`; `Session::bootstrap_servers`
  + a Tauri command + a Mail-page "Fetch" control pull it. `POST /servers`
  edits the row, gated by `QW_ADMIN_TOKEN`. `POST /api/{servers,set_servers}`
  stay the per-account path.
- ***Admission pre-filter + coefficient pricing*** (2026-09-07) —
  `SyncState.admission` (`trust::AdmissionPolicy`, now `Serialize`);
  `Session::{admission_policy, set_admission_policy}` + an `admits()` helper.
  Both checks are the abstract.md §"Basic Use Cases" score calculations:
  **min-reputation** is `trust::assess_reputation` (shortest verified
  `CreditIssuance` path, closing-edge value × hop-decay, unknown-risk when
  no path); **position limit** is a multiplier `admits()` applies to
  `trust::counterparty_recent_volume` (90-day window), so the ceiling
  scales with the requester's recent completed work. `NegotiationView.
  passes_filter` is `false` only for an inbound proposal still open that
  fails the policy (flagged on the Contracts list, never hidden). A new
  **Filter** tab; `POST /api/{admission,set_admission}` on both hosts + a
  Tauri command. Separately, the propose / counter box now exposes the
  optional `ko` / `km` multipliers (`Quants = Hours × Rate × ko × km`) with
  a live total, and an **avg** button fills Rate from the mean of this
  identity's own settled contracts (skill-tag-matched when possible).
- ***Profile: skill level, provenance, grouping, external links***
  (2026-09-08) — kind 10020 (`ProfileSkillTags`) gained three optional,
  additive fields: `skill_levels` (`beginner`/`intermediate`/`senior`/
  `expert`, self-assessed, routing-neutral), `skill_sources`
  (`self` default / `commit-analysis`), and `links` (`{network, url}`,
  http(s), also emitted as `["r", url]` tags). `profile::build_signed` now
  takes a richer `ProfileEdit { display_name, skills: [{tag, level,
  source}], links }`; `SkillView` gained `group` (`sector/domain`, so the
  UI groups without re-parsing), `level`, and a three-state `evidence`
  class — `approved_by_job` (a countersigned contract carries the tag) >
  `algorithmic` (kept from a `commit-analysis` suggestion) > `unproven`.
  `IdentityView` gained `links`. UI: per-chip level dropdown, a links
  editor, skills grouped by field on both Identity and Profile, the
  evidence icon on every row. NIP-QW03 "Level, provenance and external
  links". Still open below: the **NIP-39 `["i", …]` proof-link** shape
  (this is the simpler flat list, no ownership proof) and **broker-signed
  reviews** (the `source`/`level` vocabulary is deliberately the VC claim
  shape those will reuse).

**Standing client / protocol gaps — the protocol is well ahead of the client:**

1. **Finish the routing / replication wiring** (§8, §7 items) — landed
   2026-09-05: `rank_servers`, `HttpLedger` + single-user `/ledger/*`, a
   `Node` in the client, and a per-viewer trust display. Left: a
   **genuine second `MailboxTransport`** (so mailbox delivery degrades
   rather than stops), the **multi-user `qw-web` `/ledger/*`** (needs a
   device-subkey auth channel), a **scheduled `sync_now` / `ledger_round`
   loop**, and the fully-hidden-requester DM to hop 1.
2. **Contract lifecycle past Accept** (§7 item) — milestones (9002),
   completion (9003), review requests (9005), two-phase credit issuance
   (9010) have no UI; the first countersigned completion is what makes a
   *proven* skill badge reachable.
3. **Key backup / reveal in the app** (§7 — the Tauri half; the `qw-web`
   key page above already does this for a multi-user host via
   `Session::identity_secret_hex`), **OS deep links + external signer**
   (§7), **CI-built attested Play releases** (§7 Google Play).

**Low priority / deferred (do not block the list above):**

- **Email verification on the multi-user host** (was item 1, dropped down
  2026-09-05). `qw-web`'s account is nick + passphrase with no email at
  all; verification would mean *adding* one, for a "recovery used" notice
  channel and account-takeover friction. Both are real but neither
  blocks: rate-limiting covers the abuse case and the recovery code
  covers lockout. A public host can add it later without reshaping
  anything.
- **Delta blob format** (perf only — `EncHistoryStore::append` and
  `persist_sync_state` re-seal the whole payload, O(ledger)). Fine until a
  real ledger makes it a problem.

Then §10 (public invite links as the entry point, cascade-skip for those
edges); §9 legal track last.

**Sequencing caveat, recorded rather than smoothed over:** §9 gates
external launch, and its first item is a written tax-attorney opinion
before any public launch. Invite-only was dropped (§10, 2026-08-25), so §10
*is* the public launch: it runs ahead of that opinion, deliberately — a
business-risk call, not an engineering one.
