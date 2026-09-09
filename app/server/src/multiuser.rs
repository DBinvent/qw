//! The multi-user `qw-web` host (`server/multi-user.md`).
//!
//! One process, many accounts. `register` / `login` open a `Session` from
//! the encrypted account blob and park it in a token-keyed table for the
//! life of a browser session; the `/api/<cmd>` routes are the single-user
//! ones, resolved against that table instead of one fixed session, and
//! each one flushes the re-sealed blob back to Postgres before it returns.
//!
//! Selected by `QW_MULTIUSER` being set; otherwise `main` runs the
//! single-user server unchanged. Config comes from `marg`
//! (`--db` / `db` env / `--file`); the directory-tier key is
//! `QW_SERVER_KEY_FILE` (a file of 64 hex chars, the deployed form) or
//! `QW_SERVER_KEY` inline, the coordination servers `QW_SERVERS`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use qw_client_core::negotiation::{AcceptArgs, AnnotateArgs, CounterArgs, ProposeArgs};
use qw_client_core::profile::ProfileEdit;
use qw_client_core::{taxonomy, ClientPrefs, HttpMailbox, PropagationConfigWire, Session};
use qw_protocol::identity::Identity;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::account_session::{self, OpenAccount, SealedHandle};
use crate::accounts::{self, AccountError, AccountRecord, AccountStore, PgAccountStore, SERVER_KEY_LEN};
use crate::envelope::{MasterKey, Wrapping};
use crate::ratelimit::RateLimiter;
use crate::unlock::{self, UnlockError};

/// Idle timeout — a session unused this long is evicted (and its master
/// key zeroized) on the next sweep or request.
const IDLE_TTL: Duration = Duration::from_secs(30 * 60);
/// Absolute lifetime, regardless of activity.
const ABS_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const SWEEP_EVERY: Duration = Duration::from_secs(60);
const COOKIE: &str = "qw_session";

/// `/auth/register`, per source IP — registration-abuse limit
/// (`multi-user.md`: "registration-abuse limits").
const REGISTER_LIMIT: u32 = 5;
const REGISTER_WINDOW: Duration = Duration::from_secs(60 * 60);
/// `/auth/login` — Argon2id's ~10-40 ms/attempt is friction, not a defence
/// on its own, so both the source IP and the target account are capped:
/// the IP limit blocks a spray across many accounts, the account limit
/// blocks a focused brute force spread across many source IPs.
const LOGIN_IP_LIMIT: u32 = 20;
const LOGIN_ACCT_LIMIT: u32 = 10;
const LOGIN_WINDOW: Duration = Duration::from_secs(5 * 60);
/// Sweep interval for stale rate-limit windows — at least the longest limit
/// window above, so an in-progress window is never swept out from under it.
const RATE_LIMIT_SWEEP_MAX_AGE: Duration = REGISTER_WINDOW;

/// True when this process should run the multi-user host.
pub fn configured() -> bool {
    std::env::var_os("QW_MULTIUSER").is_some()
}

/// The multi-user entrypoint. `main` calls this instead of the single-user
/// body when [`configured`].
pub async fn serve() {
    let cfg = marg::ArgConfig::from_args().unwrap_or_else(|e| panic!("marg config: {e}"));
    let db_url = cfg.db_url();
    accounts::migrate(&db_url)
        .await
        .unwrap_or_else(|e| panic!("apply accounts.schema.yaml: {e}"));
    let store = PgAccountStore::connect(&db_url)
        .await
        .unwrap_or_else(|e| panic!("connect {}: {e}", cfg.table));

    let servers: Vec<String> = std::env::var("QW_SERVERS")
        .unwrap_or_else(|_| "https://qw-dash-api.knownby.work".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // The public host-default list lives in Postgres (host_config). Seed it
    // from QW_SERVERS on first boot; after that the row is authoritative and
    // QW_SERVERS is only a fallback if the row somehow goes missing.
    if store
        .host_servers()
        .await
        .unwrap_or_else(|e| panic!("read host_config: {e}"))
        .is_none()
    {
        if let Ok(clean) = qw_client_core::clean_server_list(servers.clone()) {
            store
                .set_host_servers(&clean)
                .await
                .unwrap_or_else(|e| panic!("seed host_config: {e}"));
            eprintln!("host_config seeded from QW_SERVERS: {clean:?}");
        }
    }

    let web = Arc::new(MuWeb::new(Arc::new(store), load_server_key(), servers));
    MuWeb::spawn_sweeper(web.clone());

    let addr = std::env::var("QW_WEB_ADDR").unwrap_or_else(|_| "127.0.0.1:8788".into());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    eprintln!("qw-web (multi-user) on http://{addr}");
    axum::serve(
        listener,
        router(web).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("serve");
}

fn load_server_key() -> [u8; SERVER_KEY_LEN] {
    // A file (the deployed form — key material out of any env listing),
    // else an inline value.
    let hex = match std::env::var("QW_SERVER_KEY_FILE") {
        Ok(path) => std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read QW_SERVER_KEY_FILE {path}: {e}")),
        Err(_) => std::env::var("QW_SERVER_KEY").expect(
            "QW_SERVER_KEY_FILE or QW_SERVER_KEY (64 hex chars) is required in multi-user mode",
        ),
    };
    let raw = hex::decode(hex.trim()).expect("server key must be 64 hex chars");
    raw.try_into()
        .unwrap_or_else(|v: Vec<u8>| panic!("server key must decode to 32 bytes, got {}", v.len()))
}

// --- state -------------------------------------------------------------

pub struct MuWeb {
    store: Arc<dyn AccountStore>,
    server_key: [u8; SERVER_KEY_LEN],
    servers: Vec<String>,
    live: Mutex<HashMap<String, Arc<Mutex<Live>>>>,
    limiter: RateLimiter,
}

struct Live {
    account_id: Uuid,
    session: Session,
    state: SealedHandle,
    /// A clone of the account master key, held for the session's life so
    /// the KEK-management routes can wrap it under a new factor. Zeroized
    /// with the rest of `Live` on logout / TTL / sweep.
    master: MasterKey,
    created: Instant,
    last_seen: Instant,
}

impl Live {
    fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.created) > ABS_TTL || now.duration_since(self.last_seen) > IDLE_TTL
    }
}

impl MuWeb {
    pub fn new(
        store: Arc<dyn AccountStore>,
        server_key: [u8; SERVER_KEY_LEN],
        servers: Vec<String>,
    ) -> Self {
        Self {
            store,
            server_key,
            servers,
            live: Mutex::new(HashMap::new()),
            limiter: RateLimiter::new(),
        }
    }

