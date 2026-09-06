//! The QW client shell: a window over `qw_client_core`.
//!
//! Every command is the same three lines — lock the one [`Session`], call
//! the matching `qw_client_core` operation, stringify the error. No
//! behaviour lives here: it cannot be tested on a machine that cannot
//! compile Tauri (webkit2gtk et al), so all of it lives one directory over
//! in `app/core`, which builds and tests anywhere, and a `qw-web` HTTP
//! handler is the same shim over the same `Session` (`todo-impl.md` §7).
//!
//! **Compiles, never run.** `cargo build`/`cargo clippy` are clean here
//! since webkit2gtk and `tauri-cli` were installed (2026-08-25), but
//! running it needs a display. On Android it is `run()` the generated
//! activity calls, via `mobile_entry_point` at the bottom.

use std::sync::{Mutex, MutexGuard};

use qw_client_core::negotiation::{
    AcceptArgs, AnnotateArgs, CounterArgs, NegotiationView, ProposeArgs,
};
use qw_client_core::profile::{ProfileEdit, ProfileView};
use qw_client_core::session::{
    ContactView, FinalAnswerView, FollowResult, IdentityView, ReferralView, SyncView, TrustView,
};
use qw_client_core::{taxonomy, EventStore, HttpMailbox, ServerCandidate, Session, Vault};
use tauri::{Manager, State};

pub struct AppState {
    session: Mutex<Session>,
}

fn session<'a>(state: &'a State<'_, AppState>) -> Result<MutexGuard<'a, Session>, String> {
    state.session.lock().map_err(|e| e.to_string())
}

#[tauri::command]
fn identity(state: State<'_, AppState>) -> Result<IdentityView, String> {
    session(&state)?.identity_view().map_err(|e| e.to_string())
}

/// Follow an invite link. Returns the queued intro's id plus the contact
/// it connects to, so the UI can open a contract proposal aimed at them.
#[tauri::command]
fn follow(link: String, state: State<'_, AppState>) -> Result<FollowResult, String> {
    session(&state)?.follow(&link).map_err(|e| e.to_string())
}

/// One sync pass: flush the outbox, poll for mail, persist what arrived.
#[tauri::command]
fn sync_now(state: State<'_, AppState>) -> Result<SyncView, String> {
    session(&state)?
        .sync_now(&mut HttpMailbox::new())
        .map_err(|e| e.to_string())
}

/// The full taxonomy leaf list for the editor's picker — static for the
/// life of the build.
#[tauri::command]
fn taxonomy_leaves() -> Vec<String> {
    taxonomy::leaves().to_vec()
}

#[tauri::command]
fn profile_get(state: State<'_, AppState>) -> Result<ProfileView, String> {
    Ok(session(&state)?.profile_view())
}

/// Resolve, sign and queue a new replaceable profile event (kind 10020,
/// `revision` bumped). `sync_now` publishes it.
#[tauri::command]
fn profile_set(edit: ProfileEdit, state: State<'_, AppState>) -> Result<String, String> {
    session(&state)?.set_profile(edit).map_err(|e| e.to_string())
}

#[tauri::command]
fn negotiations(state: State<'_, AppState>) -> Result<Vec<NegotiationView>, String> {
    Ok(session(&state)?.negotiations())
}

/// Sign and queue a fresh proposal (kind 9000). Returns the offer id.
#[tauri::command]
fn propose(args: ProposeArgs, state: State<'_, AppState>) -> Result<String, String> {
    session(&state)?.propose(args).map_err(|e| e.to_string())
}

/// Reject-amend-reapply in one event: supersede the head terms (kind 9004).
#[tauri::command]
fn counter(args: CounterArgs, state: State<'_, AppState>) -> Result<String, String> {
    session(&state)?.counter(args).map_err(|e| e.to_string())
}

/// The worker's Accept against the current head (kind 9001).
#[tauri::command]
fn accept_contract(args: AcceptArgs, state: State<'_, AppState>) -> Result<String, String> {
    session(&state)?.accept(args).map_err(|e| e.to_string())
}

/// Attach a dispute annotation (kind 9030, NIP-QW04) to a contract — a
/// reply, an audit request, or a third-party audit opinion.
#[tauri::command]
fn annotate(args: AnnotateArgs, state: State<'_, AppState>) -> Result<String, String> {
    session(&state)?.annotate(args).map_err(|e| e.to_string())
}

/// This identity's hop-1 contacts and the skill tags routing knows them by.
#[tauri::command]
fn contacts(state: State<'_, AppState>) -> Result<Vec<ContactView>, String> {
    Ok(session(&state)?.contacts())
}

/// Originate a referral query (NIP-QW06). `sync_now` carries the forwards
/// out; `referral_results(query_id)` collects the answers.
#[tauri::command]
fn find_by_skill(skill: String, state: State<'_, AppState>) -> Result<ReferralView, String> {
    session(&state)?.find_by_skill(&skill).map_err(|e| e.to_string())
}

#[tauri::command]
fn referral_results(
    qid: String,
    state: State<'_, AppState>,
) -> Result<Vec<FinalAnswerView>, String> {
    Ok(session(&state)?.referral_results(&qid).to_vec())
}

/// The full profile this client holds for another pubkey — for a
/// non-contact, whatever rode back on a referral answer (NIP-QW06).
/// `None` when none is held.
#[tauri::command]
fn profile_of(pubkey: String, state: State<'_, AppState>) -> Result<Option<ProfileView>, String> {
    Ok(session(&state)?.profile_of(&pubkey))
}

/// This viewer's trust read on a pubkey (§5 — per-viewer, never global).
#[tauri::command]
fn trust(pubkey: String, state: State<'_, AppState>) -> Result<TrustView, String> {
    Ok(session(&state)?.trust(&pubkey))
}

/// This identity's own global net position over verified credit.
#[tauri::command]
fn net_position(state: State<'_, AppState>) -> Result<f64, String> {
    Ok(session(&state)?.net_position())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // The key and the event log live in the OS app-data directory,
            // created 0700 by the core. First run generates the identity;
            // every later run must find the same file — there is no account
            // to recover it from.
            let dir = app.path().app_data_dir()?;
            let servers = vec!["https://qw-dash-api.knownby.work".to_string()];
            let mut session = Session::open(
                &Vault::at(&dir),
                Box::new(EventStore::open(&dir)?),
                servers.clone(),
            )?;
            // §8 forbids hard-coding one server as authoritative — the list
            // is ordered by this identity's own trust view of each. The
            // `pubkey` is blank until servers advertise one (unknown-risk
            // → fee-order); wiring the call now means real ranking lands
            // for free when they do, and a `QW_SERVERS`-style multi-entry
            // list is trust-ordered from day one.
            session.rank_servers(
                &servers
                    .iter()
                    .map(|url| ServerCandidate {
                        pubkey: String::new(),
                        base_url: url.clone(),
                        fee: 0.0,
                    })
                    .collect::<Vec<_>>(),
            );
            app.manage(AppState {
                session: Mutex::new(session),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            identity,
            follow,
            sync_now,
            taxonomy_leaves,
            profile_get,
            profile_set,
            negotiations,
            propose,
            counter,
            accept_contract,
            annotate,
            contacts,
            find_by_skill,
            referral_results,
            profile_of,
            trust,
            net_position
        ])
        .run(tauri::generate_context!())
        .expect("error while running QW");
}
