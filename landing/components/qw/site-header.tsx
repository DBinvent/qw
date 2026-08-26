'use client'

import { useState } from 'react'
import { Menu, X } from 'lucide-react'
import { QwLogo } from './logo'
import { cn } from '@/lib/utils'
import { SOURCE_LABEL, SOURCE_URL } from '@/lib/links'

const navLinks = [
  { label: 'Architecture', href: '/#architecture' },
  { label: 'FAQ', href: '/#faq' },
  { label: 'How to join', href: '/join' },
]

export function SiteHeader() {
  const [open, setOpen] = useState(false)

  return (
    <header className="sticky top-0 z-50 border-b border-border/60 bg-background/80 backdrop-blur-xl">
      <div className="mx-auto flex h-16 max-w-5xl items-center justify-between px-4 sm:px-6">
        <a href="/#top" aria-label="QW home">
          <QwLogo />
        </a>

        <nav className="hidden items-center gap-8 md:flex" aria-label="Primary">
          {navLinks.map((link) => (
            <a key={link.label} href={link.href} className="text-sm text-muted-foreground transition-colors hover:text-foreground">
              {link.label}
            </a>
          ))}
        </nav>

        <div className="flex items-center gap-3">
          <a
            href={SOURCE_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="glow-violet hidden rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground transition-transform hover:-translate-y-px sm:inline-flex"
          >
            {SOURCE_LABEL}
          </a>
          <button
            type="button"
            onClick={() => setOpen((v) => !v)}
            className="inline-flex size-9 items-center justify-center rounded-lg border border-border text-foreground md:hidden"
            aria-label={open ? 'Close menu' : 'Open menu'}
            aria-expanded={open}
          >
            {open ? <X className="size-4" /> : <Menu className="size-4" />}
          </button>
        </div>
      </div>

      {/* max-h grows with navLinks: three links plus the CTA button no longer
          fit in the 10rem this used to be, and the overflow is hidden. */}
      <div className={cn('overflow-hidden border-t border-border/60 md:hidden', open ? 'max-h-64' : 'max-h-0')}>
        <nav className="flex flex-col gap-1 px-4 py-3" aria-label="Mobile">
          {navLinks.map((link) => (
            <a
              key={link.label}
              href={link.href}
              onClick={() => setOpen(false)}
              className="rounded-md px-2 py-2 text-sm text-muted-foreground hover:bg-muted hover:text-foreground"
            >
              {link.label}
            </a>
          ))}
          <a
            href={SOURCE_URL}
            target="_blank"
            rel="noopener noreferrer"
            onClick={() => setOpen(false)}
            className="mt-1 rounded-lg bg-primary px-4 py-2 text-center text-sm font-medium text-primary-foreground"
          >
            {SOURCE_LABEL}
          </a>
        </nav>
      </div>
    </header>
  )
}
