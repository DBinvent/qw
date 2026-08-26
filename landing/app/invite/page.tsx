'use client'

import { useEffect, useState } from 'react'
import { ArrowUpRight, Download, Link2, ShieldQuestion, UserPlus } from 'lucide-react'
import { SiteHeader } from '@/components/qw/site-header'
import { Footer } from '@/components/qw/footer'
import { InviteQr } from '@/components/qw/invite-qr'
import { ANDROID_APK_URL, GITHUB_URL } from '@/lib/links'
import { AndroidFacts } from '@/components/qw/android-release'

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
  // The absolute URL, for the QR. Read from the browser rather than built
  // from a constant: a code that encodes a different origin than the one
  // the person is looking at is the kind of mismatch nobody checks.
  const [href, setHref] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    setTarget(targetFromPath(window.location.pathname))
    setHref(`${window.location.origin}${window.location.pathname}`)
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

            {/* Shown to the holder of the link as much as to the visitor:
                this is the page you put on a screen when the person you are
                introducing yourself to is standing in front of you. One
                scan takes them to this same page on their own phone, where
                the app is a tap away. */}
            {target && href ? (
              <div className="mt-6 flex flex-col items-center gap-4 rounded-xl border border-border bg-card p-6 sm:flex-row sm:items-center sm:gap-6">
                <InviteQr value={href} className="size-40 shrink-0 rounded-lg" />
                <div className="text-center sm:text-left">
                  <p className="font-mono text-xs uppercase tracking-widest text-primary">
                    Or scan it
                  </p>
                  <p className="mt-2 text-sm leading-relaxed text-muted-foreground">
                    Any phone camera. It opens this page, which is where the app is — so a
                    scan is both halves of joining: install the client, then follow this
                    link with it. Nothing is signed by scanning.
                  </p>
                </div>
              </div>
            ) : null}

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

            {/* The client runs as of 2026-08-26, so this is a real button
                and the text no longer hedges about whether it starts. What
                stays is the true remainder: sideload, Android only. A
                download whose limits are stated beats both a button that
                does nothing and one that oversells. */}
            <div className="mt-10 rounded-xl border border-border bg-card p-6">
              <p className="font-mono text-xs uppercase tracking-widest text-muted-foreground">
                What happens next
              </p>
              <p className="mt-3 text-sm leading-relaxed text-muted-foreground">
                Nothing was sent by opening this page. Joining takes a client — a time book for
                open-source work, which generates your identity on first run and is what turns this
                key into two signed introductions. The Android build runs; it installs by sideload
                rather than from a store, and there is no iOS or desktop package yet. Keep the link
                either way: it stays valid, because it is just a public key.
              </p>
              <div className="mt-5 flex flex-wrap gap-3">
                <a
                  href={ANDROID_APK_URL}
                  className="glow-violet inline-flex h-11 items-center justify-center gap-2 rounded-lg bg-primary px-5 text-sm font-medium text-primary-foreground transition-transform hover:-translate-y-px"
                >
                  <Download className="size-4" />
                  Android APK · <AndroidFacts />
                </a>
                <a
                  href="/join"
                  className="inline-flex h-11 items-center justify-center gap-2 rounded-lg border border-border px-5 text-sm font-medium text-foreground transition-colors hover:bg-secondary/60"
                >
                  How to join
                  <ArrowUpRight className="size-4" />
                </a>
                <a
                  href={GITHUB_URL}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex h-11 items-center justify-center gap-2 rounded-lg border border-border px-5 text-sm font-medium text-foreground transition-colors hover:bg-secondary/60"
                >
                  Read the protocol
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
