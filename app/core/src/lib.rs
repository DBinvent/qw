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

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
