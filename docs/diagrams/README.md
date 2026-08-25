# Diagrams

QW infographics as [Eraser](https://eraser.io) diagram-as-code. Text, so they
review in a diff and can't drift from the NIPs unnoticed the way an exported
PNG does.

| file | type | draws | source of truth |
|---|---|---|---|
| `joining.eraserdiagram` | sequence | identity → introduction → skill tags; no registration step exists | NIP-QW07, NIP-QW03, `protocol/src/identity.rs` |
| `job-lifecycle.eraserdiagram` | sequence | 9000 → 9004* → 9001 → 9002? → 9003 ×2 → 9010, nothing atomic | NIP-QW01, NIP-QW02 |
| `referral-routing.eraserdiagram` | flowchart | greedy forwarding, TTL, and why identity stops at hop 1 | NIP-QW06, `node/src/routing.rs` |

## Rendering

Paste a file's contents into a new diagram at [app.eraser.io](https://app.eraser.io)
and pick the type from the table — free, and the fastest way to iterate on the
layout.

For a repeatable export, `./render.sh` posts each file to Eraser's
`render/elements` API and writes `out/<name>.svg`:

```sh
ERASER_API_KEY=… ./render.sh                 # all
ERASER_API_KEY=… ./render.sh referral-routing # one
./render.sh --check                          # no key: list files and their types
FORMAT=png THEME=light ./render.sh           # overrides
```

The API key comes from an Eraser workspace under Settings → API, and that
endpoint is a paid-plan feature. `--check` is there so the file/type mapping
can be verified without one.

**These have not been rendered yet** — the API needs a key this host doesn't
have. The DSL is written against Eraser's documented syntax but unverified
against their renderer, so expect a first pass of layout fixes.

## Rules for editing

- The NIP is the source of truth, not the diagram. When a kind number, tag or
  step changes, the NIP moves first and the diagram follows — the header
  comment in each file names which spec it answers to.
- Kind numbers come from `protocol/src/events/kinds.rs`. They are easy to get
  wrong from memory: `9003` is job completion, profile skill tags are `9020`.
- Keep the comments. Each file carries the *why* — the Gnutella flooding
  number, why completion is two separate signatures — which is what makes an
  infographic worth more than a box diagram.
