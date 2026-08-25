//! Client-side mailbox sync (§7 + §8): collecting what arrived while the
//! client was offline, and getting what it signed while offline out to
//! someone else.
//!
//! Transport-agnostic on purpose. This crate has no HTTP client and no
//! async runtime — a Tauri app supplies `reqwest`, a test supplies a
//! `HashMap` — the same separation `network::Network` already draws
//! between routing logic and delivery. What lives here is the part that
//! is easy to get subtly wrong and hard to notice: the cursor, the
//! deduplication, and what a client is willing to believe from a server.
//!
//! Three rules that fall directly out of §8's "the server is a
//! convenience, never a source of truth":
//!
//! - **Everything delivered is verified locally.** The cache signs
//!   nothing and is not trusted for anything; an event that fails
//!   `verify()`, or that is addressed to somebody else, is dropped rather
//!   than folded in. A hostile cache can withhold mail — no client can
//!   prevent that — but it cannot inject any.
//! - **The cursor only moves on success.** A fetch that fails leaves the
//!   cursor where it was, so the next poll re-asks for the same window.
//!   Advancing optimistically would silently lose whatever was in flight.
//! - **`since` is inclusive, and duplicates are dropped by id.** The
//!   tempting `since = cursor + 1` loses every event sharing that second
//!   with the newest one already seen. Overlapping by one second and
//!   deduplicating is the cheap, correct direction to be wrong in.

use std::collections::{HashMap, HashSet};

use qw_protocol::events::Event;

/// What a server said about an event this client tried to publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// Newly held for the recipient.
    Accepted,
    /// The server already had it — a retry after an answer this client
    /// never saw. Success, not an error.
    AlreadyHeld,
    /// The recipient's mailbox is full. Deliberately not an error: the
    /// event stays queued and another server may take it, whereas
    /// retrying this one immediately would just fail again.
    MailboxFull,
}

/// The I/O half, supplied by whatever is actually running: HTTP in an
/// app, a fake in tests.
pub trait MailboxTransport {
    type Error: std::fmt::Display;

    /// `GET /mailbox?pubkey=…&since=…` — events addressed to `pubkey`,
    /// oldest first.
    fn fetch(
        &mut self,
        base_url: &str,
        pubkey: &str,
        since: Option<u64>,
    ) -> Result<Vec<Event>, Self::Error>;

    /// `POST /mailbox` — hand one signed event to a server.
    fn publish(&mut self, base_url: &str, event: &Event) -> Result<PublishOutcome, Self::Error>;
}

#[derive(Debug, Default, PartialEq)]
pub struct PollReport {
    /// New, verified events addressed to this client, oldest first.
    pub delivered: Vec<Event>,
    /// Events a server returned that failed verification or were
    /// addressed elsewhere. Non-zero means a server is misbehaving; it is
    /// surfaced rather than logged-and-forgotten so a client can rank that
    /// server down (`crate::server_registry`).
    pub rejected: usize,
    /// Per-server failures, `(base_url, message)`. One unreachable server
    /// never stops the others.
    pub errors: Vec<(String, String)>,
}

#[derive(Debug, Default, PartialEq)]
pub struct FlushReport {
    pub published: usize,
    pub still_queued: usize,
    pub errors: Vec<(String, String)>,
}

pub struct MailboxSync {
    self_pubkey: String,
    /// Newest `created_at` successfully taken from each server. Per
    /// server, because two servers hold different subsets and a shared
    /// cursor would skip whatever the slower one had not yet received.
    cursors: HashMap<String, u64>,
    seen: HashSet<String>,
    outbox: Vec<Event>,
}

impl MailboxSync {
    pub fn new(self_pubkey: impl Into<String>) -> Self {
        Self {
            self_pubkey: self_pubkey.into(),
            cursors: HashMap::new(),
            seen: HashSet::new(),
            outbox: Vec::new(),
        }
    }

    /// Restore a persisted cursor — a client that stored one across
    /// restarts hands it back here so a cold start does not re-download
    /// everything the mailbox still holds.
    pub fn restore_cursor(&mut self, base_url: impl Into<String>, created_at: u64) {
        self.cursors.insert(base_url.into(), created_at);
    }

