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

- [ ] **External identity links in the profile** (added 2026-08-26, from
      conversation). Accept and expect a list of links to the places a
      person's work already lives — GitHub, LinkedIn, GitLab, a homepage —
      carried in the kind 9020 profile (NIP-QW03) beside `skill_tags`, and
      offered as part of a self-introduction rather than as a separate
      lookup. Today a QW identity is a bare key: correct, and unrecognisable
      to someone who knows the person by their GitHub handle.

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

The tooling gap that scoped this section is closed and so is the one after
it: as of 2026-08-26 the Android build is signed, published, installed and
run on a device. What is left is not "does it work" but reach — the OS
integration that a sideloaded APK cannot have, the platform signer, the
other platforms, and a distribution story that is not "trust this file".

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
- [ ] **Profile editing in the client** (added 2026-08-26, from
      conversation — this was a gap, not a deferral: nothing in §7 mentioned
      it). `qw_protocol::events::profile_skill_tags` builds kind 9020 and
      has tests; no UI calls it, so a person running the app today cannot
      say what they do. That is the one thing a referral query matches
      against (NIP-QW06), which makes an unpublishable profile the reason
      the network cannot route to a new member at all.

      **Not a text field.** NIP-QW03 requires normalization through
      `/synonyms.yaml` *before* signing, because tag fragmentation
      ("nodejs" vs "node.js") is unrecoverable once it is in a signed
      record. So the editor is a picker over `/taxonomy.yaml` leaves with
      synonyms resolving typed input — and both files have to ship inside
      the app or be fetched and cached, which is a payload decision nobody
      has made yet.

      **Two surfaces, deliberately not merged.** Kind 9020 is the standing
      self-description with no expiry; kind 9091 (NIP-QW11) is the
      time-scoped "available for X" posting meant to be browsed. "Open to a
      project" is the second one. An editor that collapses them turns a
      standing claim into an advert that never expires.

      **Partial disclosure is publish-or-not, per tag.** NIP-QW03 makes
      skill tags public by necessity — relays holding pending referral
      queries have to read them to route. There is no half-public tag, and
      the editor must say so rather than implying one exists; the only
      lever is which tags you publish, and an unpublished tag is simply
      unroutable. Real selective disclosure lives on the *evidence* side
      instead (`protocol/src/vc.rs`'s SD-JWT, where every field is
      individually disclosable, and NIP-QW02's opt-in exact figure), which
      is the right place for it: what you claim is cheap and public, what
      you proved is yours to reveal.

- [ ] **Earned-skill routing** — protocol, node and client halves are
      **built** (2026-08-26); `qw_node`'s own refresh is not yet driven by
      anything outside tests.

      The bug: routing matched `cached_skill_tags` only, so someone with ten
      countersigned Rust contracts and no `rust` tag published was
      unreachable by a Rust query — the participant with the most evidence
      was the hardest to find. Worse, `relay_for` self-matched on declared
      tags too, so such a node stayed *silent* even when the query arrived.

      Built: `qw_protocol::trust::earned_skill_tags` (tags of contracts
      completed *and* countersigned — both sides' `KIND_JOB_COMPLETION` on
      one offer; one side alone earns nothing); `Contact::earned_skill_tags`;
      `routing::select_forward_targets_ranked` with `MatchSource` so a
      querier can tell a countersigned match from a claimed one;
      `Node::refresh_earned_skill_tags`; the self-match accepting either
      source. On the client, `qw_client_core::EventStore` gives it something
      to read and `app/src-tauri` surfaces the identity's own earned skills.

      Remaining: nothing calls `Node::refresh_earned_skill_tags` outside
      tests, because nothing yet runs a `Node` on the client. Until it does,
      routing behaviour is byte-for-byte what it was.

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

      Now unblocked by it, and none of it built: contract composition, a
      referral query from the client, any trust display, and driving a
      `Node` so earned-skill routing runs for real.

- [ ] **Move the profile to a replaceable event kind** (decided
      2026-08-26). Kind 9020 sits in Nostr's *regular* range (1000-9999):
      relays keep every event and none supersedes another. NIP-QW03 says so
      — "publishing a new one does not retract the old one at the protocol
      level" — and that was fine while nobody could edit a profile.

      What settles it is what a profile *is*. Skill tags and availability
      are a statement of intent about future contact; they do not confront
      the past and are not evidence of anything. The ledger of countersigned
      contracts is the record, and it is separate. So profile revision
      history is noise, not history: keeping every edit forever preserves
      nothing anyone should be reading, while costing three things — a
      permanent public trail of every version of yourself, a fetch-all-and-
      sort on every client, and a relay able to serve a stale profile
      indistinguishably from a current one, which is a withholding surface
      rather than mere storage waste.

      Nostr's replaceable range (10000-19999, latest per pubkey+kind) is the
      fix; addressable (30000-39999, plus a `d` tag) if profiles ever need
      to be plural. Its own kind 0 metadata is replaceable for this exact
      reason, and moving there also puts QW's profile where NIP-39 external
      identity claims conventionally live.

      Remaining work is the migration, not the decision: a new kind
      constant, `profile_skill_tags` emitting it, readers preferring the new
      kind and falling back to the most recent 9020, and NIP-QW03 amended.
      Contracts stay append-only — evidence must accumulate, a current
      statement must not. Do this before the editor ships, or the first
      person to edit five times has five permanent public claims and no
      protocol-level statement of which is current.

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


- [ ] **"Optional" is not true of the client yet** (added 2026-08-26, from
      the client/architecture gap analysis). This section's own first
      principle is that a client must still work — degraded, not broken —
      against direct relays alone, because otherwise the landing page's "no
      central server" is false. Two concrete violations, both in the client
      rather than here:

      - **There is no relay path at all.** `qw_client_core::HttpMailbox` is
        the only `MailboxTransport` that exists. If the coordination server
        is unreachable the client does not degrade, it stops. Nothing in the
        protocol requires this — the transport trait is already the seam —
        but no second implementation exists.
      - **`server_registry::rank_servers` is never called from anywhere.**
        It is written and tested; `AppState::servers` is a hardcoded
        one-element `vec!["https://qw-dash-api.knownby.work"]`. Hard-coding
        one server as authoritative is the exact thing this section forbids,
        and the code to avoid it is already sitting there unused.

      Both are wiring, not design. Until they land, "optional coordination
      server" describes the protocol and not the product.

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