    pub fn spawn_sweeper(web: Arc<Self>) {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(SWEEP_EVERY);
            loop {
                tick.tick().await;
                let now = Instant::now();
                web.live.lock().unwrap().retain(|_, live| {
                    let g = live.lock().unwrap();
                    now.duration_since(g.created) <= ABS_TTL
                        && now.duration_since(g.last_seen) <= IDLE_TTL
                });
                web.limiter.sweep(RATE_LIMIT_SWEEP_MAX_AGE);
            }
        });
    }

    /// Park a freshly opened account as a live session; return its token
    /// and the identity view.
    fn start_session(&self, account_id: Uuid, open: OpenAccount) -> Result<(String, Value), ApiError> {
        let mut session =
            Session::with_identity(open.identity, Box::new(open.history), self.servers.clone());
        // Re-queue anything the last process signed but had not yet handed
        // to a server, and restore the poll cursors (`todo-impl.md` §7).
        session.restore_sync_state(&open.sync);
        let view = serde_json::to_value(session.identity_view().map_err(ApiError::internal)?)
            .map_err(ApiError::internal)?;
        let token = new_token();
        let now = Instant::now();
        self.live.lock().unwrap().insert(
            token.clone(),
            Arc::new(Mutex::new(Live {
                account_id,
                session,
                state: open.state,
                master: open.master,
                created: now,
                last_seen: now,
            })),
        );
        Ok((token, view))
    }

    /// Returns `(token, identity view, recovery code)`. The recovery code
    /// is minted here as a second KEK and handed back **once** — the UI
    /// shows it to write down; it is never stored, only its wrapping is.
    async fn register(
        &self,
        nick: &str,
        secret: &[u8],
    ) -> Result<(String, Value, String), ApiError> {
        let idx = accounts::nick_blind_index(&self.server_key, nick).map_err(ApiError::bad)?;
        if !self
            .store
            .by_blind_index(&idx)
            .await
            .map_err(ApiError::internal)?
            .is_empty()
        {
            return Err(ApiError::Conflict("that nickname is taken".into()));
        }

        let account_id = Uuid::new_v4();
        let master = MasterKey::random();
        let identity = Identity::generate();
        let blob = account_session::seal_new_account(&master, &account_id, &identity)
            .map_err(ApiError::internal)?;
        let mut wrappings = Vec::new();
        unlock::add_secret(&mut wrappings, &master, PRIMARY_TAG, secret)
            .map_err(ApiError::internal)?;
        let recovery_code = generate_recovery_code();
        unlock::add_secret(&mut wrappings, &master, RECOVERY_TAG, recovery_code.as_bytes())
            .map_err(ApiError::internal)?;
        let rec = AccountRecord::new(&self.server_key, account_id, nick, wrappings, blob)
            .map_err(ApiError::bad)?;
        match self.store.insert(&rec).await {
            Ok(()) => {}
            Err(AccountError::Conflict) => {
                return Err(ApiError::Conflict("that nickname is taken".into()))
            }
            Err(e) => return Err(ApiError::internal(e)),
        }

        let open = account_session::open_account(master, account_id, &rec.blob)
            .map_err(ApiError::internal)?;
        let (token, view) = self.start_session(account_id, open)?;
        Ok((token, view, recovery_code))
    }

    async fn login(&self, nick: &str, secret: &[u8]) -> Result<(String, Value, bool), ApiError> {
        let idx = accounts::nick_blind_index(&self.server_key, nick).map_err(|_| ApiError::Unauthorized)?;
        let records = self
            .store
            .by_blind_index(&idx)
            .await
            .map_err(ApiError::internal)?;

        for rec in records {
            let unlocked = match unlock::present_secret(&rec.wrappings, secret) {
                Ok(u) => u,
                Err(_) => continue, // wrong secret / corrupt row — try the next record
            };
            let stale = unlocked.stale;
            let open = account_session::open_account(unlocked.master, rec.account_id, &rec.blob)
                .map_err(ApiError::internal)?;

            // opportunistic housekeeping: drop stale wrappings if a fresh
            // one survives. Best-effort — a failure here does not fail login.
            let mut wrappings = rec.wrappings.clone();
            if !unlock::sweep_stale(&mut wrappings).is_empty() {
                let _ = self.store.set_wrappings(rec.account_id, &wrappings).await;
            }

            let (token, view) = self.start_session(rec.account_id, open)?;
            return Ok((token, view, stale));
        }
        Err(ApiError::Unauthorized)
    }

    /// The live session a cookie points at, if any — the KEK routes and
    /// `/session` resolve through this.
    fn resolve(&self, headers: &HeaderMap) -> Option<Arc<Mutex<Live>>> {
        let token = token_from(headers)?;
        self.live.lock().unwrap().get(&token).cloned()
    }
}

// --- routing ---------------------------------------------------------

pub fn router(web: Arc<MuWeb>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/api/identity", post(identity))
        .route("/api/follow", post(follow))
        .route("/api/sync_now", post(sync_now))
        .route("/api/taxonomy_leaves", post(taxonomy_leaves))
        .route("/api/profile_get", post(profile_get))
        .route("/api/profile_set", post(profile_set))
        .route("/api/request_recognition", post(request_recognition))
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
        .route("/api/servers", post(servers))
        .route("/api/set_servers", post(set_servers))
        .route("/api/admission", post(admission))
        .route("/api/set_admission", post(set_admission))
        .route("/api/propagation", post(propagation))
        .route("/api/set_propagation", post(set_propagation))
        .route("/api/client_prefs", post(client_prefs))
        .route("/api/set_client_prefs", post(set_client_prefs))
        .route("/session", get(session_info))
        .route("/api/kek_list", post(kek_list))
        .route("/api/kek_add", post(kek_add))
        .route("/api/kek_drop", post(kek_drop))
        .route("/api/kek_rotate", post(kek_rotate))
        .route("/api/kek_flag", post(kek_flag))
        .route("/api/kek_recovery", post(kek_recovery))
        .route("/api/seed_export", post(seed_export))
        .route("/servers", get(host_servers_get).post(host_servers_set))
        .with_state(web)
}

/// The public host-default coordination-server list — the "bootstrap"
/// endpoint a fresh web client and the mobile app pull to seed their own
/// list (`Session::bootstrap_servers`). Unauthenticated, `GET`, CORS-open.
async fn host_servers_get(State(web): State<Arc<MuWeb>>) -> Response {
    let list = web
        .store
        .host_servers()
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| web.servers.clone());
    (
        StatusCode::OK,
        [("access-control-allow-origin", "*")],
        Json(list),
    )
        .into_response()
}

