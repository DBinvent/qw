//! `qw-web` — the QW client as a local web app.
//!
//! It is the Tauri shell with a different front door: one
//! [`qw_client_core::Session`] behind a `Mutex`, and one HTTP route per
//! operation that does exactly what the matching `#[tauri::command]` does
//! — lock the session, call the core, hand back JSON. No behaviour lives
//! here; that is the rule that lets this stand in for the app in review
//! and CI, where a Tauri build cannot run.
//!
//! **Single-user** by default. The identity key and the event log are a
//! `Vault` and an `EventStore` in one data directory, exactly as on the
//! phone — meant to run on a box the operator already trusts (their own,
//! encrypted as they see fit). Config (env): `QW_DATA_DIR` (default
//! `qw-data`), `QW_WEB_ADDR` (default `127.0.0.1:8787`), `QW_SERVERS`
//! (comma-separated, default the one coordination server).
//!
//! **Multi-user** when `QW_MULTIUSER` is set: `main` hands off to
//! [`multiuser::serve`]. Each account is a per-account master key (never
//! at rest in the clear) wrapped by a KEK set, its identity + ledger in
//! one AEAD blob in Postgres; `login` unlocks a `Session` into a
//! token-keyed table for the life of a browser session. Spec and the
//! full env list are in `multi-user.md`. The single-user path below this
//! line is untouched by it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use qw_client_core::negotiation::{AcceptArgs, AnnotateArgs, CounterArgs, ProposeArgs};
use qw_client_core::profile::ProfileEdit;
use qw_client_core::{
    taxonomy, EventStore, HttpMailbox, LedgerCoverage, PullResponse, ServerCandidate, Session, Vault,
};
use qw_protocol::events::Event;
use serde::Deserialize;

// The multi-user host (`server/multi-user.md`): `marg` for the connection
// string, `schema_guard_tokio` for the schema, `sqlx` for the queries, one
// master key per account wrapped by a KEK set. `main` runs it instead of
// the single-user body when `QW_MULTIUSER` is set.
mod multiuser;
// These carry a little surface the routes never reach — operator
// nickname re-index, some envelope/blob accessors the test suites assert
// through, and `Unlocked::via` (which wrapping let a login in — for a
// future "signed in with your recovery code" notice). The KEK-set ops
// themselves are wired now (`multiuser::kek_*`).
#[allow(dead_code)]
mod account_session;
#[allow(dead_code)]
mod accounts;
#[allow(dead_code)]
mod envelope;
mod ratelimit;
#[allow(dead_code)]
mod unlock;

struct Web {
    session: Mutex<Session>,
}

