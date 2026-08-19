import { ChevronDown } from 'lucide-react'

const FAQ_DOC_URL = 'https://github.com/vkrinitsyn/vkrinitsyn.github.io/blob/master/qw/qw-design-faq.md'

const faqs = [
  {
    q: 'Is Quant a cryptocurrency?',
    a: 'No. A Quant is a unit of measure — 1 Quant = a quarter hour of work at minimum qualification, analogous to a meter or an hour. It denominates the value of work on a contract but has no ledger of its own; what you hold is your counterparties’ signed records, not a balance or a tradeable token.',
  },
  {
    q: 'Do I need a blockchain to use this?',
    a: 'No. Global ordering is unnecessary — bilateral signatures already make omission provable by production. Consensus is unnecessary — the design accepts detection over prevention. And token transfer is unnecessary, since the unit is time, not a tradeable asset.',
  },
  {
    q: 'Who computes my reputation?',
    a: 'You do, locally, from the public graph of signed contribution records — with your own weights and tolerances. There is no global credit score; there are as many subjective readings of the same public record as there are participants.',
  },
  {
    q: 'What stops fake accounts or bot farms?',
    a: 'Trust flows down a signed chain, and a block propagates up it. Every account is signed into the web of trust by someone, so behind any bot farm sits a small number of real signing accounts — find and block those, and the whole farm falls with them. Detection is social, not algorithmic.',
  },
  {
    q: 'Does exchanging work through this count as taxable barter?',
    a: 'The structural defense only holds when work is on a declared open-source project and no project involved is controlled by the counterparty in a way that privatizes the benefit — the same shape as ordinary open-source co-authorship. Direct bilateral work-for-work, or contribution to a counterparty-controlled private project, falls outside that framing and is the participants’ own responsibility to assess. This has not been confirmed by a written tax attorney opinion — treat it as an engineering description of the system, not legal advice.',
  },
  {
    q: 'Can I delete something I published?',
    a: 'Deletion here is advisory only. A relay may honor a deletion request, but nothing requires it to, and any relay, contact, or archive that already copied a record may keep it indefinitely. If your jurisdiction grants a legal right to deletion (e.g. GDPR), publishing through this protocol may not by itself satisfy it — don’t publish anything you may later be legally required to delete.',
  },
  {
    q: 'What’s the technical stack?',
    a: 'did:key / did:web for identity, W3C Verifiable Credentials with SD-JWT selective disclosure for attestations, Nostr relays for storage and transport (ATProto as a migration target), and Tauri v2 for Android, iOS, and web from one codebase, with signing delegated to an external signer app.',
  },
  {
    q: 'What’s actually built today?',
    a: 'An early prototype: the protocol layer plus a local referral-routing demo. Nothing here is ready for real transactions or real personal data yet.',
  },
]

export function Faq() {
  return (
    <section id="faq" className="relative scroll-mt-20 border-t border-border">
      <div className="mx-auto max-w-3xl px-4 py-20 sm:px-6 sm:py-28">
        <p className="font-mono text-xs uppercase tracking-widest text-primary">FAQ</p>
        <h2 className="mt-3 text-balance text-3xl font-semibold tracking-tight text-foreground sm:text-4xl">
          Common questions
        </h2>

        <div className="mt-10 divide-y divide-border rounded-xl border border-border bg-card">
          {faqs.map((item) => (
            <details key={item.q} className="group p-5 open:pb-5">
              <summary className="flex cursor-pointer list-none items-center justify-between gap-4 text-sm font-medium text-foreground marker:content-none">
                {item.q}
                <ChevronDown className="size-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-180" />
              </summary>
              <p className="mt-3 text-sm leading-relaxed text-muted-foreground">{item.a}</p>
            </details>
          ))}
        </div>

        <p className="mt-8 text-sm leading-relaxed text-muted-foreground">
          More open questions and the reasoning behind each decision are in the{' '}
          <a href={FAQ_DOC_URL} target="_blank" rel="noopener noreferrer" className="text-primary hover:text-primary/80">
            full design FAQ
          </a>
          .
        </p>
      </div>
    </section>
  )
}
