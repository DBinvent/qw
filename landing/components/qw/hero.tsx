import { ArrowRight, Download, ExternalLink } from 'lucide-react'
import { ANDROID_APK_URL, SOURCE_LABEL, SOURCE_URL } from '@/lib/links'
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

{/* The download takes the filled style and "View source" drops to an
            outline: two glow buttons side by side compete, and of the two the
            APK is the one a visitor can act on. Both are still the same size
            and adjacent, so neither reads as a footnote. */}
        <div className="mt-8 flex flex-wrap items-center justify-center gap-3">
          <a
            href={ANDROID_APK_URL}
            className="glow-violet inline-flex h-11 items-center justify-center gap-2 rounded-lg bg-primary px-5 text-sm font-medium text-primary-foreground transition-transform hover:-translate-y-px"
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

        {/* One line, because a front-page download button that says nothing
            about what it installs is how people end up surprised. Says what
            the app is and what it is not, and links to the page carrying the
            checksum and the rest of the limits. */}
        <p className="mt-4 font-mono text-[11px] leading-relaxed text-muted-foreground">
          A time book and time bank for open-source work · <AndroidFacts /> ·{' '}
          <a href="/join" className="text-primary hover:text-primary/80">
            sideload, not a store build
          </a>
        </p>
      </div>
    </section>
  )
}
