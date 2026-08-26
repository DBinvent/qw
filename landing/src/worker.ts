/// <reference types="@cloudflare/workers-types" />

export interface Env {
  ASSETS: Fetcher;
  BACKEND_ORIGIN: string;
  /** Optional. Unauthenticated GitHub is 60 req/h per IP; a token lifts it
   *  to 5,000. Absent is fine — the edge cache absorbs most of it. */
  GITHUB_TOKEN?: string;
}

// Four routes, everything else is static assets.
//
// 1. /api/* — the newsletter signup in the footer POSTs to
//    /api/news/subscribe, forwarded to qw-server at home, which owns the
//    subscribers table in Postgres. The browser only ever talks to this
//    origin, so the forward is same-origin from the page's point of view and
//    needs no CORS. An absolute fetch to BACKEND_ORIGIN from the page would
//    defeat that; don't add one.
//
// 2. /i/<npub> — public invite links (NIP-QW07). A static export cannot have
//    a route per npub, so one exported page (/invite/) is served for all of
//    them, with its title and OG tags rewritten per key. That rewrite is the
//    whole reason this is a Worker route and not a redirect: these links are
//    posted on LinkedIn, and the preview card is most of what a reader sees
//    before deciding to click.
//
// 3. /gh/<handle> — the same invite, short enough for a profile bio. 76
//    characters of npub URL does not fit where the link is most useful, and
//    a bio is exactly where it belongs. This borrows GitHub's namespace
//    rather than creating one: GitHub already decides who owns a handle, so
//    the mapping is self-certifying, there is no registry to keep and
//    nothing to squat. `/@name` was the alternative and would have made this
//    project allocate and arbitrate names.
//
//    It always redirects to the /i/<npub> form and never replaces it: that
//    URL carries the key, so it survives this site disappearing and anyone
//    can re-host it, where /gh/vk carries nothing but a promise.
//
// 4. /download/android — the current APK, without this site knowing which
//    one that is. Reads the release manifest and redirects to the exact
//    versioned file, so publishing a build needs no deploy here.
//
//    Why not link `-latest.apk` directly: it is served
//    `max-age=300, must-revalidate` so it can track releases, which is right
//    for a pointer and ruinous for 18 MB of payload — every download would
//    revalidate at the edge. The redirect is the short-lived thing; what it
//    points at is the versioned name, cached immutably for a year.

// Shape check only — bech32 charset and length, or a bare 32-byte hex key.
// The authority is qw_protocol::invite::parse_invite_target; this is the
// same rule expressed where a Worker can run it. Deliberately not a full
// bech32 checksum verify: an npub that passes here but fails there gets the
// same page and no harm done, since nothing is signed or published on the
// strength of it.
const NPUB = /^npub1[023456789acdefghjklmnpqrstuvwxyz]{58}$/;
const HEX = /^[0-9a-fA-F]{64}$/;

// The same shape, unanchored — for finding a key inside prose. A GitHub bio
// may hold the bare npub or a full knownby.work/i/<npub> URL; both are the
// same needle.
const NPUB_IN_TEXT = /npub1[023456789acdefghjklmnpqrstuvwxyz]{58}/;

// The release host, and the two names on it this Worker needs. Kept here
// rather than imported from lib/links.ts: that module is bundled into the
// page, this one runs on the edge, and they are deployed as separate
// artifacts — a shared import would only look like it kept them in sync.
const RELEASE_BASE = "https://app.knownby.work";
const ANDROID_MANIFEST = `${RELEASE_BASE}/qw-android-arm64.json`;
const ANDROID_FALLBACK = `${RELEASE_BASE}/qw-android-arm64-latest.apk`;

/**
 * Resolve /download/android to the current versioned APK.
 *
 * On any failure this falls back to `-latest` rather than erroring: a
 * download that is merely uncached beats a download page with a dead button,
 * and the fallback is always the same file the manifest would have named.
 */
