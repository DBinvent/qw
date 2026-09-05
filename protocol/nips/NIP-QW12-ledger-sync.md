# NIP-QW12: Personal ledger replication

`draft` — no new event kinds; a replication protocol over the existing
9000–9099 set.

## Abstract

One identity's record — every signed event it authored or was handed —
is a **private ledger** that may be held on more than one device at once:
a phone, and one or more self-hosted or multi-user web instances
(`qw-web`). This NIP defines how those replicas converge: how a replica
learns what it is missing, pulls it, and merges it without a coordinator,
and why a write from a replica with a stale view can never corrupt the
ledger or a derived profile.

It is **not** the offline mailbox. `qw_server::mailbox` (a coordination
service, not a NIP) is store-and-forward *between different identities* —
one recipient, delete-past-cursor, 30-day expiry, untrusted carriage.
This NIP is anti-entropy *between replicas of one identity* — bidirectional,
whole-set, permanent. The mailbox MAY be used as one carrier when replicas
cannot reach each other directly, but ledger sync MUST NOT assume any
mailbox semantics (no TTL, no delete-on-read, no single-recipient queue),
and the two share no code.

Distinct also from NIP-QW08 (history request/response), which is a contact
asking *another* person to curate a scoped view. Here every replica is the
same person.

## The model: a grow-only set

A ledger is a **G-Set** of content-addressed, self-verifying events:

- `id = sha256([0, pubkey, created_at, kind, tags, content])` (NIP-01), so
  two replicas that hold "the same event" hold a bit-identical record with
  the same id — there is nothing to reconcile field by field.
- `Event::verify` gates every event on the way in, on every replica. A
  replica is untrusted infrastructure in exactly the sense §8 gives the
  mailbox: **it may withhold or serve stale data; it cannot inject or
  forge one.** `qw_client_core::EventStore` already enforces this on
  `append` and on load.
- **merge = set union**, keyed by id. Union is commutative, associative and
  idempotent, so replicas that have seen the same events — in any order,
  with any gaps between — hold the same set. Strong eventual consistency,
  no locks, no round the counterparty has to be awake for (§4).

Every view the client computes — profile (NIP-QW03), contract state
(NIP-QW01 §"State machine"), `net_position` and trust paths (§5) — is a
**pure fold over that set**, recomputed fresh each time (`qw_protocol::
contract`, `qw_protocol::trust`: "no separate balance store, ever"). A
replica with a stale view therefore cannot break an algorithm or a
profile: signing a new event only adds one element to a set, and the fold
re-runs over the union. A divergence is a temporarily incomplete *view*,
and it heals on the next anti-entropy round.

## Anti-entropy

Transport-agnostic, the same split `qw_node::sync` already draws: the
task lives in `qw_node` (alongside `MailboxSync`) over a `LedgerTransport`
trait; HTTP replica-to-replica, via a vault/relay, or over the mailbox as
a fallback carrier are all just implementations.

One round, either direction:

1. **Advertise coverage.** A replica states what it holds compactly —
   per-author high-water `created_at`, or a range/set digest for large
   ledgers (a negentropy-style reconciliation is a valid optimisation, not
   required).
2. **Request the difference.** The peer names ids (or a `since`-per-author
   window) the advertiser appears to lack.
3. **Transfer and verify.** Each event is re-verified with `Event::verify`
   before it is merged. An event that fails is dropped and counted, never
   stored (`EventStore::rejected`).
4. **Idempotent.** Re-offering an event a replica already holds is a
   no-op. `since` windows are **inclusive** and dedup is by id, so
   same-second siblings from two of the author's own devices are picked up
   on the next round rather than silently lost — the same off-by-one
   `qw_node::sync` already guards against.

A replica publishes its own new events (its outbox) to every peer it can
reach, and re-queues anything nobody accepted, exactly as `MailboxSync::
flush` does now.

## Two hazards a stale writer creates, and their fixes

