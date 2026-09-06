import { ArrowRight, Download, ExternalLink, Globe } from 'lucide-react'
import { ANDROID_APK_URL, SOURCE_LABEL, SOURCE_URL, WEB_UI_URL } from '@/lib/links'
import { AndroidFacts } from '@/components/qw/android-release'

export function Hero() {
  return (
    <section className="relative overflow-hidden">
      <div
        aria-hidden="true"
        className="pointer-events-none absolute left-1/2 top-0 h-72 w-[40rem] -translate-x-1/2 rounded-full bg-primary/15 blur-[120px]"
      />
      <div className="relative mx-auto max-w-4xl px-4 py-24 text-center sm:px-6 sm:py-32">
        <div className="inline-flex items-center gap-2 rounded-full border border-border bg-card/60 px-3 py-1 font-mono text-[11px] uppercase tracking-wider text-muted-foreground">
          Early prototype · open source
        </div>
        <h1 className="mt-6 text-balance text-4xl font-semibold tracking-tight text-foreground sm:text-5xl">
          Skills confirmed by the people you worked with
        </h1>
        <p className="mx-auto mt-5 max-w-xl text-pretty leading-relaxed text-muted-foreground">
          Found through friends of friends. A peer-verified contribution network: time
          contributed to shared projects, signed by the counterparties who received it — no
          blockchain, no tokens-as-currency, no central authority.
        </p>

{/* Web UI first because it costs a click, not an install — it is the
            fastest way to see what QW does. The glow moves to it for that
            reason; Android keeps the filled style right beside it (same size,
            adjacent) because it is the client that actually holds your key,
            and "View source" stays an outline. Three CTAs, one glow. */}
        <div className="mt-8 flex flex-wrap items-center justify-center gap-3">
          <a
            href={WEB_UI_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="glow-violet inline-flex h-11 items-center justify-center gap-2 rounded-lg bg-primary px-5 text-sm font-medium text-primary-foreground transition-transform hover:-translate-y-px"
          >
            <Globe className="size-4" />
            Open the web UI
          </a>
          <a
            href={ANDROID_APK_URL}
            className="inline-flex h-11 items-center justify-center gap-2 rounded-lg bg-primary px-5 text-sm font-medium text-primary-foreground transition-transform hover:-translate-y-px"
          >
            <Download className="size-4" />
            Download for Android
          </a>
          <a
            href={SOURCE_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex h-11 items-center justify-center gap-2 rounded-lg border border-border px-5 text-sm font-medium text-foreground transition-colors hover:bg-secondary/60"
          >
            <ExternalLink className="size-4" />
            {SOURCE_LABEL}
          </a>
          <a
            href="#architecture"
            className="inline-flex h-11 items-center justify-center gap-2 rounded-lg border border-border px-5 text-sm font-medium text-foreground transition-colors hover:bg-secondary/60"
          >
            How it works
            <ArrowRight className="size-4" />
          </a>
        </div>

        {/* One line, because a front-page button that says nothing about
            what it opens is how people end up surprised. The web UI is a
            hosted try-it: the key it makes lives in server memory while you
            are signed in, not on your device — fine for a look, not for
            anything you need to keep. The app is where the key is yours. */}
        <p className="mx-auto mt-4 max-w-2xl font-mono text-[11px] leading-relaxed text-muted-foreground">
          A time book and time bank for open-source work · <AndroidFacts /> ·{' '}
          <a href="/join" className="text-primary hover:text-primary/80">
            sideload, not a store build
          </a>{' '}
          · the web UI keeps your key on the server — a preview, not the
          client you trust with real work
        </p>
      </div>
    </section>
  )
}
