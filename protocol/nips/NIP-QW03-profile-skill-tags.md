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
tag in `skill_tags`.

```json
{
  "display_name": "vk",
  "skill_tags": [
    "it/backend/languages#rust",
    "it/backend/frameworks#axum"
  ]
}
```

`display_name` is optional. `skill_tags` are taxonomy leaves
(`/taxonomy.yaml`, format `sector/domain[/area]#skill`, max 5 per the
taxonomy's own rule) — normalize free-text input through `/synonyms.yaml`
**before** signing this event; tag fragmentation ("nodejs" vs "node.js")
is unrecoverable once it's in a signed record; see the header comment in
`/synonyms.yaml`.

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