#[tokio::main]
async fn main() {
    if multiuser::configured() {
        multiuser::serve().await;
        return;
    }

    let dir = PathBuf::from(std::env::var("QW_DATA_DIR").unwrap_or_else(|_| "qw-data".into()));
    std::fs::create_dir_all(&dir).expect("create QW_DATA_DIR");

    let servers: Vec<String> = std::env::var("QW_SERVERS")
        .unwrap_or_else(|_| "https://qw-dash-api.knownby.work".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut session = Session::open(
        &Vault::at(dir.as_path()),
        Box::new(EventStore::open(dir.as_path()).expect("open event store")),
        servers.clone(),
    )
    .expect("open session");
    // §8: no server is hard-coded as authoritative — the list is ordered
    // by this identity's own trust view. `pubkey` is blank until servers
    // advertise one (unknown-risk → fee-order); the call is wired so real
    // ranking arrives for free once they do.
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
    let web = Arc::new(Web {
        session: Mutex::new(session),
    });

    let addr = std::env::var("QW_WEB_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    eprintln!("qw-web on http://{addr}  (data dir: {})", dir.display());
    axum::serve(listener, router(web)).await.expect("serve");
}

fn router(web: Arc<Web>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/session", get(session_info))
        .route("/api/identity", post(identity))
        .route("/api/follow", post(follow))
        .route("/api/sync_now", post(sync_now))
        .route("/api/taxonomy_leaves", post(taxonomy_leaves))
        .route("/api/profile_get", post(profile_get))
        .route("/api/profile_set", post(profile_set))
        .route("/api/negotiations", post(negotiations))
        .route("/api/propose", post(propose))
        .route("/api/counter", post(counter))
        .route("/api/accept_contract", post(accept_contract))
        .route("/api/annotate", post(annotate))
        .route("/api/contacts", post(contacts))
        .route("/api/find_by_skill", post(find_by_skill))
        .route("/api/referral_results", post(referral_results))
        .route("/api/profile_of", post(profile_of))
        .route("/api/trust", post(trust))
        .route("/api/net_position", post(net_position))
        .route("/ledger/pull", post(ledger_pull))
        .route("/ledger/push", post(ledger_push))
        .with_state(web)
}

// --- NIP-QW12 ledger replication (single-user host is one identity, so a
//     peer replica — the operator's phone, another of their own boxes —
//     syncs against it directly). The multi-user host needs a per-account
//     auth channel (a device subkey) before it can expose this. ------

/// Answer a peer's coverage with the events it lacks, plus this replica's
/// own coverage so the peer can push its reverse difference.
async fn ledger_pull(State(web): State<Arc<Web>>, Json(have): Json<LedgerCoverage>) -> Response {
    reply(
        on_session(web, move |s| {
            let held: Vec<Event> = s.held_events().to_vec();
            let events = held.iter().filter(|e| have.wants(e)).cloned().collect();
            Ok::<_, String>(PullResponse {
                events,
                coverage: LedgerCoverage::of(&held),
            })
        })
        .await,
    )
}

/// Merge a peer's events into the ledger (verify-on-ingest); answer with
/// the count that were new.
async fn ledger_push(State(web): State<Arc<Web>>, Json(events): Json<Vec<Event>>) -> Response {
    reply(on_session(web, move |s| s.ingest(&events).map_err(text)).await)
}

/// The one static asset — the same file Tauri loads, which picks
/// HTTP-fetch over the Tauri IPC bridge when `window.__TAURI__` is absent.
async fn index() -> Html<&'static str> {
    Html(include_str!("../../ui/index.html"))
}

/// Mirrors the multi-user host's `/session` so the shared UI has one probe
/// for "is this a login host, and am I signed in". A single-user server
/// has one always-open session and no key page.
async fn session_info() -> Response {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "multi_user": false, "authenticated": true })),
    )
        .into_response()
}

// --- operations: one per Tauri command, same request shape ----------

async fn identity(State(web): State<Arc<Web>>) -> Response {
    reply(on_session(web, |s| s.identity_view().map_err(text)).await)
}

#[derive(Deserialize)]
struct FollowBody {
    link: String,
}
async fn follow(State(web): State<Arc<Web>>, Json(b): Json<FollowBody>) -> Response {
    reply(on_session(web, move |s| s.follow(&b.link).map_err(text)).await)
}

async fn sync_now(State(web): State<Arc<Web>>) -> Response {
    reply(on_session(web, |s| s.sync_now(&mut HttpMailbox::new()).map_err(text)).await)
}

async fn taxonomy_leaves() -> Response {
    reply(Ok(taxonomy::leaves().to_vec()))
}

async fn profile_get(State(web): State<Arc<Web>>) -> Response {
    reply(on_session(web, |s| Ok::<_, String>(s.profile_view())).await)
}

#[derive(Deserialize)]
struct ProfileSetBody {
    edit: ProfileEdit,
}
async fn profile_set(State(web): State<Arc<Web>>, Json(b): Json<ProfileSetBody>) -> Response {
    reply(on_session(web, move |s| s.set_profile(b.edit).map_err(text)).await)
}