**1. A wrong clock on `created_at`.** For regular events this is
harmless — they join the set regardless. It bites only *replaceable*
state: the profile is kind `10020` in the replaceable band (NIP-QW03,
built 2026-09-05), and "latest per (pubkey, kind)" by timestamp alone
would let a replica with a fast clock overwrite a newer profile. **A
replaceable QW record MUST carry an author-monotonic `revision` integer**
in a `["revision", n]` tag; readers order by `(revision, created_at, id)`,
never by `created_at` alone. The author bumps `revision` on every edit; a
stale replica that has not seen the latest cannot mint a higher one by
accident. (`qw_protocol::events::Event::revision`, `revision_tag`.)

**2. An outbox signed under a since-rotated key.** A replica offline
across a key rotation (NIP-QW09) holds queued events signed by the
superseded key. If that key was *revoked* with `effective_at` before
those events' `created_at`, NIP-QW09 requires a verifier to raise an
alert — and an honest week-old device is indistinguishable from a hostile
post-revocation signer by signature alone. Mitigations, in order:

- The `account_id` anchor and the chain of `PersonRecordAmendment` (kind
  9081) are the **retained list of which pubkey was authoritative when**.
  Verification of any event is: signature valid, *and* the signer was
  authoritative for the account at `created_at` per that chain.
  Non-retroactive — pre-`effective_at` events under the old key stay
  valid, and there is no mass re-signing (re-signing would change every
  id).
- A **grace window** measured from when the replica could first have
  learned of the amendment (best-effort, transport-dependent).
- On learning of the rotation, the replica **re-signs its unsent outbox
  under the current key and keeps the prior signature** — a
  re-attestation envelope carrying the new `sig` plus the prior
  `(pubkey, sig, id)`, so nothing queued is lost and the old→new lineage
  is auditable. Envelope shape: *proposed, TBD* (see NIP-QW09
  §"Device subkeys").

## Replica trust and the signing surface

Sync itself grants a replica nothing: verify-on-ingest means a compromised
web box can stall or serve stale data but cannot forge. What a warm
`qw-web` instance *can* do is sign genuine new events while it holds a
live key — the custody surface analysed for the hybrid deployment, and the
reason multi-user server signing ships with an explicit "no memory-dump
protection, no uptime guarantee" disclaimer and the single-user server is
the path for anyone who needs the key never to leave their own box.

The structural bound is **device subkeys** (NIP-QW09, proposed kind
9082): a controller delegates routine signing to per-device keys that are
revocable by a controller-signed record *without* quorum. A `qw-web`
instance holds a subkey; its compromise is then a bounded, cheap
recovery — revoke the subkey, re-sign in-flight items under a fresh one,
retain the old signatures — not a lost identity.

## Rust

`qw_node::ledger` (built 2026-09-05): `LedgerSync` (the anti-entropy
round), the `LedgerTransport` trait (sibling to `qw_node::sync`),
`LedgerCoverage`, `PullResponse`. `qw_client_core::Session::ledger_round(
transport, peers)` drives one round and merges what it pulls into the
`HistoryStore`. `qw_client_core::HttpLedger` is the HTTP implementation,
against a peer's `POST /ledger/pull` (body = coverage, reply =
`PullResponse`) and `POST /ledger/push` (body = events, reply = new-count).
The **single-user** `qw-web` host serves both routes — it is one identity,
so the operator's phone or another of their own boxes syncs against it
directly. `EventStore` is the on-disk `HistoryStore`; the `qw-web`
multi-user host adds an encrypted-at-rest one. None of it depends on
`qw_server::mailbox`.

**Coverage is the exact set of held ids** (`LedgerCoverage`), not a
per-author high-water. The advertise/request steps above describe a
high-water *or* a digest; a high-water alone cannot express "I have
everything after T but am missing something before it", which is exactly
what two devices that both authored for one identity produce, so the
first cut sends the full id set (O(ledger), always correct). A
negentropy-style digest is the sanctioned later optimisation for a large
ledger.

Still open: the **multi-user** `qw-web` `/ledger/*` routes — they need a
per-account authentication channel (a device subkey, NIP-QW09) before a
box holding many identities can safely serve one identity's ledger to its
replicas — a scheduled `ledger_round` loop in the client, and the
re-attestation envelope of hazard 2.
