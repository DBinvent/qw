# QW knownby.work — Skills confirmed by the people you worked with. Found through friends of friends.

A referral-network protocol for trust-based work exchange, built on
Nostr. See `todo-impl.md` for the implementation plan and current status.

**Status:** early prototype (protocol layer + a local referral-routing
demo). Nothing here is ready for real transactions or real personal data.

## Try it

Rust stable, no services, no network. Everything below runs locally and
publishes nothing anywhere — there is no relay and no public gateway yet,
so this is a developer path, not a way to join a running network. What
joining *will* mean is below.

```sh
cargo test --workspace
cargo run -p qw-node --example referral_demo
cargo run -p qw-node --example bootstrap_from_git -- <path/to/a/git/repo>
```

**`referral_demo [n] [skill_tag] [max_hops]`** builds a small-world contact
graph of `n` synthetic nodes (300 by default), fires a skill query from a
random one, and reports what greedy routing cost against a full-flood
estimate:

```
First match at 1 hop(s); 34 total matches found within 3 hops, deduped by pubkey
Messages sent: 93 queries, 113 answers
Naive full-flood estimate at avg degree ~5 over 3 hops: ~125 messages
```

It is a demo, not a statistical claim — the graph generator is untuned and
run once. See NIP-QW06's scope note.

**`bootstrap_from_git [repo_path]`** reads a real repository's history and
prints *candidates*: skill tags inferred from the file types a contributor
touched, and `Introduction` pairs from `Co-authored-by:` trailers — real
evidence that two people worked together. It signs and publishes nothing.
Git identifies people by email, not by a QW key, and the tool holds nobody's
signing key; each contributor generates their own identity and signs only
what they confirm.

## How joining works

Not reachable yet — no relay, no gateway — but the mechanism is specified
and implemented at the protocol layer, so it is worth stating plainly rather
than leaving to a future FAQ. The same thing written for a visitor rather
than a developer, including what is and is not released, is at
<https://knownby.work/join> (`landing/app/join/page.tsx`).

1. **Generate an identity.** In the app — the client is a phone app first
   (`app/`, Tauri v2), and first launch makes the key on the device. Under
   it that is `qw_protocol::identity::Identity::generate()` — one secp256k1
   keypair behind both a `did:key` controller id and a Nostr pubkey. There
   is no account to create and no server that could issue one; losing the
   key is losing every record signed with it.
2. **Get connected**, by any of the three shapes of
   [NIP-QW07](protocol/nips/NIP-QW07-introduction.md) kind `9060`:
   - **Follow someone's invite link** — `knownby.work/i/<npub>`, posted on
     LinkedIn or anywhere else a professional history already lives. You and
     the publisher exchange introductions and you are a hop-1 contact
     immediately, whether you were four hops away in the graph or not in it
     at all. No invitation to wait for, no cohort to be admitted to.
   - **Self-introduction** to someone you found via a
     [NIP-QW06](protocol/nips/NIP-QW06-referral-query.md) referral query.
   - **Mutual introduction**, where an existing contact introduces you to
     another of theirs and carries the connecting chain.
3. **Publish what you can do.**
   [NIP-QW03](protocol/nips/NIP-QW03-profile-skill-tags.md) skill tags, with
   completed contracts as the evidence behind them.

The contact graph is the membership list; there is nothing else to sign up
to. Note what an edge does and does not mean: it makes you *reachable* —
queries can route to you, offers can arrive. It says nothing about your
competence, because trust in this system is computed only from completed,
countersigned work. A stranger who arrives through a link and a stranger
four hops away score the same until either of them finishes a contract.

## Legal notices

Read these before publishing anything through this protocol, and before
referencing this project in any external pitch, investor conversation, or
public description. Neither notice is legal advice.

### Co-authorship, not barter

This protocol's co-authorship framing is not a general exemption from
barter/service taxation. It holds only when work is on a declared
open-source project (the output is public and non-appropriable) and no
project involved is controlled by the counterparty in a way that
privatizes the benefit. Direct bilateral work-for-work, or contribution
to a counterparty-controlled private project, falls outside this framing
and is the participants' own tax responsibility to assess — this
protocol does not determine or launder that responsibility. This framing
has not been confirmed by a written tax attorney opinion; treat it as an
engineering description of the system's structure, not legal advice.

### Deletion rights

Deletion on this protocol is advisory only. A relay may honor a deletion
request, but nothing requires it to, and any relay, contact, or archive
that already copied a record may keep it indefinitely — publishing here
is not equivalent to deleting or fully controlling that data afterward.
If your jurisdiction grants you a legal right to deletion of personal
data (for example under the EU GDPR or an applicable US state law),
publishing a record through this protocol may not by itself satisfy that
right. Do not publish information here that you may later be legally
required to be able to delete.

### Not built, and not on the roadmap

This protocol does not include Shadow Quant, any cross-token exchange
mechanism, or any mirrored on-chain/ERC-20 ledger of Quants — each would
reintroduce the money-transmission, Sybil, and tax-classification
problems the design exists to avoid.

---

Both disclosures above are also kept as constants
(`qw_protocol::legal::CO_AUTHORSHIP_BOUNDARY_NOTICE`,
`qw_protocol::legal::DELETION_RIGHTS_DISCLOSURE`) so a future client can
display them verbatim instead of re-deriving the wording. Keep the two
copies in sync by hand — same convention as the NIP docs in
`protocol/nips/`.
