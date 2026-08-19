import { ArrowRight, ExternalLink } from 'lucide-react'

const GITHUB_URL = 'https://github.com/DBinvent/qw'

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

        <div className="mt-8 flex flex-wrap items-center justify-center gap-3">
          <a
            href={GITHUB_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="glow-violet inline-flex h-11 items-center justify-center gap-2 rounded-lg bg-primary px-5 text-sm font-medium text-primary-foreground transition-transform hover:-translate-y-px"
          >
            <ExternalLink className="size-4" />
            View source
          </a>
          <a
            href="#architecture"
            className="inline-flex h-11 items-center justify-center gap-2 rounded-lg border border-border px-5 text-sm font-medium text-foreground transition-colors hover:bg-secondary/60"
          >
            How it works
            <ArrowRight className="size-4" />
          </a>
        </div>
      </div>
    </section>
  )
}
