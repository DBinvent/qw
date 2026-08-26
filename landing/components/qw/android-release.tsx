'use client'

import { useEffect, useState } from 'react'
import { Download } from 'lucide-react'
import { ANDROID_MANIFEST_URL, androidFileUrl, type AndroidRelease } from '@/lib/links'

// The current release, read from the origin at view time rather than baked
// into the page at build time.
//
// Why it is a hook and not a constant: publishing a build should not require
// deploying the site. The release host writes a manifest and repoints a
// `-latest` symlink; anything here that named a version would go stale the
// moment a build shipped, and — worse — a page that prints a checksum it is
// no longer serving is actively misleading on the one page that tells people
// to verify what they downloaded.
//
// `null` while loading and on failure alike: the download button is a plain
// link to `-latest` and never depends on this, so a failed fetch costs the
// version line and nothing else. Better a missing fact than a stale one.
export function useAndroidRelease(): AndroidRelease | null {
  const [release, setRelease] = useState<AndroidRelease | null>(null)

  useEffect(() => {
    let live = true
    fetch(ANDROID_MANIFEST_URL)
      .then((r) => (r.ok ? (r.json() as Promise<unknown>) : null))
      .then((data) => {
        // Only accept a manifest that actually carries what we print. A
        // half-written or unrelated JSON body should leave the page saying
        // nothing rather than saying something wrong.
        const m = data as Partial<AndroidRelease> | null
        if (live && m && typeof m.version === 'string' && typeof m.sha256 === 'string') {
          setRelease(m as AndroidRelease)
        }
      })
      .catch(() => {
        /* keep null; the link still works */
      })
    return () => {
      live = false
    }
  }, [])

  return release
}

/** `17.2 MB`, or null before the manifest lands. */
export function useAndroidSize(): string | null {
  const release = useAndroidRelease()
  return release ? `${(release.bytes / 1024 / 1024).toFixed(1)} MB` : null
}

/**
 * `arm64 · v0.1.1 · 17.2 MB`, degrading to just `arm64` until the manifest
 * arrives. Inline, for a caption beside a download button.
 *
 * The architecture is stated unconditionally because it is a property of
 * this build channel rather than of any one release; version and size are
 * properties of a file and therefore only ever come from the manifest.
 */
export function AndroidFacts() {
  const release = useAndroidRelease()
  if (!release) return <>arm64</>
  return (
    <>
      arm64 · v{release.version} · {(release.bytes / 1024 / 1024).toFixed(1)} MB
    </>
  )
}

/**
 * The download block on /join: the button, the facts, and the checksum.
 *
 * The button is a plain link rendered immediately — it does not wait for the
 * manifest and does not break if the manifest never arrives, because a
 * download page whose download depends on a second request is a download
 * page that is sometimes broken.
 *
 * The checksum is the reason none of this is hard-coded. This page tells
 * people to verify what they downloaded; printing a hash from build time
 * while the origin serves a newer file would make that instruction fail for
 * the honest reader and teach them to ignore it. It is either the hash of
 * the file currently behind the link, or it is absent.
 */
export function AndroidDownload({ href }: { href: string }) {
  const release = useAndroidRelease()

  // Upgrade to the exact file once it is known, so the click lands on the
  // immutable name and is served from the edge. The fallback is the Worker
  // redirect, not `-latest`: that symlink revalidates every 5 minutes, which
  // is right for a pointer and wrong for the payload behind it.
  const target = release ? androidFileUrl(release) : href

  return (
    <>
      <div className="mt-5 flex flex-wrap items-center gap-x-4 gap-y-3">
        <a
          href={target}
          className="glow-violet inline-flex h-11 items-center justify-center gap-2 rounded-lg bg-primary px-5 text-sm font-medium text-primary-foreground transition-transform hover:-translate-y-px"
        >
          <Download className="size-4" />
          Download the APK
        </a>
        <span className="font-mono text-xs text-muted-foreground">
          {release ? `${(release.bytes / 1024 / 1024).toFixed(1)} MB · ` : ''}
          arm64-v8a · Android {release ? androidFor(release.min_sdk) : '7.0'}+
        </span>
      </div>
      <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
        Android will ask you to allow installs from whatever app opened it; that prompt is what
        sideloading is. Check what you got before you tap it —{' '}
        <code className="font-mono text-xs text-foreground">sha256sum</code> on the file must
        print:
      </p>
      <p className="mt-2 break-all font-mono text-[11px] leading-relaxed text-muted-foreground">
        {release ? (
          release.sha256
        ) : (
          <span>
            fetching from{' '}
            <a href={ANDROID_MANIFEST_URL} className="text-primary hover:text-primary/80">
              the release manifest
            </a>
            …
          </span>
        )}
      </p>
    </>
  )
}

/** minSdk -> the Android version a person recognises. */
function androidFor(minSdk: number): string {
  const known: Record<number, string> = { 24: '7.0', 26: '8.0', 28: '9', 29: '10', 30: '11' }
  return known[minSdk] ?? `API ${minSdk}`
}

/** `v0.1.1`, or nothing at all until the manifest lands. */
export function AndroidVersion() {
  const release = useAndroidRelease()
  return release ? <>v{release.version}, </> : null
}
