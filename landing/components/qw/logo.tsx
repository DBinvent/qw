import { cn } from '@/lib/utils'

export function QwLogo({ className }: { className?: string }) {
  return (
    <div className={cn('flex items-center gap-2', className)}>
      <span className="relative flex size-8 items-center justify-center rounded-md border border-border bg-card">
        <svg viewBox="0 0 24 24" fill="none" className="size-[18px] text-primary" aria-hidden="true">
          {/* Nodes + edges — a small web of trust */}
          <circle cx="12" cy="5" r="1.8" fill="currentColor" />
          <circle cx="5" cy="16" r="1.8" fill="currentColor" />
          <circle cx="19" cy="16" r="1.8" fill="currentColor" />
          <path d="M12 6.8 6 14.6M12 6.8l6 7.8M6.8 16h10.4" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
        </svg>
      </span>
      <span className="text-[17px] font-semibold tracking-tight text-foreground">QW</span>
    </div>
  )
}