async function androidDownload(): Promise<Response> {
  let location = ANDROID_FALLBACK;
  try {
    const res = await fetch(ANDROID_MANIFEST, { cf: { cacheTtl: 300 } });
    if (res.ok) {
      const manifest = (await res.json()) as { file?: unknown };
      if (typeof manifest.file === "string" && /^[A-Za-z0-9._-]+$/.test(manifest.file)) {
        location = `${RELEASE_BASE}/${manifest.file}`;
      }
    }
  } catch {
    // keep the fallback
  }
  return new Response(null, {
    status: 302,
    headers: {
      location,
      // Only the redirect is short-lived. Its target carries a year.
      "cache-control": "public, max-age=300, must-revalidate",
    },
  });
}

// GitHub's own rule: alphanumerics and single inner hyphens, 1-39 chars.
// Checked before the fetch so a junk path never becomes an outbound request.
const GH_HANDLE = /^[A-Za-z0-9](?:[A-Za-z0-9]|-(?=[A-Za-z0-9])){0,38}$/;

function inviteTarget(pathname: string): string | null {
  if (!pathname.startsWith("/i/")) return null;
  let raw = pathname.slice(3).replace(/\/$/, "");
  try {
    raw = decodeURIComponent(raw);
  } catch {
    return null;
  }
  return NPUB.test(raw) || HEX.test(raw) ? raw : null;
}

const escapeAttr = (s: string) =>
  s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

class MetaRewriter {
  constructor(
    private readonly title: string,
    private readonly description: string,
    private readonly canonical: string,
  ) {}

  element(el: Element) {
    const property = el.getAttribute("property") ?? el.getAttribute("name");
    if (property === "og:title" || property === "twitter:title") {
      el.setAttribute("content", this.title);
    } else if (
      property === "og:description" ||
      property === "twitter:description" ||
      property === "description"
    ) {
      el.setAttribute("content", this.description);
    } else if (property === "og:url") {
      // Without this, every invite shares one canonical URL and a scraper
      // may collapse them into a single cached preview.
      el.setAttribute("content", this.canonical);
    }
  }
}

/** `/gh/<handle>` -> the handle, or null when this is not that route. */
function ghHandle(pathname: string): string | null {
  if (!pathname.startsWith("/gh/")) return null;
  let raw = pathname.slice(4).replace(/\/$/, "");
  try {
    raw = decodeURIComponent(raw);
  } catch {
    return null;
  }
  return GH_HANDLE.test(raw) ? raw : null;
}

type Resolution =
  | { kind: "found"; npub: string }
  | { kind: "absent" }
  | { kind: "unavailable" };

/**
 * Ask GitHub who owns the handle and whether they published a key.
 *
 * Server-side on purpose. The same fetch from the visitor's phone would tell
 * GitHub who is looking at whom; from here GitHub sees Cloudflare.
 *
 * "absent" and "unavailable" are kept apart deliberately. A rate-limited or
 * broken GitHub is not evidence that nobody claims the handle, and rendering
 * it as "no such person" would be a confident lie.
 */
async function resolveGitHub(handle: string, env: Env): Promise<Resolution> {
  const headers: Record<string, string> = {
    // GitHub rejects unidentified clients outright.
    "user-agent": "knownby.work-invite-resolver",
    accept: "application/vnd.github+json",
  };
  if (env.GITHUB_TOKEN) headers.authorization = `Bearer ${env.GITHUB_TOKEN}`;

  let res: Response;
  try {
    res = await fetch(`https://api.github.com/users/${encodeURIComponent(handle)}`, { headers });
  } catch {
    return { kind: "unavailable" };
  }

  if (res.status === 404) return { kind: "absent" };
  if (!res.ok) return { kind: "unavailable" };

  let user: { bio?: string | null; blog?: string | null; name?: string | null };
  try {
    user = (await res.json()) as typeof user;
  } catch {
    return { kind: "unavailable" };
  }

  // Bio first, then the profile's website field — a person who set their
  // site to their invite link has already published the key.
  const haystack = [user.bio, user.blog, user.name].filter(Boolean).join(" ");
  const hit = haystack.match(NPUB_IN_TEXT);
  return hit ? { kind: "found", npub: hit[0] } : { kind: "absent" };
}

