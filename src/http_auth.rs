//! `http` mode's authentication edge: bearer-token extraction, per-session
//! credential binding, and the task-local that carries the token into rmcp's
//! session factory.
//!
//! # Why session binding exists
//!
//! rmcp's `StreamableHttpService` authorizes every request *after* `initialize`
//! on the `Mcp-Session-Id` header alone — it re-checks nothing else (see
//! `handle_post` in `rmcp::transport::streamable_http_server::tower`). Since a
//! session owns one `EngramoClient` built from the token presented at
//! `initialize`, anyone replaying a live session id with *any* bearer token
//! would otherwise act as that session's owner, with their EngrAmo token, and
//! would keep doing so after the owner rotated it. [`SessionTokens`] closes that
//! by remembering which token opened each session and rejecting a mismatch
//! (or an unbound session id) with a 404 byte-identical to rmcp's own
//! unknown-session 404, so a probe learns nothing about which session ids are live.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, RandomState};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::{
    extract::{Request, State},
    http::{
        HeaderValue, Method, StatusCode,
        header::{AUTHORIZATION, RETRY_AFTER, WWW_AUTHENTICATE},
    },
    middleware::Next,
    response::{IntoResponse, Response},
};
use rmcp::transport::streamable_http_server::session::{
    SessionManager, local::LocalSessionManager,
};
use tokio::sync::{RwLock, Semaphore};

use crate::client::{EngramoClient, HardenedClient};
use crate::error::ApiError;

/// Header rmcp uses to carry the MCP session id, on both requests and the
/// `initialize` response.
const HEADER_SESSION_ID: &str = "mcp-session-id";

/// Upper bound on an accepted bearer token. EngrAmo API tokens are far shorter;
/// this only stops a caller from making the server allocate state for a
/// megabyte-long "token" before anything else looks at it.
const MAX_TOKEN_LEN: usize = 4096;

/// How long rmcp keeps an idle session alive. Set explicitly on the session manager
/// in `build_app` (rather than relying on rmcp's default) so [`BINDING_TTL`] and the
/// per-token eviction below are derived from the value rmcp actually uses.
pub const SESSION_KEEP_ALIVE: Duration = Duration::from_secs(300);

/// Slack on top of the keep-alive window for in-flight requests and handler-side
/// activity (rmcp also resets its idle timer on handler events such as progress
/// notifications, which never pass through this module's `last_seen`).
const BINDING_SLACK: Duration = Duration::from_secs(60);

/// How long a session→token binding is kept after its last client request. An
/// expired binding locks the session's owner out (the check fails closed with a 404),
/// so it must never expire before rmcp could still consider the session live:
/// at least twice the keep-alive plus slack.
const BINDING_TTL: Duration =
    Duration::from_secs(2 * SESSION_KEEP_ALIVE.as_secs() + BINDING_SLACK.as_secs());

/// A binding idle this long belongs to a session rmcp has almost certainly closed; only
/// such bindings are evicted to make room for a token's new session.
const EVICTABLE_IDLE: Duration =
    Duration::from_secs(SESSION_KEEP_ALIVE.as_secs() + BINDING_SLACK.as_secs());

const _: () = assert!(
    BINDING_TTL.as_secs() >= EVICTABLE_IDLE.as_secs(),
    "BINDING_TTL must outlive the eviction threshold"
);

/// Minimum spacing between table-wide prunes triggered from the capacity pre-check
/// (the prune in `bind` always runs; it is already authenticated and probe-bounded).
const PRUNE_INTERVAL: Duration = Duration::from_secs(30);

/// Cap on live bindings per bearer token, so one account cannot fill the table
/// and block session opens for everyone else.
const MAX_SESSIONS_PER_TOKEN: usize = 32;

/// Cap on concurrent session-open probes (`GET /me`) against the EngrAmo API, so
/// unauthenticated traffic cannot become an unbounded flood upstream.
const MAX_CONCURRENT_PROBES: usize = 32;

/// Upper bound on one session-open probe, so a slow upstream cannot hold a probe
/// permit for the client's full request timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a token the upstream rejected is remembered, so repeats skip the probe.
const NEGATIVE_TTL: Duration = Duration::from_secs(60);

/// Cap on remembered rejected tokens (8 bytes + an `Instant` each).
const MAX_NEGATIVE_ENTRIES: usize = 4096;

/// Largest request body accepted in `http` mode. rmcp buffers the whole body in memory
/// before parsing it (`expect_json`) and axum's `DefaultBodyLimit` does not apply to a
/// `fallback_service`, so without this an unauthenticated caller can drive the process
/// out of memory with one POST. 16 MiB leaves room for the ~13.4 MiB of base64 a maximum
/// 10 MB `upload_media` produces, plus JSON-RPC framing.
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// An `initialize` request is a few KB; nothing larger is a legitimate session open.
const MAX_INITIALIZE_BYTES: usize = 64 * 1024;

/// Upper bound on reading a session-less POST body, so a slow-drip upload cannot pin memory.
const INITIALIZE_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// True when `to_bytes` failed because the body exceeded the limit (vs. a transport error).
fn is_length_limit_error(e: &axum::Error) -> bool {
    std::error::Error::source(e).is_some_and(|s| s.to_string().contains("length limit exceeded"))
}

/// How long a session open waits for a free probe slot before answering 503.
#[cfg(not(test))]
const PROBE_QUEUE_WAIT: Duration = Duration::from_secs(2);
#[cfg(test)]
const PROBE_QUEUE_WAIT: Duration = Duration::from_millis(50);

/// Hard cap on tracked sessions. Reached only under a session-creation flood
/// (every entry is ~100 bytes); new sessions are refused with 503 rather than
/// silently dropping bindings, which would weaken the check this module exists
/// to enforce.
const MAX_TRACKED_SESSIONS: usize = 10_000;

/// Prefix of newly minted EngrAmo API tokens (user-minted and OAuth-issued); mirrors
/// engram-api's `CURRENT_TOKEN_PREFIX` (`db/src/repo/api_token/mod.rs`).
const TOKEN_PREFIX_CURRENT: &str = "engramo_";

/// Prefix of pre-rename tokens that still validate; mirrors engram-api's
/// `LEGACY_TOKEN_PREFIX`.
const TOKEN_PREFIX_LEGACY: &str = "engram_";

/// A bearer with neither prefix cannot be valid, so it is rejected at session open
/// with no upstream call.
fn has_known_token_prefix(token: &str) -> bool {
    token.starts_with(TOKEN_PREFIX_CURRENT) || token.starts_with(TOKEN_PREFIX_LEGACY)
}

// Carries the per-request bearer token from [`bearer_auth_middleware`] to the rmcp
// `StreamableHttpService` session factory, which is a plain `Fn() -> Result<S, io::Error>`
// with no access to the HTTP request. The factory is invoked synchronously, inline,
// while handling the `initialize` request that opens a new MCP session (see
// `StreamableHttpService::handle_post` in rmcp) — i.e. still inside the async task this
// task-local is scoped over — so `try_with` reliably sees the value the middleware set.
tokio::task_local! {
    static CURRENT_BEARER_TOKEN: String;
}