async fn negotiations(State(web): State<Arc<Web>>) -> Response {
    reply(on_session(web, |s| Ok::<_, String>(s.negotiations())).await)
}

#[derive(Deserialize)]
struct ProposeBody {
    args: ProposeArgs,
}
async fn propose(State(web): State<Arc<Web>>, Json(b): Json<ProposeBody>) -> Response {
    reply(on_session(web, move |s| s.propose(b.args).map_err(text)).await)
}

#[derive(Deserialize)]
struct CounterBody {
    args: CounterArgs,
}
async fn counter(State(web): State<Arc<Web>>, Json(b): Json<CounterBody>) -> Response {
    reply(on_session(web, move |s| s.counter(b.args).map_err(text)).await)
}

#[derive(Deserialize)]
struct AcceptBody {
    args: AcceptArgs,
}
async fn accept_contract(State(web): State<Arc<Web>>, Json(b): Json<AcceptBody>) -> Response {
    reply(on_session(web, move |s| s.accept(b.args).map_err(text)).await)
}

#[derive(Deserialize)]
struct AnnotateBody {
    args: AnnotateArgs,
}
/// Attach a dispute annotation (kind 9030, NIP-QW04) to a contract.
async fn annotate(State(web): State<Arc<Web>>, Json(b): Json<AnnotateBody>) -> Response {
    reply(on_session(web, move |s| s.annotate(b.args).map_err(text)).await)
}

async fn contacts(State(web): State<Arc<Web>>) -> Response {
    reply(on_session(web, |s| Ok::<_, String>(s.contacts())).await)
}

#[derive(Deserialize)]
struct FindBySkillBody {
    skill: String,
}
async fn find_by_skill(State(web): State<Arc<Web>>, Json(b): Json<FindBySkillBody>) -> Response {
    reply(on_session(web, move |s| s.find_by_skill(&b.skill).map_err(text)).await)
}

#[derive(Deserialize)]
struct ReferralResultsBody {
    qid: String,
}
async fn referral_results(
    State(web): State<Arc<Web>>,
    Json(b): Json<ReferralResultsBody>,
) -> Response {
    reply(on_session(web, move |s| Ok::<_, String>(s.referral_results(&b.qid).to_vec())).await)
}

#[derive(Deserialize)]
struct ProfileOfBody {
    pubkey: String,
}
/// The full profile this session holds for another pubkey — for a
/// non-contact, whatever rode back on a referral answer (NIP-QW06).
/// `null` when none is held.
async fn profile_of(State(web): State<Arc<Web>>, Json(b): Json<ProfileOfBody>) -> Response {
    reply(on_session(web, move |s| Ok::<_, String>(s.profile_of(&b.pubkey))).await)
}

#[derive(Deserialize)]
struct TrustBody {
    pubkey: String,
}
async fn trust(State(web): State<Arc<Web>>, Json(b): Json<TrustBody>) -> Response {
    reply(on_session(web, move |s| Ok::<_, String>(s.trust(&b.pubkey))).await)
}

async fn net_position(State(web): State<Arc<Web>>) -> Response {
    reply(on_session(web, |s| Ok::<_, String>(s.net_position())).await)
}

// --- plumbing ------------------------------------------------------

fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Run a session operation on the blocking pool — `Session` methods are
/// synchronous and `sync_now` makes a blocking HTTP call, neither of which
/// belongs on an async worker thread. The `MutexGuard` never crosses an
/// `.await`: it lives and dies inside the closure.
async fn on_session<T, F>(web: Arc<Web>, f: F) -> Result<T, String>
where
    F: FnOnce(&mut Session) -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(move || {
        let mut guard = web.session.lock().map_err(|_| "session lock poisoned".to_string())?;
        f(&mut guard)
    })
    .await
    {
        Ok(inner) => inner,
        Err(_) => Err("operation task panicked".to_string()),
    }
}

