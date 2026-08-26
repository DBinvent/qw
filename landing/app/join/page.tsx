import type { Metadata } from 'next'
import {
  ArrowUpRight,
  CheckCircle2,
  CircleDashed,
  Download,
  Hammer,
  KeyRound,
  Link2,
  Send,
  Smartphone,
  Tags,
} from 'lucide-react'
import { SiteHeader } from '@/components/qw/site-header'
import { Footer } from '@/components/qw/footer'
import {
  ANDROID_APK_SHA256,
  ANDROID_APK_SIZE,
  ANDROID_APK_URL,
  ANDROID_APK_VERSION,
  APP_README_URL,
  GITHUB_URL,
  REPO_README_URL,
  nipUrl,
} from '@/lib/links'

// The detail behind the "How to join" card on the home page. The card states
// the property (open, no gate); this page is the instruction — what a person
// installs, what the app does on first launch, and which of it exists today.
//
// Rule for editing this page: every status claim below has to be true of the
// repo at `main`, not of the plan. There is an Android APK now, so the button
// is real — but the same rule is what makes the block around it say, in the
// same breath, that the build has never been launched on a phone. A download
// that oversells what it is would be the one thing that makes the rest of the
// page untrustworthy; a download that states its own limits is not.

export const metadata: Metadata = {
  title: 'How to join — QW',
  description:
    'What joining QW means: an app on your phone that generates a keypair, an invite link that makes you reachable, and skill tags that say what you do. No signup, no gate, no waiting list.',
}

const steps = [
  {
    icon: Smartphone,
    n: '01',
    title: 'Get the app on your phone',
    lead: 'QW is a mobile client first — the phone holds the key and signs; there is nothing else to log in to.',
    body: [
      'One codebase (Rust + Tauri v2) targets Android, iOS and desktop. The phone is the primary shape because a signing device is something you carry, and because a thin client that syncs when it next wakes is the only realistic model when the counterparty is asleep half the time.',
      'The client runs no relay and no DHT. It signs what you tell it to, hands the result to whatever server or relay it can reach, and collects what arrived for you. That is the whole job.',
    ],
  },
  {
    icon: KeyRound,
    n: '02',
    title: 'First launch generates your identity',
    lead: 'One secp256k1 keypair, made on the device. That is the account — there is no email, no password and no server that could issue you one.',
    body: [
      'The same key is both a did:key controller id and a Nostr pubkey, so the identity that signs your contracts is the identity that publishes your events.',
      'It is stored in the app data directory, private to the app, file mode 0600. Back it up. Nobody can reissue it: losing the key is losing every record signed with it, and a client that quietly generated a fresh one instead of reporting a corrupt key file would look like being logged out while abandoning your history. So it reports.',
    ],
  },
  {
    icon: Link2,
    n: '03',
    title: 'Open an invite link — or post your own',
    lead: 'Following knownby.work/i/<npub> exchanges two signed introductions and puts you one hop from the publisher.',
    body: [
      'An invite link is just a public key in a URL, so it works anywhere a URL works — a LinkedIn profile, a talk slide, an email signature, a README badge. Whoever publishes it has consented in advance to the introduction, so your client signs your half and the publisher’s client answers with theirs.',
      'You do not need one to join. A self-introduction to someone you found through a referral query, or a mutual introduction carried by a shared contact, are the other two shapes of the same event. There is no admission step behind any of them — a signed introduction is the membership.',
    ],
  },
  {
    icon: Tags,
    n: '04',
    title: 'Say what you do, then do some of it',
    lead: 'Skill tags are the claim; completed, countersigned contracts are the evidence.',
    body: [
      'Publishing a tag costs nothing and proves nothing, by design. Trust is computed from finished work — a hop-1 contact with no contracts counts exactly as much as a stranger four hops out: nothing.',
      'That is also why an invite link cannot be used to inflate anyone. The link edge makes you reachable, so queries can route to you and offers can arrive. It vouches for no one, and a link you posted publicly is marked as such so it never lends your vouchers to a cascade block.',
    ],
  },
]