/// The bearer token of the request currently being served, or `None` when called
/// outside [`bearer_auth_middleware`]'s scope (which the middleware makes
/// unreachable for the MCP endpoint — it rejects tokenless requests with 401).
pub fn current_bearer_token() -> Option<String> {
    CURRENT_BEARER_TOKEN.try_with(|t| t.clone()).ok()
}

/// State of [`bearer_auth_middleware`].
#[derive(Clone)]
pub struct AuthState {
    pub sessions: SessionTokens,
    /// `WWW-Authenticate` value for a missing/malformed bearer (RFC 9728 §5.1),
    /// pointing at this server's `/.well-known/oauth-protected-resource`.
    challenge: HeaderValue,
    /// Same challenge plus `error="invalid_token"`.
    challenge_invalid_token: HeaderValue,
    /// Bounds concurrent session-open probes.
    probe_permits: Arc<Semaphore>,
    /// Shared upstream client (same one the session factory uses), for the
    /// session-open token probe.
    http: HardenedClient,
    api_url: String,
    /// Tokens the upstream recently rejected (keyed hashes, never the raw token).
    recently_invalid: Arc<NegativeCache>,
    /// rmcp's session manager, so a session whose binding was refused can be closed
    /// instead of lingering unusable until its keep-alive expires.
    session_manager: Option<Arc<LocalSessionManager>>,
}

/// Short-lived memory of rejected tokens. Keys are randomly keyed SipHash digests, so
/// the raw token is never retained.
struct NegativeCache {
    hasher: RandomState,
    seen: Mutex<HashMap<u64, Instant>>,
}

impl NegativeCache {
    fn new() -> Self {
        Self {
            hasher: RandomState::new(),
            seen: Mutex::new(HashMap::new()),
        }
    }

    fn contains(&self, token: &str) -> bool {
        let key = self.hasher.hash_one(token);
        let seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        seen.get(&key).is_some_and(|t| t.elapsed() < NEGATIVE_TTL)
    }

    fn insert(&self, token: &str) {
        let key = self.hasher.hash_one(token);
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        if seen.len() >= MAX_NEGATIVE_ENTRIES {
            seen.retain(|_, t| t.elapsed() < NEGATIVE_TTL);
        }
        if seen.len() < MAX_NEGATIVE_ENTRIES {
            seen.insert(key, Instant::now());
        }
    }
}

/// Result of probing a bearer token against `GET /me`.
#[derive(Debug, PartialEq, Eq)]
enum TokenProbe {
    Valid,
    Invalid,
    Unavailable,
}

impl AuthState {
    /// Builds the state, precomputing both `WWW-Authenticate` values from
    /// `resource_metadata_url`. Fails when the URL cannot be a quoted header
    /// parameter, so a bad `MCP_PUBLIC_URL` stops startup instead of silently
    /// dropping the challenge from every 401.
    pub fn new(
        sessions: SessionTokens,
        resource_metadata_url: impl AsRef<str>,
        http: HardenedClient,
        api_url: impl Into<String>,
    ) -> Result<Self, String> {
        let url = resource_metadata_url.as_ref();
        if url.contains(['"', '\\']) {
            return Err("resource metadata URL contains a quote or backslash".to_string());
        }
        let header = |s: String| {
            HeaderValue::try_from(s)
                .map_err(|e| format!("resource metadata URL is not a valid header value: {e}"))
        };
        Ok(Self {
            sessions,
            challenge: header(format!("Bearer resource_metadata=\"{url}\""))?,
            challenge_invalid_token: header(format!(
                "Bearer error=\"invalid_token\", resource_metadata=\"{url}\""
            ))?,
            probe_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_PROBES)),
            http,
            api_url: api_url.into(),
            recently_invalid: Arc::new(NegativeCache::new()),
            session_manager: None,
        })
    }

    /// Registers the rmcp session manager so refused session opens can be closed.
    #[must_use]
    pub fn with_session_manager(mut self, manager: Arc<LocalSessionManager>) -> Self {
        self.session_manager = Some(manager);
        self
    }

    /// Closes the rmcp session that was just created for a refused open.
    async fn discard_session(&self, session_id: &str) {
        if let Some(manager) = &self.session_manager {
            let _ = manager.close_session(&Arc::<str>::from(session_id)).await;
        }
    }

    fn invalid_token_response(&self) -> Response {
        let mut resp = (
            StatusCode::UNAUTHORIZED,
            "EngrAmo authorization is invalid, expired, or revoked",
        )
            .into_response();
        resp.headers_mut()
            .insert(WWW_AUTHENTICATE, self.challenge_invalid_token.clone());
        resp
    }

    /// Asks the EngrAmo API whether `token` is currently valid. A 403 is treated as
    /// valid: the backend collapses every 403 to a bare "Forbidden", which does not
    /// prove the credential is dead. A 2xx whose body does not decode is valid too:
    /// the status alone settles it, and a body-shape change must not take down every
    /// session open. Never logs the token or the upstream error text.
    async fn probe(&self, token: &str) -> TokenProbe {
        let client = EngramoClient::with_http(self.http.clone(), &self.api_url, token);
        match tokio::time::timeout(PROBE_TIMEOUT, client.get_me()).await {
            Ok(Ok(_) | Err(ApiError::PermissionDenied(_) | ApiError::Decode(_))) => {
                TokenProbe::Valid
            }
            Ok(Err(ApiError::Unauthorized)) => TokenProbe::Invalid,
            Ok(Err(_)) | Err(_) => TokenProbe::Unavailable,
        }
    }
}

/// Remembers which bearer token opened each MCP session, so later requests
/// carrying that session id must present the same token.
#[derive(Clone, Default)]
pub struct SessionTokens {
    inner: Arc<RwLock<Table>>,
}

#[derive(Default)]
struct Table {
    by_session: HashMap<String, Binding>,
    /// Session ids per token: the capacity count and the eviction scan are bounded by
    /// `MAX_SESSIONS_PER_TOKEN`, not by the table size.
    per_token: HashMap<String, HashSet<String>>,
    last_prune: Option<Instant>,
}

struct Binding {
    token: String,
    last_seen: Instant,
}

/// Outcome of checking a request's session id against its bearer token.
#[derive(Debug, PartialEq, Eq)]
enum SessionCheck {
    /// Token matches the one that opened this session.
    Ok,
    /// A different token than the one that opened this session, or a session id
    /// we hold no binding for — reject (fail closed).
    Mismatch,
}

/// Result of the read-only capacity assessment for a prospective new session.
#[derive(Debug, PartialEq, Eq)]
enum Capacity {
    Available,
    /// The whole table is full (flood) — 503.
    TableFull,
    /// This token holds its maximum number of live sessions; none is old enough to
    /// evict. Retry after roughly this long — 429.
    TokenFull(Duration),
}

/// Outcome of [`SessionTokens::bind`]; anything but `Bound` refuses the open.
#[derive(Debug, PartialEq, Eq)]
enum BindOutcome {
    Bound,
    TableFull,
    TokenFull(Duration),
}