/// Operator edit of the host default. Gated by a bearer token that must
/// match `QW_ADMIN_TOKEN`; if that env var is unset, host-default editing
/// is disabled and the seeded list stands.
async fn host_servers_set(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<SetServersBody>,
) -> Response {
    let Ok(token) = std::env::var("QW_ADMIN_TOKEN") else {
        return (
            StatusCode::FORBIDDEN,
            "host-default editing is disabled (QW_ADMIN_TOKEN unset)",
        )
            .into_response();
    };
    let presented = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if presented != Some(token.as_str()) {
        return (StatusCode::FORBIDDEN, "bad admin token").into_response();
    }
    match qw_client_core::clean_server_list(b.urls) {
        Ok(clean) => match web.store.set_host_servers(&clean).await {
            Ok(()) => (StatusCode::OK, Json(clean)).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// Enough for the shared UI to decide whether to show the sign-in panel
/// and the key-management section without waiting for a 401. The
/// single-user server answers the same shape with `multi_user: false`.
async fn session_info(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    let authed = web
        .resolve(&headers)
        .is_some_and(|live| !live.lock().unwrap().expired(Instant::now()));
    (
        StatusCode::OK,
        Json(json!({
            "multi_user": true,
            "authenticated": authed,
            "version": env!("CARGO_PKG_VERSION"),
        })),
    )
        .into_response()
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../../ui/index.html"))
}

#[derive(Deserialize)]
struct AuthBody {
    nick: String,
    secret: String,
}

async fn register(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    Json(b): Json<AuthBody>,
) -> Response {
    let ip = client_ip(&headers, connect);
    if let Err(r) = web
        .limiter
        .check(&format!("register:ip:{ip}"), REGISTER_LIMIT, REGISTER_WINDOW)
    {
        return too_many_requests(r.retry_after_secs);
    }
    match web.register(&b.nick, b.secret.as_bytes()).await {
        Ok((token, view, recovery_code)) => with_cookie(
            StatusCode::OK,
            json!({ "identity": view, "recovery_code": recovery_code }),
            &set_cookie(&token),
        ),
        Err(e) => e.into_response(),
    }
}

async fn login(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    Json(b): Json<AuthBody>,
) -> Response {
    let ip = client_ip(&headers, connect);
    if let Err(r) = web
        .limiter
        .check(&format!("login:ip:{ip}"), LOGIN_IP_LIMIT, LOGIN_WINDOW)
    {
        return too_many_requests(r.retry_after_secs);
    }
    // Keyed on the normalised nick (not the blind index — no server_key
    // needed here, and the nick already arrived in cleartext in the
    // request body) so a distributed brute force against one account is
    // capped even when spread across many source IPs.
    let acct = accounts::normalize_nick(&b.nick).unwrap_or_default();
    if let Err(r) = web
        .limiter
        .check(&format!("login:acct:{acct}"), LOGIN_ACCT_LIMIT, LOGIN_WINDOW)
    {
        return too_many_requests(r.retry_after_secs);
    }
    match web.login(&b.nick, b.secret.as_bytes()).await {
        Ok((token, view, key_stale)) => with_cookie(
            StatusCode::OK,
            json!({ "identity": view, "key_stale": key_stale }),
            &set_cookie(&token),
        ),
        Err(e) => e.into_response(),
    }
}

async fn logout(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    if let Some(token) = token_from(&headers) {
        web.live.lock().unwrap().remove(&token);
    }
    with_cookie(StatusCode::OK, json!({}), &clear_cookie())
}

async fn taxonomy_leaves() -> Response {
    (StatusCode::OK, Json(taxonomy::leaves().to_vec())).into_response()
}

async fn identity(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    reply(with_session(web, headers, |s| s.identity_view().map_err(text)).await)
}

#[derive(Deserialize)]
struct FollowBody {
    link: String,
}
async fn follow(State(web): State<Arc<MuWeb>>, headers: HeaderMap, Json(b): Json<FollowBody>) -> Response {
    reply(with_session(web, headers, move |s| s.follow(&b.link).map_err(text)).await)
}

async fn sync_now(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    reply(with_session(web, headers, |s| s.sync_now(&mut HttpMailbox::new()).map_err(text)).await)
}

async fn profile_get(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    reply(with_session(web, headers, |s| Ok::<_, String>(s.profile_view())).await)
}

#[derive(Deserialize)]
struct ProfileSetBody {
    edit: ProfileEdit,
}
async fn profile_set(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<ProfileSetBody>,
) -> Response {
    reply(with_session(web, headers, move |s| s.set_profile(b.edit).map_err(text)).await)
}

#[derive(Deserialize)]
struct RequestRecognitionBody {
    bureau_url: String,
    #[serde(default)]
    skill_tags: Vec<String>,
    #[serde(default)]
    append_public_profile: bool,
}
/// Ask a bureau to corroborate skills (NIP-QW15). Nothing is stored.
async fn request_recognition(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<RequestRecognitionBody>,
) -> Response {
    reply(
        with_session(web, headers, move |s| {
            s.request_recognition(&b.bureau_url, b.skill_tags, b.append_public_profile)
                .map_err(text)
        })
        .await,
    )
}

async fn negotiations(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    reply(with_session(web, headers, |s| Ok::<_, String>(s.negotiations())).await)
}

#[derive(Deserialize)]
struct ProposeBody {
    args: ProposeArgs,
}
async fn propose(State(web): State<Arc<MuWeb>>, headers: HeaderMap, Json(b): Json<ProposeBody>) -> Response {
    reply(with_session(web, headers, move |s| s.propose(b.args).map_err(text)).await)
}

#[derive(Deserialize)]
struct CounterBody {
    args: CounterArgs,
}
async fn counter(State(web): State<Arc<MuWeb>>, headers: HeaderMap, Json(b): Json<CounterBody>) -> Response {
    reply(with_session(web, headers, move |s| s.counter(b.args).map_err(text)).await)
}

#[derive(Deserialize)]
struct AcceptBody {
    args: AcceptArgs,
}
async fn accept_contract(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<AcceptBody>,
) -> Response {
    reply(with_session(web, headers, move |s| s.accept(b.args).map_err(text)).await)
}

#[derive(Deserialize)]
struct AnnotateBody {
    args: AnnotateArgs,
}
/// Attach a dispute annotation (kind 9030, NIP-QW04) to a contract.
async fn annotate(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<AnnotateBody>,
) -> Response {
    reply(with_session(web, headers, move |s| s.annotate(b.args).map_err(text)).await)
}

async fn contacts(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    reply(with_session(web, headers, |s| Ok::<_, String>(s.contacts())).await)
}

#[derive(Deserialize)]
struct FindBySkillBody {
    skill: String,
}
async fn find_by_skill(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<FindBySkillBody>,
) -> Response {
    reply(with_session(web, headers, move |s| s.find_by_skill(&b.skill).map_err(text)).await)
}

#[derive(Deserialize)]
struct ReferralResultsBody {
    qid: String,
}
async fn referral_results(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<ReferralResultsBody>,
) -> Response {
    reply(
        with_session(web, headers, move |s| {
            Ok::<_, String>(s.referral_results(&b.qid).to_vec())
        })
        .await,
    )
}

#[derive(Deserialize)]
struct ProfileOfBody {
    pubkey: String,
}
/// The full profile this session holds for another pubkey — for a
/// non-contact, whatever rode back on a referral answer (NIP-QW06).
/// `null` when none is held.
async fn profile_of(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<ProfileOfBody>,
) -> Response {
    reply(
        with_session(web, headers, move |s| {
            Ok::<_, String>(s.profile_of(&b.pubkey))
        })
        .await,
    )
}

#[derive(Deserialize)]
struct TrustBody {
    pubkey: String,
}
async fn trust(State(web): State<Arc<MuWeb>>, headers: HeaderMap, Json(b): Json<TrustBody>) -> Response {
    reply(with_session(web, headers, move |s| Ok::<_, String>(s.trust(&b.pubkey))).await)
}

async fn net_position(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    reply(with_session(web, headers, |s| Ok::<_, String>(s.net_position())).await)
}

/// The coordination servers this account syncs its mailbox against.
async fn servers(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    reply(with_session(web, headers, |s| Ok::<_, String>(s.servers().to_vec())).await)
}

#[derive(Deserialize)]
struct SetServersBody {
    urls: Vec<String>,
}
/// Replace the coordination-server list. Folded into the account's sealed
/// blob (`SyncState`), so it survives sign-out and a host restart.
async fn set_servers(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<SetServersBody>,
) -> Response {
    reply(
        with_session(web, headers, move |s| {
            s.set_servers(b.urls).map_err(text)?;
            Ok::<_, String>(s.servers().to_vec())
        })
        .await,
    )
}

fn admission_json(min: Option<f64>, lim: Option<f64>) -> serde_json::Value {
    json!({ "min_reputation": min, "position_limit": lim })
}

/// The offer-time admission pre-filter (abstract.md §"Basic Use Cases").
async fn admission(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    reply(
        with_session(web, headers, |s| {
            let (min, lim) = s.admission_policy();
            Ok::<_, String>(admission_json(min, lim))
        })
        .await,
    )
}

#[derive(Deserialize)]
struct SetAdmissionBody {
    #[serde(default)]
    min_reputation: Option<f64>,
    #[serde(default)]
    position_limit: Option<f64>,
}
/// Configure that pre-filter; folded into the account's sealed blob.
async fn set_admission(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<SetAdmissionBody>,
) -> Response {
    reply(
        with_session(web, headers, move |s| {
            s.set_admission_policy(b.min_reputation, b.position_limit)
                .map_err(text)?;
            let (min, lim) = s.admission_policy();
            Ok::<_, String>(admission_json(min, lim))
        })
        .await,
    )
}

/// The per-message-type broadcast propagation policy (NIP-QW14 §4).
async fn propagation(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    reply(
        with_session(web, headers, |s| Ok::<_, String>(s.propagation_config())).await,
    )
}

async fn client_prefs(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    reply(with_session(web, headers, |s| Ok::<_, String>(s.client_prefs())).await)
}
async fn set_client_prefs(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<ClientPrefs>,
) -> Response {
    reply(
        with_session(web, headers, move |s| {
            s.set_client_prefs(b).map_err(text)?;
            Ok::<_, String>(s.client_prefs())
        })
        .await,
    )
}

/// Replace that table; folded into the account's sealed blob.
async fn set_propagation(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<PropagationConfigWire>,
) -> Response {
    reply(
        with_session(web, headers, move |s| {
            s.set_propagation_config(b).map_err(text)?;
            Ok::<_, String>(s.propagation_config())
        })
        .await,
    )
}

// --- KEK management (server/multi-user.md "KEK set") ----------------
//
// The master key never moves: adding, dropping or rotating a factor only
// rewrites the `wrappings` list, O(1) metadata, no blob re-encryption and
// no identity change. Every op runs inside `spawn_blocking` under the
// per-session lock (Argon2id is CPU-bound, and holding the lock across
// the fetch → mutate → `set_wrappings` write serialises two tabs of one
// account) — the same shape as `with_session`.

/// Shortest user-supplied KEK secret, matched to the nickname floor. A
/// server-generated recovery code is exempt — it is high-entropy by
/// construction.
const MIN_KEK_SECRET_LEN: usize = 8;

/// Tag of the primary passphrase wrapping, minted at `register`. Refused
/// as a target for `kek_add` / `kek_drop` — change it with `kek_rotate`,
/// which is the passphrase-change path.
const PRIMARY_TAG: &str = "passphrase";
/// Tag of the recovery-code wrapping `kek_recovery` mints (and re-mints).
const RECOVERY_TAG: &str = "recovery";

fn unlock_err(e: UnlockError) -> ApiError {
    match e {
        UnlockError::DuplicateTag(t) => ApiError::Conflict(format!("a key tagged {t:?} already exists")),
        UnlockError::WouldOrphan => {
            ApiError::Domain("that is the only remaining key — add another before removing it".into())
        }
        UnlockError::UnknownTag(t) => ApiError::Domain(format!("no key tagged {t:?}")),
        UnlockError::NoMatch | UnlockError::Envelope(_) => ApiError::internal(e),
    }
}

/// A 128-bit code as `xxxxxxxx-xxxxxxxx-xxxxxxxx-xxxxxxxx` — written on
/// paper once, entered verbatim at the sign-in box like any passphrase.
fn generate_recovery_code() -> String {
    let mut raw = [0u8; 16];
    OsRng.fill_bytes(&mut raw);
    let hex = hex::encode(raw);
    hex.as_bytes()
        .chunks(8)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join("-")
}

/// Run a `wrappings` mutation under the per-session lock and persist it,
/// answering with the refreshed [`unlock::enumerate`] view. `f` gets the
/// master key (for the wrapping ops that need it) and the current set.
async fn kek_op<F>(web: Arc<MuWeb>, headers: HeaderMap, f: F) -> Response
where
    F: FnOnce(&MasterKey, Vec<Wrapping>) -> Result<Vec<Wrapping>, ApiError> + Send + 'static,
{
    let Some(live) = web.resolve(&headers) else {
        return ApiError::Unauthorized.into_response();
    };
    let store = web.store.clone();
    let handle = tokio::runtime::Handle::current();

    let joined = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        let mut g = live.lock().unwrap();
        if g.expired(Instant::now()) {
            return Err(ApiError::Unauthorized);
        }
        g.last_seen = Instant::now();

        let rec = handle
            .block_on(store.by_account_id(g.account_id))
            .map_err(ApiError::internal)?
            .ok_or(ApiError::Unauthorized)?;
        let next = f(&g.master, rec.wrappings)?;
        handle
            .block_on(store.set_wrappings(g.account_id, &next))
            .map_err(|e| ApiError::Internal(format!("save failed: {e}")))?;
        Ok(serde_json::to_value(unlock::enumerate(&next)).unwrap_or(Value::Null))
    })
    .await;

    match joined {
        Ok(Ok(v)) => (StatusCode::OK, Json(v)).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(_) => ApiError::Internal("kek task panicked".into()).into_response(),
    }
}

async fn kek_list(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    let Some(live) = web.resolve(&headers) else {
        return ApiError::Unauthorized.into_response();
    };
    let account_id = {
        let mut g = live.lock().unwrap();
        if g.expired(Instant::now()) {
            return ApiError::Unauthorized.into_response();
        }
        g.last_seen = Instant::now();
        g.account_id
    };
    match web.store.by_account_id(account_id).await {
        Ok(Some(rec)) => (StatusCode::OK, Json(unlock::enumerate(&rec.wrappings))).into_response(),
        Ok(None) => ApiError::Unauthorized.into_response(),
        Err(e) => ApiError::internal(e).into_response(),
    }
}

#[derive(Deserialize)]
struct KekAddBody {
    tag: String,
    secret: String,
}
async fn kek_add(State(web): State<Arc<MuWeb>>, headers: HeaderMap, Json(b): Json<KekAddBody>) -> Response {
    let KekAddBody { tag, secret } = b;
    if tag == PRIMARY_TAG {
        return ApiError::BadRequest(format!(
            "{PRIMARY_TAG:?} is the primary passphrase — use rotate to change it"
        ))
        .into_response();
    }
    if tag.trim().is_empty() {
        return ApiError::BadRequest("a key needs a label".into()).into_response();
    }
    if secret.len() < MIN_KEK_SECRET_LEN {
        return ApiError::BadRequest(format!(
            "the secret must be at least {MIN_KEK_SECRET_LEN} characters"
        ))
        .into_response();
    }
    kek_op(web, headers, move |master, mut w| {
        unlock::add_secret(&mut w, master, &tag, secret.as_bytes()).map_err(unlock_err)?;
        Ok(w)
    })
    .await
}

#[derive(Deserialize)]
struct KekTagBody {
    tag: String,
}
async fn kek_drop(State(web): State<Arc<MuWeb>>, headers: HeaderMap, Json(b): Json<KekTagBody>) -> Response {
    let tag = b.tag;
    if tag == PRIMARY_TAG {
        return ApiError::BadRequest(format!("{PRIMARY_TAG:?} cannot be removed")).into_response();
    }
    kek_op(web, headers, move |_master, mut w| {
        unlock::drop_tag(&mut w, &tag).map_err(unlock_err)?;
        Ok(w)
    })
    .await
}

async fn kek_flag(State(web): State<Arc<MuWeb>>, headers: HeaderMap, Json(b): Json<KekTagBody>) -> Response {
    let tag = b.tag;
    kek_op(web, headers, move |_master, mut w| {
        unlock::flag_for_rotation(&mut w, &tag).map_err(unlock_err)?;
        Ok(w)
    })
    .await
}

#[derive(Deserialize)]
struct KekRotateBody {
    tag: String,
    new_secret: String,
}
async fn kek_rotate(
    State(web): State<Arc<MuWeb>>,
    headers: HeaderMap,
    Json(b): Json<KekRotateBody>,
) -> Response {
    let KekRotateBody { tag, new_secret } = b;
    if new_secret.len() < MIN_KEK_SECRET_LEN {
        return ApiError::BadRequest(format!(
            "the new secret must be at least {MIN_KEK_SECRET_LEN} characters"
        ))
        .into_response();
    }
    kek_op(web, headers, move |master, mut w| {
        unlock::rotate_secret(&mut w, master, &tag, new_secret.as_bytes()).map_err(unlock_err)?;
        Ok(w)
    })
    .await
}

/// Mint (or re-mint) the recovery-code wrapping and hand the plaintext
/// code back **once** — it is never stored, only its wrapping is. Losing
/// every other factor, this is the way back in; `present_secret` already
/// accepts it at `/auth/login` like any secret.
async fn kek_recovery(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    let Some(live) = web.resolve(&headers) else {
        return ApiError::Unauthorized.into_response();
    };
    let store = web.store.clone();
    let handle = tokio::runtime::Handle::current();
    let code = generate_recovery_code();
    let code_for_task = code.clone();

    let joined = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        let mut g = live.lock().unwrap();
        if g.expired(Instant::now()) {
            return Err(ApiError::Unauthorized);
        }
        g.last_seen = Instant::now();

        let rec = handle
            .block_on(store.by_account_id(g.account_id))
            .map_err(ApiError::internal)?
            .ok_or(ApiError::Unauthorized)?;
        let mut w = rec.wrappings;
        if w.iter().any(|x| x.tag == RECOVERY_TAG) {
            unlock::rotate_secret(&mut w, &g.master, RECOVERY_TAG, code_for_task.as_bytes())
                .map_err(unlock_err)?;
        } else {
            unlock::add_secret(&mut w, &g.master, RECOVERY_TAG, code_for_task.as_bytes())
                .map_err(unlock_err)?;
        }
        handle
            .block_on(store.set_wrappings(g.account_id, &w))
            .map_err(|e| ApiError::Internal(format!("save failed: {e}")))?;
        Ok(serde_json::to_value(unlock::enumerate(&w)).unwrap_or(Value::Null))
    })
    .await;

    match joined {
        Ok(Ok(keys)) => (StatusCode::OK, Json(json!({ "code": code, "keys": keys }))).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(_) => ApiError::Internal("kek task panicked".into()).into_response(),
    }
}

