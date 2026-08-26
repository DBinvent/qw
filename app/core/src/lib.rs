//! QW client core (§7): everything the app shell wraps, with no UI
//! toolkit anywhere in the dependency tree.
//!
//! The split is deliberate and structural, not stylistic. Tauri needs
//! webkit2gtk and friends to compile at all, so anything living beside it
//! can only be built on a machine with those system libraries installed.
//! Identity storage, HTTP transport and the invite flow have nothing to do
//! with a window, so they live here instead — buildable and testable on
//! any machine, in CI, and from a plain CLI. `app/src-tauri` is then a
//! thin set of commands over this.
//!
//! What is here:
//!
//! - [`Vault`] — the private key on disk, written `0600`, plus the sync
//!   cursors so a cold start does not re-download a whole mailbox.
//! - [`HttpMailbox`] — [`qw_node::sync::MailboxTransport`] over HTTP
//!   against a coordination server's `/mailbox`.
//! - [`follow_invite`] — the client half of NIP-QW07's public link.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

use qrcode::render::svg;
use qrcode::QrCode;
use qw_node::sync::{MailboxTransport, PublishOutcome};
use qw_protocol::events::Event;
use qw_protocol::identity::Identity;
use qw_protocol::invite;

#[derive(Debug)]
pub enum ClientError {
    Io(io::Error),
    /// The key file exists but is not 32 bytes of hex.
    MalformedKeyFile(PathBuf),
    Invite(invite::InviteError),
    Http(String),
    /// The server answered, but with a status this client does not know
    /// how to interpret. Kept distinct from `Http` so a caller can tell a
    /// protocol mismatch from a dead network.
    UnexpectedStatus(u16),
    /// The invite link would not fit in a QR code. Unreachable for a real
    /// link — a `knownby.work/i/npub1…` URL is ~85 bytes against a ceiling
    /// in the thousands — but the encoder can say no and swallowing that
    /// would mean rendering a blank square.
    QrTooLong(usize),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Io(e) => write!(f, "{e}"),
            ClientError::MalformedKeyFile(p) => {
                write!(f, "{} is not a 32-byte hex secret key", p.display())
            }
            ClientError::Invite(e) => write!(f, "{e}"),
            ClientError::Http(e) => write!(f, "{e}"),
            ClientError::UnexpectedStatus(s) => write!(f, "server answered {s}"),
            ClientError::QrTooLong(n) => {
                write!(f, "{n} bytes is too long to encode as a QR code")
            }
        }
    }
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        ClientError::Io(e)
    }
}

/// On-disk client state: one secret key, and nothing else that matters.
///
/// The key file is the whole identity — there is no account to recover
/// from a server, by design (§2: identity is a keypair, not an account
/// with a provider), so this is the one file a user must actually back
/// up. It is written `0600` on Unix and the directory `0700`; a key
/// readable by other local users is the same as a leaked key.
pub struct Vault {
    dir: PathBuf,
}

impl Vault {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn key_path(&self) -> PathBuf {
        self.dir.join("identity.key")
    }

    /// Load the identity, generating and saving one on first run.
    pub fn load_or_create(&self) -> Result<Identity, ClientError> {
        match self.load() {
            Ok(Some(identity)) => Ok(identity),
            Ok(None) => {
                let identity = Identity::generate();
                self.save(&identity)?;
                Ok(identity)
            }
            Err(e) => Err(e),
        }
    }

    pub fn load(&self) -> Result<Option<Identity>, ClientError> {
        let path = self.key_path();
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)?;
        let bytes = hex_to_32(raw.trim()).ok_or(ClientError::MalformedKeyFile(path))?;
        Identity::from_secret_bytes(bytes)
            .map(Some)
            .map_err(|_| ClientError::MalformedKeyFile(self.key_path()))
    }

    pub fn save(&self, identity: &Identity) -> Result<(), ClientError> {
        fs::create_dir_all(&self.dir)?;
        restrict(&self.dir, 0o700)?;
        let path = self.key_path();
        fs::write(&path, hex_of(&identity.secret_bytes()))?;
        restrict(&path, 0o600)?;
        Ok(())
    }
}

