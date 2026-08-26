import { cn } from '@/lib/utils'

export function QwLogo({ className }: { className?: string }) {
  return (
    <div className={cn('flex items-center gap-2', className)}>
      <span className="relative flex size-8 items-center justify-center rounded-md border border-border bg-card">
        <svg viewBox="0 0 24 24" fill="none" className="size-[18px] text-primary" aria-hidden="true">
          {/* Nodes + edges as an hourglass: two triangles sharing a waist.
              A small web of trust, and the shape of what it measures. Kept
              byte-for-byte in sync with app/icon.svg, which is the favicon
              and cannot use currentColor. */}
          <circle cx="5.5" cy="3.5" r="1.7" fill="currentColor" />
          <circle cx="18.5" cy="3.5" r="1.7" fill="currentColor" />
          <circle cx="12" cy="12" r="1.7" fill="currentColor" />
          <circle cx="5.5" cy="20.5" r="1.7" fill="currentColor" />
          <circle cx="18.5" cy="20.5" r="1.7" fill="currentColor" />
          <path
            d="M5.5 3.5h13M5.5 3.5 12 12l6.5-8.5M12 12l-6.5 8.5M12 12l6.5 8.5M5.5 20.5h13"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </span>
      <span className="text-[17px] font-semibold tracking-tight text-foreground">QW</span>
    </div>
  )
}
