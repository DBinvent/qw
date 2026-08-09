//! Shared server state: the server's own signing identity, and an
//! in-memory event store standing in for real persistence — same
//! explicit "this is a stand-in, not production infra" precedent as
//! `qw_node::network::Network` (§3).

use std::sync::{Arc, RwLock};

use qw_protocol::events::Event;
use qw_protocol::identity::Identity;

#[derive(Clone)]
pub struct AppState {
    pub identity: Arc<Identity>,
    pub events: Arc<RwLock<Vec<Event>>>,
}

impl AppState {
    pub fn new(identity: Identity) -> Self {
        Self {
            identity: Arc::new(identity),
            events: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn with_events(identity: Identity, events: Vec<Event>) -> Self {
        Self {
            identity: Arc::new(identity),
            events: Arc::new(RwLock::new(events)),
        }
    }
}