    pub fn cursor(&self, base_url: &str) -> Option<u64> {
        self.cursors.get(base_url).copied()
    }

    /// Queue a signed event for delivery. Publishing is separate from
    /// signing so an offline client can compose all it likes and hand the
    /// results over whenever a network appears.
    pub fn queue(&mut self, event: Event) {
        if !self.outbox.iter().any(|e| e.id == event.id) {
            self.outbox.push(event);
        }
    }

    pub fn pending(&self) -> &[Event] {
        &self.outbox
    }

    /// Collect new mail from every server, in order.
    pub fn poll<T: MailboxTransport>(&mut self, transport: &mut T, servers: &[&str]) -> PollReport {
        let mut report = PollReport::default();

        for base_url in servers {
            let since = self.cursors.get(*base_url).copied();
            let batch = match transport.fetch(base_url, &self.self_pubkey, since) {
                Ok(batch) => batch,
                Err(e) => {
                    // Cursor untouched: the next poll re-asks for the same
                    // window rather than stepping over it.
                    report.errors.push((base_url.to_string(), e.to_string()));
                    continue;
                }
            };

            let mut newest = since.unwrap_or(0);
            for event in batch {
                if event.verify().is_err()
                    || event.first_tag_value("p") != Some(self.self_pubkey.as_str())
                {
                    report.rejected += 1;
                    continue;
                }
                newest = newest.max(event.created_at);
                // Dedupe across servers *and* across the one-second
                // overlap the inclusive cursor deliberately re-fetches.
                if self.seen.insert(event.id.clone()) {
                    report.delivered.push(event);
                }
            }
            self.cursors.insert(base_url.to_string(), newest);
        }

        report.delivered.sort_by_key(|e| e.created_at);
        report
    }

    /// Try to hand every queued event to a server. An event leaves the
    /// outbox as soon as any one server takes it — this cache is never
    /// the only copy, so one accepting server is delivery; the rest is
    /// redundancy the sender does not have to pay for.
    pub fn flush<T: MailboxTransport>(
        &mut self,
        transport: &mut T,
        servers: &[&str],
    ) -> FlushReport {
        let mut report = FlushReport::default();
        let queued = std::mem::take(&mut self.outbox);

        for event in queued {
            let mut delivered = false;
            for base_url in servers {
                match transport.publish(base_url, &event) {
                    Ok(PublishOutcome::Accepted) | Ok(PublishOutcome::AlreadyHeld) => {
                        delivered = true;
                        break;
                    }
                    // Full: this server will keep saying no, so move on to
                    // the next one without recording an error.
                    Ok(PublishOutcome::MailboxFull) => continue,
                    Err(e) => report.errors.push((base_url.to_string(), e.to_string())),
                }
            }
            if delivered {
                report.published += 1;
            } else {
                self.outbox.push(event);
            }
        }

        report.still_queued = self.outbox.len();
        report
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use qw_protocol::events::{p_tag, UnsignedEvent, KIND_JOB_OFFER};
    use qw_protocol::identity::Identity;

    use super::*;

    /// In-memory stand-in for the HTTP client an app would supply.
    #[derive(Default)]
    struct FakeServers {
        held: HashMap<String, Vec<Event>>,
        /// base_url -> error to return instead of answering.
        broken: HashMap<String, String>,
        full: HashMap<String, bool>,
        /// A server in here returns everything it holds regardless of who
        /// it is addressed to — what a misbehaving or hostile cache does.
        /// Without this the fake would filter the bad cases out before the
        /// code under test ever saw them, and the rejection path would be
        /// tested only in the comments.
        hostile: HashMap<String, bool>,
        fetches: Vec<(String, Option<u64>)>,
    }

    impl FakeServers {
        fn hold(&mut self, base_url: &str, event: Event) {
            self.held
                .entry(base_url.to_string())
                .or_default()
                .push(event);
        }
    }

    impl MailboxTransport for FakeServers {
        type Error = String;

        fn fetch(
            &mut self,
            base_url: &str,
            pubkey: &str,
            since: Option<u64>,
        ) -> Result<Vec<Event>, String> {
            self.fetches.push((base_url.to_string(), since));
            if let Some(err) = self.broken.get(base_url) {
                return Err(err.clone());
            }
            let hostile = self.hostile.get(base_url).copied().unwrap_or(false);
            let mut out: Vec<Event> = self
                .held
                .get(base_url)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|e| hostile || e.first_tag_value("p") == Some(pubkey))
                .filter(|e| since.is_none_or(|s| e.created_at >= s))
                .collect();
            out.sort_by_key(|e| e.created_at);
            Ok(out)
        }

        fn publish(&mut self, base_url: &str, event: &Event) -> Result<PublishOutcome, String> {
            if let Some(err) = self.broken.get(base_url) {
                return Err(err.clone());
            }
            if self.full.get(base_url).copied().unwrap_or(false) {
                return Ok(PublishOutcome::MailboxFull);
            }
            let held = self.held.entry(base_url.to_string()).or_default();
            if held.iter().any(|e| e.id == event.id) {
                return Ok(PublishOutcome::AlreadyHeld);
            }
            held.push(event.clone());
            Ok(PublishOutcome::Accepted)
        }
    }

