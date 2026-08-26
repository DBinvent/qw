// Outbound links, in one place — the header, hero and footer all pointed at
// the repo separately before, so a change meant editing three files.
//
// `DBinvent/qw` went public 2026-08-24, so "View source" resolves for a
// visitor. If it ever goes private again, point SOURCE_URL at ABSTRACT_URL
// and relabel — a private repo makes the site's most prominent button a 404.
export const GITHUB_URL = 'https://github.com/DBinvent/qw'

// Design docs live in the GitHub Pages repo, rendered on github.com.
const DOCS_BASE = 'https://github.com/vkrinitsyn/vkrinitsyn.github.io/blob/master/qw'
export const ABSTRACT_URL = `${DOCS_BASE}/abstract.md`
export const FAQ_DOC_URL = `${DOCS_BASE}/qw-design-faq.md`

export const SOURCE_URL = GITHUB_URL
export const SOURCE_LABEL = 'View source'

// Files inside the public repo, linked from the join guide. `main` is the
// default branch and the one Workers Builds deploys this site from, so a
// blob link here and the deployed page always describe the same tree.
const REPO_BLOB = `${GITHUB_URL}/blob/main`
export const REPO_README_URL = `${REPO_BLOB}/README.md`
export const APP_README_URL = `${REPO_BLOB}/app/README.md`
export const nipUrl = (file: string) => `${REPO_BLOB}/protocol/nips/${file}`

// The Android build. Binaries are not in this repo — an APK is a build
// output, not source, and in git it bloats every clone forever while still
// only reaching a visitor when the site redeploys.
//
// app.knownby.work is the release host: nginx on the build machine, behind
// the tunnel and the Cloudflare edge.
//
// **Nothing here names a version.** A publish drops a new file, updates the
// manifest and repoints the `-latest` symlink; the site follows without
// being rebuilt or redeployed. Hard-coding the filename made every release
// also a site deploy, and left the page able to be wrong about its own
// checksum in between. Same shape as the Atria eval kit
// (`atria-eval.json` + `atria-eval-latest.tar.gz`) and RDBM's downloads
// list — three projects, one convention.
const DOWNLOADS_BASE = 'https://app.knownby.work'

/** Shape of that manifest. Written by the publish step, never by hand. */
export type AndroidRelease = {
  version: string
  file: string
  abi: string
  min_sdk: number
  sha256: string
  bytes: number
}

/**
 * The pre-JS href: a Worker route on this origin that reads the manifest and
 * redirects to the current versioned file (see `src/worker.ts`).
 *
 * Deliberately *not* `-latest.apk`. That symlink is served with
 * `max-age=300, must-revalidate` so it can track releases — correct for a
 * pointer, ruinous for 18 MB of payload, which would then revalidate at the
 * edge on every download. The redirect is what is short-lived; its target is
 * the versioned name, cached immutably for a year.
 */
export const ANDROID_APK_URL = '/download/android'

/** The exact file for a resolved release — immutable, and what a client
 *  with JavaScript should link to directly, skipping the redirect hop. */
export const androidFileUrl = (release: AndroidRelease) =>
  `${DOWNLOADS_BASE}/${release.file}`

/**
 * Read at view time for the facts a download page should state — version,
 * size, checksum. Served `Access-Control-Allow-Origin: *` by the origin,
 * since everything under it is a public download anyway.
 */
export const ANDROID_MANIFEST_URL = `${DOWNLOADS_BASE}/qw-android-arm64.json`

