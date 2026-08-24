/// <reference types="@cloudflare/workers-types" />

export interface Env {
  ASSETS: Fetcher;
  BACKEND_ORIGIN: string;
}

// This file exists for one route: the newsletter signup in the footer POSTs to
// /api/news/subscribe, and this forwards it to qw-server at home, which owns
// the subscribers table in Postgres. Everything else is static assets.
//
// The browser only ever talks to this origin — the forward is server-side, so
// it is same-origin from the page's point of view and needs no CORS. An
// absolute fetch to BACKEND_ORIGIN from the page would defeat that; don't add
// one.
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname.startsWith("/api/")) {
      // The API server mounts the list at /news/*, knowing nothing about the
      // /api prefix, so it is stripped here rather than forwarded.
      const target = new URL(url.pathname.slice(4) + url.search, env.BACKEND_ORIGIN);
      return fetch(new Request(target.toString(), request));
    }

    return env.ASSETS.fetch(request);
  },
};