impl Table {
    fn insert(&mut self, session_id: String, binding: Binding) {
        self.per_token
            .entry(binding.token.clone())
            .or_default()
            .insert(session_id.clone());
        if let Some(old) = self.by_session.insert(session_id.clone(), binding)
            && !constant_time_eq(&old.token, &self.by_session[&session_id].token)
        {
            self.detach(&old.token, &session_id);
        }
    }

    fn remove(&mut self, session_id: &str) {
        if let Some(old) = self.by_session.remove(session_id) {
            self.detach(&old.token, session_id);
        }
    }

    fn detach(&mut self, token: &str, session_id: &str) {
        if let Some(ids) = self.per_token.get_mut(token) {
            ids.remove(session_id);
            if ids.is_empty() {
                self.per_token.remove(token);
            }
        }
    }

    fn count_of(&self, token: &str) -> usize {
        self.per_token.get(token).map_or(0, HashSet::len)
    }

    fn prune(&mut self) {
        let expired: Vec<String> = self
            .by_session
            .iter()
            .filter(|(_, b)| b.last_seen.elapsed() >= BINDING_TTL)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.remove(&id);
        }
        self.last_prune = Some(Instant::now());
    }

    /// The session of `token` idle the longest, with its idle time.
    fn oldest_of(&self, token: &str) -> Option<(&String, Duration)> {
        self.per_token
            .get(token)?
            .iter()
            .filter_map(|id| self.by_session.get_key_value(id))
            .map(|(id, b)| (id, b.last_seen.elapsed()))
            .max_by_key(|(_, idle)| *idle)
    }

    /// Read-only: can a new session for `token` be tracked (possibly after evicting
    /// one of its own sessions idle past [`EVICTABLE_IDLE`])? The scan, if any, is
    /// bounded by `MAX_SESSIONS_PER_TOKEN`.
    fn assess(&self, token: &str) -> Capacity {
        if self.by_session.len() >= MAX_TRACKED_SESSIONS {
            return Capacity::TableFull;
        }
        if self.count_of(token) < MAX_SESSIONS_PER_TOKEN {
            return Capacity::Available;
        }
        match self.oldest_of(token) {
            Some((_, idle)) if idle <= EVICTABLE_IDLE => {
                Capacity::TokenFull((EVICTABLE_IDLE - idle).max(Duration::from_secs(1)))
            }
            _ => Capacity::Available,
        }
    }
}

impl SessionTokens {
    pub fn new() -> Self {
        Self::default()
    }

    async fn check(&self, session_id: &str, token: &str) -> SessionCheck {
        let mut guard = self.inner.write().await;
        match guard.by_session.get_mut(session_id) {
            Some(binding) if constant_time_eq(&binding.token, token) => {
                binding.last_seen = Instant::now();
                SessionCheck::Ok
            }
            Some(_) | None => SessionCheck::Mismatch,
        }
    }

    /// Records the binding unless the table (or this token's share of it) is full.
    /// Prunes expired bindings and, when the token is at its cap, first evicts that
    /// token's own longest-idle binding if it has been idle past the keep-alive plus slack
    /// (rmcp has closed that session, or the client will re-initialize on its 404). Capacity
    /// is re-checked under the same write lock as the insert, so many concurrent opens
    /// cannot all slip past an earlier `has_capacity`.
    async fn bind(&self, session_id: &str, token: String) -> BindOutcome {
        let mut guard = self.inner.write().await;
        guard.prune();
        if !guard.by_session.contains_key(session_id) {
            match guard.assess(&token) {
                Capacity::Available => {}
                Capacity::TableFull => return BindOutcome::TableFull,
                Capacity::TokenFull(retry) => return BindOutcome::TokenFull(retry),
            }
            if guard.count_of(&token) >= MAX_SESSIONS_PER_TOKEN
                && let Some(victim) = guard.oldest_of(&token).map(|(id, _)| id.clone())
            {
                guard.remove(&victim);
            }
        }
        guard.insert(
            session_id.to_string(),
            Binding {
                token,
                last_seen: Instant::now(),
            },
        );
        BindOutcome::Bound
    }

    async fn unbind(&self, session_id: &str) {
        self.inner.write().await.remove(session_id);
    }

    /// Whether a new session for `token` can still be tracked. Read-only; the work
    /// is bounded by `MAX_SESSIONS_PER_TOKEN` on the hot path; only when the table is full does it take the write lock for a
    /// rate-limited prune, so a flood cannot serialize the table.
    async fn has_capacity(&self, token: &str) -> Capacity {
        {
            let guard = self.inner.read().await;
            let cap = guard.assess(token);
            if cap != Capacity::TableFull {
                return cap;
            }
        }
        let mut guard = self.inner.write().await;
        if guard
            .last_prune
            .is_none_or(|t| t.elapsed() >= PRUNE_INTERVAL)
        {
            guard.prune();
        }
        guard.assess(token)
    }

    #[cfg(test)]
    async fn len(&self) -> usize {
        self.inner.read().await.by_session.len()
    }
}

/// Length-independent byte comparison. The caller controls one side, so an
/// early-exit `==` would leak the bound token a byte at a time.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The 404 for a session id that is not this caller's. Byte-identical (status, body, and
/// no `content-type`) to rmcp's own unknown-session 404, so a probe cannot tell "not yours"
/// from "does not exist".
fn session_not_found() -> Response {
    let mut resp = Response::new(axum::body::Body::from("Not Found: Session not found"));
    *resp.status_mut() = StatusCode::NOT_FOUND;
    resp
}

fn table_full() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "Too many active sessions, try again later",
    )
        .into_response()
}

/// 429 for a token at its per-token session cap, with `Retry-After` in whole seconds.
fn too_many_sessions(retry_after: Duration) -> Response {
    let mut resp = (
        StatusCode::TOO_MANY_REQUESTS,
        "Too many active sessions for this token, try again later",
    )
        .into_response();
    resp.headers_mut()
        .insert(RETRY_AFTER, HeaderValue::from(retry_after.as_secs().max(1)));
    resp
}

fn api_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "EngrAmo API unavailable, try again",
    )
        .into_response()
}

/// Pulls a well-formed `Authorization: Bearer <token>` out of the request.
///
/// Rejects anything that is not printable ASCII or is longer than
/// [`MAX_TOKEN_LEN`]: a token is forwarded verbatim as the `X-Api-Key` header
/// value on every upstream call, so it must be header-safe before it is stored
/// anywhere.
fn extract_bearer(headers: &axum::http::HeaderMap) -> Option<String> {
    let value = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok())?;
    // The auth-scheme token is case-insensitive per RFC 7235 §2.1 — some
    // clients send `bearer` rather than `Bearer`.
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();

    if token.is_empty()
        || token.len() > MAX_TOKEN_LEN
        || !token.bytes().all(|b| b.is_ascii_graphic())
    {
        return None;
    }
    Some(token.to_string())
}

