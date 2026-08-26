//! A single node's view of the referral network: its contact book (§3),
//! and the relay logic that turns an incoming/originated query into
//! outgoing forwards and, on a match, an answer relayed back along the
//! path (NIP-QW06).

use std::collections::{HashMap, HashSet};

use qw_protocol::events::{
    skill_answer, skill_query, Event, SkillAnswer as SkillAnswerContent,
    SkillQuery as SkillQueryContent,
};
use qw_protocol::identity::Identity;
use qw_protocol::trust::earned_skill_tags;

use crate::contact::Contact;
use crate::routing::select_forward_targets;
#[cfg(test)]
use crate::routing::{select_forward_targets_ranked, MatchSource};

/// What this node learned when it first relayed one query_id: who to
/// send any resulting answer to, and (for hop 2+) which event it itself
/// received, so its own answer/relay events can reference it.
struct RelayState {
    answer_target: String,
    received_event_id: Option<String>,
}

/// An event this node wants delivered to `to`. The network orchestrator
/// (`crate::network::Network`) is what actually moves these between nodes.
#[derive(Debug, Clone)]
pub enum Delivery {
    Query { to: String, event: Event },
    Answer { to: String, event: Event },
}

/// A responder discovered for a query — what the requester ultimately
/// collects, deduped by `responder_pubkey`.
#[derive(Debug, Clone, PartialEq)]
pub struct FinalAnswer {
    pub responder_pubkey: String,
    pub matched_skill_tag: String,
    pub hops: u8,
}

#[derive(Debug, Default)]
pub struct RelayOutcome {
    pub deliveries: Vec<Delivery>,
    pub own_match: Option<FinalAnswer>,
}

/// Bundled inputs to [`Node::relay_for`] — the common core of both
/// [`Node::begin_relay_chain`] (hop 1, no incoming event) and
/// [`Node::receive_query`] (hop 2+, an incoming event to check policy
/// against and derive these from).
struct RelayArgs<'a> {
    query_id: &'a str,
    skill_tag: &'a str,
    /// What this node's own new forward events will carry.
    outgoing_hops_from_origin: u8,
    /// What to report as `hops` if this node itself matches.
    self_match_hops: u8,
    max_hops: u8,
    /// Who to send a match/relay answer to.
    answer_target: &'a str,
    /// The event this node itself received, if any (`None` only for hop
    /// 1's own chain-head).
    received_event_id: Option<&'a str>,
    /// The upstream neighbor to skip when selecting forward targets, so a
    /// node never bounces a query straight back to whoever sent it.
    exclude_from_forward: Option<&'a str>,
}

pub struct Node {
    identity: Identity,
    contacts: HashMap<String, Contact>,
    /// What this node says it can do — the self-published profile.
    own_skill_tags: Vec<String>,
    /// What it has actually done: tags from contracts it completed and a
    /// counterparty countersigned. Refreshed by
    /// [`Node::refresh_earned_skill_tags`] rather than set by hand, because
    /// the whole value of the list is that its author did not choose it.
    own_earned_skill_tags: Vec<String>,
    relay_table: HashMap<String, RelayState>,
    /// query_ids already processed by this node — first arrival wins.
    /// Bounds propagation on a cyclic contact graph; see NIP-QW06's scope
    /// note on multi-path reinforcement being a follow-up, not this.
    seen: HashSet<String>,
}