/** Small self-contained page; the export's 404 cannot explain this case. */
function shortLinkPage(status: number, heading: string, body: string): Response {
  return new Response(
    `<!doctype html><meta charset="utf-8">` +
      `<meta name="viewport" content="width=device-width,initial-scale=1">` +
      `<title>${heading} — QW</title>` +
      `<style>body{margin:0;min-height:100vh;display:grid;place-items:center;` +
      `background:#09090b;color:#e4e4e7;font:15px/1.65 ui-sans-serif,system-ui,sans-serif;padding:2rem}` +
      `main{max-width:32rem}h1{font-size:1.15rem;margin:0 0 .75rem}` +
      `p{color:#a1a1aa;margin:0 0 1rem}code{font-family:ui-monospace,monospace;font-size:.85em;color:#e4e4e7}` +
      `a{color:#a78bfa}</style>` +
      `<main><h1>${heading}</h1>${body}` +
      `<p><a href="/join">How QW works</a></p></main>`,
    {
      status,
      headers: {
        "content-type": "text/html; charset=utf-8",
        // Never cache a negative answer for long: the fix is someone editing
        // their bio, and they should see it work almost immediately.
        "cache-control": "public, max-age=60, must-revalidate",
      },
    },
  );
}

class TitleRewriter {
  constructor(private readonly title: string) {}
  element(el: Element) {
    el.setInnerContent(this.title);
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname.startsWith("/api/")) {
      // The API server mounts the list at /news/*, knowing nothing about the
      // /api prefix, so it is stripped here rather than forwarded.
      const target = new URL(url.pathname.slice(4) + url.search, env.BACKEND_ORIGIN);
      return fetch(new Request(target.toString(), request));
    }

    if (url.pathname === "/download/android" || url.pathname === "/download/android/") {
      return androidDownload();
    }

    const handle = ghHandle(url.pathname);
    if (handle) {
      const found = await resolveGitHub(handle, env);
      if (found.kind === "found") {
        return new Response(null, {
          status: 302,
          headers: {
            location: `/i/${found.npub}`,
            // Short, so a renamed handle stops resolving quickly. This is
            // only how long a hit may outlive the bio it came from.
            "cache-control": "public, max-age=300, must-revalidate",
          },
        });
      }
      if (found.kind === "unavailable") {
        return shortLinkPage(
          503,
          "Could not reach GitHub",
          `<p>This link resolves by reading <code>github.com/${escapeAttr(handle)}</code>, and that ` +
            `lookup failed just now — usually a rate limit. It is not a statement about whether ` +
            `that account is on QW. Try again in a minute.</p>`,
        );
      }
      return shortLinkPage(
        404,
        "No QW key on that profile",
        `<p><code>github.com/${escapeAttr(handle)}</code> does not publish a QW key, or the handle ` +
          `does not exist. This link works by reading the npub out of a GitHub bio or website ` +
          `field, so nothing here is a registration — the owner of the handle is the only one ` +
          `who can make it resolve.</p>` +
          `<p>If that is you: put your <code>npub1…</code>, or your full ` +
          `<code>knownby.work/i/npub1…</code> link, in your GitHub bio.</p>`,
      );
    }

    const invite = inviteTarget(url.pathname);
    if (invite) {
      // Fetch the exported page by its own path, not by rewriting the
      // request URL — the assets binding looks it up directly, and the
      // browser's location stays /i/<npub> so the page can read the key.
      // No trailing slash: the export writes out/invite.html and serves it
      // at /invite, while /invite/ 307-redirects there.
      const page = await env.ASSETS.fetch(new URL("/invite", url.origin).toString());
      if (!page.ok) return page;

      const short = `${invite.slice(0, 12)}…${invite.slice(-6)}`;
      const title = "You've been invited to connect on QW";
      const description = `${short} shared a QW invite link. Following it makes you a direct contact — skills confirmed by the people you worked with.`;

      return new HTMLRewriter()
        .on("title", new TitleRewriter(title))
        .on(
          "meta",
          new MetaRewriter(escapeAttr(title), escapeAttr(description), escapeAttr(url.toString())),
        )
        .transform(
          new Response(page.body, {
            status: page.status,
            headers: {
              ...Object.fromEntries(page.headers),
              // Per-key OG tags, so a shared link must not be served from a
              // cache entry keyed on a different key's page.
              "cache-control": "public, max-age=300, must-revalidate",
            },
          }),
        );
    }

    return env.ASSETS.fetch(request);
  },
};
