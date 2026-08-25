'use client'

import { useEffect, useState } from 'react'
import { ArrowUpRight, Link2, ShieldQuestion, UserPlus } from 'lucide-react'
import { SiteHeader } from '@/components/qw/site-header'
import { Footer } from '@/components/qw/footer'
import { GITHUB_URL } from '@/lib/links'

// Served for every /i/<npub> path by src/worker.ts, which rewrites this
// page's title and OG tags for the specific npub before handing it to a
// scraper. The npub itself is read here from the URL rather than injected:
// the browser's location is the real /i/<npub>, and a static export has no
// per-request rendering to inject into.
//
// Validation is the bech32 charset and length only — the same shape check
// the Worker does. The authority is qw_protocol::invite (Rust); nothing on
// this page can verify a key anyway, since it has no client and no relay.
const NPUB = /^npub1[023456789acdefghjklmnpqrstuvwxyz]{58}$/
const HEX = /^[0-9a-fA-F]{64}$/

function targetFromPath(pathname: string): string | null {
  const raw = decodeURIComponent(pathname.replace(/^\/i\//, '').replace(/\/$/, ''))
  if (NPUB.test(raw) || HEX.test(raw)) return raw
  return null
}

export default function InvitePage() {
  const [target, setTarget] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    setTarget(targetFromPath(window.location.pathname))
  }, [])

  const short = target ? `${target.slice(0, 12)}…${target.slice(-6)}` : null

  return (
    <div id="top" className="min-h-screen bg-background">
      <SiteHeader />
      <main>
        <section className="relative">
          <div className="mx-auto max-w-3xl px-4 py-20 sm:px-6 sm:py-28">
            <p className="font-mono text-xs uppercase tracking-widest text-primary">Invite link</p>
            <h1 className="mt-3 text-balance text-3xl font-semibold tracking-tight text-foreground sm:text-4xl">
              Someone wants to connect with you on QW
            </h1>

            {target ? (
              <div className="mt-6 flex flex-wrap items-center gap-3 rounded-lg border border-border bg-card px-4 py-3">
                <Link2 className="size-4 shrink-0 text-primary" />
                <code className="font-mono text-sm text-foreground" title={target}>
                  {short}
                </code>
                <button
                  type="button"
                  onClick={() => {
                    navigator.clipboard?.writeText(target).then(
                      () => {
                        setCopied(true)
                        setTimeout(() => setCopied(false), 2000)
                      },
                      () => setCopied(false),
                    )
                  }}
                  className="ml-auto font-mono text-[11px] uppercase tracking-widest text-muted-foreground hover:text-foreground"
                >
                  {copied ? 'copied' : 'copy key'}
                </button>
              </div>
            ) : (
              <p className="mt-6 rounded-lg border border-dashed border-border bg-card/40 px-4 py-3 text-sm text-muted-foreground">
                This link is missing a valid key. An invite link looks like{' '}
                <code className="font-mono text-xs text-foreground">knownby.work/i/npub1…</code> —
                ask whoever shared it for the full URL.
              </p>
            )}

            <div className="mt-10 grid gap-px overflow-hidden rounded-xl border border-border bg-border sm:grid-cols-3">
              {[
                {
                  icon: UserPlus,
                  title: 'You become a direct contact',
                  desc: 'Following this link exchanges two signed introductions — yours and theirs. You land as a hop-1 contact whether you were four hops away in the graph or not in it at all.',
                },
                {
                  icon: ShieldQuestion,
                  title: 'It is not a recommendation',
                  desc: 'Nobody who posts a link knows who will click it, so this edge vouches for no one. It makes you reachable: queries can route to you, offers can arrive.',
                },
                {
                  icon: ArrowUpRight,
                  title: 'Reputation still comes from work',
                  desc: 'Trust is computed only from completed, countersigned contracts. A contact at hop 1 with no contracts counts for exactly as much as a stranger at hop 4: nothing.',
                },
              ].map((c) => (
                <div key={c.title} className="flex flex-col gap-3 bg-card p-6">
                  <span className="flex size-9 items-center justify-center rounded-lg border border-border bg-secondary text-primary">
                    <c.icon className="size-4" />
                  </span>
                  <h2 className="text-sm font-semibold text-foreground">{c.title}</h2>
                  <p className="text-sm leading-relaxed text-muted-foreground">{c.desc}</p>
                </div>
              ))}
            </div>

            {/* The client that would complete this exchange is §7 and is not
                released. Saying so beats a button that does nothing. */}
            <div className="mt-10 rounded-xl border border-border bg-card p-6">
              <p className="font-mono text-xs uppercase tracking-widest text-muted-foreground">
                What happens next
              </p>
              <p className="mt-3 text-sm leading-relaxed text-muted-foreground">
                There is no released client yet — QW is an early prototype, and the app that would
                take this key, generate your identity and publish the two introductions is still
                being built. Nothing was sent by opening this page. Keep the link: it stays valid,
                because it is just a public key.
              </p>
              <div className="mt-5 flex flex-wrap gap-3">
                <a
                  href={GITHUB_URL}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="glow-violet inline-flex h-11 items-center justify-center gap-2 rounded-lg bg-primary px-5 text-sm font-medium text-primary-foreground transition-transform hover:-translate-y-px"
                >
                  Read the protocol
                  <ArrowUpRight className="size-4" />
                </a>
                <a
                  href="/#architecture"
                  className="inline-flex h-11 items-center justify-center gap-2 rounded-lg border border-border px-5 text-sm font-medium text-foreground transition-colors hover:bg-secondary/60"
                >
                  How QW works
                </a>
              </div>
            </div>
          </div>
        </section>
      </main>
      <Footer />
    </div>
  )
}
