# QW client (§7)

Two crates, split along the line of what can be built anywhere:

| | builds here | contains |
|---|---|---|
| `core/` — `qw-client-core` | **yes**, 6 tests | identity on disk, HTTP mailbox transport, invite-link following |
| `src-tauri/` — `qw-app` | **no** (see below) | the window: Tauri commands over `core`, and `../ui` |

Everything with a rule in it lives in `core/`, deliberately. A Tauri crate
cannot compile without GTK/WebKit development packages, so anything beside it
is unbuildable and untestable on a machine that lacks them — including CI.
`src-tauri/` is therefore as thin as it can be: parse an argument, call the
core, hand back JSON.

`src-tauri/` is **not** a workspace member for the same reason; a member that
cannot build would break `cargo test --workspace` for everyone. It has its own
lockfile and is built from its own directory.

## Status

`core/` is tested, including against a real HTTP server on a real socket.

**`src-tauri/` and `ui/` have never been compiled.** `cargo tauri` is not
installed on the development host and none of `webkit2gtk-4.1`,
`javascriptcoregtk-4.1`, `gtk+-3.0` or `libsoup-3.0` are present. Treat that
directory as a scaffold to compile and correct on a machine that has them, not
as working code.

## Building the shell

System libraries need root, so this is a script rather than a command to
paste:

```sh
sudo bash /tmp/install-tauri-deps.sh      # written by the same session; see its header
cargo install tauri-cli --version '^2.0' --locked
```

Then, from `app/src-tauri`:

```sh
cargo tauri dev          # run it
cargo tauri build        # bundle it
```

`ui/` is a single dependency-free HTML file — no npm, no bundler, no build
step before `cargo tauri`.

## What the shell does today

- Shows your invite link (`knownby.work/i/<npub>`) and copies it.
- Follows a link someone else posted: signs your half of the NIP-QW07
  introduction and queues it. The publisher's half is theirs to sign — a
  client that produced both sides would be forging half the edge.
- Syncs: flush the outbox, then poll the mailbox, reporting counts and any
  per-server errors.

## What it does not do yet

- **OS deep links.** Clicking `knownby.work/i/<npub>` in a browser does not
  open the app; the link has to be pasted. Wiring `tauri-plugin-deep-link` for
  a `qw://` scheme (and Android intents / iOS universal links) is the next
  step, and is exactly the OS integration §7 says cannot be tested here.
- **External signer.** The key sits in the app's data directory, `0600`. §7's
  QR/deep-link delegated signing is unbuilt.
- **Anything but the mailbox.** No contract composition, no referral queries,
  no trust display — those exist in `qw-protocol`/`qw-node` and have no UI.
- **Server ranking.** The server list is config order, not
  `qw_node::server_registry::rank_servers` output. It is a `Vec` from day one
  so that stays a swap rather than a rewrite.