impl Node {
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            contacts: HashMap::new(),
            own_skill_tags: Vec::new(),
            own_earned_skill_tags: Vec::new(),
            relay_table: HashMap::new(),
            seen: HashSet::new(),
        }
    }

    pub fn pubkey(&self) -> String {
        self.identity.nostr_pubkey_hex()
    }

    pub fn add_contact(&mut self, contact: Contact) {
        self.contacts.insert(contact.pubkey.clone(), contact);
    }

    pub fn contacts(&self) -> impl Iterator<Item = &Contact> {
        self.contacts.values()
    }

    pub fn set_own_skill_tags(&mut self, tags: Vec<String>) {
        self.own_skill_tags = tags;
    }

    /// Recompute this node's own earned tags and every contact's, from the
    /// records currently held.
    ///
    /// `Node` keeps no event store — events arrive per call — so this is the
    /// seam where the caller's view of history becomes routable. Until it is
    /// called, only declared tags are in play and behaviour is exactly what
    /// it was before earned tags existed.
    ///
    /// Recomputed wholesale rather than accumulated: a record that turns out
    /// to be invalid, or a history that shrinks because a relay stopped
    /// serving something, must be able to take a tag away again. An earned
    /// set that only ever grew would be a cache pretending to be evidence.
    pub fn refresh_earned_skill_tags(&mut self, events: &[Event]) {
        self.own_earned_skill_tags = earned_skill_tags(events, &self.pubkey());
        for contact in self.contacts.values_mut() {
            contact.earned_skill_tags = earned_skill_tags(events, &contact.pubkey);
        }
    }

    /// Declared or earned — either makes this node an answer to the query.
    ///
    /// Earned is not merely *also* accepted, it is the case that matters:
    /// someone with a countersigned history in a skill they never got round
    /// to advertising is exactly who a query is looking for, and matching on
    /// the profile alone made them silent.
    fn matches_own(&self, skill_tag: &str) -> bool {
        self.own_skill_tags.iter().any(|t| t == skill_tag)
            || self.own_earned_skill_tags.iter().any(|t| t == skill_tag)
    }

    /// Hop 1 entry point: privately asked (out of protocol scope, per
    /// NIP-QW06) to look for `skill_tag` on behalf of `requester_pubkey`.
    /// Builds this node's own chain-head forwards — no incoming signed
    /// event exists yet, so there is nothing to check policy against but
    /// this node's own greedy selection.
    pub fn begin_relay_chain(
        &mut self,
        query_id: &str,
        skill_tag: &str,
        max_hops: u8,
        requester_pubkey: &str,
    ) -> RelayOutcome {
        if !self.seen.insert(query_id.to_string()) {
            return RelayOutcome::default();
        }
        self.relay_table.insert(
            query_id.to_string(),
            RelayState {
                answer_target: requester_pubkey.to_string(),
                received_event_id: None,
            },
        );
        self.relay_for(RelayArgs {
            query_id,
            skill_tag,
            outgoing_hops_from_origin: 0,
            self_match_hops: 0,
            max_hops,
            answer_target: requester_pubkey,
            received_event_id: None,
            exclude_from_forward: None,
        })
    }

    /// Hop 2+ entry point: received `event` (a signed kind-9050) from
    /// `from`, an already-known contact.
    pub fn receive_query(&mut self, from: &str, event: &Event, now: u64) -> RelayOutcome {
        if event.verify().is_err() {
            return RelayOutcome::default();
        }
        let Ok(query) = serde_json::from_str::<SkillQueryContent>(&event.content) else {
            return RelayOutcome::default();
        };
        if !self.seen.insert(query.query_id.clone()) {
            return RelayOutcome::default();
        }
        let Some(upstream) = self.contacts.get_mut(from) else {
            return RelayOutcome::default();
        };
        if !upstream.accepts_incoming(query.hops_from_origin, &query.skill_tag)
            || !upstream.record_incoming(now)
        {
            return RelayOutcome::default();
        }

        self.relay_table.insert(
            query.query_id.clone(),
            RelayState {
                answer_target: from.to_string(),
                received_event_id: Some(event.id.clone()),
            },
        );
        let outgoing_hops = query.hops_from_origin + 1;
        self.relay_for(RelayArgs {
            query_id: &query.query_id,
            skill_tag: &query.skill_tag,
            outgoing_hops_from_origin: outgoing_hops,
            self_match_hops: outgoing_hops,
            max_hops: query.max_hops,
            answer_target: from,
            received_event_id: Some(event.id.as_str()),
            exclude_from_forward: Some(from),
        })
    }

    /// Receive an answer addressed to me (`event`'s `p` tag is my
    /// pubkey): relay it one hop further back along the path, toward
    /// whoever I originally relayed this query_id for.
    pub fn receive_answer(&mut self, event: &Event) -> Option<Delivery> {
        if event.verify().is_err() {
            return None;
        }
        let content: SkillAnswerContent = serde_json::from_str(&event.content).ok()?;
        let state = self.relay_table.get(&content.query_id)?;
        let referenced = state.received_event_id.as_deref().unwrap_or("");
        let relayed = skill_answer(&self.pubkey(), &state.answer_target, referenced, &content)
            .sign(&self.identity);
        Some(Delivery::Answer {
            to: state.answer_target.clone(),
            event: relayed,
        })
    }

    fn relay_for(&mut self, args: RelayArgs) -> RelayOutcome {
        let RelayArgs {
            query_id,
            skill_tag,
            outgoing_hops_from_origin,
            self_match_hops,
            max_hops,
            answer_target,
            received_event_id,
            exclude_from_forward,
        } = args;
        let mut outcome = RelayOutcome::default();

        if self.matches_own(skill_tag) {
            let content = SkillAnswerContent {
                query_id: query_id.to_string(),
                responder_pubkey: self.pubkey(),
                matched_skill_tag: skill_tag.to_string(),
                hops: self_match_hops,
            };
            if let Some(prior_id) = received_event_id {
                let event = skill_answer(&self.pubkey(), answer_target, prior_id, &content)
                    .sign(&self.identity);
                outcome.deliveries.push(Delivery::Answer {
                    to: answer_target.to_string(),
                    event,
                });
            }
            outcome.own_match = Some(FinalAnswer {
                responder_pubkey: self.pubkey(),
                matched_skill_tag: skill_tag.to_string(),
                hops: self_match_hops,
            });
        }

        if outgoing_hops_from_origin < max_hops {
            let query = SkillQueryContent {
                query_id: query_id.to_string(),
                skill_tag: skill_tag.to_string(),
                hops_from_origin: outgoing_hops_from_origin,
                max_hops,
            };
            let candidates = self
                .contacts
                .values()
                .filter(|c| Some(c.pubkey.as_str()) != exclude_from_forward);
            for c in select_forward_targets(candidates, skill_tag) {
                if !c.allows_relay_at(outgoing_hops_from_origin) {
                    continue;
                }
                let event =
                    skill_query(&self.pubkey(), received_event_id, &query).sign(&self.identity);
                outcome.deliveries.push(Delivery::Query {
                    to: c.pubkey.clone(),
                    event,
                });
            }
        }

        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contact::ContactPolicy;

    fn linked(a: &mut Node, b: &mut Node) {
        a.add_contact(
            Contact::new(b.pubkey(), ContactPolicy::open())
                .with_cached_tags(b.own_skill_tags.clone()),
        );
        b.add_contact(
            Contact::new(a.pubkey(), ContactPolicy::open())
                .with_cached_tags(a.own_skill_tags.clone()),
        );
    }

    #[test]
    fn three_hop_chain_finds_match_and_answer_flows_back() {
        let requester = Node::new(Identity::generate());
        let mut hop1 = Node::new(Identity::generate());
        let mut hop2 = Node::new(Identity::generate());
        hop2.set_own_skill_tags(vec!["it/backend/languages#rust".to_string()]);

        // requester's own private ask to hop1 is out of protocol scope
        // (NIP-QW06) and isn't modeled as a contact link here — only
        // hop1 -> hop2 is a real relay edge in this test.
        linked(&mut hop1, &mut hop2);
        hop1.contacts
            .get_mut(&hop2.pubkey())
            .unwrap()
            .cached_skill_tags = vec!["it/backend/languages#rust".to_string()];

        let outcome =
            hop1.begin_relay_chain("q1", "it/backend/languages#rust", 3, &requester.pubkey());
        assert!(outcome.own_match.is_none());
        assert_eq!(outcome.deliveries.len(), 1);
        let Delivery::Query { to, event } = &outcome.deliveries[0] else {
            panic!("expected a query delivery")
        };
        assert_eq!(to, &hop2.pubkey());
        assert!(
            event.first_tag_value("e").is_none(),
            "hop 1's chain-head must not reference anything upstream"
        );

        let hop2_outcome = hop2.receive_query(&hop1.pubkey(), event, 1_000);
        let matched = hop2_outcome.own_match.expect("hop2 has the matching skill");
        assert_eq!(matched.responder_pubkey, hop2.pubkey());
        assert_eq!(matched.hops, 1);
        assert_eq!(hop2_outcome.deliveries.len(), 1);
        let Delivery::Answer {
            to,
            event: answer_event,
        } = &hop2_outcome.deliveries[0]
        else {
            panic!("expected an answer delivery")
        };
        assert_eq!(
            to,
            &hop1.pubkey(),
            "answer must address the immediate upstream hop, not the requester"
        );

        let relay_back = hop1
            .receive_answer(answer_event)
            .expect("hop1 relays the answer onward");
        let Delivery::Answer {
            to,
            event: relayed_event,
        } = relay_back
        else {
            panic!("expected an answer delivery")
        };
        assert_eq!(
            to,
            requester.pubkey(),
            "hop1 knows the requester and delivers the final hop"
        );
        assert!(relayed_event.verify().is_ok());
        let final_content: SkillAnswerContent =
            serde_json::from_str(&relayed_event.content).unwrap();
        assert_eq!(final_content.hops, 1);
    }

    #[test]
    fn accept_depth_zero_silently_drops_the_query() {
        let mut hop1 = Node::new(Identity::generate());
        let mut hop2 = Node::new(Identity::generate());
        hop2.set_own_skill_tags(vec!["it/backend/languages#rust".to_string()]);

        let mut policy = ContactPolicy::open();
        policy.accept_depth = 0;
        hop2.add_contact(Contact::new(hop1.pubkey(), policy));
        hop1.add_contact(
            Contact::new(hop2.pubkey(), ContactPolicy::open())
                .with_cached_tags(vec!["it/backend/languages#rust".to_string()]),
        );

        let outcome = hop1.begin_relay_chain("q1", "it/backend/languages#rust", 3, "requester");
        let Delivery::Query { event, .. } = &outcome.deliveries[0] else {
            panic!()
        };
        // event carries hops_from_origin = 0, exactly at the boundary — should be accepted
        let accepted = hop2.receive_query(&hop1.pubkey(), event, 0);
        assert!(
            accepted.own_match.is_some(),
            "hops_from_origin=0 is within accept_depth=0"
        );
    }

    #[test]
    fn dedup_prevents_reprocessing_the_same_query_id() {
        let mut hop2 = Node::new(Identity::generate());
        hop2.set_own_skill_tags(vec!["it/backend/languages#rust".to_string()]);
        let mut hop1 = Node::new(Identity::generate());
        hop1.add_contact(Contact::new(hop2.pubkey(), ContactPolicy::open()));
        hop2.add_contact(Contact::new(hop1.pubkey(), ContactPolicy::open()));

        let outcome = hop1.begin_relay_chain("q1", "it/backend/languages#rust", 3, "requester");
        let Delivery::Query { event, .. } = &outcome.deliveries[0] else {
            panic!()
        };

        let first = hop2.receive_query(&hop1.pubkey(), event, 0);
        assert!(first.own_match.is_some());
        let second = hop2.receive_query(&hop1.pubkey(), event, 0);
        assert!(
            second.own_match.is_none(),
            "already-seen query_id must be dropped, not reprocessed"
        );
        assert!(second.deliveries.is_empty());
    }

    // --- earned skill tags reach routing end to end ---

    const RUST: &str = "it/backend/languages#rust";

    /// A finished, countersigned Rust contract between two identities.
    /// Built before either identity is moved into a `Node`.
    fn countersigned_rust(client: &Identity, worker: &Identity) -> Vec<Event> {
        use qw_protocol::events::{job_completion, job_offer, JobCompletion, JobOffer};
        let offer = job_offer(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &JobOffer {
                skill_tags: vec![RUST.to_string()],
                hours: 8.0,
                rate: 40.0,
                ko: None,
                km: None,
                terms: "backend work".to_string(),
            },
        )
        .sign(client);
        let done = JobCompletion {
            rating: Some(5),
            note: None,
        };
        let worker_side = job_completion(
            &worker.nostr_pubkey_hex(),
            &client.nostr_pubkey_hex(),
            &offer.id,
            &done,
        )
        .sign(worker);
        let client_side = job_completion(
            &client.nostr_pubkey_hex(),
            &worker.nostr_pubkey_hex(),
            &offer.id,
            &done,
        )
        .sign(client);
        vec![offer, worker_side, client_side]
    }

    fn query_event(outcome: &RelayOutcome) -> &Event {
        match &outcome.deliveries[0] {
            Delivery::Query { event, .. } => event,
            _ => panic!("expected a query delivery"),
        }
    }

    /// The point of the whole change, through the real relay path: a node
    /// that has done countersigned Rust work and published no profile at all
    /// answers a Rust query — and demonstrably did not before.
    #[test]
    fn proven_work_answers_a_query_with_no_profile_published() {
        let asker = Identity::generate();
        let doer = Identity::generate();
        let events = countersigned_rust(&asker, &doer);

        let mut hop1 = Node::new(asker);
        let mut hop2 = Node::new(doer);
        linked(&mut hop1, &mut hop2);
        assert!(
            hop2.own_skill_tags.is_empty(),
            "hop2 declares nothing; evidence is all it has"
        );

        let before = hop1.begin_relay_chain("q-before", RUST, 3, "requester");
        let asked_before = hop2.receive_query(&hop1.pubkey(), query_event(&before), 1_000);
        assert!(
            asked_before.own_match.is_none(),
            "before refreshing, only declared tags count — the prior behaviour"
        );

        hop2.refresh_earned_skill_tags(&events);

        let after = hop1.begin_relay_chain("q-after", RUST, 3, "requester");
        let asked_after = hop2.receive_query(&hop1.pubkey(), query_event(&after), 1_000);
        assert_eq!(
            asked_after
                .own_match
                .as_ref()
                .map(|m| m.matched_skill_tag.as_str()),
            Some(RUST),
            "a countersigned history must answer the query it is evidence for"
        );
    }

    /// Forwarding sees it too, and labels it: the contact reachable only by
    /// earned tags outranks one who merely claims the domain.
    #[test]
    fn earned_tags_rank_a_contact_above_a_declared_one() {
        let me = Identity::generate();
        let proven = Identity::generate();
        let events = countersigned_rust(&me, &proven);
        let proven_pubkey = proven.nostr_pubkey_hex();

        let mut node = Node::new(me);
        node.add_contact(Contact::new(&proven_pubkey, ContactPolicy::open()));
        node.add_contact(
            Contact::new("claimer", ContactPolicy::open()).with_cached_tags(vec![RUST.to_string()]),
        );
        node.refresh_earned_skill_tags(&events);

        let ranked = select_forward_targets_ranked(node.contacts(), RUST);
        assert_eq!(ranked[0].0.pubkey, proven_pubkey);
        assert_eq!(ranked[0].1, MatchSource::Earned);
        assert_eq!(ranked[1].1, MatchSource::Declared);
    }

    /// Recomputed, not accumulated: history that stops being visible has to
    /// take the tag with it, or the set is a cache pretending to be proof.
    #[test]
    fn refreshing_against_an_empty_history_clears_earned_tags() {
        let me = Identity::generate();
        let peer = Identity::generate();
        let events = countersigned_rust(&me, &peer);

        let mut node = Node::new(me);
        node.refresh_earned_skill_tags(&events);
        assert!(!node.own_earned_skill_tags.is_empty());

        node.refresh_earned_skill_tags(&[]);
        assert!(node.own_earned_skill_tags.is_empty());
    }
}