/// Extracts `Authorization: Bearer <token>` and scopes it into
/// `CURRENT_BEARER_TOKEN` for the duration of the request. Missing, empty or
/// malformed bearer tokens are rejected with 401 (plus a `WWW-Authenticate`
/// pointing at the protected-resource metadata) at the edge — before ever
/// reaching the rmcp session factory (which would 500 on a missing task-local,
/// since it has no HTTP-status-aware rejection path of its own).
///
/// A POST without an `Mcp-Session-Id` (a session open) is additionally validated:
/// a bearer with neither the `engramo_` nor the legacy `engram_` prefix gets 401
/// `invalid_token` with no upstream call; otherwise `GET /me` decides — upstream
/// 401 → 401 `invalid_token`, 403 or an undecodable 2xx → valid, anything else
/// (network, timeout, 5xx, or all probe slots busy) → 503 without
/// `WWW-Authenticate`.
///
/// A request that carries an `Mcp-Session-Id` must present the same token that
/// opened that session, otherwise (or if the id is unknown) it gets rmcp's own
/// 404 (no `WWW-Authenticate`: the caller's token is fine, the session just is
/// not theirs); see the module docs for why rmcp cannot enforce this.
pub async fn bearer_auth_middleware(
    State(auth): State<AuthState>,
    mut req: Request,
    next: Next,
) -> Response {
    let sessions = &auth.sessions;
    let Some(token) = extract_bearer(req.headers()) else {
        let mut resp = (
            StatusCode::UNAUTHORIZED,
            "Missing or malformed Authorization: Bearer <token> header",
        )
            .into_response();
        resp.headers_mut()
            .insert(WWW_AUTHENTICATE, auth.challenge.clone());
        return resp;
    };

    let session_id = req
        .headers()
        .get(HEADER_SESSION_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    match &session_id {
        Some(id) => {
            if sessions.check(id, &token).await == SessionCheck::Mismatch {
                // Deliberately not logged with the session id: it is a
                // credential-equivalent value (see `init_logging`).
                tracing::warn!(
                    "rejected request whose bearer token does not match its MCP session"
                );
                return session_not_found();
            }
        }
        // No session id yet. Only a POST can open a session (GET/DELETE without one are
        // rejected by rmcp itself), so only a POST is vetted, cheapest check first:
        // prefix / recently-rejected cache, then the (small, time-bounded) body shape,
        // then the read-only capacity check, then the bounded upstream probe.
        None => {
            if req.method() == Method::POST {
                if !has_known_token_prefix(&token) || auth.recently_invalid.contains(&token) {
                    return auth.invalid_token_response();
                }
                // Every session opens with `initialize`, and rmcp decides from the
                // *body* (not the `MCP-Protocol-Version` header) whether a POST is
                // stateless (per-request metadata, no session id). A stateless POST
                // would bypass session binding and turn the probe pool into a
                // throughput cap, so a session-less POST must be an `initialize`;
                // rmcp always routes `initialize` down its session path. The body is
                // classified with rmcp's own message type so gate and transport agree.
                let (parts, body) = req.into_parts();
                let read = tokio::time::timeout(
                    INITIALIZE_READ_TIMEOUT,
                    axum::body::to_bytes(body, MAX_INITIALIZE_BYTES),
                )
                .await;
                let bytes = match read {
                    Err(_) => {
                        return (StatusCode::REQUEST_TIMEOUT, "Request body read timed out")
                            .into_response();
                    }
                    Ok(Ok(b)) => b,
                    Ok(Err(e)) if is_length_limit_error(&e) => {
                        return (StatusCode::PAYLOAD_TOO_LARGE, "Request body too large")
                            .into_response();
                    }
                    Ok(Err(_)) => {
                        return (StatusCode::BAD_REQUEST, "Failed to read request body")
                            .into_response();
                    }
                };
                let is_initialize = matches!(
                    serde_json::from_slice::<rmcp::model::ClientJsonRpcMessage>(&bytes),
                    Ok(rmcp::model::JsonRpcMessage::Request(r))
                        if matches!(r.request, rmcp::model::ClientRequest::InitializeRequest(_))
                );
                if !is_initialize {
                    return (
                        StatusCode::BAD_REQUEST,
                        "A request without Mcp-Session-Id must be an initialize request",
                    )
                        .into_response();
                }
                req = Request::from_parts(parts, axum::body::Body::from(bytes));
                // A token at its cap is refused only after the probe, so a revoked
                // token still gets 401 rather than a misleading 429.
                let mut token_full = None;
                match sessions.has_capacity(&token).await {
                    Capacity::Available => {}
                    Capacity::TableFull => {
                        tracing::error!(
                            "session binding table is full — refusing new MCP sessions"
                        );
                        return table_full();
                    }
                    Capacity::TokenFull(retry) => token_full = Some(retry),
                }
                // Queue briefly so short bursts are absorbed instead of turning the
                // upstream-flood bound into an instant lockout for legitimate opens.
                let permit =
                    tokio::time::timeout(PROBE_QUEUE_WAIT, auth.probe_permits.acquire()).await;
                let Ok(Ok(_permit)) = permit else {
                    tracing::warn!("token validation skipped: too many concurrent probes");
                    return api_unavailable();
                };
                match auth.probe(&token).await {
                    TokenProbe::Valid => {}
                    TokenProbe::Invalid => {
                        auth.recently_invalid.insert(&token);
                        return auth.invalid_token_response();
                    }
                    TokenProbe::Unavailable => {
                        tracing::warn!("token validation failed: EngrAmo API unavailable");
                        return api_unavailable();
                    }
                }
                if let Some(retry) = token_full {
                    return too_many_sessions(retry);
                }
            }
        }
    }

    let is_delete = req.method() == Method::DELETE;
    let response = CURRENT_BEARER_TOKEN
        .scope(token.clone(), next.run(req))
        .await;

    // rmcp returns the new session's id on the `initialize` response; that is
    // the only place the binding can be learned.
    if let Some(new_id) = response
        .headers()
        .get(HEADER_SESSION_ID)
        .and_then(|v| v.to_str().ok())
        && session_id.as_deref() != Some(new_id)
    {
        match sessions.bind(new_id, token).await {
            BindOutcome::Bound => {}
            BindOutcome::TableFull => {
                tracing::error!("session binding table is full — dropping new MCP session");
                auth.discard_session(new_id).await;
                return table_full();
            }
            BindOutcome::TokenFull(retry) => {
                auth.discard_session(new_id).await;
                return too_many_sessions(retry);
            }
        }
    }

    // MCP clients end a session with DELETE; drop the binding with it.
    if is_delete
        && response.status().is_success()
        && let Some(id) = session_id
    {
        sessions.unbind(&id).await;
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router, body::Body, http::Request as HttpRequest, middleware, routing::get, routing::post,
    };
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const VICTIM: &str = "engramo_victim_token";
    const ATTACKER: &str = "engramo_attacker_token";
    const SESSION: &str = "6ea7c668-e947-446f-9e8f-7977869284b7";

    const META_URL: &str = "https://mcp.example.com/.well-known/oauth-protected-resource";

    /// Router whose probe is answered by a wiremock server that accepts every
    /// token (`GET /me` → 200). The server is owned by the router (an `Extension`)
    /// so it lives exactly as long as the router does.
    async fn app(sessions: SessionTokens) -> Router {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        app_with(sessions, &server.uri()).layer(axum::Extension(Arc::new(server)))
    }

    async fn mock_me(server: &MockServer, status: u16) {
        let body = serde_json::json!({"user_id": "11111111-2222-3333-4444-555555555555"});
        Mock::given(method("GET"))
            .and(path("/me"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(server)
            .await;
    }

    async fn me_calls(server: &MockServer) -> usize {
        server.received_requests().await.unwrap().len()
    }

    /// A stand-in for rmcp's service: echoes the token it saw via the
    /// task-local (as the real session factory does) and mints a session id on
    /// a request that has none, exactly like an `initialize` response.
    fn app_with(sessions: SessionTokens, api_url: &str) -> Router {
        router_for(test_auth(sessions, api_url))
    }

    fn test_auth(sessions: SessionTokens, api_url: &str) -> AuthState {
        AuthState::new(
            sessions,
            META_URL,
            EngramoClient::build_http_client(),
            api_url,
        )
        .unwrap()
    }

    fn router_for(auth: AuthState) -> Router {
        Router::new()
            .route(
                "/",
                post(|req: HttpRequest<Body>| async move {
                    let token = current_bearer_token().unwrap_or_default();
                    let mut resp = Response::new(Body::from(token));
                    if req.headers().get(HEADER_SESSION_ID).is_none() {
                        resp.headers_mut()
                            .insert(HEADER_SESSION_ID, SESSION.parse().unwrap());
                    }
                    resp
                })
                .delete(|| async { StatusCode::OK }),
            )
            .route("/get", get(|| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                auth.clone(),
                bearer_auth_middleware,
            ))
            .with_state(auth)
    }

    fn request(token: Option<&str>, session: Option<&str>) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder().method("POST").uri("/");
        if let Some(t) = token {
            b = b.header(AUTHORIZATION, t);
        }
        if let Some(s) = session {
            b = b.header(HEADER_SESSION_ID, s);
        }
        let body = if session.is_none() {
            Body::from(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
            )
        } else {
            Body::empty()
        };
        b.body(body).unwrap()
    }

    async fn body_string(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn test_middleware_rejects_missing_authorization_header() {
        let resp = app(SessionTokens::new())
            .await
            .oneshot(request(None, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_missing_bearer_401_carries_www_authenticate() {
        let resp = app(SessionTokens::new())
            .await
            .oneshot(request(None, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get(WWW_AUTHENTICATE).unwrap(),
            "Bearer resource_metadata=\"https://mcp.example.com/.well-known/oauth-protected-resource\""
        );
    }

    #[tokio::test]
    async fn test_malformed_bearer_401_carries_www_authenticate_without_error_param() {
        let resp = app(SessionTokens::new())
            .await
            .oneshot(request(Some("Basic dXNlcjpwYXNz"), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let h = resp
            .headers()
            .get(WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(h.starts_with("Bearer resource_metadata="), "{h}");
        assert!(!h.contains("error="), "{h}");
    }

    #[tokio::test]
    async fn test_middleware_rejects_empty_bearer_token() {
        let resp = app(SessionTokens::new())
            .await
            .oneshot(request(Some("Bearer "), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_middleware_rejects_non_bearer_scheme() {
        let resp = app(SessionTokens::new())
            .await
            .oneshot(request(Some("Basic dXNlcjpwYXNz"), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_middleware_rejects_oversized_token() {
        let huge = format!("Bearer {}", "a".repeat(MAX_TOKEN_LEN + 1));
        let resp = app(SessionTokens::new())
            .await
            .oneshot(request(Some(&huge), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_middleware_rejects_token_with_non_graphic_bytes() {
        // A space inside the token would break the `X-Api-Key` header it is
        // forwarded as; anything non-graphic is refused.
        let resp = app(SessionTokens::new())
            .await
            .oneshot(request(Some("Bearer engramo tok"), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_middleware_accepts_lowercase_bearer_scheme() {
        // RFC 7235 §2.1: the auth-scheme token is case-insensitive.
        let resp = app(SessionTokens::new())
            .await
            .oneshot(request(Some(&format!("bearer {VICTIM}")), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_string(resp).await, VICTIM);
    }

    #[tokio::test]
    async fn test_middleware_scopes_token_for_the_inner_service() {
        let resp = app(SessionTokens::new())
            .await
            .oneshot(request(Some(&format!("Bearer {VICTIM}")), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_string(resp).await, VICTIM);
    }

    #[tokio::test]
    async fn test_initialize_response_binds_session_to_its_token() {
        let sessions = SessionTokens::new();
        let resp = app(sessions.clone())
            .await
            .oneshot(request(Some(&format!("Bearer {VICTIM}")), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(sessions.len().await, 1);
    }

    #[tokio::test]
    async fn test_session_rejects_a_different_bearer_token() {
        let sessions = SessionTokens::new();
        app(sessions.clone())
            .await
            .oneshot(request(Some(&format!("Bearer {VICTIM}")), None))
            .await
            .unwrap();

        let resp = app(sessions.clone())
            .await
            .oneshot(request(Some(&format!("Bearer {ATTACKER}")), Some(SESSION)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(resp.headers().get(WWW_AUTHENTICATE).is_none());
        assert!(!body_string(resp).await.contains(ATTACKER));
    }

    #[tokio::test]
    async fn test_session_accepts_the_token_that_opened_it() {
        let sessions = SessionTokens::new();
        app(sessions.clone())
            .await
            .oneshot(request(Some(&format!("Bearer {VICTIM}")), None))
            .await
            .unwrap();

        let resp = app(sessions.clone())
            .await
            .oneshot(request(Some(&format!("Bearer {VICTIM}")), Some(SESSION)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_string(resp).await, VICTIM);
    }

    #[tokio::test]
    async fn test_unknown_and_mismatched_sessions_get_identical_404() {
        let sessions = SessionTokens::new();
        app(sessions.clone())
            .await
            .oneshot(request(Some(&format!("Bearer {VICTIM}")), None))
            .await
            .unwrap();

        let unknown = app(sessions.clone())
            .await
            .oneshot(request(
                Some(&format!("Bearer {ATTACKER}")),
                Some("11111111-1111-1111-1111-111111111111"),
            ))
            .await
            .unwrap();
        let mismatched = app(sessions)
            .await
            .oneshot(request(Some(&format!("Bearer {ATTACKER}")), Some(SESSION)))
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert_eq!(unknown.status(), mismatched.status());
        assert_eq!(unknown.headers(), mismatched.headers());
        assert!(unknown.headers().get("content-type").is_none());
        let (a, b) = (body_string(unknown).await, body_string(mismatched).await);
        assert_eq!(a, "Not Found: Session not found");
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn test_delete_releases_the_binding() {
        let sessions = SessionTokens::new();
        app(sessions.clone())
            .await
            .oneshot(request(Some(&format!("Bearer {VICTIM}")), None))
            .await
            .unwrap();
        assert_eq!(sessions.len().await, 1);

        let req = HttpRequest::builder()
            .method("DELETE")
            .uri("/")
            .header(AUTHORIZATION, format!("Bearer {VICTIM}"))
            .header(HEADER_SESSION_ID, SESSION)
            .body(Body::empty())
            .unwrap();
        let resp = app(sessions.clone()).await.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(sessions.len().await, 0);
    }

    #[tokio::test]
    async fn test_new_sessions_refused_when_binding_table_is_full() {
        let sessions = SessionTokens::new();
        {
            let mut guard = sessions.inner.write().await;
            for i in 0..MAX_TRACKED_SESSIONS {
                guard.insert(
                    format!("session-{i}"),
                    Binding {
                        token: VICTIM.to_string(),
                        last_seen: Instant::now(),
                    },
                );
            }
        }
        let resp = app(sessions)
            .await
            .oneshot(request(Some(&format!("Bearer {VICTIM}")), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_expired_bindings_are_pruned_to_make_room() {
        let sessions = SessionTokens::new();
        {
            let mut guard = sessions.inner.write().await;
            for i in 0..MAX_TRACKED_SESSIONS {
                guard.insert(
                    format!("session-{i}"),
                    Binding {
                        token: VICTIM.to_string(),
                        // Older than BINDING_TTL — rmcp closed these long ago.
                        last_seen: Instant::now() - BINDING_TTL - Duration::from_secs(1),
                    },
                );
            }
        }
        let resp = app(sessions.clone())
            .await
            .oneshot(request(Some(&format!("Bearer {VICTIM}")), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(sessions.len().await, 1);
    }

    #[tokio::test]
    async fn test_current_bearer_token_is_none_outside_the_middleware() {
        assert!(current_bearer_token().is_none());
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq("engramo_abc", "engramo_abc"));
        assert!(!constant_time_eq("engramo_abc", "engramo_abd"));
        assert!(!constant_time_eq("engramo_abc", "engramo_abcd"));
        assert!(!constant_time_eq("", "x"));
        assert!(constant_time_eq("", ""));
    }

    fn open_req(method_: &str, token: &str, session: Option<&str>) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder()
            .method(method_)
            .uri("/")
            .header(AUTHORIZATION, format!("Bearer {token}"));
        if let Some(s) = session {
            b = b.header(HEADER_SESSION_ID, s);
        }
        let body = if method_ == "POST" {
            Body::from(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
            )
        } else {
            Body::empty()
        };
        b.body(body).unwrap()
    }

    #[tokio::test]
    async fn test_session_open_with_valid_token_passes_and_probes_once() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let resp = app_with(SessionTokens::new(), &server.uri())
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(me_calls(&server).await, 1);
    }

    #[tokio::test]
    async fn test_non_engram_prefix_is_401_invalid_token_without_upstream_call() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let resp = app_with(SessionTokens::new(), &server.uri())
            .oneshot(open_req("POST", "junk_token", None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get(WWW_AUTHENTICATE).unwrap(),
            "Bearer error=\"invalid_token\", resource_metadata=\"https://mcp.example.com/.well-known/oauth-protected-resource\""
        );
        assert!(!body_string(resp).await.contains("junk_token"));
        assert_eq!(me_calls(&server).await, 0);
    }

    #[tokio::test]
    async fn test_legacy_engram_prefix_is_probed_and_accepted() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let resp = app_with(SessionTokens::new(), &server.uri())
            .oneshot(open_req("POST", "engram_legacy_token", None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(me_calls(&server).await, 1);
    }

    #[tokio::test]
    async fn test_near_miss_prefixes_are_rejected_without_upstream_call() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        for t in [
            "foo_token",
            "engramX_token",
            "engramox_token",
            "engra_token",
        ] {
            let resp = app_with(SessionTokens::new(), &server.uri())
                .oneshot(open_req("POST", t, None))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{t}");
        }
        assert_eq!(me_calls(&server).await, 0);
    }

    #[tokio::test]
    async fn test_upstream_401_is_401_invalid_token() {
        let server = MockServer::start().await;
        mock_me(&server, 401).await;
        let sessions = SessionTokens::new();
        let resp = app_with(sessions.clone(), &server.uri())
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get(WWW_AUTHENTICATE).unwrap(),
            "Bearer error=\"invalid_token\", resource_metadata=\"https://mcp.example.com/.well-known/oauth-protected-resource\""
        );
        assert_eq!(sessions.len().await, 0);
    }

    #[tokio::test]
    async fn test_upstream_403_counts_as_valid() {
        let server = MockServer::start().await;
        mock_me(&server, 403).await;
        let resp = app_with(SessionTokens::new(), &server.uri())
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_upstream_500_is_503_without_www_authenticate() {
        let server = MockServer::start().await;
        mock_me(&server, 500).await;
        let resp = app_with(SessionTokens::new(), &server.uri())
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(resp.headers().get(WWW_AUTHENTICATE).is_none());
        assert_eq!(
            body_string(resp).await,
            "EngrAmo API unavailable, try again"
        );
    }

    #[tokio::test]
    async fn test_unreachable_upstream_is_503() {
        let resp = app_with(SessionTokens::new(), "http://127.0.0.1:1")
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_request_with_session_id_is_not_probed() {
        let server = MockServer::start().await;
        mock_me(&server, 500).await;
        let sessions = SessionTokens::new();
        assert_eq!(
            sessions.bind(SESSION, VICTIM.to_string()).await,
            BindOutcome::Bound
        );
        let resp = app_with(sessions, &server.uri())
            .oneshot(open_req("POST", VICTIM, Some(SESSION)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(me_calls(&server).await, 0);
    }

    #[tokio::test]
    async fn test_get_without_session_id_is_not_probed() {
        let server = MockServer::start().await;
        mock_me(&server, 500).await;
        let req = HttpRequest::builder()
            .method("GET")
            .uri("/get")
            .header(AUTHORIZATION, format!("Bearer {VICTIM}"))
            .body(Body::empty())
            .unwrap();
        let resp = app_with(SessionTokens::new(), &server.uri())
            .oneshot(req)
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(me_calls(&server).await, 0);
    }

    #[tokio::test]
    async fn test_full_table_is_checked_before_probe() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let sessions = SessionTokens::new();
        {
            let mut guard = sessions.inner.write().await;
            for i in 0..MAX_TRACKED_SESSIONS {
                guard.insert(
                    format!("session-{i}"),
                    Binding {
                        token: VICTIM.to_string(),
                        last_seen: Instant::now(),
                    },
                );
            }
        }
        let resp = app_with(sessions, &server.uri())
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(me_calls(&server).await, 0);
    }

    #[tokio::test]
    async fn test_probe_forwards_presented_bearer_as_x_api_key() {
        let server = MockServer::start().await;
        // Only the exact presented token is accepted; anything else falls through to 401.
        Mock::given(method("GET"))
            .and(path("/me"))
            .and(wiremock::matchers::header("X-Api-Key", VICTIM))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"user_id": "11111111-2222-3333-4444-555555555555"}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let ok = app_with(SessionTokens::new(), &server.uri())
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let bad = app_with(SessionTokens::new(), &server.uri())
            .oneshot(open_req("POST", ATTACKER, None))
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_upstream_200_with_unexpected_body_counts_as_valid() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
            .mount(&server)
            .await;
        let resp = app_with(SessionTokens::new(), &server.uri())
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_probe_refused_with_503_when_all_permits_are_taken() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let auth = test_auth(SessionTokens::new(), &server.uri());
        let _held = auth
            .probe_permits
            .clone()
            .acquire_many_owned(MAX_CONCURRENT_PROBES as u32)
            .await
            .unwrap();
        let resp = router_for(auth)
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(resp.headers().get(WWW_AUTHENTICATE).is_none());
        assert_eq!(me_calls(&server).await, 0);
    }

    async fn fill(sessions: &SessionTokens, token: &str, n: usize, idle: Duration) {
        let mut guard = sessions.inner.write().await;
        for i in 0..n {
            guard.insert(
                format!("{token}-{i}"),
                Binding {
                    token: token.to_string(),
                    last_seen: Instant::now() - idle - Duration::from_secs(i as u64),
                },
            );
        }
    }

    #[tokio::test]
    async fn test_per_token_session_cap_blocks_one_token_but_not_others() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let sessions = SessionTokens::new();
        for i in 0..MAX_SESSIONS_PER_TOKEN {
            assert_eq!(
                sessions.bind(&format!("s-{i}"), VICTIM.to_string()).await,
                BindOutcome::Bound
            );
        }
        // The capped token is refused with 429 + Retry-After once its bearer is validated...
        let resp = app_with(sessions.clone(), &server.uri())
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry: u64 = resp.headers()[RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=EVICTABLE_IDLE.as_secs()).contains(&retry));
        assert_eq!(me_calls(&server).await, 1);
        // ...and a direct bind past the cap is refused too, while another token still works.
        assert!(matches!(
            sessions.bind("extra", VICTIM.to_string()).await,
            BindOutcome::TokenFull(_)
        ));
        let resp = app_with(sessions, &server.uri())
            .oneshot(open_req("POST", ATTACKER, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_token_at_cap_evicts_its_own_idle_binding_instead_of_locking_out() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let sessions = SessionTokens::new();
        // Idle past keep-alive plus slack (so rmcp has closed them) but within BINDING_TTL.
        fill(
            &sessions,
            VICTIM,
            MAX_SESSIONS_PER_TOKEN,
            EVICTABLE_IDLE + Duration::from_secs(10),
        )
        .await;
        let resp = app_with(sessions.clone(), &server.uri())
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // One evicted, one bound: still at the cap, never above it.
        assert_eq!(sessions.len().await, MAX_SESSIONS_PER_TOKEN);
        // The longest-idle binding (index cap-1) was the one evicted.
        let guard = sessions.inner.read().await;
        assert!(
            !guard
                .by_session
                .contains_key(&format!("{VICTIM}-{}", MAX_SESSIONS_PER_TOKEN - 1))
        );
        assert!(guard.by_session.contains_key(SESSION));
    }

    #[tokio::test]
    async fn test_eviction_never_touches_another_tokens_bindings() {
        let sessions = SessionTokens::new();
        let idle = EVICTABLE_IDLE + Duration::from_secs(10);
        fill(&sessions, VICTIM, MAX_SESSIONS_PER_TOKEN, idle).await;
        fill(&sessions, ATTACKER, 3, idle).await;
        assert_eq!(
            sessions.bind("new", VICTIM.to_string()).await,
            BindOutcome::Bound
        );
        let guard = sessions.inner.read().await;
        assert_eq!(guard.per_token[ATTACKER].len(), 3);
        assert_eq!(guard.per_token[VICTIM].len(), MAX_SESSIONS_PER_TOKEN);
        assert!(guard.per_token[VICTIM].contains("new"));
    }

    #[tokio::test]
    async fn test_per_token_counts_stay_in_sync_through_unbind_and_prune() {
        let sessions = SessionTokens::new();
        fill(&sessions, VICTIM, 3, BINDING_TTL + Duration::from_secs(1)).await;
        sessions.bind("live", ATTACKER.to_string()).await;
        // Binding prunes the three expired ones and their count entry.
        let guard = sessions.inner.read().await;
        assert!(!guard.per_token.contains_key(VICTIM));
        assert!(guard.per_token[ATTACKER].contains("live"));
        drop(guard);
        sessions.unbind("live").await;
        assert!(sessions.inner.read().await.per_token.is_empty());
    }

    #[tokio::test]
    async fn test_binding_between_keep_alive_and_evictable_idle_is_not_evicted() {
        let sessions = SessionTokens::new();
        fill(
            &sessions,
            VICTIM,
            MAX_SESSIONS_PER_TOKEN,
            SESSION_KEEP_ALIVE + Duration::from_secs(10),
        )
        .await;
        assert!(matches!(
            sessions.bind("extra", VICTIM.to_string()).await,
            BindOutcome::TokenFull(_)
        ));
        assert_eq!(sessions.len().await, MAX_SESSIONS_PER_TOKEN);
    }

    #[tokio::test]
    async fn test_revoked_token_at_cap_gets_401_not_429() {
        let server = MockServer::start().await;
        mock_me(&server, 401).await;
        let sessions = SessionTokens::new();
        fill(&sessions, VICTIM, MAX_SESSIONS_PER_TOKEN, Duration::ZERO).await;
        let resp = app_with(sessions, &server.uri())
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_capacity_check_runs_after_prefix_check_and_only_for_posts() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let sessions = SessionTokens::new();
        fill(&sessions, "junk", MAX_SESSIONS_PER_TOKEN, Duration::ZERO).await;
        // A junk-prefix bearer is 401 even though it is "at cap"; a GET without a
        // session id is passed through untouched.
        let resp = app_with(sessions.clone(), &server.uri())
            .oneshot(open_req("POST", "junk", None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(me_calls(&server).await, 0);
    }

    #[tokio::test]
    async fn test_bind_refusal_at_open_answers_429() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let sessions = SessionTokens::new();
        let auth = test_auth(sessions.clone(), &server.uri());
        // Pre-fill cap-1 bindings so the pre-check passes; the inner handler binds one more
        // before the middleware's own `bind`, which must then refuse and answer 429.
        for i in 0..MAX_SESSIONS_PER_TOKEN - 1 {
            assert_eq!(
                sessions.bind(&format!("s-{i}"), VICTIM.to_string()).await,
                BindOutcome::Bound
            );
        }
        let racing = sessions.clone();
        let router = Router::new()
            .route(
                "/",
                post(move || {
                    let racing = racing.clone();
                    async move {
                        racing.bind("racer", VICTIM.to_string()).await;
                        let mut resp = Response::new(Body::empty());
                        resp.headers_mut()
                            .insert(HEADER_SESSION_ID, SESSION.parse().unwrap());
                        resp
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(
                auth.clone(),
                bearer_auth_middleware,
            ))
            .with_state(auth);
        let resp = router
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(resp.headers().contains_key(RETRY_AFTER));
        assert_eq!(sessions.len().await, MAX_SESSIONS_PER_TOKEN);
    }

    #[test]
    fn test_auth_state_rejects_unusable_metadata_url() {
        for bad in ["https://x/\"y", "https://x/\\y", "https://x/\ny"] {
            assert!(
                AuthState::new(
                    SessionTokens::new(),
                    bad,
                    EngramoClient::build_http_client(),
                    "http://localhost"
                )
                .is_err(),
                "{bad:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_bind_refusal_when_table_fills_during_open_answers_503() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let sessions = SessionTokens::new();
        let auth = test_auth(sessions.clone(), &server.uri());
        {
            let mut guard = sessions.inner.write().await;
            for i in 0..MAX_TRACKED_SESSIONS - 1 {
                guard.insert(
                    format!("pre-{i}"),
                    Binding {
                        token: format!("engramo_o{i}"),
                        last_seen: Instant::now(),
                    },
                );
            }
        }
        let racing = sessions.clone();
        let router = Router::new()
            .route(
                "/",
                post(move || {
                    let racing = racing.clone();
                    async move {
                        racing.bind("racer", "engramo_other".to_string()).await;
                        let mut resp = Response::new(Body::empty());
                        resp.headers_mut()
                            .insert(HEADER_SESSION_ID, SESSION.parse().unwrap());
                        resp
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(
                auth.clone(),
                bearer_auth_middleware,
            ))
            .with_state(auth);
        let resp = router
            .oneshot(open_req("POST", VICTIM, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body_string(resp).await,
            "Too many active sessions, try again later"
        );
        assert!(!sessions.inner.read().await.by_session.contains_key(SESSION));
    }

    #[tokio::test]
    async fn test_rebinding_a_session_to_another_token_moves_its_per_token_entry() {
        let sessions = SessionTokens::new();
        assert_eq!(
            sessions.bind("s", VICTIM.to_string()).await,
            BindOutcome::Bound
        );
        assert_eq!(
            sessions.bind("s", ATTACKER.to_string()).await,
            BindOutcome::Bound
        );
        {
            let g = sessions.inner.read().await;
            assert!(!g.per_token.contains_key(VICTIM));
            assert!(g.per_token[ATTACKER].contains("s"));
            assert!(constant_time_eq(&g.by_session["s"].token, ATTACKER));
            assert_eq!(g.by_session.len(), 1);
        }
        assert_eq!(
            sessions.bind("s", ATTACKER.to_string()).await,
            BindOutcome::Bound
        );
        let g = sessions.inner.read().await;
        assert_eq!(g.per_token[ATTACKER].len(), 1);
        assert!(g.per_token[ATTACKER].contains("s"));
    }

    #[tokio::test]
    async fn test_capacity_precheck_prune_is_rate_limited() {
        let sessions = SessionTokens::new();
        {
            let mut guard = sessions.inner.write().await;
            let stale = Instant::now() - BINDING_TTL - Duration::from_secs(1);
            for i in 0..MAX_TRACKED_SESSIONS {
                guard.insert(
                    format!("s-{i}"),
                    Binding {
                        token: format!("engramo_o{i}"),
                        last_seen: stale,
                    },
                );
            }
            guard.last_prune = Some(Instant::now());
        }
        assert_eq!(sessions.has_capacity(VICTIM).await, Capacity::TableFull);
        assert_eq!(sessions.len().await, MAX_TRACKED_SESSIONS);
        sessions.inner.write().await.last_prune =
            Some(Instant::now() - PRUNE_INTERVAL - Duration::from_secs(1));
        assert_eq!(sessions.has_capacity(VICTIM).await, Capacity::Available);
        assert_eq!(sessions.len().await, 0);
    }

    #[tokio::test]
    async fn test_failed_or_foreign_delete_keeps_the_binding() {
        // (a) a DELETE the inner service answers with 500 keeps the binding.
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let sessions = SessionTokens::new();
        sessions.bind(SESSION, VICTIM.to_string()).await;
        let auth = test_auth(sessions.clone(), &server.uri());
        let failing = Router::new()
            .route(
                "/",
                axum::routing::delete(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
            )
            .layer(middleware::from_fn_with_state(
                auth.clone(),
                bearer_auth_middleware,
            ))
            .with_state(auth);
        let resp = failing
            .oneshot(open_req("DELETE", VICTIM, Some(SESSION)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(sessions.len().await, 1);

        // (b) a DELETE with a different bearer is 404'd and the owner keeps the session.
        let router = app(sessions.clone()).await;
        let resp = router
            .clone()
            .oneshot(open_req("DELETE", ATTACKER, Some(SESSION)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let resp = router
            .oneshot(open_req("POST", VICTIM, Some(SESSION)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(sessions.len().await, 1);
    }

    #[tokio::test]
    async fn test_rejected_token_is_not_probed_again_within_ttl() {
        let server = MockServer::start().await;
        mock_me(&server, 401).await;
        let router = app_with(SessionTokens::new(), &server.uri());
        for _ in 0..2 {
            let resp = router
                .clone()
                .oneshot(open_req("POST", VICTIM, None))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
            assert!(resp.headers().contains_key(WWW_AUTHENTICATE));
        }
        assert_eq!(me_calls(&server).await, 1);
    }

    #[tokio::test]
    async fn test_sessionless_non_initialize_post_is_rejected_without_probe() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let router = app_with(SessionTokens::new(), &server.uri());
        let build = |version: &str, body: &'static str| {
            HttpRequest::builder()
                .method("POST")
                .uri("/")
                .header(AUTHORIZATION, format!("Bearer {VICTIM}"))
                .header("mcp-protocol-version", version)
                .body(Body::from(body))
                .unwrap()
        };
        // Header claims an old revision but the body carries stateless `_meta`.
        let meta_call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_catalogs","arguments":{},"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-06-18","io.modelcontextprotocol/clientCapabilities":{}}}}"#;
        for (version, body) in [
            ("2025-06-18", meta_call),
            ("2026-07-28", meta_call),
            ("2025-06-18", ""),
            ("2025-06-18", "not json"),
        ] {
            let resp = router.clone().oneshot(build(version, body)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{version} {body}");
        }
        assert_eq!(me_calls(&server).await, 0);
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
        let resp = router.oneshot(build("2025-06-18", init)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(me_calls(&server).await, 1);
    }

    #[tokio::test]
    async fn test_sessionless_bad_prefix_with_non_initialize_body_is_401_not_400() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let router = app_with(SessionTokens::new(), &server.uri());
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/")
            .header(AUTHORIZATION, "Bearer x")
            .body(Body::from(vec![b'a'; MAX_INITIALIZE_BYTES + 1]))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(resp.headers().contains_key(WWW_AUTHENTICATE));
        assert_eq!(me_calls(&server).await, 0);
    }

    #[tokio::test]
    async fn test_sessionless_post_over_initialize_cap_is_413_without_probe() {
        let server = MockServer::start().await;
        mock_me(&server, 200).await;
        let router = app_with(SessionTokens::new(), &server.uri());
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/")
            .header(AUTHORIZATION, format!("Bearer {VICTIM}"))
            .body(Body::from(vec![b'a'; MAX_INITIALIZE_BYTES + 1]))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(me_calls(&server).await, 0);
    }
}