/// Hand back the 32-byte identity secret as hex — the same spelling as a
/// single-user `identity.key`, so it can be carried to the Tauri app or a
/// personal `qw-web` and survives total loss of this host's blob. The
/// account *is* this key (§2); the UI reveals it only on an explicit tap
/// and never implies anyone can help if it is lost.
async fn seed_export(State(web): State<Arc<MuWeb>>, headers: HeaderMap) -> Response {
    let Some(live) = web.resolve(&headers) else {
        return ApiError::Unauthorized.into_response();
    };
    let (secret_hex, pubkey) = {
        let mut g = live.lock().unwrap();
        if g.expired(Instant::now()) {
            return ApiError::Unauthorized.into_response();
        }
        g.last_seen = Instant::now();
        (g.session.identity_secret_hex(), g.session.pubkey_hex())
    };
    (
        StatusCode::OK,
        Json(json!({ "secret_hex": secret_hex, "pubkey": pubkey })),
    )
        .into_response()
}

// --- session resolution + blob flush --------------------------------

/// Resolve the session from the cookie, run `f` on the blocking pool under
/// the per-session lock, and — still holding that lock — flush the
/// re-sealed blob to the store, so two concurrent ops on one session
/// persist in order.
async fn with_session<T, F>(web: Arc<MuWeb>, headers: HeaderMap, f: F) -> Result<T, ApiError>
where
    F: FnOnce(&mut Session) -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    let token = token_from(&headers).ok_or(ApiError::Unauthorized)?;
    let live = web
        .live
        .lock()
        .unwrap()
        .get(&token)
        .cloned()
        .ok_or(ApiError::Unauthorized)?;
    let store = web.store.clone();
    let handle = tokio::runtime::Handle::current();

    let joined = tokio::task::spawn_blocking(move || -> Result<T, ApiError> {
        let mut g = live.lock().unwrap();
        let now = Instant::now();
        if now.duration_since(g.created) > ABS_TTL || now.duration_since(g.last_seen) > IDLE_TTL {
            return Err(ApiError::Unauthorized);
        }
        g.last_seen = now;

        let out = f(&mut g.session).map_err(ApiError::Domain)?;

        let pending = {
            let s = g.state.lock().unwrap();
            s.dirty.then(|| s.blob.clone())
        };
        if let Some(blob) = pending {
            handle
                .block_on(store.set_blob(g.account_id, &blob))
                .map_err(|e| ApiError::Internal(format!("save failed: {e}")))?;
            g.state.lock().unwrap().dirty = false;
        }
        Ok(out)
    })
    .await
    .map_err(|_| ApiError::Internal("op task panicked".into()))?;

    joined
}

