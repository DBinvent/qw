//! The QW client shell: a window over `qw_client_core`.
//!
//! Everything here is deliberately thin — parse an argument, call the
//! core, hand back JSON. Nothing in this file knows how to sign, verify,
//! or decide anything, because none of that can be tested on a machine
//! that cannot compile Tauri (webkit2gtk et al). The logic lives one
//! directory over in `app/core`, which builds and tests anywhere, and is
//! where every rule worth arguing about is pinned by a test.
//!
//! **Compiles, never run.** `cargo build` and `cargo clippy` are clean here
//! since webkit2gtk and `tauri-cli` were installed (2026-08-25), but running
//! it needs a display and nothing in this file has a test. On Android it is
//! `run()` that the generated activity calls, via the `mobile_entry_point`
//! attribute at the bottom — see `app/README.md` for that toolchain.

use std::sync::Mutex;

use qw_client_core::{follow_invite, invite_qr_svg, EventStore, HttpMailbox, Vault};
use qw_node::sync::MailboxSync;
use qw_protocol::identity::Identity;
use qw_protocol::invite;
use qw_protocol::trust::earned_skills;
use serde::Serialize;
use tauri::{Manager, State};

pub struct AppState {
    identity: Identity,
    sync: Mutex<MailboxSync>,
    /// Coordination servers this client will talk to, best first. Plural
    /// from the first line of code on purpose: §8 forbids hard-coding one
    /// server as authoritative, and `qw_node::server_registry::rank_servers`
    /// is what should eventually order this list by the node's own trust
    /// view rather than by config order.
    servers: Vec<String>,
    /// Everything this client has been handed, on disk. Before it existed
    /// `sync_now` counted what arrived and dropped it, which quietly meant
    /// no trust path, no contract list and no earned skill could ever be
    /// computed — all three are functions over held history.
    store: Mutex<EventStore>,
}

#[derive(Serialize)]
pub struct IdentityView {
    pubkey: String,
    npub: String,
    invite_link: String,
    /// One row per skill, whatever the evidence for it. Not three lists —
    /// a skill is the row and the evidence is decoration on it, because
    /// "reviewed" and "contract-approved" are independent of each other and
    /// of whether you declared the tag. Splitting them would make a phone
    /// screen ask the reader to join three tables by eye.
    skills: Vec<SkillView>,
    /// The same link as a QR code, an SVG document ready to inject. Sent
    /// with the identity rather than fetched separately: the UI has no
    /// bundler and so no QR library of its own, and the link never changes
    /// for the life of the identity, so there is nothing to refresh.
    invite_qr: String,
}

/// A skill and its badges. Both badge fields are `None` when that kind of
/// evidence does not exist, which is the normal case for most skills.
#[derive(Serialize)]
pub struct SkillView {
    tag: String,
    /// Declared by the holder in their profile. Every skill they published
    /// is `true`; a skill that only shows up in contract history is not.
    declared: bool,
    /// Badge one: a broker's review. Unpopulated — reviewed skills are
    /// specced (`todo-impl.md` §2) and unbuilt, and the field exists so the
    /// UI that renders badges does not have to change shape when they land.
    reviewed: Option<String>,
    /// Badge two: countersigned contracts carrying this tag, and the mean
    /// rating counterparties gave. `rating` stays `None` when nobody
    /// recorded one — absent is not zero.
    contracts: usize,
    rating: Option<f64>,
}

#[derive(Serialize)]
pub struct SyncView {
    delivered: usize,
    rejected: usize,
    /// Records now held on disk after this pass — the running total, not
    /// the delta. This is what every later computation actually reads.
    held: usize,
    published: usize,
    still_queued: usize,
    errors: Vec<String>,
}

#[tauri::command]
fn identity(state: State<'_, AppState>) -> Result<IdentityView, String> {
    let pubkey = state.identity.nostr_pubkey_hex();
    let npub = invite::npub_encode(&pubkey).map_err(|e| e.to_string())?;
    let invite_link =
        invite::invite_url("https://knownby.work", &pubkey).map_err(|e| e.to_string())?;
    let invite_qr = invite_qr_svg(&invite_link).map_err(|e| e.to_string())?;
    let store = state.store.lock().map_err(|e| e.to_string())?;
    // Earned rows only, for now: there is no profile editor, so nothing is
    // declared, and no reviews exist to merge in. Both are `false`/`None`
    // rather than absent so the shape does not change when they arrive.
    let skills = earned_skills(store.events(), &pubkey)
        .into_iter()
        .map(|s| SkillView {
            tag: s.tag,
            declared: false,
            reviewed: None,
            contracts: s.contracts,
            rating: s.rating,
        })
        .collect();
    Ok(IdentityView {
        pubkey,
        npub,
        invite_link,
        skills,
        invite_qr,
    })
}

/// Follow someone's invite link: sign our half of the introduction and
/// queue it. The publisher's half is theirs to sign — a client that
/// produced both sides would be forging half the edge.
#[tauri::command]
fn follow(link: String, state: State<'_, AppState>) -> Result<String, String> {
    let event = follow_invite(&state.identity, &link).map_err(|e| e.to_string())?;
    let id = event.id.clone();
    state.sync.lock().map_err(|e| e.to_string())?.queue(event);
    Ok(id)
}

/// One sync pass: send what is queued, then collect what arrived. Send
/// first so a reply this client just wrote is on its way before it blocks
/// on downloading anything.
#[tauri::command]
fn sync_now(state: State<'_, AppState>) -> Result<SyncView, String> {
    let mut transport = HttpMailbox::new();
    let servers: Vec<&str> = state.servers.iter().map(String::as_str).collect();
    let mut sync = state.sync.lock().map_err(|e| e.to_string())?;

    let flushed = sync.flush(&mut transport, &servers);
    let polled = sync.poll(&mut transport, &servers);

    // Persist before reporting. A count of delivered events that no longer
    // exist anywhere is the bug this replaced.
    let mut store = state.store.lock().map_err(|e| e.to_string())?;
    store
        .append(polled.delivered.iter().cloned())
        .map_err(|e| e.to_string())?;

    let mut errors: Vec<String> = flushed
        .errors
        .iter()
        .chain(polled.errors.iter())
        .map(|(server, message)| format!("{server}: {message}"))
        .collect();
    errors.dedup();

    Ok(SyncView {
        delivered: polled.delivered.len(),
        held: store.len(),
        rejected: polled.rejected,
        published: flushed.published,
        still_queued: flushed.still_queued,
        errors,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // The key lives in the OS app-data directory, created 0700 by
            // the core. First run generates one; every later run must find
            // the same file, because there is no account to recover it
            // from — losing it is losing the identity.
            let dir = app.path().app_data_dir()?;
            let identity = Vault::at(&dir).load_or_create()?;
            let store = EventStore::open(&dir)?;
            let sync = MailboxSync::new(identity.nostr_pubkey_hex());
            app.manage(AppState {
                identity,
                sync: Mutex::new(sync),
                store: Mutex::new(store),
                servers: vec!["https://qw-dash-api.knownby.work".to_string()],
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![identity, follow, sync_now])
        .run(tauri::generate_context!())
        .expect("error while running QW");
}
