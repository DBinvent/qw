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