// --- cookies + plumbing --------------------------------------------

fn new_token() -> String {
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    hex::encode(raw)
}

fn set_cookie(token: &str) -> String {
    format!(
        "{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}; Secure",
        ABS_TTL.as_secs()
    )
}

fn clear_cookie() -> String {
    format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")
}

/// The caller's address, for rate limiting only (never trust this for
/// authorization). `qw.knownby.work` sits behind a Cloudflare tunnel, which
/// forwards the edge's `CF-Connecting-IP` — checked first since it names
/// the actual client rather than the tunnel's loopback socket. Falls back
/// to `X-Forwarded-For` (first hop), then the TCP peer address for a
/// direct/local connection, then a shared bucket if neither is available.
fn client_ip(headers: &HeaderMap, connect: Option<ConnectInfo<SocketAddr>>) -> String {
    if let Some(ip) = headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return ip.to_string();
    }
    if let Some(ip) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return ip.to_string();
    }
    if let Some(ConnectInfo(addr)) = connect {
        return addr.ip().to_string();
    }
    "unknown".to_string()
}

fn too_many_requests(retry_after_secs: u64) -> Response {
    let mut r = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({ "error": "too many attempts, try again later" })),
    )
        .into_response();
    if let Ok(v) = header::HeaderValue::from_str(&retry_after_secs.to_string()) {
        r.headers_mut().insert(header::RETRY_AFTER, v);
    }
    r
}

