# Multi-user `qw-web` — the encrypted account store and its unlock

Design + partial build (see **Status** below). `qw_client_core::Session` and the
`KeyStore` / `HistoryStore` traits do **not** change — everything here lives
inside the `qw-web` crate: a Postgres account store, two encrypted store
implementations over an in-RAM key, and an `unlock` module that holds that key's
wrappings. The single-user server (`src/main.rs`) is untouched; this is a second
binary mode, or a second `Web` behind the same router.

Supersedes the converged 2026-09-02 note (password → KEK → DEK): the **DEK is
now the master key**, and the password-derived KEK is one wrapping among
several.

**Status (2026-09-03).** Landed, tested, not yet wired into `main`:

- `src/envelope.rs` — `MasterKey` (`seal` / `open` the account blob,
  `Debug`-redacted, zeroized on drop), `Wrapping` (one dated KEK row —
  `seal_secret` / `unwrap_secret` through Argon2id, `created_at` /
  `not_after` / `rotate` / `stale`), `KekKind` (`#[non_exhaustive]`,
  `Secret` only).
- `src/accounts.rs` + `src/accounts.schema.yaml` — the directory tier in
  **Postgres**, on the sibling-service stack (register-bo / dashta):
  `marg` supplies the connection string at the entrypoint,
  `accounts::migrate()` applies the YAML through **`schema_guard_tokio`**
  at startup, `PgAccountStore` runs the queries on **`sqlx`**.
  `AccountRecord`, `normalize_nick` (trim + NFC + casefold), `blind_index`
  (keyed BLAKE2b-256), `seal_nick` / `open_nick` (nickname AEAD under the
  server key, AAD-bound to `account_id`). `AccountStore` trait +
  `PgAccountStore` (`by_blind_index` `LIMIT 5` fan-out, unique-violation →
  `Conflict`) + an in-memory test double.
- `src/account_session.rs` — the decrypted, in-RAM side of an account.
  `seal_new_account` (identity, no events — for `register`) and
  `open_account` (master + `account_id` + blob -> the `Identity` for
  `Session::with_identity` and an `EncHistoryStore`). `EncHistoryStore`
  **is** the encrypted `HistoryStore`: `events()` a plain RAM slice,
  verify-on-open (bad events dropped, counted in `rejected()`), `append`
  verifies + dedupes + re-seals the whole payload under the same master
  and sets `dirty`; `sealed()` / `mark_persisted()` let the caller write
  it back through `AccountStore::set_blob`. There is **no separate
  `KeyStore` impl** — the identity travels inside the same sealed payload
  (one blob, one seal, atomic) and is handed to `Session::with_identity`;
  the single-user `Vault` key file has no multi-user equivalent.
- `src/unlock.rs` — the KEK set. `present_secret(wrappings, secret)` tries
  every `Secret` wrapping, fresh rows first, returns the recovered
  `MasterKey` + which tag + whether that row was stale. `add_secret` /
  `drop_tag` (never the last — `WouldOrphan`) / `rotate_secret`
  (passphrase change) / `flag_for_rotation` / `sweep_stale` (drop stale
  rows only if a fresh one survives). `enumerate` → a `WrappingInfo` view
  (tag / kind / dates / rotate / stale, no key material).
- `src/multiuser.rs` — the host. `QW_MULTIUSER` set → `main` calls
  `serve()`: `marg::ArgConfig` → `accounts::migrate` → `PgAccountStore`,
  `QW_SERVER_KEY` (64 hex) is the directory key, a sweeper task evicts
  idle/expired sessions. `MuWeb` holds `HashMap<token, Arc<Mutex<Live>>>`.
  `POST /auth/register` (nick + secret → mint identity, `seal_new_account`,
  one passphrase `Wrapping`, `AccountRecord::new`, `insert`, session
  cookie), `POST /auth/login` (`by_blind_index` → `present_secret` fan-out
  → `open_account` → `Session`, opportunistic `sweep_stale`, `key_stale`
  flag), `POST /auth/logout` (drop the session → master zeroized). The ten
  `/api/<cmd>` routes are the single-user handlers behind `with_session`:
  resolve the cookie, run the op on the blocking pool under the
  per-session lock, and — still holding that lock — flush the re-sealed
  blob via `AccountStore::set_blob` (`Handle::block_on`, so two ops on one
  session persist in order). Cookie is `HttpOnly; SameSite=Strict;
  Secure`. Idle TTL 30 min, absolute 12 h. **Since 2026-09-05** also
  `GET /session` (login-host + auth probe for the shared UI),
  `POST /api/kek_{list,add,drop,rotate,flag,recovery}` — the KEK-set ops
  over `unlock::*`, each a fetch -> mutate -> `AccountStore::set_wrappings`
  under the per-session lock (a clone of the master lives in `Live` for
  the wrap-requiring ones) — and `POST /api/seed_export`
  (`Session::identity_secret_hex`, the 32-byte secret as `identity.key`
  hex). `register` now mints a recovery-code KEK too and returns the code
  once in `{ identity, recovery_code }`; `kek_recovery` re-mints it.
