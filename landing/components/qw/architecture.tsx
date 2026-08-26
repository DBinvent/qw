import {
  ArrowRight,
  Clock,
  GitBranch,
  Inbox,
  KeyRound,
  type LucideIcon,
  Network,
  ScrollText,
  ShieldCheck,
  UserPlus,
} from 'lucide-react'
import { ABSTRACT_URL } from '@/lib/links'

type Point = {
  icon: LucideIcon
  title: string
  desc: string
  tag: string
  /** Set on the one card that leads somewhere; see the note on "How to join". */
  href?: string
  cta?: string
}

const points: Point[] = [
  {
    icon: Clock,
    title: 'Quant: a unit, not a currency',
    desc: '1 Quant = a quarter hour of work at minimum qualification. It denominates value on a contract but has no ledger of its own — what you hold is your counterparties’ signed records, not a balance.',
    tag: 'Time-denominated',
  },
  {
    icon: Network,
    title: 'Web of trust, from completed work',
    desc: 'Reputation isn’t a separate score — it emerges from the job lifecycle itself. Trust is domain-specific and computed locally by each participant, with no global credit score.',
    tag: 'Local & subjective',
  },
  {
    icon: ShieldCheck,
    title: 'No blockchain needed',
    desc: 'Every contract is bilateral and both parties sign, so omission is provable by production, not by gap analysis. No consensus, no mining, no validator set, no gas token.',
    tag: 'Detection over prevention',
  },
  {
    icon: ScrollText,
    title: 'Signed records, no consensus',
    desc: 'Contribution records are Verifiable Credentials, dual-indexed under both parties’ keys and checked against a publication window — omission is detectable within hours, not assumed absent.',
    tag: 'W3C VC',
  },
  {
    icon: GitBranch,
    title: 'Sybil resistance via cascade block',
    desc: 'Trust flows down a signed chain; a block propagates up it. Behind any bot farm sits a limited number of real signing accounts — find and block those, and the farm falls with them.',
    tag: 'Social, not algorithmic',
  },
  {
    icon: Inbox,
    title: 'Neither side has to be online',
    desc: 'No step of a contract needs the counterparty — or a network — reachable at the moment you take it; each one is composed and signed from what you already hold. Relays carry the signed records on to the other side whenever their client next wakes.',
    tag: 'Store and forward',
  },
  {
    icon: KeyRound,
    title: 'did:key / did:web + Nostr',
    desc: 'Identity is a keypair, not an account with a provider. Storage and transport run over Nostr relays; a client is a thin signing device — the same model Signal uses for messages.',
    tag: 'No central server',
  },
  {
    icon: UserPlus,
    title: 'How to join',
    desc: 'No gate and no waiting list — install the app, let it generate a keypair, and you are in. Your invite link works anywhere you can post a URL, and someone who follows it arrives as a first-degree contact instead of a stranger several hops away.',
    tag: 'Open, not invite-only',
    // The only card with a href: the others state a property, this one is an
    // instruction, and "you are in" invites the obvious follow-up question of
    // what exactly you install. /join answers it, including what does not
    // exist yet.
    href: '/join',
    cta: 'Read the joining guide',
  },
]

export function Architecture() {
  return (
    <section id="architecture" className="relative scroll-mt-20 border-t border-border">
      <div className="mx-auto max-w-5xl px-4 py-20 sm:px-6 sm:py-28">
        <div className="max-w-2xl">
          <p className="font-mono text-xs uppercase tracking-widest text-primary">How it works</p>
          <h2 className="mt-3 text-balance text-3xl font-semibold tracking-tight text-foreground sm:text-4xl">
            A contribution graph, not a ledger
          </h2>
          <p className="mt-4 text-pretty leading-relaxed text-muted-foreground">
            Every contract runs the same lifecycle — <span className="text-foreground">Job Request → Acceptance →
            Milestones → Completion → Multi-party Sign</span> — and each signed contract is itself a trust
            relationship. If Alice and Bob completed a job, Alice trusts Bob in that domain; Carol can walk the
            chain from Alice to evaluate Bob without a central authority computing anything on her behalf.
          </p>
        </div>

        <div className="mt-12 grid gap-px overflow-hidden rounded-xl border border-border bg-border sm:grid-cols-2 lg:grid-cols-3">
          {points.map((p) => {
            // A card with a href is a link end to end rather than a div with a
            // link inside it: the whole tile is already a hover target, and a
            // 6rem-wide "read more" would be the only thing on this grid you
            // have to aim at.
            const Card = p.href ? 'a' : 'div'
            return (
              <Card
                key={p.title}
                {...(p.href ? { href: p.href } : {})}
                className="group relative block bg-card p-6 transition-colors hover:bg-secondary/60"
              >
                <div className="flex items-center justify-between">
                  <span className="flex size-10 items-center justify-center rounded-lg border border-border bg-secondary text-primary transition-colors group-hover:border-primary/40">
                    <p.icon className="size-5" />
                  </span>
                  <span className="font-mono text-[10px] uppercase tracking-wider text-muted-foreground">{p.tag}</span>
                </div>
                <h3 className="mt-5 text-base font-semibold text-foreground">{p.title}</h3>
                <p className="mt-2 text-sm leading-relaxed text-muted-foreground">{p.desc}</p>
                {p.cta ? (
                  <span className="mt-4 inline-flex items-center gap-1.5 text-sm font-medium text-primary">
                    {p.cta}
                    <ArrowRight className="size-4 transition-transform group-hover:translate-x-0.5" />
                  </span>
                ) : null}
              </Card>
            )
          })}
        </div>

        <p className="mt-8 text-sm leading-relaxed text-muted-foreground">
          Full design rationale — platform selection, mobile architecture, data model, privacy, and legal framing
          — lives in{' '}
          <a
            href={ABSTRACT_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="text-primary hover:text-primary/80"
          >
            abstract.md
          </a>
          .
        </p>
      </div>
    </section>
  )
}
