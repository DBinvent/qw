'use client'

import { useMemo } from 'react'
import QRCode from 'qrcode'

// The invite link as a scannable code. Rendered from the module matrix
// rather than from the library's own SVG string: that path would need
// dangerouslySetInnerHTML, and going through the matrix also lets the
// quiet zone, the colours and the run-merging below be this file's
// decision instead of the library's.
//
// Mirrors qw_client_core::invite_qr_svg, which does the same job inside the
// app. Two implementations because the two sides share no runtime — but
// they encode the same string, the full https://knownby.work/i/<npub> URL,
// so a code from either scans to the same place.

/** Modules per side of the quiet zone. Four is the spec's minimum. */
const QUIET = 4

function pathFor(value: string): { d: string; size: number } {
  const { modules } = QRCode.create(value, { errorCorrectionLevel: 'M' })
  const size = modules.size
  const parts: string[] = []

  // One rect per dark module would be correct and about 8x larger. Merging
  // each row's consecutive dark modules into a single rect keeps the
  // rendered page small enough not to notice.
  for (let y = 0; y < size; y++) {
    let run = 0
    for (let x = 0; x <= size; x++) {
      const dark = x < size && modules.data[y * size + x] === 1
      if (dark) {
        run += 1
        continue
      }
      if (run > 0) {
        parts.push(`M${x - run} ${y}h${run}v1h-${run}z`)
        run = 0
      }
    }
  }
  return { d: parts.join(''), size }
}

export function InviteQr({ value, className }: { value: string; className?: string }) {
  const { d, size } = useMemo(() => pathFor(value), [value])
  const span = size + QUIET * 2

  return (
    <svg
      viewBox={`0 0 ${span} ${span}`}
      className={className}
      role="img"
      aria-label="QR code for this invite link"
      shapeRendering="crispEdges"
    >
      {/* Light modules have to actually be light and dark ones dark, in both
          site themes: a camera reads contrast, and an inverted code fails on
          enough scanners that theming it would cost the only thing it does.
          So the plate is painted here rather than inherited. */}
      <rect width={span} height={span} fill="#ffffff" />
      <g transform={`translate(${QUIET} ${QUIET})`}>
        <path d={d} fill="#0f0d1a" />
      </g>
    </svg>
  )
}