/// Success is `200` + the JSON value; a domain error is `422` +
/// `{"error": "..."}`, which the UI's fetch adapter turns back into a
/// thrown string — the same thing a rejected Tauri `invoke` gives it.
fn reply<T: serde::Serialize>(r: Result<T, String>) -> Response {
    match r {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    fn app(dir: &std::path::Path) -> Router {
        let session = Session::open(
            &Vault::at(dir),
            Box::new(EventStore::open(dir).unwrap()),
            vec![],
        )
        .unwrap();
        router(Arc::new(Web {
            session: Mutex::new(session),
        }))
    }

    async fn call(app: &Router, path: &str, body: Value) -> (StatusCode, Value) {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn index_serves_the_shared_ui() {
        let dir = tempfile::tempdir().unwrap();
        let res = app(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("<title>QW</title>"));
    }

    #[tokio::test]
    async fn identity_profile_round_trip_and_a_bad_tag_is_a_422() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());

        let (st, id) = call(&app, "/api/identity", json!({})).await;
        assert_eq!(st, StatusCode::OK);
        assert!(id["npub"].as_str().unwrap().starts_with("npub1"));
        assert!(id["invite_qr"].as_str().unwrap().contains("<svg"));

        let (st, _) = call(
            &app,
            "/api/profile_set",
            json!({ "edit": { "display_name": "Vlad", "tags": ["Rust Lang"] } }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);

        let (_, prof) = call(&app, "/api/profile_get", json!({})).await;
        assert_eq!(prof["tags"][0], "it/backend/languages#rust");
        assert_eq!(prof["display_name"], "Vlad");

        let (st, err) = call(
            &app,
            "/api/profile_set",
            json!({ "edit": { "display_name": null, "tags": ["underwater basket weaving"] } }),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err["error"].as_str().unwrap().contains("not a known skill"));
    }

    #[tokio::test]
    async fn a_proposal_shows_up_in_negotiations() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());

        let (_, empty) = call(&app, "/api/negotiations", json!({})).await;
        assert!(empty.as_array().unwrap().is_empty());

        // Any 64-hex string is a well-formed counterparty for this test.
        let (st, id) = call(
            &app,
            "/api/propose",
            json!({ "args": {
                "counterparty": "a".repeat(64),
                "from_introduction": null,
                "terms": {
                    "skill_tags": ["rust"], "hours": 8.0, "rate": 40.0,
                    "ko": null, "km": null, "terms": "sprint 12"
                }
            }}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(id.as_str().unwrap().len() == 64, "returns the offer id");

        let (_, negs) = call(&app, "/api/negotiations", json!({})).await;
        let negs = negs.as_array().unwrap();
        assert_eq!(negs.len(), 1);
        assert_eq!(negs[0]["am_client"], true);
        assert_eq!(negs[0]["head_terms"]["rate"], 40.0);
        let offer_id = negs[0]["offer_event_id"].as_str().unwrap().to_string();

        // annotate that contract with an audit request (NIP-QW04)
        let (st, ann) = call(
            &app,
            "/api/annotate",
            json!({ "args": {
                "offer_event_id": offer_id, "kind": "audit_request",
                "body": "milestone was never delivered"
            }}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(ann.as_str().unwrap().len(), 64, "returns the annotation id");

        let (_, negs) = call(&app, "/api/negotiations", json!({})).await;
        assert_eq!(negs[0]["disputes"].as_array().unwrap().len(), 1);
        assert_eq!(negs[0]["disputes"][0]["annotation_type"], "audit_request");
        assert_eq!(negs[0]["under_review"], true);

        // a bad audit outcome is a 422 naming the fix
        let (st, err) = call(
            &app,
            "/api/annotate",
            json!({ "args": {
                "offer_event_id": offer_id, "kind": "audit_opinion", "body": "x"
            }}),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err["error"].as_str().unwrap().contains("outcome"));
    }

    #[tokio::test]
    async fn ledger_push_then_pull_merges_and_verifies() {
        use qw_protocol::events::{profile_skill_tags, ProfileSkillTags};
        use qw_protocol::identity::Identity;

        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());

        let author = Identity::generate();
        let evt = profile_skill_tags(
            &author.nostr_pubkey_hex(),
            1,
            &ProfileSkillTags {
                display_name: None,
                skill_tags: vec!["it/backend/languages#rust".into()],
            },
        )
        .sign(&author);

        // push merges it and answers with the new-event count; a re-push
        // is a no-op (union merge)
        let (st, accepted) = call(&app, "/ledger/push", json!([evt])).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(accepted, json!(1));
        assert_eq!(call(&app, "/ledger/push", json!([evt.clone()])).await.1, json!(0));

        // a peer with empty coverage pulls everything the host now holds
        let (st, resp) = call(&app, "/ledger/pull", json!([])).await;
        assert_eq!(st, StatusCode::OK);
        let ids: Vec<&str> = resp["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&evt.id.as_str()));
        assert!(resp["coverage"].as_array().unwrap().iter().any(|c| c == &json!(evt.id)));

        // verify-on-ingest: a forged event is refused on push
        let mut forged = evt.clone();
        forged.content = "swapped after signing".into();
        assert_eq!(call(&app, "/ledger/push", json!([forged])).await.1, json!(0));
    }

    #[tokio::test]
    async fn referral_routes_are_wired() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());

        // a fresh identity has no contacts
        let (st, cs) = call(&app, "/api/contacts", json!({})).await;
        assert_eq!(st, StatusCode::OK);
        assert!(cs.as_array().unwrap().is_empty());

        // publishing "rust" makes find_by_skill self-match, and returns a
        // query_id even with nobody to forward to
        call(
            &app,
            "/api/profile_set",
            json!({ "edit": { "display_name": null, "tags": ["Rust Lang"] } }),
        )
        .await;
        let (st, r) = call(&app, "/api/find_by_skill", json!({ "skill": "rust" })).await;
        assert_eq!(st, StatusCode::OK);
        let qid = r["query_id"].as_str().unwrap();
        assert_eq!(r["own_match"]["matched_skill_tag"], "it/backend/languages#rust");
        // the own-match row carries the full declared profile, not just the tag
        assert_eq!(
            r["own_match"]["declared_skill_tags"],
            json!(["it/backend/languages#rust"])
        );

        let (st, res) = call(&app, "/api/referral_results", json!({ "qid": qid })).await;
        assert_eq!(st, StatusCode::OK);
        assert!(res.as_array().unwrap().is_empty());

        // profile_of resolves a held profile (here, the host's own) and is
        // null for a pubkey nothing is held for
        let (_, id) = call(&app, "/api/identity", json!({})).await;
        let me = id["pubkey"].as_str().unwrap();
        let (st, p) = call(&app, "/api/profile_of", json!({ "pubkey": me })).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(p["tags"], json!(["it/backend/languages#rust"]));
        let (_, none) = call(&app, "/api/profile_of", json!({ "pubkey": "b".repeat(64) })).await;
        assert_eq!(none, json!(null));

        // an unresolvable skill is a 422 naming it
        let (st, err) = call(&app, "/api/find_by_skill", json!({ "skill": "totally not a skill" })).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err["error"].as_str().unwrap().contains("not a known skill"));
    }

    #[tokio::test]
    async fn trust_routes_report_unknown_risk_and_zero_position() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());

        // a fresh host: any stranger is unknown-risk, not zero
        let (st, t) = call(&app, "/api/trust", json!({ "pubkey": "a".repeat(64) })).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(t["trust_hops"], json!(null));
        assert_eq!(t["trust_score"], json!(null));
        assert_eq!(t["net_position"], json!(0.0));
        assert!(t["path_edge_ids"].as_array().unwrap().is_empty());

        let (st, np) = call(&app, "/api/net_position", json!({})).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(np, json!(0.0));
    }
}