- `src/ratelimit.rs` — fixed-window rate limiting on `/auth/register` and
  `/auth/login`. Register: 5/hour per source IP. Login: 20/5min per source
  IP *and* 10/5min per target account (the normalised nick, checked before
  Argon2id runs), so neither a spray across many accounts from one IP nor
  a focused brute force spread across many IPs gets far; a 429 carries
  `Retry-After`. Source IP is `CF-Connecting-IP` first (the tunnel forwards
  the Cloudflare edge's header), then `X-Forwarded-For`, then the TCP peer
  address via `ConnectInfo` for a direct/local connection — used only for
  rate-limit bucketing, never for authorization. One process, in-memory;
  a multi-instance deployment would need a shared store, same caveat as
  the session table.

Workspace `cargo test` 274 green + 2 Postgres round-trips (`#[ignore]`,
`QW_TEST_DATABASE_URL`) verified against a throwaway PG 16 — the accounts
CRUD, and a full register → write → drop-all-sessions → relogin →
read-back through `sqlx`. The shared `app/ui/index.html` carries the
sign-in panel and (since 2026-09-05) a "Keys" section — add / remove a
passphrase, (re)generate a recovery code, reveal the identity key. A live
curl smoke of `register (recovery code in body) → /session →
kek_list/add/recovery/rotate/drop → seed_export → login-with-each` was run
against a throwaway PG on 2026-09-05.
`raw/scripts/deploy-qw-web.sh` installs it (default mode);
`qw.knownby.work` (public, no Access) tunnels to `:8112`.

Remaining, roughly in priority for a public host: email-verify (parked
low-priority — see `todo-impl.md`), the delta blob format, and the
multi-user `/ledger/*` routes (NIP-QW12; need a per-account auth channel —
a device subkey — before a many-identity box serves one identity's ledger
to its replicas; the single-user host already serves them).

- **Outbox persistence — built 2026-09-05.** `qw_client_core` gained
  `SyncState` (outbox event ids + per-server poll cursors),
  `Session::sync_state` / `Session::restore_sync_state`, and a default-no-op
  `HistoryStore::persist_sync_state` hook. `EncHistoryStore` folds the
  `SyncState` into the same sealed payload (`PayloadIn`/`PayloadOut` gained
  a `sync` field, `#[serde(default)]` so pre-existing blobs read as empty),
  re-sealing only when it changes; `start_session` calls
  `restore_sync_state` after `open_account`. A restart between
  `Session::author` and a mailbox flush no longer loses the queued event,
  and a cold start re-asks only the one-second cursor overlap.
- **Full re-seal per `append` (and per sync-state change)** is O(ledger).
  Fine now; a chunked/delta blob format is a later optimisation.

## What a database dump may reveal

Only *"N accounts exist"*. No pubkey, no identity key, no contract graph, no
skill tags — every identifying byte is under the master key (below), which is
never at rest in the clear. The one plaintext-adjacent column is a blind index
over the nickname — a keyed hash (BLAKE2b-256 under `server_key`), useless
without that key — beside the nickname's own AEAD ciphertext.

## The record — two confidentiality tiers

| tier | columns | protected by |
|---|---|---|
| **directory** | `nick_cipher` (AEAD), `nick_index = keyed-BLAKE2b(server_key, nfc+casefold(nick))` | a per-instance `server_key` the operator holds — so the operator can re-index rows on a server merge, and nothing more |
| **account** | identity key, event log (`HistoryStore` blob), KEK wrapping list, recovery wrapping | the **master key**, unwrapped only into the RAM of a live session |

- `account_id` — random UUID, the stable primary key. Nothing else is stable
  enough: a nickname can be re-used after a merge, a pubkey is inside the
  encrypted blob.
- `nick_index` is `UNIQUE` in closed (single-instance) mode. After a merge the
  constraint is dropped; login then fans out to the K ≤ 5 rows sharing an index
  and trial-decrypts each account blob — the AEAD tag is the disambiguator.
  Rate-limit per `nick_index`, cap K.
- Nickname: "anything like an email", min length 8, NFC + casefold + trim.
  Uniqueness is a per-instance choice, never global.

## Master key + KEK set

**Master key** — one per account, random, generated once at registration. It is
the only long-lived secret and the root of the account's secrecy:

- it encrypts the account blob directly (XChaCha20-Poly1305, or `age` with the
  master as the single file key);
- it is **never written at rest in the clear, never sent to the client, never
  logged**;
- it exists in plaintext only inside a live session's process memory, and is
  zeroized on logout, TTL expiry, and restart;
- it **never changes** for the life of the account. Rotating what protects it
  does not touch it or the ciphertext blob.

**KEK set** — an ordered list of independent wrappings of the master key. Each
row: `{ tag, kind, kdf/params, wrapped_master, created_at, not_after?, rotate }`.

- **Login** presents any one KEK. Its holder (browser, the user's cloud, the
  phone) unwraps the master; `qw-web` decrypts the blob and starts the session.
- **Additive.** Adding a factor = wrap the *same* master under a new KEK and
  append the row. Removing one = drop the row. Changing a passphrase = re-wrap
  under the new Argon2id output, replace that one row. No blob re-encryption, no
  re-index, no identity change — all O(1) metadata.
- **Order** is fallback/preference for the UI. Any single row unlocks unless the
  operator opts into "top-N required".

The sources, weakest-standalone first:

1. **User secret** — a passphrase, a raw key, or a BIP-39 seed phrase.
   `KEK = Argon2id(secret, per-account salt)`, derived **in the browser**; the
   secret never transits, only the derived KEK over TLS. Always present — the
   floor every account has.
2. **Hardware key fob** — a FIDO2 / YubiKey-class token. `KEK` from the token's
   `hmac-secret` / PRF extension, or the PIV / `age-plugin-yubikey` path. Unwrap
   needs the physical device and a touch. *(User's term: "eom fob" — device
   class to confirm; see open questions.)*
3. **User-attached KMS** — AWS KMS, GCP KMS, or Vault transit. The wrap/unwrap
   is a `Decrypt` call against a CMK in the **user's own** account, under
   credentials the user delegates. `qw-web` stores only `wrapped_master` + the
   key ARN, never the CMK. The user revoking the grant on their side instantly
   dead-bolts this path.
4. **Authenticator / approval app** — an OATH or push-approval app holding a
   wrapping secret it releases on explicit user action. No extra hardware;
   weaker than (2) because the secret is shared and phishable.
5. **qw mobile app, biometric-gated** — the phone app holds a wrapping key in
   the device secure enclave / Android Keystore and releases the unwrap only
   after a local biometric check, over the companion / `LedgerTransport`
   channel. This is "your phone approves your web session". The wrapping key is
   a **revocable device subkey** (NIP-QW09, proposed kind 9082), not the
   controller, and the phone is already a ledger replica (NIP-QW12) — so a lost
   phone is revoked like any other device without touching the master.

**Dating and rotation.** Every wrapping row carries `created_at`, an optional
`not_after`, and a `rotate` flag the user or operator can set. A flagged or
expired row is refused for new logins; on the next successful login by another
KEK it is re-wrapped fresh or dropped. Because the master never moves, a KEK
compromise is contained by deleting one row. Losing **every** KEK is identity
loss — see recovery.

## Session lifecycle

Login → unwrap master into RAM → decrypt blob → `Session::with_identity(...)`
with an encrypted-at-rest `HistoryStore` that keeps plaintext events in RAM and
re-seals under the master on every ledger change. Idle TTL **and** absolute TTL;
on either, on logout, and on restart, zeroize the master, the unwrapped KEK, and
all derived material.

A warm session is a signing oracle for that user. Login shows the disclaimer
NIP-QW12 already mandates for multi-user signing: **no memory-dump protection,
no uptime guarantee** — the server may restart at any time and every warm
session dies with it.

## Recovery and portability

- **No operator reset.** The operator holds only `server_key` (directory tier).
  It never holds a KEK that unwraps a user's master — the passphrase KEK is
  derived client-side, the KMS and phone KEKs are the user's.
- **Recovery code** — issued once at registration, printed once: an extra
  wrapping row (`tag = recovery`, `kind = code`). Store it offline; it is a full
  KEK.
- **Seed export any time** — the identity (secp256k1 key) leaves as a seed
  phrase on demand, so it survives even total loss of the server blob. The
  ledger re-syncs from replicas / the mailbox (NIP-QW12).

## Deployment

- **Env.** `QW_MULTIUSER` (set = run this host, unset = single-user),
  `QW_SERVER_KEY_FILE` (a file of 64 hex chars — the directory-tier key,
  the deployed form; `QW_SERVER_KEY` inline is the fallback) — **stable per
  instance** or every blind index breaks, `QW_SERVERS` (coordination
  servers, comma-separated), `QW_WEB_ADDR` (default `127.0.0.1:8788`; the
  deploy sets `127.0.0.1:8112`).
- **Postgres.** Records live in one `accounts` table. `marg::ArgConfig`
  reads the connection string from `--db` / a `--file` / the `db` env var;
  the deploy passes the socket form both drivers accept —
  `postgres:///<db>?host=/var/run/postgresql&user=<role>` (sqlx rejects the
  `//user:@/db?host=…` spelling with "empty host"). A dedicated
  role/database, provisioned by the deploy script — the user's call, like
  the single-user deploy. `schema_guard_tokio` applies the table on
  startup; no `sqlx migrate`, no hand-run DDL.
- **No Cloudflare Access.** `qw.knownby.work` is public — the multi-user
  host is meant for open self-registration and carries its own
  nickname/passphrase auth (`--single` on a personal box is the alternative
  for someone who wants no shared surface at all). **Rate-limiting** on
  `/auth/register` and `/auth/login` is built (`src/ratelimit.rs`, above).
  **Recovery** is built — `register` mints a recovery-code KEK and returns
  the code once (the UI reveals it straight after sign-up), `POST
  /api/kek_recovery` re-mints it, and `POST /api/seed_export` gives the
  portable identity-key hex. Email verification stays open, and is now
  parked as low-priority — `todo-impl.md`'s `## Priority` list has the
  reasoning.
- **Where each unwrap runs.** Passphrase KEK: browser (server sees only the
  derived KEK, transiently). KMS: browser, with the user's delegated creds.
  Phone approval: the phone returns the unwrapped master (or a short-lived
  session token) over the companion channel. Server-side the master is held
  only for the live session. Confirm per factor — see open questions.

## What changes in the code (all in `qw-web`)

- **[done]** `src/envelope.rs` — the master key and the KEK wrapping set
  (`Secret` kind only).
- **[done]** `src/accounts.rs` + `src/accounts.schema.yaml` — one
  `accounts` row per account (`account_id` UUID, `nick_index` BYTEA
  UNIQUE, `nick_cipher` BYTEA, `wrappings` JSONB, `blob` BYTEA,
  `created_at` / `updated_at`) in **Postgres**. `AccountStore` trait
  (`insert` / `by_blind_index` / `set_wrappings` / `set_blob`) with a
  `PgAccountStore` and an in-memory test double, so `cargo test
  --workspace` needs no database. Schema is the declarative
  `accounts.schema.yaml`, applied by `accounts::migrate()` through
  `schema_guard_tokio` at startup (safe every boot — it diffs). Connection
  string comes from `marg` (`--db` / `db` env / `--file`), not a bespoke
  env var.
- **[done]** `src/account_session.rs` — `EncHistoryStore`, the encrypted
  `HistoryStore`: `events.jsonl` semantics (verify-on-open, verify +
  dedupe on `append`) but the backing bytes are one AEAD blob sealed under
  the session master key. `open_account` also yields the `Identity` from
  the same payload for `Session::with_identity` — so no separate encrypted
  `KeyStore`; the identity has no key file to store, it is one field in
  the sealed blob. `seal_new_account` builds the initial blob for
  `register`. Since 2026-09-05 the payload also carries a `SyncState`
  (`Session::sync_state`), re-sealed by `persist_sync_state` only when it
  changes. Still deferred: a delta blob format instead of full re-seal per
  `append`.
- **[done]** `src/unlock.rs` — the KEK set: `enumerate` (metadata view),
  `present_secret` (fresh-first, returns master + tag + stale flag),
  `add_secret` / `drop_tag` (never the last) / `rotate_secret` /
  `flag_for_rotation` / `sweep_stale`. Argon2id is the only adapter;
  WebAuthn-PRF / KMS / companion add a `present_*` + `add_*` pair each.
- **[done]** `src/multiuser.rs` — the `MuWeb` host: `register` / `login` /
  `logout`, the `/api/<cmd>` routes behind `with_session` (cookie →
  per-session lock → op on the blocking pool → in-lock blob flush), a
  token-keyed session table with idle + absolute TTL and a sweeper,
  `Secure; HttpOnly; SameSite=Strict` cookie. `main` runs it when
  `QW_MULTIUSER` is set; the single-user path is untouched.
- **[done]** `app/ui/index.html` — a sign-in / register panel. The first
  `/api/*` call comes back 401 on a multi-user host, so `boot()` shows the
  panel; `register` / `login` set the cookie and the same UI runs
  unchanged; a "Sign out" button appears once signed in. The Tauri shell
  and a single-user server never 401, so the panel never shows there.
  **Since 2026-09-05** a `boot()` `GET /session` probe makes the
  login-host / signed-in state reliable on a plain reload too, and gates a
  new **Keys** section — list factors, add / remove a passphrase,
  (re)generate a recovery code, and (behind a disclosure) reveal the
  identity key. `doAuth` surfaces the registration recovery code once, in
  the same accent box.
- **[done]** deploy — `../../../raw/scripts/deploy-qw-web.sh` (default mode)
  + `deploy/qw-web-mu.service`. One script, two modes (`--single` is the
  key-on-disk client). Opens a sudo session up front (`sudo su - -c whoami`,
  like `br.sh`), builds as the invoking user, provisions the `qwweb` system
  user + Postgres role/db (peer auth), generates `/etc/qw-web-mu/server-key.hex`
  once, writes `/etc/qw-web-mu/env`, installs the unit, health-checks
  (`GET /` 200, unauth `/api/*` 401, `accounts` table present). Written,
  **not run** — the user deploys. `cloudflared-config.yml` points
  `qw.knownby.work` at `:8112`.
- **[done]** `src/ratelimit.rs` — fixed-window limiter behind `/auth/register`
  (5/hour/IP) and `/auth/login` (20/5min/IP, 10/5min/account), `Retry-After`
  on a 429.
- **[done 2026-09-05]** outbox persistence — the `SyncState` in the sealed
  payload plus `Session::{sync_state,restore_sync_state}` and the
  `HistoryStore::persist_sync_state` hook in `qw_client_core`.
- **[done 2026-09-05]** KEK management + recovery — `POST /api/kek_{list,
  add,drop,rotate,flag,recovery}` and `POST /api/seed_export` over
  `unlock::*` / `Session::identity_secret_hex` + `AccountStore::
  by_account_id`; `Live` carries a master-key clone; `register` mints a
  128-bit recovery-code KEK and returns it once. Plus `GET /session` and
  the key-page UI.
- still open: the **delta blob format** (see the Status note); email
  verification (parked low-priority).
- `Session` gained `sync_state` / `restore_sync_state` /
  `identity_secret_hex` and `HistoryStore` a default-no-op
  `persist_sync_state`; otherwise **unchanged** (checked against
  `core/src/session.rs`).

## Open questions

- **"eom fob" device class** — FIDO2 `hmac-secret`, PIV, or an OATH-HOTP fob?
  Picks the adapter and whether the browser or the server does the unwrap.
- **`age` as the one wrapping format** — one `age` recipient per KEK is almost
  exactly "one master, many wrappings", and `age-plugin-{fido2-hmac,yubikey,tpm}`
  covers factors 2 and a future TPM path for free. Versus a hand-rolled
  `{kind, params, wrapped}` list. Decide before writing `unlock`.
- **KEK ordering semantics** — UI hint only (any one unlocks), or does the
  operator get "must present the top N" for a high-assurance instance?
- **KMS unwrap locus** — purely in-browser (server never sees the master until
  the browser hands it back post-unwrap) needs a KMS SDK in the page; a
  server-side call needs the user's delegated creds to reach the server. Prefer
  in-browser.
- **Recovery code strength** — a 128-bit random string is a KEK a phisher can
  ask for like any password. Pair it with a "recovery used" out-of-band notice,
  or require a second factor to redeem it.
