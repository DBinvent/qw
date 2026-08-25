import { QwLogo } from './logo'
import { SubscribeForm } from '@/components/news/subscribe-form'
import { SOURCE_LABEL, SOURCE_URL } from '@/lib/links'

export function Footer() {
  return (
    <footer className="relative border-t border-border">
      <div className="mx-auto max-w-5xl px-4 py-12 sm:px-6">
        {/*
          Generated from ../../../../user_mgmt/web — the same signup box the
          dash sites use, posting to /api/news/subscribe, which src/worker.ts
          forwards to qw-server. Double opt-in: nothing is sent to the address
          until the confirmation link is clicked.
        */}
        <div className="mb-10 flex justify-center border-b border-border pb-10">
          <SubscribeForm
            source="qw-landing"
            heading="QW updates"
            blurb="Occasional notes on the protocol and releases. One click to stop."
          />
        </div>
        <div className="flex flex-col items-center justify-between gap-4 sm:flex-row">
          <QwLogo />
          <p className="font-mono text-xs text-muted-foreground">
            &copy; {new Date().getFullYear()}  Vladimir Krinitsyn &middot; open source, MIT licensed
          </p>
          <nav className="flex items-center gap-5 text-xs text-muted-foreground">
            <a href="#architecture" className="hover:text-foreground">
              Architecture
            </a>
            <a href="#faq" className="hover:text-foreground">
              FAQ
            </a>
            <a href={SOURCE_URL} target="_blank" rel="noopener noreferrer" className="hover:text-foreground">
              {SOURCE_LABEL}
            </a>
          </nav>
        </div>
        <p className="mt-6 text-center text-xs leading-relaxed text-muted-foreground sm:text-left">
          Early prototype. Deletion is advisory only, and the co-authorship tax framing has not been confirmed by a
          written tax opinion — see the repository README before publishing anything or referencing this project
          externally.
        </p>
      </div>
    </footer>
  )
}