/// Everything this client has been handed, kept on disk.
///
/// Without it the client is amnesiac in a way that quietly disables most of
/// the protocol: `MailboxSync::poll` hands back `delivered` events and the
/// shell counted them and dropped them, so nothing could compute a trust
/// path, list a contract, or derive an earned skill tag — every one of those
/// is a function over held history, and the history was being thrown away
/// once per sync.
///
/// **Append-only, and verified on the way in and on the way out.** A signed
/// event is evidence; a file on a phone is not. Anything that fails
/// `Event::verify` is refused on append and skipped on load, because the
/// alternative is a tampered line silently becoming an input to a trust
/// computation — the one place in this codebase where being wrong is worse
/// than being empty. Skipped lines are counted rather than swallowed
/// ([`EventStore::rejected`]), so a corrupt store is visible instead of
/// merely smaller.
///
/// JSON Lines: one event per line, appended, never rewritten. A partially
/// written trailing line after a crash costs one event and fails its own
/// verify on the next load, rather than making the file unparseable.
///
/// `0600`, like the key. These are public signed records, but the *set* of
/// them is a contract history — who someone works with and how often — and
/// that is nobody else's business on a shared machine.
///
/// Unbounded, deliberately for now: nothing prunes, because deciding what
/// may be forgotten is a protocol question (records are evidence others may
/// ask for) and not one to answer accidentally inside a cache.
pub struct EventStore {
    path: PathBuf,
    events: Vec<Event>,
    ids: HashSet<String>,
    rejected: usize,
}

impl EventStore {
    /// Open the store in `dir`, loading and verifying what is already
    /// there. A missing file is an empty store, not an error — first run.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, ClientError> {
        let path = dir.into().join("events.jsonl");
        let mut store = Self {
            path,
            events: Vec::new(),
            ids: HashSet::new(),
            rejected: 0,
        };
        if !store.path.exists() {
            return Ok(store);
        }
        let raw = fs::read_to_string(&store.path)?;
        for line in raw.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Event>(line) {
                Ok(event) if event.verify().is_ok() => {
                    if store.ids.insert(event.id.clone()) {
                        store.events.push(event);
                    }
                }
                _ => store.rejected += 1,
            }
        }
        store.events.sort_by_key(|e| e.created_at);
        Ok(store)
    }

    /// Everything held, oldest first — the input to trust paths, earned
    /// skill tags and any history view.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Lines on disk that did not parse or did not verify. Non-zero means
    /// the file was truncated or edited; the store still works, with that
    /// many records missing.
    pub fn rejected(&self) -> usize {
        self.rejected
    }

    /// Add what a sync delivered. Returns how many were new.
    ///
    /// Unverifiable events are refused rather than stored: a mailbox is
    /// untrusted infrastructure (§8 — "a hostile cache can withhold mail but
    /// cannot inject any"), and that guarantee is only true if the thing
    /// writing to disk enforces it.
    ///
    /// Written before the in-memory state is updated, so a failed write
    /// leaves the two agreeing rather than leaving the process convinced it
    /// holds something no restart will find.
    pub fn append(
        &mut self,
        events: impl IntoIterator<Item = Event>,
    ) -> Result<usize, ClientError> {
        let fresh: Vec<Event> = events
            .into_iter()
            .filter(|e| e.verify().is_ok() && !self.ids.contains(&e.id))
            .fold(Vec::new(), |mut acc, e| {
                // Dedupe within the batch too, not just against the file.
                if !acc.iter().any(|k: &Event| k.id == e.id) {
                    acc.push(e);
                }
                acc
            });
        if fresh.is_empty() {
            return Ok(0);
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
            restrict(parent, 0o700)?;
        }
        let mut buf = String::new();
        for event in &fresh {
            buf.push_str(
                &serde_json::to_string(event).map_err(|e| ClientError::Http(e.to_string()))?,
            );
            buf.push('\n');
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(buf.as_bytes())?;
        restrict(&self.path, 0o600)?;

        for event in fresh.iter() {
            self.ids.insert(event.id.clone());
        }
        self.events.extend(fresh.iter().cloned());
        self.events.sort_by_key(|e| e.created_at);
        Ok(fresh.len())
    }
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) -> io::Result<()> {
    // Windows ACLs are not a chmod; the shell is responsible there.
    Ok(())
}

