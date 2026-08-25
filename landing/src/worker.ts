/// <reference types="@cloudflare/workers-types" />

export interface Env {
  ASSETS: Fetcher;
  BACKEND_ORIGIN: string;
}

// Two routes, everything else is static assets.
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

// Shape check only — bech32 charset and length, or a bare 32-byte hex key.
// The authority is qw_protocol::invite::parse_invite_target; this is the
// same rule expressed where a Worker can run it. Deliberately not a full
// bech32 checksum verify: an npub that passes here but fails there gets the
// same page and no harm done, since nothing is signed or published on the
// strength of it.
const NPUB = /^npub1[023456789acdefghjklmnpqrstuvwxyz]{58}$/;
const HEX = /^[0-9a-fA-F]{64}$/;

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
