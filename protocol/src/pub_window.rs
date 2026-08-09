//! Publication window enforcement (§2): before making an offer, the client
//! checks "has the counterparty signed *anything* within the last T?" and
//! shows a plain pass/fail signal — this is what replaces manual liveness
//! auditing (FAQ: "Client checks at offer time and shows a plain signal —
//! manual verification doesn't happen").

use crate::events::Event;

/// Default window T, per §2.
pub const DEFAULT_WINDOW_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PublicationWindowStatus {
    /// Counterparty signed something at `last_seen` (unix seconds), within
    /// the window as of `now`.
    WithinWindow { last_seen: u64 },
    /// Counterparty's most recent signature is older than the window.
    Stale { last_seen: u64, elapsed_secs: u64 },
    /// No signed record from this pubkey was found at all.
    NoRecords,
}

impl PublicationWindowStatus {
    /// The plain pass/fail signal the client surfaces at offer time.
    pub fn passes(&self) -> bool {
        matches!(self, PublicationWindowStatus::WithinWindow { .. })
    }
}

/// Most recent `created_at` among events authored (i.e. `pubkey`-matching,
/// not just referencing) by `pubkey_hex`.
pub fn last_signature_timestamp(events: &[Event], pubkey_hex: &str) -> Option<u64> {
    events
        .iter()
        .filter(|e| e.pubkey == pubkey_hex)
        .map(|e| e.created_at)
        .max()
}

pub fn check_publication_window(
    events: &[Event],
    counterparty_pubkey_hex: &str,
    now: u64,
    window_secs: u64,
) -> PublicationWindowStatus {
    match last_signature_timestamp(events, counterparty_pubkey_hex) {
        None => PublicationWindowStatus::NoRecords,
        Some(last_seen) => {
            let elapsed = now.saturating_sub(last_seen);
            if elapsed <= window_secs {
                PublicationWindowStatus::WithinWindow { last_seen }
            } else {
                PublicationWindowStatus::Stale {
                    last_seen,
                    elapsed_secs: elapsed,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{profile_skill_tags, ProfileSkillTags, UnsignedEvent, KIND_JOB_OFFER};
    use crate::identity::Identity;

    fn signed_at(identity: &Identity, created_at: u64) -> crate::events::Event {
        let profile = ProfileSkillTags {
            display_name: None,
            skill_tags: vec![],
        };
        let unsigned = profile_skill_tags(&identity.nostr_pubkey_hex(), &profile);
        UnsignedEvent::with_created_at(
            unsigned.pubkey,
            unsigned.kind,
            unsigned.tags,
            unsigned.content,
            created_at,
        )
        .sign(identity)
    }

    #[test]
    fn within_window_passes() {
        let worker = Identity::generate();
        let events = vec![signed_at(&worker, 1_000)];
        let status = check_publication_window(
            &events,
            &worker.nostr_pubkey_hex(),
            1_000 + 3600,
            DEFAULT_WINDOW_SECS,
        );
        assert_eq!(
            status,
            PublicationWindowStatus::WithinWindow { last_seen: 1_000 }
        );
        assert!(status.passes());
    }

    #[test]
    fn older_than_window_fails() {
        let worker = Identity::generate();
        let events = vec![signed_at(&worker, 1_000)];
        let now = 1_000 + DEFAULT_WINDOW_SECS + 1;
        let status = check_publication_window(
            &events,
            &worker.nostr_pubkey_hex(),
            now,
            DEFAULT_WINDOW_SECS,
        );
        assert_eq!(
            status,
            PublicationWindowStatus::Stale {
                last_seen: 1_000,
                elapsed_secs: DEFAULT_WINDOW_SECS + 1
            }
        );
        assert!(!status.passes());
    }

    #[test]
    fn takes_the_most_recent_signature_not_the_first() {
        let worker = Identity::generate();
        let events = vec![
            signed_at(&worker, 1_000),
            signed_at(&worker, 5_000),
            signed_at(&worker, 2_000),
        ];
        let status = check_publication_window(
            &events,
            &worker.nostr_pubkey_hex(),
            5_100,
            DEFAULT_WINDOW_SECS,
        );
        assert_eq!(
            status,
            PublicationWindowStatus::WithinWindow { last_seen: 5_000 }
        );
    }

    #[test]
    fn no_records_at_all_fails_open_to_unknown() {
        let stranger = Identity::generate();
        let unrelated = Identity::generate();
        let events = vec![signed_at(&unrelated, 1_000)];
        let status = check_publication_window(
            &events,
            &stranger.nostr_pubkey_hex(),
            1_050,
            DEFAULT_WINDOW_SECS,
        );
        assert_eq!(status, PublicationWindowStatus::NoRecords);
        assert!(!status.passes());
    }

    #[test]
    fn events_merely_referencing_the_pubkey_dont_count_as_their_signature() {
        // A KIND_JOB_OFFER tagging `worker` as counterparty is signed by
        // the client, not the worker — it must not count as the worker
        // having published anything.
        let client = Identity::generate();
        let worker = Identity::generate();
        let unsigned = UnsignedEvent::with_created_at(
            client.nostr_pubkey_hex(),
            KIND_JOB_OFFER,
            vec![crate::events::p_tag(worker.nostr_pubkey_hex())],
            "{}",
            9_999,
        );
        let events = vec![unsigned.sign(&client)];
        assert_eq!(
            last_signature_timestamp(&events, &worker.nostr_pubkey_hex()),
            None
        );
    }
}