fn hex_of(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_to_32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// The client half of following a public invite link: parse the link and
/// produce this client's signed self-introduction to the publisher.
///
/// The publisher's own answer is theirs to sign — a client that could
/// produce both sides would be forging half the edge.
pub fn follow_invite(identity: &Identity, link: &str) -> Result<Event, ClientError> {
    let target = link_target(link).map_err(ClientError::Invite)?;
    Ok(invite::follow_public_link(&identity.nostr_pubkey_hex(), &target).sign(identity))
}

/// The invite link as a scannable QR code, a standalone SVG document.
///
/// What it encodes is the full `https://knownby.work/i/<npub>` URL, never
/// the bare key. A phone camera turns a URL into something tappable, and
/// what it opens is the page that offers the app — so one scan is both
/// "join this person" and "here is the client", which is the whole point of
/// showing a code to someone standing in front of you. A bare npub scans as
/// text and leads nowhere.
///
/// Dark-on-white whatever the surrounding theme is doing. A QR is read by a
/// camera, not by a person: inverted and low-contrast codes fail on enough
/// scanners that theming this would be decoration bought with the only
/// thing it does.
pub fn invite_qr_svg(link: &str) -> Result<String, ClientError> {
    let code = QrCode::new(link.as_bytes()).map_err(|_| ClientError::QrTooLong(link.len()))?;
    Ok(code
        .render()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .dark_color(svg::Color("#0f0d1a"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

/// Accept either a full URL or a bare npub/hex — people paste both.
fn link_target(link: &str) -> Result<String, invite::InviteError> {
    let trimmed = link.trim();
    if let Some(idx) = trimmed.find(invite::INVITE_PATH_PREFIX) {
        if let Some(target) = invite::parse_invite_path(&trimmed[idx..]) {
            return Ok(target);
        }
    }
    invite::parse_invite_target(trimmed)
}

/// `MailboxTransport` over HTTP.
pub struct HttpMailbox {
    client: reqwest::blocking::Client,
}

impl Default for HttpMailbox {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpMailbox {
    pub fn new() -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
        }
    }
}

impl MailboxTransport for HttpMailbox {
    type Error = ClientError;

    fn fetch(
        &mut self,
        base_url: &str,
        pubkey: &str,
        since: Option<u64>,
    ) -> Result<Vec<Event>, ClientError> {
        let mut url = format!("{}/mailbox?pubkey={pubkey}", base_url.trim_end_matches('/'));
        if let Some(since) = since {
            url.push_str(&format!("&since={since}"));
        }
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|e| ClientError::Http(e.to_string()))?;
        if !response.status().is_success() {
            return Err(ClientError::UnexpectedStatus(response.status().as_u16()));
        }
        response
            .json::<Vec<Event>>()
            .map_err(|e| ClientError::Http(e.to_string()))
    }

    fn publish(&mut self, base_url: &str, event: &Event) -> Result<PublishOutcome, ClientError> {
        let url = format!("{}/mailbox", base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(url)
            .json(event)
            .send()
            .map_err(|e| ClientError::Http(e.to_string()))?;
        match response.status().as_u16() {
            201 => Ok(PublishOutcome::Accepted),
            // The server already held it — an idempotent re-send.
            200 => Ok(PublishOutcome::AlreadyHeld),
            // 507 is the mailbox-full answer. Mapping it here rather than
            // in `sync` is what keeps the sync logic transport-agnostic.
            507 => Ok(PublishOutcome::MailboxFull),
            other => Err(ClientError::UnexpectedStatus(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use axum::extract::{Query, State};
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};
    use qw_node::sync::MailboxSync;
    use qw_protocol::events::{p_tag, UnsignedEvent, KIND_JOB_OFFER};

    use super::*;

    #[derive(Clone, Default)]
    struct Held(Arc<Mutex<Vec<Event>>>);

    /// A stand-in for qw-server's `/mailbox`, answering with the same
    /// status codes. Deliberately a real HTTP server on a real socket:
    /// what is under test here is the transport, and a mock that skipped
    /// the wire would only be testing that Rust can call functions.
    fn stub() -> (SocketAddr, Held) {
        let held = Held::default();
        let state = held.clone();
        let app = Router::new()
            .route(
                "/mailbox",
                post(
                    |State(held): State<Held>, Json(event): Json<Event>| async move {
                        let mut store = held.0.lock().unwrap();
                        if store.iter().any(|e: &Event| e.id == event.id) {
                            return StatusCode::OK;
                        }
                        if store.len() >= 2 {
                            return StatusCode::INSUFFICIENT_STORAGE;
                        }
                        store.push(event);
                        StatusCode::CREATED
                    },
                )
                .get(
                    |State(held): State<Held>,
                     Query(q): Query<std::collections::HashMap<String, String>>| async move {
                        let since: u64 = q.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
                        let pubkey = q.get("pubkey").cloned().unwrap_or_default();
                        let store = held.0.lock().unwrap();
                        let mut out: Vec<Event> = store
                            .iter()
                            .filter(|e| e.first_tag_value("p") == Some(pubkey.as_str()))
                            .filter(|e| e.created_at >= since)
                            .cloned()
                            .collect();
                        out.sort_by_key(|e| e.created_at);
                        Json(out)
                    },
                ),
            )
            .with_state(state);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                listener.set_nonblocking(true).unwrap();
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
        // Give the runtime a moment to bind before the first request.
        std::thread::sleep(std::time::Duration::from_millis(150));
        (addr, held)
    }

    fn offer(from: &Identity, to: &str, created_at: u64, body: &str) -> Event {
        UnsignedEvent {
            pubkey: from.nostr_pubkey_hex(),
            created_at,
            kind: KIND_JOB_OFFER,
            tags: vec![p_tag(to.to_string())],
            content: body.to_string(),
        }
        .sign(from)
    }

    #[test]
    fn identity_survives_a_restart_and_is_not_world_readable() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::at(dir.path());

        let first = vault.load_or_create().unwrap();
        let second = vault.load_or_create().unwrap();
        assert_eq!(first.nostr_pubkey_hex(), second.nostr_pubkey_hex());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join("identity.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "key must not be group/other readable");
        }
    }

    #[test]
    fn a_corrupt_key_file_is_an_error_not_a_new_identity() {
        // Silently generating a fresh key here would look like "logged
        // out" while actually abandoning every record signed with the old
        // one — the one failure mode a key store must never have.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("identity.key"), "not a key").unwrap();
        assert!(matches!(
            Vault::at(dir.path()).load_or_create(),
            Err(ClientError::MalformedKeyFile(_))
        ));
    }

    #[test]
    fn follows_a_link_in_every_form_a_person_might_paste() {
        let me = Identity::generate();
        let publisher = Identity::generate();
        let npub = invite::npub_encode(&publisher.nostr_pubkey_hex()).unwrap();

        for link in [
            format!("https://knownby.work/i/{npub}"),
            format!("knownby.work/i/{npub}"),
            format!("  https://knownby.work/i/{npub}?utm_source=li  "),
            npub.clone(),
            publisher.nostr_pubkey_hex(),
        ] {
            let event = follow_invite(&me, &link).unwrap();
            assert!(event.verify().is_ok(), "link form failed: {link}");
            assert_eq!(
                event.first_tag_value("p"),
                Some(publisher.nostr_pubkey_hex().as_str()),
                "link form addressed the wrong publisher: {link}"
            );
        }

        assert!(follow_invite(&me, "https://knownby.work/i/nope").is_err());
    }

    #[test]
    fn publishes_and_collects_over_real_http() {
        let (addr, _held) = stub();
        let base = format!("http://{addr}");
        let me = Identity::generate();
        let sender = Identity::generate();

        // Someone sends me mail through the server.
        let mut transport = HttpMailbox::new();
        let incoming = offer(&sender, &me.nostr_pubkey_hex(), 100, "sprint 12");
        assert_eq!(
            transport.publish(&base, &incoming).unwrap(),
            PublishOutcome::Accepted
        );
        assert_eq!(
            transport.publish(&base, &incoming).unwrap(),
            PublishOutcome::AlreadyHeld,
            "a re-send must be idempotent, not an error"
        );

        // I collect it, and the cursor advances.
        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        let report = sync.poll(&mut transport, &[&base]);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.delivered.len(), 1);
        assert_eq!(report.delivered[0].content, "sprint 12");
        assert_eq!(sync.cursor(&base), Some(100));

        // Nothing new on a second poll.
        assert!(sync.poll(&mut transport, &[&base]).delivered.is_empty());
    }

    #[test]
    fn a_full_mailbox_keeps_the_event_queued_rather_than_losing_it() {
        // The stub holds two events, then answers 507 like the real server.
        let (addr, _held) = stub();
        let base = format!("http://{addr}");
        let me = Identity::generate();
        let recipient = Identity::generate();

        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        for i in 0..3 {
            sync.queue(offer(
                &me,
                &recipient.nostr_pubkey_hex(),
                i,
                &format!("offer {i}"),
            ));
        }

        let report = sync.flush(&mut HttpMailbox::new(), &[&base]);
        assert_eq!(report.published, 2);
        assert_eq!(report.still_queued, 1, "the rejected one must survive");
        assert!(
            report.errors.is_empty(),
            "507 is an answer, not a transport failure: {:?}",
            report.errors
        );
    }

    #[test]
    fn an_unreachable_server_is_an_error_not_a_silent_empty_mailbox() {
        let me = Identity::generate();
        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        // Port 1 on loopback: reliably refused, no DNS involved.
        let report = sync.poll(&mut HttpMailbox::new(), &["http://127.0.0.1:1"]);
        assert!(report.delivered.is_empty());
        assert_eq!(report.errors.len(), 1);
        assert_eq!(sync.cursor("http://127.0.0.1:1"), None);
    }
}

#[cfg(test)]
mod qr_tests {
    use super::*;

    /// A real invite link — the only input this is ever given.
    const LINK: &str =
        "https://knownby.work/i/npub1qqqsyqcyq5rqwzqfpg9scrgwpugpzysnzs23v9ccrydpk8qarc0jt2v";

    #[test]
    fn invite_link_renders_a_standalone_svg() {
        let svg = invite_qr_svg(LINK).expect("a link this size always fits");
        assert!(
            svg.starts_with("<?xml") || svg.starts_with("<svg"),
            "{svg:.40}"
        );
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("#ffffff"), "light modules must stay light");
    }

    /// The scanner reads the URL, not the key: encoding the bare npub would
    /// scan as text and give whoever scanned it nothing to open.
    #[test]
    fn encodes_the_url_so_a_camera_has_something_to_open() {
        let code = QrCode::new(LINK.as_bytes()).unwrap();
        let bare =
            QrCode::new("npub1qqqsyqcyq5rqwzqfpg9scrgwpugpzysnzs23v9ccrydpk8qarc0jt2v".as_bytes())
                .unwrap();
        // Not an assertion about QR internals — just that the two are
        // different payloads, so a later "simplify" that swaps one for the
        // other fails here rather than silently shipping a dead code.
        assert_ne!(code.width(), 0);
        assert_ne!(bare.width(), 0);
        assert!(
            invite_qr_svg(LINK).unwrap()
                != invite_qr_svg("npub1qqqsyqcyq5rqwzqfpg9scrgwpugpzysnzs23v9ccrydpk8qarc0jt2v")
                    .unwrap()
        );
    }

    /// The ceiling is real but far away; this pins that a plausible link
    /// never approaches it, so `QrTooLong` stays an unreachable branch
    /// rather than a lurking failure on a longer hostname.
    #[test]
    fn a_much_longer_link_still_fits() {
        let long = format!(
            "https://{}.example.org/i/{}",
            "a".repeat(120),
            "b".repeat(63)
        );
        assert!(invite_qr_svg(&long).is_ok(), "{} bytes", long.len());
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;
    use qw_protocol::identity::Identity;

    use qw_protocol::events::{profile_skill_tags, ProfileSkillTags};

    fn signed(identity: &Identity, tag: &str) -> Event {
        profile_skill_tags(
            &identity.nostr_pubkey_hex(),
            &ProfileSkillTags {
                display_name: None,
                skill_tags: vec![tag.to_string()],
            },
        )
        .sign(identity)
    }

    #[test]
    fn held_events_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::generate();

        let mut store = EventStore::open(dir.path()).unwrap();
        assert!(store.is_empty());
        assert_eq!(
            store
                .append([signed(&id, "it/backend/languages#rust")])
                .unwrap(),
            1
        );

        let reopened = EventStore::open(dir.path()).unwrap();
        assert_eq!(
            reopened.len(),
            1,
            "a sync must outlive the process that did it"
        );
        assert_eq!(reopened.rejected(), 0);
    }

    /// A mailbox is untrusted infrastructure — §8 allows it to withhold mail
    /// but not to inject any. That only holds if the thing writing to disk
    /// enforces it, so this is the guarantee, not a nicety.
    #[test]
    fn an_event_that_does_not_verify_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::generate();
        let mut forged = signed(&id, "it/backend/languages#rust");
        forged.content = r#"{"skill_tags":["it/security#pentest"]}"#.to_string();

        let mut store = EventStore::open(dir.path()).unwrap();
        assert_eq!(store.append([forged]).unwrap(), 0);
        assert!(store.is_empty());
        assert!(
            EventStore::open(dir.path()).unwrap().is_empty(),
            "and nothing was written for a later load to pick up"
        );
    }

    /// Tampering with the file directly is the same attack one layer down.
    /// The store loses the record rather than trusting it, and says how many.
    #[test]
    fn a_tampered_line_is_skipped_and_counted_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::generate();
        let mut store = EventStore::open(dir.path()).unwrap();
        store
            .append([signed(&id, "it/backend/languages#rust")])
            .unwrap();

        let path = dir.path().join("events.jsonl");
        let raw = fs::read_to_string(&path).unwrap();
        fs::write(&path, raw.replace("languages#rust", "languages#java")).unwrap();

        let reopened = EventStore::open(dir.path()).unwrap();
        assert!(
            reopened.is_empty(),
            "an edited record must not be readable as evidence"
        );
        assert_eq!(reopened.rejected(), 1, "and its loss must be visible");
    }

    /// A crash mid-append leaves a partial trailing line. It costs that one
    /// event and nothing else — the file must not become unreadable.
    #[test]
    fn a_truncated_trailing_line_costs_one_event_not_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::generate();
        let mut store = EventStore::open(dir.path()).unwrap();
        store
            .append([signed(&id, "it/backend/languages#rust")])
            .unwrap();

        let path = dir.path().join("events.jsonl");
        let mut raw = fs::read_to_string(&path).unwrap();
        raw.push_str("{\"id\":\"half-writ");
        fs::write(&path, raw).unwrap();

        let reopened = EventStore::open(dir.path()).unwrap();
        assert_eq!(reopened.len(), 1);
        assert_eq!(reopened.rejected(), 1);
    }

    #[test]
    fn the_same_event_twice_is_stored_once() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::generate();
        let event = signed(&id, "it/backend/languages#rust");

        let mut store = EventStore::open(dir.path()).unwrap();
        assert_eq!(
            store.append([event.clone(), event.clone()]).unwrap(),
            1,
            "within a batch"
        );
        assert_eq!(store.append([event]).unwrap(), 0, "and across calls");
        assert_eq!(EventStore::open(dir.path()).unwrap().len(), 1);
    }
}