    fn addressed(from: &Identity, to: &str, created_at: u64, body: &str) -> Event {
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
    fn collects_mail_and_remembers_where_it_got_to() {
        let me = Identity::generate();
        let sender = Identity::generate();
        let mut servers = FakeServers::default();
        servers.hold(
            "s1",
            addressed(&sender, &me.nostr_pubkey_hex(), 100, "first"),
        );
        servers.hold(
            "s1",
            addressed(&sender, &me.nostr_pubkey_hex(), 200, "second"),
        );

        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        let report = sync.poll(&mut servers, &["s1"]);

        assert_eq!(report.delivered.len(), 2);
        assert_eq!(report.delivered[0].content, "first");
        assert_eq!(sync.cursor("s1"), Some(200));

        // Second poll: the inclusive cursor re-fetches the boundary event,
        // and dedupe means it is not delivered twice.
        let again = sync.poll(&mut servers, &["s1"]);
        assert!(again.delivered.is_empty());
        assert_eq!(servers.fetches.last().unwrap().1, Some(200));
    }

    #[test]
    fn an_event_sharing_the_cursor_second_is_not_lost() {
        // The off-by-one this design exists to avoid: two events with the
        // same created_at, the second published after the first was
        // already collected.
        let me = Identity::generate();
        let sender = Identity::generate();
        let mut servers = FakeServers::default();
        servers.hold("s1", addressed(&sender, &me.nostr_pubkey_hex(), 500, "a"));

        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        assert_eq!(sync.poll(&mut servers, &["s1"]).delivered.len(), 1);

        servers.hold("s1", addressed(&sender, &me.nostr_pubkey_hex(), 500, "b"));
        let second = sync.poll(&mut servers, &["s1"]);
        assert_eq!(second.delivered.len(), 1, "same-second sibling must arrive");
        assert_eq!(second.delivered[0].content, "b");
    }

    #[test]
    fn refuses_what_a_hostile_server_sends() {
        let me = Identity::generate();
        let sender = Identity::generate();
        let stranger = Identity::generate();
        let mut servers = FakeServers::default();
        servers.hostile.insert("s1".to_string(), true);

        // Tampered after signing — the cache editing mail in flight.
        let mut forged = addressed(&sender, &me.nostr_pubkey_hex(), 10, "original");
        forged.content = "tampered by the cache".to_string();
        servers.hold("s1", forged);
        // Somebody else's mail, handed to us anyway.
        servers.hold(
            "s1",
            addressed(&sender, &stranger.nostr_pubkey_hex(), 20, "not yours"),
        );
        // One good event, so the test also proves the bad ones are dropped
        // individually rather than the whole batch being discarded.
        servers.hold("s1", addressed(&sender, &me.nostr_pubkey_hex(), 30, "real"));

        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        let report = sync.poll(&mut servers, &["s1"]);

        assert_eq!(report.delivered.len(), 1, "{:?}", report.delivered);
        assert_eq!(report.delivered[0].content, "real");
        assert_eq!(report.rejected, 2, "forged and misrouted both counted");
    }

    #[test]
    fn a_broken_server_does_not_move_its_cursor_or_stop_the_others() {
        let me = Identity::generate();
        let sender = Identity::generate();
        let mut servers = FakeServers::default();
        servers
            .broken
            .insert("s1".to_string(), "connection refused".to_string());
        servers.hold(
            "s2",
            addressed(&sender, &me.nostr_pubkey_hex(), 300, "via s2"),
        );

        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        let report = sync.poll(&mut servers, &["s1", "s2"]);

        assert_eq!(report.delivered.len(), 1);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(sync.cursor("s1"), None, "a failed fetch must not advance");
        assert_eq!(sync.cursor("s2"), Some(300));
    }

    #[test]
    fn the_same_event_from_two_servers_arrives_once() {
        let me = Identity::generate();
        let sender = Identity::generate();
        let event = addressed(&sender, &me.nostr_pubkey_hex(), 42, "duplicated");
        let mut servers = FakeServers::default();
        servers.hold("s1", event.clone());
        servers.hold("s2", event);

        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        assert_eq!(sync.poll(&mut servers, &["s1", "s2"]).delivered.len(), 1);
    }

    #[test]
    fn queued_mail_survives_an_outage_and_goes_out_later() {
        let me = Identity::generate();
        let recipient = Identity::generate();
        let mut servers = FakeServers::default();
        servers
            .broken
            .insert("s1".to_string(), "network down".to_string());

        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        sync.queue(addressed(&me, &recipient.nostr_pubkey_hex(), 1, "offer"));

        let first = sync.flush(&mut servers, &["s1"]);
        assert_eq!(first.published, 0);
        assert_eq!(first.still_queued, 1);
        assert_eq!(first.errors.len(), 1);

        servers.broken.clear();
        let second = sync.flush(&mut servers, &["s1"]);
        assert_eq!(second.published, 1);
        assert_eq!(second.still_queued, 0);
        assert!(sync.pending().is_empty());
    }

    #[test]
    fn a_full_mailbox_falls_through_to_the_next_server() {
        let me = Identity::generate();
        let recipient = Identity::generate();
        let mut servers = FakeServers::default();
        servers.full.insert("s1".to_string(), true);

        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        sync.queue(addressed(&me, &recipient.nostr_pubkey_hex(), 1, "offer"));
        let report = sync.flush(&mut servers, &["s1", "s2"]);

        assert_eq!(report.published, 1);
        assert!(
            report.errors.is_empty(),
            "a full mailbox is an answer, not a failure: {:?}",
            report.errors
        );
        assert_eq!(servers.held.get("s2").map(Vec::len), Some(1));
    }

    #[test]
    fn re_flushing_an_already_held_event_is_not_a_failure() {
        let me = Identity::generate();
        let recipient = Identity::generate();
        let event = addressed(&me, &recipient.nostr_pubkey_hex(), 1, "retry");
        let mut servers = FakeServers::default();
        servers.hold("s1", event.clone());

        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        sync.queue(event);
        let report = sync.flush(&mut servers, &["s1"]);
        assert_eq!(report.published, 1);
        assert_eq!(report.still_queued, 0);
    }

    #[test]
    fn queueing_the_same_event_twice_sends_it_once() {
        let me = Identity::generate();
        let recipient = Identity::generate();
        let event = addressed(&me, &recipient.nostr_pubkey_hex(), 1, "once");

        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        sync.queue(event.clone());
        sync.queue(event);
        assert_eq!(sync.pending().len(), 1);
    }

    #[test]
    fn a_restored_cursor_skips_what_was_already_collected() {
        let me = Identity::generate();
        let sender = Identity::generate();
        let mut servers = FakeServers::default();
        servers.hold("s1", addressed(&sender, &me.nostr_pubkey_hex(), 100, "old"));
        servers.hold("s1", addressed(&sender, &me.nostr_pubkey_hex(), 900, "new"));

        let mut sync = MailboxSync::new(me.nostr_pubkey_hex());
        sync.restore_cursor("s1", 500);
        let report = sync.poll(&mut servers, &["s1"]);

        assert_eq!(report.delivered.len(), 1);
        assert_eq!(report.delivered[0].content, "new");
    }
}
