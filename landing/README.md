## QW Landing

Static Next.js site introducing QW (architecture + FAQ), sourced from
`../doc-link/abstract.md` and `../doc-link/qw-design-faq.md`.

### Building, testing and deployment

```sh
pnpm install       # or npm/yarn
pnpm dev           # http://localhost:3000
pnpm build         # static export to ./out (next.config.mjs: output: 'export')
pnpm deploy        # build, then `wrangler deploy` (Workers Static Assets, see wrangler.toml)
```

No backend, no D1, no forms — content only. `wrangler.toml` deploys `./out` as a
static-assets Worker; no bindings or secrets required.
