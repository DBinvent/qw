import { QwLogo } from './logo'

const GITHUB_URL = 'https://github.com/DBinvent/qw'

export function Footer() {
  return (
    <footer className="relative border-t border-border">
      <div className="mx-auto max-w-5xl px-4 py-12 sm:px-6">
        <div className="flex flex-col items-center justify-between gap-4 sm:flex-row">
          <QwLogo />
          <p className="font-mono text-xs text-muted-foreground">
            &copy; {new Date().getFullYear()} Vladimir Krinitsyn &middot; open source, MIT licensed
          </p>
          <nav className="flex items-center gap-5 text-xs text-muted-foreground">
            <a href="#architecture" className="hover:text-foreground">
              Architecture
            </a>
            <a href="#faq" className="hover:text-foreground">
              FAQ
            </a>
            <a href={GITHUB_URL} target="_blank" rel="noopener noreferrer" className="hover:text-foreground">
              GitHub
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
