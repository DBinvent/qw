# Structured profile fields + visibility — design

`draft` — 2026-09-09. Prompted by "profile missed details like name,
education, job history, etc … with a visibility level". Companion to
`protocol/nips/NIP-QW03-profile-skill-tags.md`. **Tiers 1–2 are built**
(see Phasing); Tier 3 is still design.

## The tension

NIP-QW03 is blunt: kind `10020` is **public by design** — relays holding
pending referral queries read it to route, and it can ride along as the
`profile` field of a kind-`9051` answer. "Do not put anything here that
isn't meant to be public."

So "a visibility level per field" cannot be a flag on a public event. The
only honest way to make a field non-public is to **not put it in `10020`**.
That splits the work into three tiers.

## Best-practice field set (what a professional profile carries)

| Field | Shape | Natural visibility | Routing value |
|---|---|---|---|
| `display_name` | line, ≤80 | per field | shown |
| `alt_name` | line, ≤80 — an alias | per field | shown |
| `headline` | line, ≤120 | per field | shown, high — "Backend engineer · Rust/axum" |
| `bio` | text, ≤600 | public | shown |
| `location` (coarse) | line, e.g. "Berlin, DE" | public or contacts | shown |
| `languages` | list of `{tag, level}` | public | shown, some |
| `employment[]` | `{title, org, start, end?, summary?}` | **per entry** | shown |
| `education[]` | `{school, field?, start?, end?}` | **per entry** | shown |
| `certifications[]` | `{name, issuer?, year?, url?}` | **per entry** | shown |
| `links[]` | `{network, url}` | public | indexed (already in QW03) |
| `contact_email` | line | **contacts / private** | never |
| `phone` | line | **contacts / private** | never |
| `availability` | enum `open` / `selective` / `closed` (+ note) | **contacts** | never |
| `rate` | line, free text | **contacts / private** | never |
| precise `location` | line | **contacts** | never |

## Visibility model

Three levels, chosen per field in the editor:

- **Public** → serialised into kind `10020` content, exactly as today.
  Additive: an older reader ignores unknown keys.
- **Contacts** → serialised into a new **kind `10021` — extended
  profile**, a replaceable event whose `content` is a NIP-44 payload
  encrypted to a **contacts key**. You hand that key to a peer when a
  contract with them is countersigned (or on a mutual follow); rotate it
  when you drop a contact, and re-wrap for the ones who remain. A
  non-contact fetches `10021` and simply cannot read it. Delivered over
  the mailbox like any other event.
- **Private** → never leaves the device. Stored in the local profile
  draft so the editor round-trips it, published nowhere. Useful for notes
  to self ("rate floor", "recruiter blocklist").

The editor shows one control per field **and per list entry**:
`Public / Contacts / Private`, defaulting to Public. `10021` (Tier 3) is
only signed once any field or entry is set to Contacts.

## Phasing

**Tier 1 — additive public text. Built 2026-09-09.** `display_name` (now
visibility-gated — keep your real name Private and publish only
`alt_name`, an alias), `alt_name`, `headline` (`< 120`), `bio` (`< 600`),
`location` (`< 80`) as optional keys on `10020` content —
`qw_protocol::events::ProfileSkillTags` + `validate`, NIP-QW03,
`qw_client_core::profile` (`ProfileField` / `FieldVisibility` /
`ProfileLocal`, `build_signed` mirrors the `Public` copy), a persisted
`SyncState.profile_local`, `Session::{profile_view, set_profile,
profile_local}`, and a field + `Public / Contacts / Private` picker each
in the profile editor.

**Tier 2 — structured lists. Built 2026-09-09.** `employment[]`,
`education[]`, `certifications[]` on `10020` content — protocol structs
`Employment` / `Education` / `Certification`, count caps (20 / 15 / 30)
and per-field length caps in `validate`. `qw_client_core` wraps each as
`Local{Employment,Education,Certification}` (`#[serde(flatten)]` entry +
`visibility`); **visibility is per entry** — `build_signed` filters to
the `Public` entries and drops the `visibility` key on the way into the
event. UI: an add/remove list per section (`makeEntryList` in
`index.html`), each row a small sub-form + its own `Public / Contacts /
Private` picker.

**Both tiers: the visibility flag works for `Public` (published) and
`Private` (device-only). `Contacts` is accepted but held locally — same
as `Private` — until Tier 3.** Not yet done: `languages`, display sort by
`start` desc, `contact_email` / `phone` / `availability` / `rate` (all
contacts-tier, so blocked on Tier 3).

**Tier 3 — kind `10021` + the contacts key (the real feature).**
New replaceable kind, NIP-44 content, contacts-key issue/rotate/re-wrap,
mailbox delivery, `profile_view` merges `10021` fields when the viewer
holds the key. This is where "visibility level" actually bites and where
the protocol decision is — worth its own NIP section (NIP-QW03 §"Contacts
key", or a NIP-QW16). Do not start Tier 3 until Tiers 1–2 are in and the
key-distribution story is agreed.

## Open questions

- Contacts key: one key rotated on removal (simple, a removed contact
  keeps the *old* snapshot until next rotation) vs. per-contact wrapping
  (no leakage, O(contacts) work per publish). Lean simple.
- Does `10021` ride along on a `9051` answer for a matched-but-not-yet
  contact? Probably not — that is exactly the person who should not see
  it yet.
- `availability` is the one contacts-tier field with real routing value
  (a query could prefer `open` people). Keeping it out of `10020` is a
  deliberate cost.
