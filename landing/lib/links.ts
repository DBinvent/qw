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

// The Android build. Binaries are not in this repo: apt.dbinvent.com/paks/ is
// the release area on the machine that builds them (/media/lab/repo/paks/,
// nginx behind Cloudflare), the same place the Atria eval kit ships from. An
// APK is a build output, not source — in git it bloats every clone forever
// and still only reaches a visitor when the site redeploys.
//
// The host is deliberately one constant. Serving downloads from dbinvent.com
// while the site is knownby.work mixes two identities; the fix, when that is
// worth doing, is a downloads.knownby.work CNAME onto the same nginx behind
// the same Cloudflare cache — after which only this line changes.
const DOWNLOADS_BASE = 'https://apt.dbinvent.com/paks'

// The versioned name carries the content hash, so the edge holds it immutable
// for a year (the $paks_cache map in nginx-apt.conf keys off the version in
// the filename). A new release publishes a new name and edits these four
// lines; it never overwrites an old one, whose bytes are cached everywhere.
export const ANDROID_APK_URL = `${DOWNLOADS_BASE}/qw-android-arm64-0.1.0-376db940.apk`
export const ANDROID_APK_VERSION = '0.1.0'
export const ANDROID_APK_SIZE = '17.2 MB'
// Printed on the join page so a sideloaded binary can be checked against the
// site that told you to install it — `sha256sum` on the file must match.
export const ANDROID_APK_SHA256 =
  '376db9403204aff432454bd5d750f58ec428e3d129e878210fd36c62c77da234'
