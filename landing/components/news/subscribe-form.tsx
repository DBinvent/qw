// "Get updates and news" signup.
//
// Generated file — the source of truth is
// ../../user_mgmt/web/components/news/subscribe-form.tsx. Edit it there and
// re-run `user_mgmt/web/sync.sh <this-app>`.
//
// Shape follows trust-atria-landing's WaitlistFooter: one field, one button,
// the form replaced by a confirmation on success. What differs is what the
// confirmation *says* — this list is double opt-in, so the address is not
// subscribed until the emailed link is clicked, and telling someone they are
// "on the list" before that would be a lie.

'use client'

import { useState } from 'react'
import { news, NewsError } from '@/lib/news-client'

export function SubscribeForm({
  source,
  heading = 'Updates and news',
  blurb = 'Occasional product updates. No more than that, and one click to stop.',
}: {
  /** Which page this signup came from, recorded against the row. */
  source?: string
  heading?: string
  blurb?: string
}) {
  const [email, setEmail] = useState('')
  const [pending, setPending] = useState(false)
  const [sent, setSent] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (pending) return
    setPending(true)
    setError(null)
    try {
      await news.subscribe(email, source)
      setSent(true)
    } catch (err) {
      setError(err instanceof NewsError ? err.message : 'Something went wrong. Try again.')
    } finally {
      setPending(false)
    }
  }

  return (
    <section className="w-full max-w-md">
      <h2 className="text-sm font-medium">{heading}</h2>
      <p className="mt-1 text-sm text-muted-foreground">{blurb}</p>

      {sent ? (
        <div
          role="status"
          className="mt-4 rounded-md border border-success/40 bg-success/10 px-3 py-2 text-sm text-success"
        >
          Check your email and click the link to confirm — we will not send anything until you do.
        </div>
      ) : (
        <form onSubmit={onSubmit} className="mt-4 flex flex-col gap-2 sm:flex-row">
          <label htmlFor="subscribe-email" className="sr-only">
            Email
          </label>
          <input
            id="subscribe-email"
            type="email"
            required
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            placeholder="you@example.com"
            disabled={pending}
            className="h-10 flex-1 rounded-md border border-input bg-background px-3 text-sm outline-none
                       transition placeholder:text-muted-foreground focus-visible:border-ring
                       focus-visible:ring-2 focus-visible:ring-ring/40 disabled:opacity-60"
          />
          <button
            type="submit"
            disabled={pending}
            className="h-10 rounded-md bg-primary px-4 text-sm font-medium text-primary-foreground
                       transition hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-60"
          >
            {pending ? 'Subscribing…' : 'Subscribe'}
          </button>
        </form>
      )}

      {error ? (
        <p role="alert" className="mt-2 text-sm text-destructive">
          {error}
        </p>
      ) : null}
    </section>
  )
}
