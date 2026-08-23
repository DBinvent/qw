// Client for the updates-and-news mailing list (`user_mgmt::subscribe_router`).
//
// Generated file — the source of truth is ../../user_mgmt/web/lib/news-client.ts.
// Edit it there and re-run `user_mgmt/web/sync.sh <this-app>`; a local edit
// will be overwritten.
//
// Same relative-fetch rule as account-client.ts: the browser only ever talks
// to its own origin, and each site's Worker forwards /api/* to the backend
// server-side.

const BASE = process.env.NEXT_PUBLIC_NEWS_API ?? '/api/news'

/** Mirrors `AccountError` — same shape, so a page can handle either. */
export class NewsError extends Error {
  status: number
  constructor(status: number, message: string) {
    super(message)
    this.name = 'NewsError'
    this.status = status
  }
}

async function post<T>(path: string, payload: unknown): Promise<T> {
  let response: Response
  try {
    response = await fetch(BASE + path, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(payload),
    })
  } catch {
    throw new NewsError(0, 'Could not reach the server. Check your connection and try again.')
  }

  const text = await response.text()
  let body: unknown = null
  try {
    body = text ? JSON.parse(text) : null
  } catch {
    // Non-JSON from a proxy or an error page — fall through to the
    // status-based message rather than showing the caller raw HTML.
  }

  if (!response.ok) {
    const message =
      (body as { error?: string } | null)?.error ??
      (response.status === 429
        ? 'Too many attempts. Try again in a few minutes.'
        : 'Something went wrong. Try again.')
    throw new NewsError(response.status, message)
  }

  return (body ?? {}) as T
}

export const news = {
  /**
   * Answers identically whether or not the address is already on the list —
   * so must the UI, or the page becomes the account-existence oracle the API
   * refuses to be. Always show "check your email".
   */
  subscribe: (email: string, source?: string) =>
    post<{ status: string; note: string }>('/subscribe', { email, source }),

  confirm: (token: string) => post<{ status: string }>('/confirm', { token }),

  unsubscribe: (token: string) => post<{ status: string }>('/unsubscribe', { token }),
}
