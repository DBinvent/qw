# NIP-QW03: Profile / skill tags

`draft` — kind `10020` (replaceable; kind `9020` is the frozen legacy
form, see "Replaceable, with a revision" below)

## Abstract

A self-published statement of who you are and what you can do. Replaces
nothing — it is not authoritative about skill *possession* (that's earned
through the signed contract history reachable via `qw_protocol::dual_index`
and scored per-viewer, §5); it's what a referral query (§3) actually
matches against and what a profile view displays alongside the earned
history.

Per `qw-design-faq.md`'s privacy table: skill tags are **public** — they
are the routing information relays holding pending referral queries need
to read (§"Q: ... Relays holding pending referral queries must read skill
tags in..."). Do not put anything here that isn't meant to be public.

## Kind 10020 — Profile / skill tags

Tags: `["revision", <n>]` (see below), then one `["t", <skill tag>]` per
tag in `skill_tags`, then one `["r", <url>]` per entry in `links` (the
Nostr URL-reference convention, so a relay can index the profile by an
external handle too).

```json
{
  "display_name": "vk",
  "alt_name": "v. krinitsyn",
  "skill_tags": [
    "it/backend/languages#rust",
    "it/backend/frameworks#axum"
  ],
  "skill_levels": {
    "it/backend/languages#rust": "expert",
    "it/backend/frameworks#axum": "senior"
  },
  "skill_sources": {
    "it/backend/languages#rust": "commit-analysis"
  },
  "headline": "Backend engineer · Rust / axum",
  "bio": "Ten years on payment systems; now mostly protocol work.",
  "location": "Berlin, DE",
  "employment": [
    { "title": "Staff Engineer", "org": "Acme", "start": "2021", "summary": "payments platform" }
  ],
  "education": [
    { "school": "State U", "field": "CS", "start": "2011", "end": "2015" }
  ],
  "certifications": [
    { "name": "CKA", "issuer": "CNCF", "year": "2022" }
  ],
  "links": [
    { "network": "github", "url": "https://github.com/vk" },
    { "network": "linkedin", "url": "https://www.linkedin.com/in/vk" }
  ]
}
```

`display_name` and `alt_name` are optional (`< MAX_NAME_LEN` = 80
characters each). `display_name` is present here only if the holder set
its visibility to `public` — a client MAY keep it local and publish only
`alt_name`, an alias (see `app/profile-fields.md`). `skill_tags` are
taxonomy leaves
(`/taxonomy.yaml`, format `sector/domain[/area]#skill`, max 5 per the
taxonomy's own rule) — normalize free-text input through `/synonyms.yaml`
**before** signing this event; tag fragmentation ("nodejs" vs "node.js")
is unrecoverable once it's in a signed record; see the header comment in
`/synonyms.yaml`.

**Size limits.** Every entry in `skill_tags` — a taxonomy leaf or a
custom label — MUST be **1 to 79 Unicode characters** (`<
MAX_SKILL_TAG_LEN`), and a profile MUST carry **fewer than 80** of them
(`< MAX_PROFILE_SKILLS`). A custom skill name is a label, not a sentence;
a profile is a focused self-description, not a keyword dump — and both
bounds keep the `["t"]` tags a relay indexes cheap. A conforming client
MUST call `ProfileSkillTags::validate` before signing; a reader SHOULD
reject an event whose content exceeds either bound rather than truncate
it silently.

`skill_levels`, `skill_sources` and `links` are all **optional and
additive** — an event that omits them is valid, and an older client that
does not know them parses the rest unchanged. Keys of `skill_levels` and
`skill_sources` MUST be members of `skill_tags`; a reader drops any that
are not.

**`headline`, `bio`, `location`** — optional free-text, additive.
`headline` is a one-line "what I do" (`< MAX_HEADLINE_LEN` = 120 Unicode
characters); `bio` a short paragraph (`< MAX_BIO_LEN` = 600); `location`
a coarse place, "Berlin, DE" not a street address (`< MAX_LOCATION_LEN` =
80).

**`employment`, `education`, `certifications`** — optional structured
lists, additive. Each entry is a small object of free-text fields (dates
are whatever the holder typed; an empty `employment.end` reads as
"current"):

| list | fields | bound |
|---|---|---|
| `employment` | `title` (required), `org`, `start`, `end`, `summary` | `< MAX_EMPLOYMENT` = 20 entries |
| `education` | `school` (required), `field`, `start`, `end` | `< MAX_EDUCATION` = 15 |
| `certifications` | `name` (required), `issuer`, `year`, `url` | `< MAX_CERTIFICATIONS` = 30 |

Each short field is `< MAX_ENTRY_FIELD_LEN` = 160 characters,
`employment.summary` `< MAX_ENTRY_SUMMARY_LEN` = 400. An entry with its
required field empty is rejected, not dropped silently.

`ProfileSkillTags::validate` enforces every bound above; a reader SHOULD
reject an event that fails it rather than truncate. Like the rest of this
event, all of these are **public**. A client MAY hold a fuller profile
locally with a **per-field and per-entry visibility** and publish only
what the holder marked public — see `app/profile-fields.md` for that
model and the contacts-only tier (encrypted, unspecced) it points at.

## Level, provenance and external links

None of these three fields is evidence. They are self-asserted decoration
on the claim, and a viewer weighs them against the countersigned contract
history (§5) exactly as they weigh the bare tag.

**`skill_levels`** — the holder's own estimate of their standing in that
skill, one of `beginner` / `intermediate` / `senior` / `expert`. It does
not affect referral routing or scoring; it is a hint for a human reading
the profile. A tag absent from the map has no stated level.

**`skill_sources`** — where the tag came from, one of:

| value | meaning |
|---|---|
| `self` (default) | the holder typed it |
| `commit-analysis` | a client suggested it from the holder's own commit history (`qw_node::bootstrap`) and the holder kept it |

A tag absent from the map is `self`. `commit-analysis` is the
"algorithmic" evidence class a UI shows before any job has approved the
skill — strictly weaker than a countersigned contract, strictly stronger
than nothing.

**`links`** — a flat list of `{ network, url }` pointing at the holder's
presence on other networks (GitHub, LinkedIn, a personal site). `url`
MUST be `http(s)`. `network` is a free lowercase label. This is the
"verify me elsewhere until a job does it for you" surface: a viewer with
no countersigned contract to go on can still corroborate a claim by hand.
Each link is also emitted as an `["r", url]` tag so a relay can route on
it. Clients SHOULD bound the list (the reference client: 8) and de-dup by
`url`.

### Evidence class, as a client renders it

A client showing a skill row picks the strongest of:

1. **`approved by job`** — at least one countersigned contract in reach
   carries the tag **and** settled through a credit issuance (NIP-QW02).
   The strongest class, and the one whose score (§5) carries the
   Quant-magnitude term.
2. **`side-settled`** — a countersigned contract in reach carries the tag
   but was settled off-system (NIP-QW01 kind 9006, no credit issuance).
   Still scored (§5), but with no Quant-magnitude term and multiplied by
   the viewer's `side_settled_factor` — below `approved by job`, above a
   self-declared or commit-analysis tag.
3. **`algorithmic`** — no such contract, but `skill_sources[tag]` is
   `commit-analysis`.
4. **`unproven`** — a bare self-declaration, nothing corroborating.

The level pill (if any) is shown independently of the class.

## Replaceable, with a revision

Kind `10020` sits in Nostr's **replaceable** range (10000-19999): a relay
keeps only the latest per `(pubkey, kind)`. This is deliberate. A profile
is a standing statement of intent about future contact — it does not
confront the past and is not evidence of anything (the ledger of
countersigned contracts is the record, and it is separate). Keeping every
edit forever would preserve nothing anyone should read while costing three
things: a permanent public trail of every past version of yourself, a
fetch-all-and-sort on every client, and a relay able to serve a stale
profile indistinguishably from the current one.

"Latest per `(pubkey, kind)`" by `created_at` alone is not enough once the
ledger is replicated across a phone and one or more `qw-web` boxes
(NIP-QW12): a replica with a fast clock could overwrite a newer profile.
So every kind-10020 event **MUST** carry an author-monotonic
`["revision", <n>]` tag — `n` a non-negative integer the author increments
on every edit — and readers **MUST** order candidates by
`(revision, created_at, id)`, taking the greatest. A missing or
non-numeric `revision` reads as `0`.

### Migration from kind 9020

Kind `9020` was the original, in NIP-01's regular (never-replaced) range;
`todo-impl.md` §7 records the decision to move. Nothing signs `9020` any
more. A reader resolving "the current profile" for a pubkey:

1. If any kind-`10020` event exists, take the one greatest by
   `(revision, created_at, id)`. A `10020` event always wins — the `9020`
   events are frozen history, not a current statement.
2. Otherwise fall back to the most recent kind-`9020` event by
   `created_at`.

`9020` events are never rewritten or re-signed (that would change their
`id`); they simply stop being authoritative the moment a `10020` exists.
Whether to surface skill-tag history at all is a client UX choice, not
fixed by this NIP.

## Visibility past a direct contact

The profile has no `p` tag and no addressee, so nothing routes it: a
peer holds yours only if they are a replica of you (NIP-QW12), synced it
off a shared relay, or received it inside another event. NIP-QW06 adds
the last of those — a `10020` may travel as the optional `profile` field
of a kind-`9051` skill answer, so a requester who matched you through a
vouched referral path, with no edge to you, still gets your whole
self-description rather than the single tag their query hit.

That is the intended reach: **open to view by anyone the referral
network legitimately connects to you, still not a broadcast.** The event
is identical whichever way it arrives, is re-verified on receipt, and is
folded by the same `(revision, created_at, id)` rule — carriage never
confers authority, the signature does.
