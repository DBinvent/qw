# QW protocol NIPs

Custom Nostr event kinds for the QW protocol, written NIP-shaped (per
[nostr-protocol/nips](https://github.com/nostr-protocol/nips)) but living
here rather than in the upstream NIPs repo — these are QW-specific, not a
proposal for the wider Nostr ecosystem.

All QW kinds live in **9000–9099**. NIP-01 splits kind-space into bands:

| Range | Meaning |
|---|---|
| 1000 ≤ kind < 10000 | Regular event — relays store it, it's never replaced or expired |
| 10000 ≤ kind < 20000 | Replaceable — only the latest per pubkey+kind is kept |
| 20000 ≤ kind < 30000 | Ephemeral — not stored at all |
| 30000 ≤ kind < 40000 | Addressable/parameterized-replaceable (needs a `d` tag) |

Every QW record is a permanent signed contract artifact — offers,
signatures, credentials, flags — so **regular** is the only correct band.
9000-9099 was picked simply as an unclaimed sub-block within it.

| Kind(s) | Name | Spec | Rust |
|---|---|---|---|
| 9000-9004 | Job lifecycle (incl. counteroffer) | [NIP-QW01](./NIP-QW01-job-lifecycle.md) | `qw_protocol::events::kinds` |
| 9010 | Credit issuance | [NIP-QW02](./NIP-QW02-credit-issuance.md) | `qw_protocol::events::kinds` |
| 9020 | Profile / skill tags | [NIP-QW03](./NIP-QW03-profile-skill-tags.md) | `qw_protocol::events::kinds` |
| 9030 | Dispute annotation | [NIP-QW04](./NIP-QW04-dispute-annotation.md) | `qw_protocol::events::kinds` |
| 9040-9041 | Cascade block | [NIP-QW05](./NIP-QW05-cascade-block.md) | `qw_protocol::events::kinds` |
| 9050-9051 | Referral query | [NIP-QW06](./NIP-QW06-referral-query.md) | `qw_protocol::events::kinds`, `qw_node` |
| 9060 | Introduction | [NIP-QW07](./NIP-QW07-introduction.md) | `qw_protocol::events::kinds` |
| 9070-9071 | History request/response | [NIP-QW08](./NIP-QW08-history-request.md) | `qw_protocol::events::kinds` |
| 9080-9081 | Person record amendment | [NIP-QW09](./NIP-QW09-person-record-amendment.md) | `qw_protocol::events::kinds`, `qw_protocol::recovery` |
| 9090 | Chain-calculation result | [NIP-QW10](./NIP-QW10-chain-calculation-result.md) | `qw_protocol::events::kinds`, `qw_server` |
| 9091 | Bulletin listing | [NIP-QW11](./NIP-QW11-bulletin-listing.md) | `qw_protocol::events::kinds`, `qw_server` |

## Conventions shared across all QW kinds

- **`id`/`sig`** follow NIP-01 exactly: `id` = sha256 of the compact JSON
  array `[0, pubkey, created_at, kind, tags, content]`; `sig` = BIP-340
  Schnorr signature over `id`, by the key behind `pubkey`.
- **`content`** is always a JSON object; the shape is documented per kind
  below and defined as a Rust struct in `qw_protocol::events::kinds`, kept
  in sync by hand (there is no schema codegen yet — if you change one,
  change the other).
- **`p` tag** = the *other* party in a 1:1 record (`["p", <hex pubkey>]`).
  This is what makes dual indexing work (see `qw_protocol::dual_index` and
  `todo-impl.md` §2): "all records about pubkey A" is a plain relay filter
  over records other people published about A, with no self-report needed.
- **`e` tag** = reference to a prior step's event id (`["e", <hex id>]`),
  or, with a marker (`["e", <hex id>, "", "cascade-source"]`), a
  non-lifecycle cross-reference — see NIP-QW05.
- **`t` tag** = one per skill tag (`["t", <tag>]`), for relay-side referral
  routing (§3). Tag values are taxonomy leaves from `/taxonomy.yaml`,
  normalized through `/synonyms.yaml` before signing.