fn token_from(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    cookies
        .split(';')
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

fn with_cookie(code: StatusCode, body: Value, cookie: &str) -> Response {
    let mut r = (code, Json(body)).into_response();
    if let Ok(v) = header::HeaderValue::from_str(cookie) {
        r.headers_mut().insert(header::SET_COOKIE, v);
    }
    r
}

fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn reply<T: Serialize>(r: Result<T, ApiError>) -> Response {
    match r {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Debug)]
enum ApiError {
    Unauthorized,
    BadRequest(String),
    Conflict(String),
    Domain(String),
    Internal(String),
}

impl ApiError {
    fn bad(e: impl std::fmt::Display) -> Self {
        Self::BadRequest(e.to_string())
    }
    fn internal(e: impl std::fmt::Display) -> Self {
        Self::Internal(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, msg) = match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "not signed in".to_string()),
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::Conflict(m) => (StatusCode::CONFLICT, m),
            Self::Domain(m) => (StatusCode::UNPROCESSABLE_ENTITY, m),
            Self::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (code, Json(json!({ "error": msg }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::MemAccountStore;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    fn web() -> Arc<MuWeb> {
        Arc::new(MuWeb::new(
            Arc::new(MemAccountStore::default()),
            [9u8; SERVER_KEY_LEN],
            vec![], // no coordination servers in the tests
        ))
    }

    async fn call(app: &Router, path: &str, cookie: Option<&str>, body: Value) -> (StatusCode, Value, Option<String>) {
        let mut req = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json");
        if let Some(c) = cookie {
            req = req.header("cookie", format!("{COOKIE}={c}"));
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let set_cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), set_cookie)
    }

    fn token_of(set_cookie: &str) -> String {
        set_cookie
            .split(';')
            .next()
            .and_then(|kv| kv.split_once('='))
            .map(|(_, v)| v.to_string())
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serves_the_ui_with_the_auth_panel() {
        let res = router(web())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let html = String::from_utf8(
            to_bytes(res.into_body(), usize::MAX).await.unwrap().to_vec(),
        )
        .unwrap();
        assert!(html.contains("<title>QW</title>"));
        assert!(html.contains(r#"id="auth""#), "the sign-in panel is in the shared UI");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn register_then_use_a_scoped_route() {
        let app = router(web());

        let (st, _v, sc) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "alice.doe", "secret": "correct horse battery" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let token = token_of(&sc.unwrap());

        // an unscoped call is 401
        let (st, _, _) = call(&app, "/api/identity", None, json!({})).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        // with the cookie it works
        let (st, id, _) = call(&app, "/api/identity", Some(&token), json!({})).await;
        assert_eq!(st, StatusCode::OK);
        assert!(id["npub"].as_str().unwrap().starts_with("npub1"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_write_persists_and_a_reopened_session_sees_it() {
        let store = Arc::new(MemAccountStore::default());
        let web = Arc::new(MuWeb::new(store.clone(), [3u8; SERVER_KEY_LEN], vec![]));
        let app = router(web.clone());

        let (_, _, sc) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "bob.somebody", "secret": "hunter2 hunter2" }),
        )
        .await;
        let token = token_of(&sc.unwrap());

        let (st, _, _) = call(
            &app,
            "/api/profile_set",
            Some(&token),
            json!({ "edit": { "display_name": "Bob", "skills": [{ "tag": "Rust Lang" }] } }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);

        // drop every live session, then log back in — the profile must
        // come from the persisted blob, not RAM.
        web.live.lock().unwrap().clear();

        let (st, out, sc2) = call(
            &app,
            "/auth/login",
            None,
            json!({ "nick": "BOB.somebody", "secret": "hunter2 hunter2" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(out["key_stale"], json!(false));
        let token2 = token_of(&sc2.unwrap());

        let (_, prof, _) = call(&app, "/api/profile_get", Some(&token2), json!({})).await;
        assert_eq!(prof["display_name"], "Bob");
        assert_eq!(prof["tags"][0], "it/backend/languages#rust");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_queued_event_survives_a_host_restart() {
        let store = Arc::new(MemAccountStore::default());
        let web = Arc::new(MuWeb::new(store.clone(), [4u8; SERVER_KEY_LEN], vec![]));
        let app = router(web.clone());

        let (_, _, sc) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "frank.person", "secret": "queue-me-a-secret" }),
        )
        .await;
        let token = token_of(&sc.unwrap());

        // The offer is `p`-tagged to the counterparty, so it only ever
        // lives in this account's outbox — a mailbox poll never returns it.
        let (st, _, _) = call(
            &app,
            "/api/propose",
            Some(&token),
            json!({ "args": {
                "counterparty": "a".repeat(64),
                "from_introduction": null,
                "terms": { "skill_tags": ["rust"], "hours": 8.0, "rate": 40.0,
                           "ko": null, "km": null, "terms": "sprint 12" }
            }}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);

        // No coordination servers configured, so a sync cannot deliver it.
        let (_, before, _) = call(&app, "/api/sync_now", Some(&token), json!({})).await;
        assert_eq!(before["still_queued"], json!(1));

        // Every warm session dies with the process.
        web.live.lock().unwrap().clear();

        let (st, _, sc2) = call(
            &app,
            "/auth/login",
            None,
            json!({ "nick": "frank.person", "secret": "queue-me-a-secret" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let token2 = token_of(&sc2.unwrap());

        let (_, after, _) = call(&app, "/api/sync_now", Some(&token2), json!({})).await;
        assert_eq!(
            after["still_queued"],
            json!(1),
            "the queued offer was rehydrated from the account blob"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wrong_secret_and_unknown_nick_are_both_401() {
        let app = router(web());
        call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "carol.person", "secret": "the-right-secret" }),
        )
        .await;

        let (st, _, _) = call(
            &app,
            "/auth/login",
            None,
            json!({ "nick": "carol.person", "secret": "the-wrong-secret" }),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        let (st, _, _) = call(
            &app,
            "/auth/login",
            None,
            json!({ "nick": "nobody.here", "secret": "whatever" }),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_duplicate_nick_is_409_and_a_short_nick_is_400() {
        let app = router(web());
        let (st, _, _) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "dave.person", "secret": "s3cr3t-value" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);

        let (st, _, _) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "DAVE.person", "secret": "another" }),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);

        let (st, _, _) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "short", "secret": "x" }),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn logout_drops_the_session() {
        let app = router(web());
        let (_, _, sc) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "erin.person", "secret": "another-secret-x" }),
        )
        .await;
        let token = token_of(&sc.unwrap());

        assert_eq!(
            call(&app, "/api/identity", Some(&token), json!({})).await.0,
            StatusCode::OK
        );
        call(&app, "/auth/logout", Some(&token), json!({})).await;
        assert_eq!(
            call(&app, "/api/identity", Some(&token), json!({})).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    /// The same round-trip as `a_write_persists_and_a_reopened_session…`
    /// but against real Postgres — exercises `handle.block_on` driving a
    /// `sqlx` `set_blob` from inside `spawn_blocking`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "needs QW_TEST_DATABASE_URL, e.g. postgres:///qw_ci?host=/var/run/postgresql&user=$USER"]
    async fn pg_write_persists_across_a_relogin() {
        let Ok(url) = std::env::var("QW_TEST_DATABASE_URL") else {
            return;
        };
        accounts::migrate(&url).await.unwrap();
        let store = Arc::new(PgAccountStore::connect(&url).await.unwrap());
        let web = Arc::new(MuWeb::new(store, [5u8; SERVER_KEY_LEN], vec![]));
        let app = router(web.clone());

        let nick = format!("pg.user.{}", Uuid::new_v4().simple());
        let (_, _, sc) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": nick, "secret": "pg-round-trip-secret" }),
        )
        .await;
        let token = token_of(&sc.unwrap());

        let (st, _, _) = call(
            &app,
            "/api/profile_set",
            Some(&token),
            json!({ "edit": { "display_name": "PG", "skills": [{ "tag": "Rust Lang" }] } }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);

        web.live.lock().unwrap().clear();
        let (st, _, sc2) = call(
            &app,
            "/auth/login",
            None,
            json!({ "nick": nick, "secret": "pg-round-trip-secret" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let token2 = token_of(&sc2.unwrap());

        let (_, prof, _) = call(&app, "/api/profile_get", Some(&token2), json!({})).await;
        assert_eq!(prof["display_name"], "PG");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn register_is_rate_limited_per_source() {
        let app = router(web());
        // no ConnectInfo and no forwarded-for header in these test
        // requests, so every call below lands in the same `client_ip`
        // bucket — exactly what lets this test hammer one limiter key.
        for i in 0..REGISTER_LIMIT {
            let (st, _, _) = call(
                &app,
                "/auth/register",
                None,
                json!({ "nick": format!("rate.user.{i}"), "secret": "a fine secret 1" }),
            )
            .await;
            assert_eq!(st, StatusCode::OK, "attempt {i} should be under the limit");
        }

        let req = Request::builder()
            .method("POST")
            .uri("/auth/register")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "nick": "rate.user.overflow", "secret": "a fine secret 1" }).to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(res.headers().get(header::RETRY_AFTER).is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn login_is_rate_limited_per_account_even_across_ips() {
        let app = router(web());
        call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "brute.force.me", "secret": "the-real-secret-1" }),
        )
        .await;

        for i in 0..LOGIN_ACCT_LIMIT {
            let (st, _, _) = call(
                &app,
                "/auth/login",
                None,
                json!({ "nick": "brute.force.me", "secret": "guess-number" }),
            )
            .await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "wrong-secret attempt {i}");
        }

        // the account limit trips even with the *correct* secret now.
        let (st, _, _) = call(
            &app,
            "/auth/login",
            None,
            json!({ "nick": "brute.force.me", "secret": "the-real-secret-1" }),
        )
        .await;
        assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_accounts_are_isolated() {
        let app = router(web());
        let (_, _, a) = call(&app, "/auth/register", None, json!({ "nick": "one.person", "secret": "aaaa-secret-11" })).await;
        let (_, _, b) = call(&app, "/auth/register", None, json!({ "nick": "two.person", "secret": "bbbb-secret-22" })).await;
        let ta = token_of(&a.unwrap());
        let tb = token_of(&b.unwrap());

        let (_, ida, _) = call(&app, "/api/identity", Some(&ta), json!({})).await;
        let (_, idb, _) = call(&app, "/api/identity", Some(&tb), json!({})).await;
        assert_ne!(ida["npub"], idb["npub"]);
    }

    // --- KEK management -------------------------------------------------

    async fn get(app: &Router, path: &str, cookie: Option<&str>) -> (StatusCode, Value) {
        let mut req = Request::builder().method("GET").uri(path);
        if let Some(c) = cookie {
            req = req.header("cookie", format!("{COOKIE}={c}"));
        }
        let res = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_probe_reports_login_state() {
        let app = router(web());
        let (st, before) = get(&app, "/session", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(before["multi_user"], json!(true));
        assert_eq!(before["authenticated"], json!(false));
        // The deploy script asserts the running process is the one it just
        // built by comparing this against app/server/Cargo.toml.
        assert_eq!(before["version"], json!(env!("CARGO_PKG_VERSION")));

        let (_, _, sc) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "probe.person", "secret": "a-fine-secret-11" }),
        )
        .await;
        let token = token_of(&sc.unwrap());
        let (_, after) = get(&app, "/session", Some(&token)).await;
        assert_eq!(after["multi_user"], json!(true));
        assert_eq!(after["authenticated"], json!(true));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kek_add_lets_a_second_secret_sign_in_and_drop_reverses_it() {
        let app = router(web());
        let (_, _, sc) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "grace.person", "secret": "first-secret-11" }),
        )
        .await;
        let token = token_of(&sc.unwrap());

        // register mints two: the passphrase and a recovery code.
        let (st, keys, _) = call(&app, "/api/kek_list", Some(&token), json!({})).await;
        assert_eq!(st, StatusCode::OK);
        let tags = |v: &Value| {
            v.as_array()
                .unwrap()
                .iter()
                .map(|k| k["tag"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(tags(&keys), vec!["passphrase", "recovery"]);

        // add a third: a second passphrase
        let (st, keys, _) = call(
            &app,
            "/api/kek_add",
            Some(&token),
            json!({ "tag": "laptop", "secret": "second-secret-22" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{keys}");
        assert_eq!(keys.as_array().unwrap().len(), 3);

        // it works as a login secret, on a fresh session
        app_relogin_ok(&app, "grace.person", "second-secret-22").await;

        // the primary is protected; a bad tag is a domain error
        assert_eq!(
            call(&app, "/api/kek_add", Some(&token), json!({ "tag": "passphrase", "secret": "xxxxxxxx" })).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(&app, "/api/kek_drop", Some(&token), json!({ "tag": "passphrase" })).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(&app, "/api/kek_drop", Some(&token), json!({ "tag": "nope" })).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        // drop the one we added
        let (st, keys, _) = call(&app, "/api/kek_drop", Some(&token), json!({ "tag": "laptop" })).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(tags(&keys), vec!["passphrase", "recovery"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kek_recovery_mints_a_code_that_logs_in_and_is_re_mintable() {
        let app = router(web());
        let (_, _, sc) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "heidi.person", "secret": "primary-secret-1" }),
        )
        .await;
        let token = token_of(&sc.unwrap());

        // register already minted one; this rotates it to a fresh code.
        let (st, out, _) = call(&app, "/api/kek_recovery", Some(&token), json!({})).await;
        assert_eq!(st, StatusCode::OK);
        let code = out["code"].as_str().unwrap().to_string();
        assert_eq!(code.len(), 35, "xxxxxxxx-xxxxxxxx-xxxxxxxx-xxxxxxxx");
        assert_eq!(
            out["keys"].as_array().unwrap().iter().filter(|k| k["tag"] == "recovery").count(),
            1,
            "still exactly one recovery row"
        );

        app_relogin_ok(&app, "heidi.person", &code).await;

        // re-mint: a new code, and the old one stops working
        let (_, out2, _) = call(&app, "/api/kek_recovery", Some(&token), json!({})).await;
        let code2 = out2["code"].as_str().unwrap().to_string();
        assert_ne!(code, code2);
        app_relogin_ok(&app, "heidi.person", &code2).await;
        let (st, _, _) = call(
            &app,
            "/auth/login",
            None,
            json!({ "nick": "heidi.person", "secret": code }),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "the superseded code no longer unlocks");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kek_rotate_is_the_passphrase_change_path() {
        let app = router(web());
        let (_, _, sc) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "ivan.person", "secret": "old-passphrase-1" }),
        )
        .await;
        let token = token_of(&sc.unwrap());

        let (st, _, _) = call(
            &app,
            "/api/kek_rotate",
            Some(&token),
            json!({ "tag": "passphrase", "new_secret": "new-passphrase-2" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);

        app_relogin_ok(&app, "ivan.person", "new-passphrase-2").await;
        let (st, _, _) = call(
            &app,
            "/auth/login",
            None,
            json!({ "nick": "ivan.person", "secret": "old-passphrase-1" }),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kek_routes_are_401_without_a_session() {
        let app = router(web());
        for (path, body) in [
            ("/api/kek_list", json!({})),
            ("/api/kek_add", json!({ "tag": "x", "secret": "yyyyyyyy" })),
            ("/api/kek_drop", json!({ "tag": "x" })),
            ("/api/kek_rotate", json!({ "tag": "x", "new_secret": "yyyyyyyy" })),
            ("/api/kek_recovery", json!({})),
            ("/api/seed_export", json!({})),
        ] {
            assert_eq!(
                call(&app, path, None, body).await.0,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn register_hands_back_a_recovery_code_that_logs_in() {
        let app = router(web());
        let (st, body, _) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "judy.person", "secret": "the-first-secret" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(body["identity"]["npub"].as_str().unwrap().starts_with("npub1"));
        let code = body["recovery_code"].as_str().unwrap().to_string();
        assert_eq!(code.len(), 35);
        // it is a real KEK: it opens the account on a fresh session
        app_relogin_ok(&app, "judy.person", &code).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn seed_export_returns_the_portable_identity_key() {
        let app = router(web());
        let (_, _, sc) = call(
            &app,
            "/auth/register",
            None,
            json!({ "nick": "karl.person", "secret": "a-solid-secret-x" }),
        )
        .await;
        let token = token_of(&sc.unwrap());

        let (id_st, id, _) = call(&app, "/api/identity", Some(&token), json!({})).await;
        assert_eq!(id_st, StatusCode::OK);

        let (st, out, _) = call(&app, "/api/seed_export", Some(&token), json!({})).await;
        assert_eq!(st, StatusCode::OK);
        let hex = out["secret_hex"].as_str().unwrap();
        assert_eq!(hex.len(), 64, "32-byte secret, hex");
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
        // the exported key belongs to this account
        assert_eq!(out["pubkey"], id["pubkey"]);

        // and it reconstructs the same identity a single-user Vault would load
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
        }
        let reconstructed = Identity::from_secret_bytes(bytes).unwrap();
        assert_eq!(reconstructed.nostr_pubkey_hex(), id["pubkey"].as_str().unwrap());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn public_servers_endpoint_reads_the_db_and_the_admin_post_is_gated() {
        let store = Arc::new(MemAccountStore::default());
        store
            .set_host_servers(&["https://relay.a".into(), "https://relay.b".into()])
            .await
            .unwrap();
        let app = router(Arc::new(MuWeb::new(store, [8u8; SERVER_KEY_LEN], vec![])));

        // unauthenticated read of the host default
        let (st, body) = get(&app, "/servers", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body, json!(["https://relay.a", "https://relay.b"]));

        // POST is refused without QW_ADMIN_TOKEN set (it is not, in tests)
        let (st, _, _) = call(&app, "/servers", None, json!({ "urls": ["https://x"] })).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }

    /// Drop every warm session, then confirm `nick` + `secret` opens a new one.
    async fn app_relogin_ok(app: &Router, nick: &str, secret: &str) {
        let (st, _, sc) = call(
            app,
            "/auth/login",
            None,
            json!({ "nick": nick, "secret": secret }),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "relogin for {nick} should succeed");
        let token = token_of(&sc.unwrap());
        assert_eq!(
            call(app, "/api/identity", Some(&token), json!({})).await.0,
            StatusCode::OK
        );
    }
}