const platforms = [
  {
    name: 'Android',
    state: 'signed apk, untested',
    tone: 'progress' as const,
    detail:
      'Tauri v2 mobile target. A signed arm64 APK is downloadable at the top of this page; it is a sideload build, not a Play listing, and it has never been launched on a device. Building it yourself needs the Android SDK, NDK and a JDK.',
  },
  {
    name: 'iOS',
    state: 'same codebase, unbuilt',
    tone: 'todo' as const,
    detail:
      'Nothing iOS-specific stands in the way, but it needs macOS and Xcode, which this project has not had access to.',
  },
  {
    name: 'Desktop',
    state: 'compiles',
    tone: 'progress' as const,
    detail:
      'Linux/macOS/Windows via the same shell. It compiles and is clippy-clean; it has not been through a release build or a usability pass.',
  },
  {
    name: 'Web',
    state: 'planned',
    tone: 'todo' as const,
    detail:
      'Compose and display only, with signing delegated by QR or deep link to an external signer app. The delegation protocol exists (qw-signer: URIs); the web app does not.',
  },
]

const built = [
  {
    done: true,
    what: 'Protocol layer',
    detail:
      'Identity, the job lifecycle, trust paths, cascade block, referral routing, introductions and invite links — 134 tests pass across the workspace.',
  },
  {
    done: true,
    what: 'Client core',
    detail:
      'On-disk identity, the HTTP mailbox transport, and the invite-link flow — tested against a real server on a real socket.',
  },
  {
    done: true,
    what: 'Store-and-forward mailbox',
    detail:
      'A coordination server holds signed events for a recipient who is offline. Everything delivered is verified on your device, so a hostile cache can withhold mail but cannot inject any. It never requires a login.',
  },
  {
    done: false,
    what: 'A released app',
    detail:
      'Android has a signed APK you can sideload from this page — one architecture, never run on hardware, on no store. Nothing is packaged for iOS or desktop, and no platform has been through a release build that someone then used.',
  },
  {
    done: false,
    what: 'Relays and deep links',
    detail:
      'No public relay or gateway runs yet, and clicking an invite link does not open the app — the link has to be pasted into it.',
  },
]

const toneStyles = {
  progress: 'border-primary/40 text-primary',
  todo: 'border-border text-muted-foreground',
}

