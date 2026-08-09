# NIP-QW03: Profile / skill tags

`draft` — kind `9020`

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

## Kind 9020 — Profile / skill tags

Tags: one `["t", <skill tag>]` per tag in `skill_tags`.

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

This is a regular (non-replaceable) event kind, not addressable/
parameterized-replaceable — publishing a new one does not retract the old
one at the protocol level. A client showing "current" skill tags should
take the most recent kind-9020 event per pubkey; whether to also surface
skill-tag history is a client UX choice, not fixed by this NIP.
