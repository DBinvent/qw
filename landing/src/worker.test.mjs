// [dot-dot] node --experimental-transform-types --test src/worker.test.mjs
// .mjs rather than .ts: `next build` type-checks every .ts file, and an
// import ending in ".ts" is a type error there.
import { test } from "node:test";
import assert from "node:assert/strict";
import worker, { pathHasDotDot } from "./worker.ts";

test("a path with .., as sent or escaped, is found; the query and a single dot are not", () => {
  for (const u of ["https://example.com/api/../x", "https://example.com/api/..%2f..%2fetc",
    "https://example.com/%252e%252e/x", "https://example.com/a..b"]) {
    assert.ok(pathHasDotDot(u), u);
  }
  for (const u of ["https://example.com/api/news/subscribe", "https://example.com/a.b/c.js",
    "https://example.com/x?q=..", "https://example.com/%2e/x"]) {
    assert.ok(!pathHasDotDot(u), u);
  }
});

test("such a request is refused with 400 before the backend or the assets are asked", async () => {
  let asked = false;
  const env = { BACKEND_ORIGIN: "http://127.0.0.1:1", ASSETS: { fetch: async () => { asked = true; return new Response("asset"); } } };
  const r = await worker.fetch(new Request("https://example.com/api/..%2fsecret"), env);
  assert.equal(r.status, 400);
  assert.equal(asked, false);
  const ok = await worker.fetch(new Request("https://example.com/index.html"), env);
  assert.equal(await ok.text(), "asset");
});