export default function JoinPage() {
  return (
    <div id="top" className="min-h-screen bg-background">
      <SiteHeader />
      <main>
        <section className="relative overflow-hidden border-b border-border">
          <div
            aria-hidden="true"
            className="pointer-events-none absolute left-1/2 top-0 h-64 w-[36rem] -translate-x-1/2 rounded-full bg-primary/15 blur-[120px]"
          />
          <div className="relative mx-auto max-w-3xl px-4 py-20 sm:px-6 sm:py-24">
            <p className="font-mono text-xs uppercase tracking-widest text-primary">How to join</p>
            <h1 className="mt-3 text-balance text-3xl font-semibold tracking-tight text-foreground sm:text-4xl">
              An app, a keypair, and one link
            </h1>
            <p className="mt-5 text-pretty leading-relaxed text-muted-foreground">
              There is no signup form on this site, and there is no list to be admitted from. Joining QW
              means installing a client, letting it generate a key, and getting introduced to one person
              who is already reachable. Everything after that is ordinary use of the protocol.
            </p>

            {/* Stated before the instructions, not after them: someone who
                follows four steps and then discovers what they installed has
                never been run has been wasted, and the page has lied by
                omission. The caveats sit next to the button, not below it. */}
            <div className="mt-8 rounded-xl border border-primary/40 bg-card p-5">
              <p className="font-mono text-xs uppercase tracking-widest text-primary">
                Android — v{ANDROID_APK_VERSION}, signed, unproven
              </p>
              <p className="mt-3 text-sm leading-relaxed text-muted-foreground">
                There is an APK. It is signed, it installs by sideload rather than from Play, and it has{' '}
                <strong className="font-medium text-foreground">never been launched on a phone</strong> —
                the protocol and client core are tested, the shell compiles, and nobody has yet watched
                this build start on real hardware. Treat it as the first thing to try, not as a release.
                iOS and desktop have no package at all.
              </p>
              <div className="mt-5 flex flex-wrap items-center gap-x-4 gap-y-3">
                {/* Cross-origin, so the `download` attribute would be
                    ignored — the file arrives as a download because nginx
                    says so, not because the anchor asks. */}
                <a
                  href={ANDROID_APK_URL}
                  className="glow-violet inline-flex h-11 items-center justify-center gap-2 rounded-lg bg-primary px-5 text-sm font-medium text-primary-foreground transition-transform hover:-translate-y-px"
                >
                  <Download className="size-4" />
                  Download the APK
                </a>
                <span className="font-mono text-xs text-muted-foreground">
                  {ANDROID_APK_SIZE} · arm64-v8a · Android 7.0+
                </span>
              </div>
              <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
                Android will ask you to allow installs from whatever app opened it; that prompt is what
                sideloading is. Check what you got before you tap it —{' '}
                <code className="font-mono text-xs text-foreground">sha256sum</code> on the file must
                print:
              </p>
              <p className="mt-2 break-all font-mono text-[11px] leading-relaxed text-muted-foreground">
                {ANDROID_APK_SHA256}
              </p>
              <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
                The file comes from <span className="font-mono text-xs">apt.dbinvent.com/paks</span>,
                the release area on the machine that builds it — same hands, a different domain, which
                is worth saying out loud on a page about not trusting things by default.
              </p>
              <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
                You can also build it from source below, or leave your address in the footer and be told
                when there is something better than this. Nothing on this page is a waiting list: joining
                needs no permission from us.
              </p>
            </div>
          </div>
        </section>

        <section className="border-b border-border">
          <div className="mx-auto max-w-3xl px-4 py-16 sm:px-6 sm:py-20">
            <h2 className="text-2xl font-semibold tracking-tight text-foreground sm:text-3xl">
              What it will look like
            </h2>
            <p className="mt-3 leading-relaxed text-muted-foreground">
              Four steps. Two of them are things the app does for you; the other two take about a minute.
            </p>

            <ol className="mt-10 space-y-px overflow-hidden rounded-xl border border-border bg-border">
              {steps.map((s) => (
                <li key={s.n} className="bg-card p-6 sm:p-8">
                  <div className="flex items-center gap-3">
                    <span className="flex size-10 shrink-0 items-center justify-center rounded-lg border border-border bg-secondary text-primary">
                      <s.icon className="size-5" />
                    </span>
                    <span className="font-mono text-xs uppercase tracking-widest text-muted-foreground">
                      Step {s.n}
                    </span>
                  </div>
                  <h3 className="mt-5 text-base font-semibold text-foreground">{s.title}</h3>
                  <p className="mt-2 text-sm leading-relaxed text-foreground/90">{s.lead}</p>
                  {s.body.map((p) => (
                    <p key={p.slice(0, 24)} className="mt-3 text-sm leading-relaxed text-muted-foreground">
                      {p}
                    </p>
                  ))}
                </li>
              ))}
            </ol>

            <p className="mt-8 text-sm leading-relaxed text-muted-foreground">
              The event behind step 3 is{' '}
              <a
                href={nipUrl('NIP-QW07-introduction.md')}
                target="_blank"
                rel="noopener noreferrer"
                className="text-primary hover:text-primary/80"
              >
                NIP-QW07
              </a>{' '}
              (kind 9060, all three shapes plus the invite-link form); step 4 is{' '}
              <a
                href={nipUrl('NIP-QW03-profile-skill-tags.md')}
                target="_blank"
                rel="noopener noreferrer"
                className="text-primary hover:text-primary/80"
              >
                NIP-QW03
              </a>
              . Why a public link does not propagate a block is in{' '}
              <a
                href={nipUrl('NIP-QW05-cascade-block.md')}
                target="_blank"
                rel="noopener noreferrer"
                className="text-primary hover:text-primary/80"
              >
                NIP-QW05
              </a>
              .
            </p>
          </div>
        </section>

        <section className="border-b border-border">
          <div className="mx-auto max-w-3xl px-4 py-16 sm:px-6 sm:py-20">
            <h2 className="text-2xl font-semibold tracking-tight text-foreground sm:text-3xl">
              The app, platform by platform
            </h2>
            <p className="mt-3 leading-relaxed text-muted-foreground">
              One Rust core, one web frontend, four targets. Where a platform is not there yet, the reason
              is tooling, not architecture.
            </p>

            <div className="mt-8 grid gap-px overflow-hidden rounded-xl border border-border bg-border sm:grid-cols-2">
              {platforms.map((p) => (
                <div key={p.name} className="bg-card p-6">
                  <div className="flex items-center justify-between gap-3">
                    <h3 className="text-base font-semibold text-foreground">{p.name}</h3>
                    <span
                      className={`rounded-full border px-2.5 py-1 font-mono text-[10px] uppercase tracking-wider ${toneStyles[p.tone]}`}
                    >
                      {p.state}
                    </span>
                  </div>
                  <p className="mt-3 text-sm leading-relaxed text-muted-foreground">{p.detail}</p>
                </div>
              ))}
            </div>
          </div>
        </section>

        <section className="border-b border-border">
          <div className="mx-auto max-w-3xl px-4 py-16 sm:px-6 sm:py-20">
            <h2 className="text-2xl font-semibold tracking-tight text-foreground sm:text-3xl">
              What exists today
            </h2>

            <ul className="mt-8 divide-y divide-border rounded-xl border border-border bg-card">
              {built.map((b) => (
                <li key={b.what} className="flex gap-4 p-5">
                  {b.done ? (
                    <CheckCircle2 className="mt-0.5 size-5 shrink-0 text-primary" aria-label="built" />
                  ) : (
                    <CircleDashed className="mt-0.5 size-5 shrink-0 text-muted-foreground" aria-label="not built" />
                  )}
                  <div>
                    <h3 className="text-sm font-semibold text-foreground">{b.what}</h3>
                    <p className="mt-1 text-sm leading-relaxed text-muted-foreground">{b.detail}</p>
                  </div>
                </li>
              ))}
            </ul>
          </div>
        </section>

        <section className="border-b border-border">
          <div className="mx-auto max-w-3xl px-4 py-16 sm:px-6 sm:py-20">
            <div className="flex items-center gap-3">
              <span className="flex size-10 items-center justify-center rounded-lg border border-border bg-secondary text-primary">
                <Hammer className="size-5" />
              </span>
              <h2 className="text-2xl font-semibold tracking-tight text-foreground sm:text-3xl">
                Build it yourself today
              </h2>
            </div>
            <p className="mt-4 leading-relaxed text-muted-foreground">
              Rust stable, no services, no network. These run locally and publish nothing anywhere — there
              is no relay and no public gateway yet, so this is a developer path rather than a way onto a
              running network.
            </p>

            <div className="mt-6 overflow-x-auto rounded-xl border border-border bg-card p-5">
              <pre className="font-mono text-xs leading-relaxed text-muted-foreground">
                <code>{`git clone ${GITHUB_URL}.git
cd qw

# the protocol, end to end
cargo test --workspace

# greedy referral routing over a synthetic contact graph
cargo run -p qw-node --example referral_demo

# skill tags and co-authorship pairs inferred from a real repo
cargo run -p qw-node --example bootstrap_from_git -- <path/to/a/git/repo>

# the client shell (needs webkit2gtk et al; see app/README.md)
cd app/src-tauri && cargo tauri dev

# the Android build (needs a JDK, the Android SDK and the NDK)
cargo tauri android init && cargo tauri android dev`}</code>
              </pre>
            </div>

            <p className="mt-5 text-sm leading-relaxed text-muted-foreground">
              Exact prerequisites, and which of these has actually been run, are in{' '}
              <a
                href={APP_README_URL}
                target="_blank"
                rel="noopener noreferrer"
                className="text-primary hover:text-primary/80"
              >
                app/README.md
              </a>
              ; the rest of the repo is described in the{' '}
              <a
                href={REPO_README_URL}
                target="_blank"
                rel="noopener noreferrer"
                className="text-primary hover:text-primary/80"
              >
                project README
              </a>
              .
            </p>
          </div>
        </section>

        <section>
          <div className="mx-auto max-w-3xl px-4 py-16 sm:px-6 sm:py-20">
            <div className="rounded-xl border border-border bg-card p-6 sm:p-8">
              <div className="flex items-center gap-3">
                <span className="flex size-10 items-center justify-center rounded-lg border border-border bg-secondary text-primary">
                  <Send className="size-5" />
                </span>
                <h2 className="text-base font-semibold text-foreground">Told when the app ships</h2>
              </div>
              <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
                The signup box at the bottom of this page is the only list QW keeps, and it is a mailing
                list, not a queue — being on it grants nothing and skips nothing, because there is nothing
                to skip. Double opt-in, one click to leave.
              </p>
              <div className="mt-6 flex flex-wrap gap-3">
                <a
                  href={GITHUB_URL}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="glow-violet inline-flex h-11 items-center justify-center gap-2 rounded-lg bg-primary px-5 text-sm font-medium text-primary-foreground transition-transform hover:-translate-y-px"
                >
                  View source
                  <ArrowUpRight className="size-4" />
                </a>
                <a
                  href="/#architecture"
                  className="inline-flex h-11 items-center justify-center gap-2 rounded-lg border border-border px-5 text-sm font-medium text-foreground transition-colors hover:bg-secondary/60"
                >
                  How QW works
                </a>
              </div>
            </div>
          </div>
        </section>
      </main>
      <Footer />
    </div>
  )
}
