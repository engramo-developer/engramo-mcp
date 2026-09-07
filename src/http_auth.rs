//! `http` mode's authentication edge: bearer-token extraction, per-session
//! credential binding, and the task-local that carries the token into rmcp's
//! session factory.
//!
//! # Why session binding exists
//!
//! rmcp's `StreamableHttpService` authorizes every request *after* `initialize`
//! on the `Mcp-Session-Id` header alone — it re-checks nothing else (see
//! `handle_post` in `rmcp::transport::streamable_http_server::tower`). Since a
//! session owns one `EngramClient` built from the token presented at
//! `initialize`, anyone replaying a live session id with *any* bearer token
//! would otherwise act as that session's owner, with their EngrAmo token, and
//! would keep doing so after the owner rotated it. [`SessionTokens`] closes that
//! by remembering which token opened each session and rejecting a mismatch.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Request, State},
    http::{Method, StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tokio::sync::RwLock;

/// Header rmcp uses to carry the MCP session id, on both requests and the
/// `initialize` response.
const HEADER_SESSION_ID: &str = "mcp-session-id";

/// Upper bound on an accepted bearer token. EngrAmo API tokens are far shorter;
/// this only stops a caller from making the server allocate state for a
/// megabyte-long "token" before anything else looks at it.
const MAX_TOKEN_LEN: usize = 4096;

/// How long a session→token binding is kept after its last use. rmcp closes an
/// idle session after 5 minutes (`SessionConfig::DEFAULT_KEEP_ALIVE`), so an
/// hour is generous; the only cost of keeping one too long is a map entry.
const BINDING_TTL: Duration = Duration::from_secs(3600);

/// Hard cap on tracked sessions. Reached only under a session-creation flood
/// (every entry is ~100 bytes); new sessions are refused with 503 rather than
/// silently dropping bindings, which would weaken the check this module exists
/// to enforce.
const MAX_TRACKED_SESSIONS: usize = 10_000;

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

/// Remembers which bearer token opened each MCP session, so later requests
/// carrying that session id must present the same token.
#[derive(Clone, Default)]
pub struct SessionTokens {
    inner: Arc<RwLock<HashMap<String, Binding>>>,
}

struct Binding {
    token: String,
    last_seen: Instant,
}

/// Outcome of checking a request's session id against its bearer token.
#[derive(Debug, PartialEq, Eq)]
enum SessionCheck {
    /// Token matches the one that opened this session, or the session is
    /// unknown to us (rmcp answers those with 404 on its own).
    Ok,
    /// A different token than the one that opened this session — reject.
    Mismatch,
}

impl SessionTokens {
    pub fn new() -> Self {
        Self::default()
    }

    async fn check(&self, session_id: &str, token: &str) -> SessionCheck {
        let mut guard = self.inner.write().await;
        match guard.get_mut(session_id) {
            Some(binding) if constant_time_eq(&binding.token, token) => {
                binding.last_seen = Instant::now();
                SessionCheck::Ok
            }
            Some(_) => SessionCheck::Mismatch,
            None => SessionCheck::Ok,
        }
    }

    async fn bind(&self, session_id: &str, token: String) {
        let mut guard = self.inner.write().await;
        guard.retain(|_, b| b.last_seen.elapsed() < BINDING_TTL);
        guard.insert(
            session_id.to_string(),
            Binding {
                token,
                last_seen: Instant::now(),
            },
        );
    }

    async fn unbind(&self, session_id: &str) {
        self.inner.write().await.remove(session_id);
    }

    /// True when a new session can still be tracked. Prunes expired bindings
    /// first, so the cap only bites under a genuine flood.
    async fn has_capacity(&self) -> bool {
        let mut guard = self.inner.write().await;
        if guard.len() < MAX_TRACKED_SESSIONS {
            return true;
        }
        guard.retain(|_, b| b.last_seen.elapsed() < BINDING_TTL);
        guard.len() < MAX_TRACKED_SESSIONS
    }

    #[cfg(test)]
    async fn len(&self) -> usize {
        self.inner.read().await.len()
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
/// malformed bearer tokens are rejected with 401 at the edge — before ever
/// reaching the rmcp session factory (which would 500 on a missing task-local,
/// since it has no HTTP-status-aware rejection path of its own).
///
/// A request that carries an `Mcp-Session-Id` must present the same token that
/// opened that session; see the module docs for why rmcp cannot enforce this.
pub async fn bearer_auth_middleware(
    State(sessions): State<SessionTokens>,
    req: Request,
    next: Next,
) -> Response {
    let Some(token) = extract_bearer(req.headers()) else {
        return (
            StatusCode::UNAUTHORIZED,
            "Missing or malformed Authorization: Bearer <token> header",
        )
            .into_response();
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
                return (
                    StatusCode::UNAUTHORIZED,
                    "Bearer token does not match this MCP session",
                )
                    .into_response();
            }
        }
        // No session id yet — this request may open one, so make sure we can
        // still track the binding before letting it allocate server state.
        None => {
            if !sessions.has_capacity().await {
                tracing::error!("session binding table is full — refusing new MCP sessions");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Too many active sessions, try again later",
                )
                    .into_response();
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
        sessions.bind(new_id, token).await;
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

    const VICTIM: &str = "engram_victim_token";
    const ATTACKER: &str = "engram_attacker_token";
    const SESSION: &str = "6ea7c668-e947-446f-9e8f-7977869284b7";

    /// A stand-in for rmcp's service: echoes the token it saw via the
    /// task-local (as the real session factory does) and mints a session id on
    /// a request that has none, exactly like an `initialize` response.
    fn app(sessions: SessionTokens) -> Router {
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
                sessions.clone(),
                bearer_auth_middleware,
            ))
            .with_state(sessions)
    }

    fn request(token: Option<&str>, session: Option<&str>) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder().method("POST").uri("/");
        if let Some(t) = token {
            b = b.header(AUTHORIZATION, t);
        }
        if let Some(s) = session {
            b = b.header(HEADER_SESSION_ID, s);
        }
        b.body(Body::empty()).unwrap()
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
            .oneshot(request(None, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_middleware_rejects_empty_bearer_token() {
        let resp = app(SessionTokens::new())
            .oneshot(request(Some("Bearer "), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_middleware_rejects_non_bearer_scheme() {
        let resp = app(SessionTokens::new())
            .oneshot(request(Some("Basic dXNlcjpwYXNz"), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_middleware_rejects_oversized_token() {
        let huge = format!("Bearer {}", "a".repeat(MAX_TOKEN_LEN + 1));
        let resp = app(SessionTokens::new())
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
            .oneshot(request(Some("Bearer engram tok"), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_middleware_accepts_lowercase_bearer_scheme() {
        // RFC 7235 §2.1: the auth-scheme token is case-insensitive.
        let resp = app(SessionTokens::new())
            .oneshot(request(Some(&format!("bearer {VICTIM}")), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_string(resp).await, VICTIM);
    }

    #[tokio::test]
    async fn test_middleware_scopes_token_for_the_inner_service() {
        let resp = app(SessionTokens::new())
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
            .oneshot(request(Some(&format!("Bearer {VICTIM}")), None))
            .await
            .unwrap();

        let resp = app(sessions.clone())
            .oneshot(request(Some(&format!("Bearer {ATTACKER}")), Some(SESSION)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(body_string(resp).await.contains("does not match"));
    }

    #[tokio::test]
    async fn test_session_accepts_the_token_that_opened_it() {
        let sessions = SessionTokens::new();
        app(sessions.clone())
            .oneshot(request(Some(&format!("Bearer {VICTIM}")), None))
            .await
            .unwrap();

        let resp = app(sessions.clone())
            .oneshot(request(Some(&format!("Bearer {VICTIM}")), Some(SESSION)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_string(resp).await, VICTIM);
    }

    #[tokio::test]
    async fn test_unknown_session_is_passed_through_for_rmcp_to_404() {
        let sessions = SessionTokens::new();
        let resp = app(sessions)
            .oneshot(request(
                Some(&format!("Bearer {ATTACKER}")),
                Some("11111111-1111-1111-1111-111111111111"),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_delete_releases_the_binding() {
        let sessions = SessionTokens::new();
        app(sessions.clone())
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
        let resp = app(sessions.clone()).oneshot(req).await.unwrap();
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
        assert!(constant_time_eq("engram_abc", "engram_abc"));
        assert!(!constant_time_eq("engram_abc", "engram_abd"));
        assert!(!constant_time_eq("engram_abc", "engram_abcd"));
        assert!(!constant_time_eq("", "x"));
        assert!(constant_time_eq("", ""));
    }
}
