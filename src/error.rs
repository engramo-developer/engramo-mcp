use serde::Deserialize;
use thiserror::Error;

use crate::subscription::{feature_label, format_reset, next_reset, resource_label};

/// How a user changes plan, appended to every quota / plan-feature message and returned as
/// `howToChange` by `get_subscription_usage`. There is no self-service plan management yet
/// (plans are changed by the EngrAmo team), so never point at an app screen here.
pub const PLAN_CHANGE_HINT: &str =
    "Plan changes aren't self-service yet — contact the EngrAmo team to change your plan.";

/// Structured error body returned by the Engramo API for quota exceeded (HTTP 429).
#[derive(Debug, Deserialize)]
pub struct QuotaExceededBody {
    pub resource_type: Option<String>,
    pub used: Option<i64>,
    pub limit: Option<i64>,
    /// Size of the rejected request; explains a rejection while `used < limit`.
    pub requested: Option<i64>,
    pub plan_id: Option<String>,
    /// `monthly`, `absolute`, `daily` or `hourly` — the window that ran out.
    pub window: Option<String>,
}

/// `code` of the API's 403 body when the caller's plan doesn't unlock a feature.
const FEATURE_NOT_ENTITLED_CODE: &str = "FEATURE_NOT_ENTITLED";

/// Structured 403 body for a plan-feature denial; a plain ownership 403 is just
/// `{"error":"Forbidden"}` and has no `code`.
#[derive(Debug, Deserialize)]
struct FeatureNotEntitledBody {
    code: Option<String>,
    feature: Option<String>,
    plan_id: Option<String>,
}

/// Keeps an identifier from an upstream body only if it looks like one (`snake_case`, short),
/// so it can be echoed into a message without carrying markup or control characters.
fn ident(raw: Option<String>) -> Option<String> {
    raw.filter(|s| {
        !s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    })
}

/// Short, escaped preview of an untrusted (LLM-supplied) value for error text / logs —
/// caps how much of a malformed, possibly multi-MB value ends up in `err_result`'s WARN log
/// line or the tool result, and escapes embedded control characters (e.g. CR/LF) so they
/// can't forge extra log lines under a non-JSON log formatter.
pub(crate) fn preview(s: &str) -> String {
    const MAX: usize = 64;
    let mut out: String = s.chars().take(MAX).flat_map(char::escape_debug).collect();
    if s.chars().count() > MAX {
        out.push('…');
    }
    out
}

/// Errors produced by the Engramo HTTP client.
/// These are always converted to `Ok(CallToolResult { is_error: true })` at the tool level —
/// never returned as a raw Rust `Err()`, which would crash the AI's tool-calling loop.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    /// The resource was modified by another client; the AI should re-fetch and retry.
    #[error("Conflict: {0}")]
    Conflict(String),

    /// HTTP 402/429. Fields are `None` when the body didn't carry them (an older API build,
    /// or an unparseable body); the message then says the details are unavailable.
    #[error("{}", quota_message(.resource_type, *.used, *.limit, *.requested, .plan_id.as_deref(), .window.as_deref(), resets_at_ref(.resets_at)))]
    QuotaExceeded {
        resource_type: String,
        used: Option<i64>,
        limit: Option<i64>,
        requested: Option<i64>,
        plan_id: Option<Box<str>>,
        window: Option<String>,
        /// Computed once when the 429 arrived, so `Display` stays a pure function of the fields.
        /// `plan_id` and this are boxed to keep `ApiError` under clippy's `result_large_err` threshold.
        resets_at: Option<Box<chrono::DateTime<chrono::Utc>>>,
    },

    /// HTTP 403 with `code: FEATURE_NOT_ENTITLED`: the caller's plan doesn't include the
    /// feature. Retrying, or retrying with another id, won't help.
    #[error("{}", feature_message(.feature, .plan_id.as_deref()))]
    FeatureNotEntitled {
        feature: String,
        plan_id: Option<String>,
    },

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error(
        "Unauthorized: EngrAmo authorization is invalid, expired, or revoked. Reconnect EngrAmo in your AI app (local installs: update ENGRAMO_API_TOKEN)."
    )]
    Unauthorized,

    #[error(
        "Server error: the Engramo API returned an internal error. Please try again in a moment."
    )]
    Internal,

    #[error("Network error: {0}")]
    Network(reqwest::Error),

    /// A 2xx response body that doesn't match the DTO we expect (e.g. a missing/renamed
    /// field). Distinct from `Network` — reqwest reports a decode failure as a vague
    /// "error decoding response body" and drops the serde detail (see issue #35); this
    /// variant carries a value-free serde summary (see [`ApiError::decode`]) through to the
    /// tool result so the calling LLM/user sees the real field name instead of "Network error".
    ///
    /// The version-mismatch hint lives in the detail (see [`ApiError::decode`]), not here: it
    /// only applies to field/`Data` errors, not to a truncated body or a proxy's HTML page.
    #[error("Unexpected response from the Engramo API ({0}).")]
    Decode(String),

    /// A response body larger than the client's read cap. Distinct from `Decode` — the
    /// fault is an abnormal upstream/proxy, not a version mismatch between engramo-mcp and
    /// the API.
    #[error(
        "The Engramo API returned an unexpectedly large response (over {0} bytes). Please try again later or narrow the request."
    )]
    ResponseTooLarge(usize),

    /// An overall deadline expired before the Engramo API finished responding (e.g. a
    /// multi-page resource read). Distinct from `BadRequest` — there is no caller input to
    /// blame; the upstream was slow.
    #[error("The Engramo API did not finish responding in time ({0}). Please try again later.")]
    Timeout(&'static str),
}

/// Upper bound on a non-2xx response body read into memory; only a short message is ever
/// extracted from it.
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// Reads a response body into memory, stopping once `max` bytes are buffered. Returns the
/// buffered bytes and whether the body was cut short at the cap. The single bounded reader
/// shared by the 2xx path (`EngramoClient::deserialize`) and the error path
/// ([`ApiError::from_response`]).
pub(crate) async fn read_bounded(
    mut resp: reqwest::Response,
    max: usize,
) -> Result<(Vec<u8>, bool), reqwest::Error> {
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        let room = max - buf.len();
        if chunk.len() > room {
            buf.extend_from_slice(&chunk[..room]);
            return Ok((buf, true));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok((buf, false))
}

impl From<reqwest::Error> for ApiError {
    /// Connectivity failures to the Engramo API (DNS, TLS, timeout, connection
    /// refused) are a genuine system failure, not a routine tool-call outcome —
    /// log at ERROR here so it reaches GCP Error Reporting (see `error_reporting`).
    ///
    /// The URL is stripped first: a `reqwest::Error`'s Display embeds the full request URL,
    /// which carries the internal upstream host and user-typed query strings (`q=`, `cursor=`)
    /// that must reach neither the logs nor the MCP client.
    fn from(e: reqwest::Error) -> Self {
        let e = e.without_url();
        tracing::error!(error = %e, "network error calling Engramo API");
        Self::Network(e)
    }
}

impl ApiError {
    /// Whether this error will recur for every remaining item of a batch (same token, same
    /// plan), so a per-item loop should stop instead of firing the rest of its requests.
    pub fn stops_batch(&self) -> bool {
        matches!(
            self,
            Self::Unauthorized | Self::QuotaExceeded { .. } | Self::FeatureNotEntitled { .. }
        )
    }

    /// Builds a `Decode` error from a `serde_json` failure while parsing an otherwise
    /// successful (2xx) response body. Logs at ERROR — this is a genuine contract mismatch
    /// between this crate's DTOs and the real API, not a routine tool-call outcome — but
    /// logs only the serde error *category* (`e.classify()`) plus `line()`/`column()`, never
    /// `e.to_string()` in full: the full message can echo back values from the response body
    /// (e.g. a string that failed an enum match), which may contain user content and must
    /// not reach stderr/Cloud Logging.
    ///
    /// The variant's message is built the same way, because `err_result` logs it again at WARN
    /// and it is returned to the (possibly remote, multi-tenant) client: only "missing /
    /// unknown / duplicate field `x`" messages are kept verbatim (they name DTO fields, which
    /// is what #35 needs), capped in length; everything else collapses to a category +
    /// position summary.
    pub fn decode(e: serde_json::Error) -> Self {
        use serde_json::error::Category;
        const MAX_DETAIL_LEN: usize = 256;
        tracing::error!(
            category = ?e.classify(),
            line = e.line(),
            column = e.column(),
            "failed to decode Engramo API response body"
        );
        let raw = e.to_string();
        let is_field_error = matches!(e.classify(), Category::Data)
            && ["missing field", "unknown field", "duplicate field"]
                .iter()
                .any(|p| raw.starts_with(p));
        let detail = if is_field_error {
            format!(
                "{}; this is a version mismatch between engramo-mcp and the API — retrying will \
                 not help, tell the user to update engramo-mcp",
                raw.chars().take(MAX_DETAIL_LEN).collect::<String>()
            )
        } else if matches!(e.classify(), Category::Data) {
            format!(
                "Data error at line {} column {}; likely a version mismatch — tell the user to \
                 update engramo-mcp",
                e.line(),
                e.column()
            )
        } else {
            format!(
                "{:?} error at line {} column {}",
                e.classify(),
                e.line(),
                e.column()
            )
        };
        Self::Decode(detail)
    }

    /// Parse an API error from an HTTP response status + body.
    /// This is the single place that converts HTTP semantics into typed errors.
    pub async fn from_response(response: reqwest::Response) -> Self {
        let status = response.status();
        // Must be read before `read_bounded` consumes the response body below.
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let bytes = read_bounded(response, MAX_ERROR_BODY_BYTES)
            .await
            .map(|(bytes, _)| bytes)
            .unwrap_or_default();
        let body = String::from_utf8_lossy(&bytes).into_owned();

        match status.as_u16() {
            401 => Self::Unauthorized,
            300..=399 => {
                // The client hardcodes `redirect::Policy::none()` (see
                // `EngramoClient::build_http_client`) so the API key is never forwarded to an
                // arbitrary `Location`. A 3xx here is a routine config mistake (e.g.
                // `ENGRAMO_API_URL=http://…` being redirected to `https://…`), not a backend
                // failure — log at WARN, not ERROR, so it doesn't page as an incident.
                //
                // Redact once and reuse the same value for the log and the client-facing
                // message: the raw `Location` can carry userinfo or a query string (e.g. a
                // signed URL, `?token=…`) that must never reach stderr/Cloud Logging or the
                // MCP client.
                let location = location.as_deref().map(redact_location);
                tracing::warn!(status = %status, location = ?location, "Engramo API responded with a redirect (not followed)");
                Self::BadRequest(format!(
                    "Engramo API responded with a redirect (HTTP {status}{}); redirects are not \
                     followed so the API key is never forwarded. Check ENGRAMO_API_URL (e.g. use \
                     https:// or the final host directly).",
                    location
                        .as_deref()
                        .map(|l| format!(" to {l}"))
                        .unwrap_or_default()
                ))
            }
            403 => match serde_json::from_str::<FeatureNotEntitledBody>(&body) {
                Ok(b) if b.code.as_deref() == Some(FEATURE_NOT_ENTITLED_CODE) => {
                    Self::FeatureNotEntitled {
                        feature: ident(b.feature).unwrap_or_else(|| "unknown".to_string()),
                        plan_id: ident(b.plan_id),
                    }
                }
                _ => Self::PermissionDenied(forbidden_hint(strip_duplicate_prefix(
                    "Permission denied",
                    extract_error_message(&body),
                ))),
            },
            404 => Self::NotFound(dedup_or_raw("Not found", extract_error_message(&body))),
            409 => Self::Conflict(
                "resource was modified by another client. Fetch the latest version and retry."
                    .to_string(),
            ),
            402 | 429 => {
                let q = serde_json::from_str::<QuotaExceededBody>(&body).ok();
                let q = q.as_ref();
                let window = ident(q.and_then(|q| q.window.clone()));
                let resets_at = window
                    .as_deref()
                    .and_then(|w| next_reset(w, chrono::Utc::now()))
                    .map(Box::new);
                Self::QuotaExceeded {
                    resource_type: ident(q.and_then(|q| q.resource_type.clone()))
                        .unwrap_or_else(|| "unknown".to_string()),
                    used: q.and_then(|q| q.used),
                    limit: q.and_then(|q| q.limit),
                    requested: q.and_then(|q| q.requested),
                    plan_id: ident(q.and_then(|q| q.plan_id.clone())).map(String::into_boxed_str),
                    window,
                    resets_at,
                }
            }
            // 422 is axum's rejection status for a request body that parses as JSON but
            // fails to deserialize into the target type (e.g. an invalid enum variant like
            // `style.textAlign: "diagonal"`) — a client input mistake, same as 400, not a
            // transient server failure. The rejection text names the exact field and valid
            // values, which is directly actionable for the calling LLM to correct and retry.
            400 | 422 => {
                Self::BadRequest(dedup_or_raw("Bad request", extract_error_message(&body)))
            }
            _ => {
                // Unrecognized/5xx status — a genuine backend failure, not a routine
                // tool-call outcome. Log at ERROR so it reaches GCP Error Reporting.
                const MAX_BODY_LEN: usize = 512;
                let truncated_body = if body.len() > MAX_BODY_LEN {
                    let cut = (0..=MAX_BODY_LEN)
                        .rev()
                        .find(|&i| body.is_char_boundary(i))
                        .unwrap_or(0);
                    format!("{}...[truncated]", &body[..cut])
                } else {
                    body.clone()
                };
                tracing::error!(
                    status = %status,
                    body = %truncated_body,
                    "Engramo API returned an unexpected status"
                );
                Self::Internal
            }
        }
    }
}

/// Text of [`ApiError::QuotaExceeded`]: names the limit, its window and reset time (UTC),
/// separates "this batch is too big" from "you're at the limit" and "not in your plan", and
/// always ends by pointing at `get_subscription_usage` and [`PLAN_CHANGE_HINT`]. Keeps the
/// `Quota exceeded:` prefix callers and tests match on.
fn quota_message(
    resource_type: &str,
    used: Option<i64>,
    limit: Option<i64>,
    requested: Option<i64>,
    plan_id: Option<&str>,
    window: Option<&str>,
    resets_at: Option<chrono::DateTime<chrono::Utc>>,
) -> String {
    let what = match resource_label(resource_type) {
        Some(label) => format!("{label} ({resource_type})"),
        None => resource_type.to_string(),
    };
    let on_plan = plan_id
        .map(|p| format!(" on your {p} plan"))
        .unwrap_or_default();
    let mut msg = format!("Quota exceeded: {what}");
    match (used, limit) {
        (_, Some(0)) => msg.push_str(&format!(
            " isn't included in your {} plan.",
            plan_id.unwrap_or("current")
        )),
        (Some(used), Some(limit)) => {
            let kind = match window {
                Some("absolute") => "lifetime ",
                Some(w @ ("monthly" | "daily" | "hourly")) => &format!("{w} "),
                _ => "",
            };
            match requested {
                Some(req) if used < limit && used.saturating_add(req) > limit => {
                    let left = limit.saturating_sub(used);
                    msg.push_str(&format!(
                        " — this request needed {req}, but only {left} of the {kind}limit of {limit} \
                         {} left ({used} used{on_plan}). Try a smaller batch (up to {left}).",
                        if left == 1 { "is" } else { "are" },
                    ));
                }
                _ => msg.push_str(&format!(
                    " — {kind}limit reached: {used} of {limit} used{on_plan}."
                )),
            }
            match window {
                Some("absolute") => msg.push_str(" This limit doesn't reset."),
                _ => {
                    if let Some(at) = resets_at {
                        msg.push_str(&format!(" It resets {}.", format_reset(at)));
                    }
                }
            }
            msg.push_str(match resource_type {
                "catalogs_total" => " Deleting catalogs you no longer need frees room.",
                "cards_total" => " Deleting cards you no longer need frees room.",
                "cards_per_catalog" => {
                    " The limit applies to each catalog separately: put further cards in \
                     another catalog, or delete cards you no longer need from this one."
                }
                "shared_catalogs" => {
                    " Stop sharing a catalog you no longer need to share (unshare_catalog \
                     removes everyone from it) to free a slot."
                }
                "collaborators_per_catalog" => {
                    " Remove people who no longer need access with unshare_catalog."
                }
                _ => "",
            });
        }
        _ => msg.push_str(" limit reached (limit details unavailable)."),
    }
    msg.push_str(" Call get_subscription_usage to see all your limits and usage. ");
    msg.push_str(PLAN_CHANGE_HINT);
    msg
}

/// Unboxes `QuotaExceeded::resets_at` for the `#[error]` format expression.
fn resets_at_ref(
    at: &Option<Box<chrono::DateTime<chrono::Utc>>>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    at.as_deref().copied()
}

/// Text of [`ApiError::FeatureNotEntitled`].
fn feature_message(feature: &str, plan_id: Option<&str>) -> String {
    let what = match feature_label(feature) {
        Some(label) => format!("{label} ({feature})"),
        None => feature.to_string(),
    };
    let plan = plan_id
        .map(|p| format!("the {p} plan"))
        .unwrap_or_else(|| "your current plan".to_string());
    format!(
        "Not included in your plan: {what} isn't available on {plan}. Call \
         get_subscription_usage to see what your plan includes. {PLAN_CHANGE_HINT}"
    )
}

/// Redacts a redirect `Location` before it is echoed to the caller or written to logs — a
/// redirect target can embed userinfo (`user:pass@host`) or a sensitive query string (e.g. a
/// signed URL's `?token=…`), and this value is both logged (WARN) and surfaced to the MCP
/// client. Strips userinfo, drops the query string and fragment, and caps the result length
/// so a maliciously long `Location` can't bloat logs or the tool response.
fn redact_location(location: &str) -> String {
    const MAX_LOCATION_LEN: usize = 256;
    const DUMMY_HOST: &str = "redacted.invalid";
    fn scrub(mut u: reqwest::Url) -> reqwest::Url {
        let _ = u.set_username("");
        let _ = u.set_password(None);
        u.set_query(None);
        u.set_fragment(None);
        u
    }
    let redacted = match reqwest::Url::parse(location) {
        Ok(u) => scrub(u).to_string(),
        // Relative reference: let the WHATWG parser (which strips tab/newline and treats
        // '\' as '/') decide whether there is an authority — never a string heuristic. A
        // hand-rolled check on the raw string can be bypassed by characters the parser
        // strips (e.g. a tab) before it ever sees an authority.
        Err(_) => {
            match reqwest::Url::parse("https://redacted.invalid/").and_then(|b| b.join(location)) {
                // Path-only reference: no authority, so echo just the parsed path — never the
                // raw input, since the parser may have normalized tabs/backslashes in it.
                Ok(u) if u.host_str() == Some(DUMMY_HOST) => u.path().to_string(),
                // Network-path reference: authority parsed, userinfo stripped.
                Ok(u) => scrub(u).to_string(),
                Err(_) => "<unparseable Location>".to_string(),
            }
        }
    };
    redacted.chars().take(MAX_LOCATION_LEN).collect()
}

/// Strips a leading, case-insensitive duplicate of `label` (followed by `:` and optional
/// whitespace) from `msg` — see issue #54B. The backend's own error body is often already
/// prefixed with the same word this variant's `Display` impl adds (e.g. a 404 body of
/// `"Not found: Learning path X not found"` wrapped as `ApiError::NotFound(..)`), which would
/// otherwise double up as `"Not found: Not found: Learning path X not found"`. A message that
/// doesn't start with the prefix is returned unchanged.
fn strip_duplicate_prefix(label: &str, msg: String) -> String {
    let mut chars = msg.trim_start().chars();
    for lc in label.chars() {
        match chars.next() {
            Some(c) if c.eq_ignore_ascii_case(&lc) => continue,
            _ => return msg,
        }
    }
    match chars.next() {
        Some(':') => chars.as_str().trim_start().to_string(),
        _ => msg,
    }
}

/// Like [`strip_duplicate_prefix`], but falls back to the original, unstripped `raw` message
/// when stripping would leave nothing (e.g. a backend body of exactly `"Not found:"`) — an
/// empty remainder tells the calling LLM less than the raw body did. Used by the 404 and
/// 400/422 arms of [`ApiError::from_response`]; the 403 arm calls `strip_duplicate_prefix`
/// directly because `forbidden_hint` turns an empty string into an actionable hint.
fn dedup_or_raw(label: &str, raw: String) -> String {
    let stripped = strip_duplicate_prefix(label, raw.clone());
    if stripped.trim().is_empty() {
        raw
    } else {
        stripped
    }
}

/// Expands a bare `"Forbidden"` (or empty) 403 body into an actionable hint — see #54A/#52.
/// The backend collapses every 403 body to the literal "Forbidden", which also covers "the id
/// doesn't exist", "it belongs to another user", and "the action needs a higher role" (e.g.
/// setting catalog visibility to 'public'/'unlisted'), so the bare word alone gives the caller
/// nothing to act on. Any other, more specific 403 message is left untouched.
fn forbidden_hint(msg: String) -> String {
    let bare = msg.trim();
    if bare.is_empty() || bare.eq_ignore_ascii_case("forbidden") {
        "Forbidden (the resource may not exist, may belong to another user, or the action — \
         e.g. setting catalog visibility to 'public'/'unlisted' — may require a higher role)"
            .to_string()
    } else {
        msg
    }
}

/// Extract a human-readable message from a JSON `{ "error": "..." }` body. The message is
/// returned to the (possibly remote, multi-tenant) MCP client and logged, so a non-JSON body
/// (e.g. a proxy's error page) is never passed through as markup, control characters are
/// replaced with spaces, and the length is capped. Axum's plain-text 422 rejection stays usable.
fn extract_error_message(body: &str) -> String {
    const MAX_MESSAGE_LEN: usize = 512;
    #[derive(Deserialize)]
    struct ErrorBody {
        error: Option<String>,
    }
    let raw = serde_json::from_str::<ErrorBody>(body)
        .ok()
        .and_then(|b| b.error)
        .unwrap_or_else(|| {
            if body.trim_start().starts_with('<') {
                "upstream returned a non-JSON error page".to_string()
            } else {
                body.to_string()
            }
        });
    raw.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_MESSAGE_LEN)
        .collect()
}

/// Test-only helpers shared by the tracing-assertion tests in this crate.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Arc, Mutex};

    /// Serializes every test in the crate that installs a capturing `tracing` subscriber as
    /// the (thread-local or global) default.
    ///
    /// `tracing-core`'s per-callsite interest cache and its global max-level hint are
    /// process-wide state, not per-thread: installing or dropping a dispatcher on one test
    /// thread can change whether a *different* thread's concurrently-running subscriber
    /// receives an event at all (a callsite once cached as "no subscriber is interested"
    /// stays that way for every thread until something rebuilds the cache). Under the default
    /// parallel test runner this made log-capturing tests flaky depending on what else
    /// happened to be running at the same time.
    ///
    /// Every test that installs a capturing subscriber (`tracing::subscriber::set_default` /
    /// `with_default`) must hold this lock for the subscriber's entire lifetime, from before
    /// it is installed until after the captured buffer has been read. That guarantees at most
    /// one capturing subscriber — and one `rebuild_interest_cache()` call, where used — is
    /// ever active at a time, so no other thread's subscriber can be starved by a stale
    /// "never interested" cache entry.
    ///
    /// This is a `tokio::sync::Mutex`, not `std::sync::Mutex`: some of the tests that need it
    /// hold the guard across `.await` points (fine under `#[tokio::test]`'s default
    /// current-thread runtime, but `clippy::await_holding_lock` rightly flags a std guard
    /// there), and sync tests take it with `blocking_lock()` instead. As a bonus, a
    /// `tokio::sync::Mutex` never poisons, so a test panicking while holding it cannot
    /// cascade into unrelated test failures — no `unwrap_or_else` recovery dance needed.
    ///
    /// A test that does *not* install a capturing subscriber itself must still take this lock
    /// if it is the first to touch a callsite that some *other*, capturing test asserts on:
    /// `rebuild_interest_cache()` inside the capturing test does not reliably undo a `never`
    /// interest already cached by a concurrent, unlocked touch of the same callsite. This was
    /// observed empirically: when the non-capturing
    /// `test_from_response_redirect_is_bad_request_with_actionable_message` ran without the lock,
    /// the capturing `test_from_response_redirect_logs_redacted_location_without_token` below
    /// (which shares the 300..=399 `tracing::warn!` callsite) failed to see its WARN line. The
    /// protection is best-effort: many other non-capturing tool-handler tests hit the same
    /// `err_result` and `ApiError::decode` callsites as capturing tests elsewhere in the crate
    /// (e.g. in `tools::catalogs`) without taking this lock at all.
    pub(crate) static TRACING_CAPTURE_LOCK: tokio::sync::Mutex<()> =
        tokio::sync::Mutex::const_new(());

    /// Minimal in-memory tracing writer so tests can assert on what a log line actually
    /// contains, without pulling in a new dev-dependency.
    #[derive(Clone, Default)]
    pub(crate) struct BufWriter(pub(crate) Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'w> tracing_subscriber::fmt::MakeWriter<'w> for BufWriter {
        type Writer = Self;
        fn make_writer(&'w self) -> Self::Writer {
            self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::BufWriter;
    use super::*;

    #[test]
    fn test_decode_display() {
        let e = ApiError::Decode("missing field `total`".into());
        assert_eq!(
            e.to_string(),
            "Unexpected response from the Engramo API (missing field `total`)."
        );
    }

    #[test]
    fn test_timeout_display_does_not_blame_input() {
        let msg = ApiError::Timeout("resource read").to_string();
        assert!(msg.contains("did not finish responding in time"), "{msg}");
        assert!(msg.contains("resource read"), "{msg}");
        assert!(!msg.contains("Bad request"), "{msg}");
    }

    #[test]
    fn test_response_too_large_display() {
        let e = ApiError::ResponseTooLarge(1024);
        let msg = e.to_string();
        assert!(msg.contains("unexpectedly large response"), "{msg}");
        assert!(msg.contains("1024"), "{msg}");
        assert!(!msg.contains("update engramo-mcp"), "{msg}");
    }

    #[test]
    fn test_decode_field_error_hints_version_mismatch() {
        #[derive(serde::Deserialize, Debug)]
        #[allow(dead_code)]
        struct Probe {
            total: i64,
        }
        let e = serde_json::from_str::<Probe>("{}").unwrap_err();
        let msg = ApiError::decode(e).to_string();
        assert!(msg.contains("missing field `total`"), "{msg}");
        assert!(msg.contains("update engramo-mcp"), "{msg}");
    }

    #[test]
    fn test_decode_syntax_error_has_no_version_mismatch_hint() {
        // A 2xx HTML page from a captive proxy is not a version mismatch.
        let e = serde_json::from_str::<serde_json::Value>("<html>SECRET</html>").unwrap_err();
        match ApiError::decode(e) {
            ApiError::Decode(msg) => {
                assert!(msg.contains("Syntax error at line 1"), "{msg}");
                assert!(!msg.contains("SECRET"), "{msg}");
                assert!(!msg.contains("update engramo-mcp"), "{msg}");
            }
            other => panic!("expected Decode, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_truncated_body_has_no_version_mismatch_hint() {
        let e = serde_json::from_str::<serde_json::Value>(r#"{"dat"#).unwrap_err();
        match ApiError::decode(e) {
            ApiError::Decode(msg) => {
                assert!(msg.contains("Eof error"), "{msg}");
                assert!(!msg.contains("update engramo-mcp"), "{msg}");
            }
            other => panic!("expected Decode, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_message_truncates_long_field_error() {
        #[derive(serde::Deserialize, Debug)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct Probe {
            a: i64,
        }
        let e = serde_json::from_str::<Probe>(&format!(r#"{{"a":1,"{}":0}}"#, "x".repeat(1000)))
            .unwrap_err();
        match ApiError::decode(e) {
            ApiError::Decode(msg) => {
                assert!(msg.starts_with("unknown field"), "{msg}");
                // The serde message (before the appended hint) is capped at 256 chars.
                let serde_part = msg.split("; this is a version mismatch").next().unwrap();
                assert!(serde_part.chars().count() <= 256, "{}", serde_part.len());
                assert!(msg.len() < 600, "{}", msg.len());
            }
            other => panic!("expected Decode, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_logs_category_but_not_body_values() {
        #[derive(serde::Deserialize, Debug)]
        #[allow(dead_code)]
        enum Kind {
            A,
            B,
        }
        #[derive(serde::Deserialize, Debug)]
        #[allow(dead_code)]
        struct Probe {
            kind: Kind,
        }
        let e = serde_json::from_str::<Probe>(r#"{"kind":"SECRET_USER_TEXT"}"#).unwrap_err();

        // See `test_support::TRACING_CAPTURE_LOCK`: must be held for the capturing
        // subscriber's entire lifetime, not just around `set_default`.
        let _lock = test_support::TRACING_CAPTURE_LOCK.blocking_lock();

        let buf = BufWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        // Rebuild so the thread-local subscriber above actually sees the event, even if a
        // callsite was previously cached as "no subscriber interested" by another test. Safe
        // now that TRACING_CAPTURE_LOCK guarantees no other capturing subscriber is active
        // concurrently.
        tracing::callsite::rebuild_interest_cache();
        let err = ApiError::decode(e);
        drop(guard);

        let logged = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(!logged.contains("SECRET_USER_TEXT"), "{logged}");
        assert!(
            logged.contains("failed to decode Engramo API response body"),
            "{logged}"
        );
        assert!(logged.contains("Data"), "{logged}");
        // The variant message is value-free too: it is re-logged by `err_result` and returned
        // to the client.
        match err {
            ApiError::Decode(msg) => assert!(!msg.contains("SECRET_USER_TEXT"), "{msg}"),
            other => panic!("expected Decode, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_message_omits_value_for_type_mismatch() {
        #[derive(serde::Deserialize, Debug)]
        #[allow(dead_code)]
        struct Probe {
            total: i64,
        }
        let e = serde_json::from_str::<Probe>(r#"{"total":"user secret text"}"#).unwrap_err();
        match ApiError::decode(e) {
            ApiError::Decode(msg) => {
                assert!(!msg.contains("user secret text"), "{msg}");
                assert!(msg.contains("Data error at line 1"), "{msg}");
            }
            other => panic!("expected Decode, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_message_keeps_missing_field_name() {
        #[derive(serde::Deserialize, Debug)]
        #[allow(dead_code)]
        struct Probe {
            total: i64,
        }
        let e = serde_json::from_str::<Probe>("{}").unwrap_err();
        match ApiError::decode(e) {
            ApiError::Decode(msg) => assert!(msg.contains("missing field `total`"), "{msg}"),
            other => panic!("expected Decode, got {other:?}"),
        }
    }

    #[test]
    fn test_extract_error_message_json() {
        let msg = extract_error_message(r#"{"error":"Not found: catalog abc"}"#);
        assert_eq!(msg, "Not found: catalog abc");
    }

    #[test]
    fn test_extract_error_message_plain() {
        let msg = extract_error_message("plain text error");
        assert_eq!(msg, "plain text error");
    }

    #[test]
    fn test_extract_error_message_html_body_is_replaced_and_capped() {
        let html = format!("<html><body>{}</body></html>", "internal-host ".repeat(200));
        let msg = extract_error_message(&html);
        assert_eq!(msg, "upstream returned a non-JSON error page");
        assert!(!msg.contains('<'), "{msg}");
    }

    #[test]
    fn test_extract_error_message_caps_length_and_strips_control_chars() {
        let long = format!("line1\nline2\t{}", "a".repeat(2000));
        let msg = extract_error_message(&long);
        assert_eq!(msg.chars().count(), 512);
        assert!(msg.starts_with("line1 line2 aaa"), "{msg}");
        assert!(!msg.chars().any(|c| c.is_control()), "{msg}");
    }

    #[tokio::test]
    async fn test_from_response_404_html_body_over_limit_is_capped_without_markup() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let html = format!("<html><body>{}</body></html>", "x".repeat(2000));
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_string(html))
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        match ApiError::from_response(resp).await {
            ApiError::NotFound(msg) => {
                assert!(msg.chars().count() <= 512, "{}", msg.len());
                assert!(!msg.contains('<'), "{msg}");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_from_response_error_body_read_is_bounded() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // The only meaningful content sits after the read cap, so the result depends on the
        // read being bounded: an unbounded read would parse the JSON and return "LATE_MESSAGE".
        let body = format!(
            r#"{{"pad":"{}","error":"LATE_MESSAGE"}}"#,
            "x".repeat(MAX_ERROR_BODY_BYTES)
        );
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(400).set_body_string(body))
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        match ApiError::from_response(resp).await {
            ApiError::BadRequest(msg) => assert!(!msg.contains("LATE_MESSAGE"), "{msg}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_read_bounded_reports_truncation() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("0123456789"))
            .mount(&server)
            .await;

        let (bytes, truncated) = read_bounded(reqwest::get(server.uri()).await.unwrap(), 4)
            .await
            .unwrap();
        assert_eq!(bytes, b"0123");
        assert!(truncated);

        let (bytes, truncated) = read_bounded(reqwest::get(server.uri()).await.unwrap(), 10)
            .await
            .unwrap();
        assert_eq!(bytes, b"0123456789");
        assert!(!truncated, "a body of exactly `max` bytes is not truncated");
    }

    #[test]
    fn test_extract_error_message_empty() {
        let msg = extract_error_message("");
        assert_eq!(msg, "");
    }

    #[test]
    fn test_not_found_display() {
        let e = ApiError::NotFound("catalog abc-123 does not exist".to_string());
        assert_eq!(e.to_string(), "Not found: catalog abc-123 does not exist");
    }

    #[test]
    fn test_permission_denied_display() {
        let e = ApiError::PermissionDenied("you don't have edit access".to_string());
        assert_eq!(
            e.to_string(),
            "Permission denied: you don't have edit access"
        );
    }

    #[test]
    fn test_conflict_display() {
        let e = ApiError::Conflict(
            "resource was modified by another client. Fetch the latest version and retry."
                .to_string(),
        );
        assert!(e.to_string().contains("Conflict:"));
        assert!(e.to_string().contains("Fetch the latest version"));
    }

    fn quota(
        resource_type: &str,
        used: Option<i64>,
        limit: Option<i64>,
        requested: Option<i64>,
        plan_id: Option<&str>,
        window: Option<&str>,
    ) -> String {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-26T13:45:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let resets_at = window.and_then(|w| next_reset(w, now));
        quota_message(
            resource_type,
            used,
            limit,
            requested,
            plan_id,
            window,
            resets_at,
        )
    }

    #[test]
    fn test_error_texts_are_neutral() {
        let q = ApiError::QuotaExceeded {
            resource_type: "cards_total".into(),
            used: Some(1),
            limit: Some(1),
            requested: None,
            plan_id: None,
            window: None,
            resets_at: None,
        }
        .to_string();
        assert!(q.ends_with(PLAN_CHANGE_HINT), "{q}");
        assert!(
            !q.contains("EngrAmo app"),
            "no in-app plan management exists: {q}"
        );
        assert!(!q.to_lowercase().contains("upgrade"), "{q}");
        let f = ApiError::FeatureNotEntitled {
            feature: "ai_chat".into(),
            plan_id: Some("free".into()),
        }
        .to_string();
        assert!(!f.to_lowercase().contains("upgrade"), "{f}");
        let u = ApiError::Unauthorized.to_string();
        assert!(u.contains("Reconnect EngrAmo"), "{u}");
        assert!(!u.contains("Check your ENGRAMO_API_TOKEN"), "{u}");
    }

    #[test]
    fn test_quota_message_monthly_names_window_plan_and_reset() {
        let m = quota(
            "cards_total",
            Some(500),
            Some(500),
            Some(1),
            Some("basic"),
            Some("monthly"),
        );
        assert!(
            m.starts_with("Quota exceeded: Cards (total) (cards_total)"),
            "{m}"
        );
        assert!(
            m.contains("monthly limit reached: 500 of 500 used on your basic plan."),
            "{m}"
        );
        assert!(m.contains("It resets 2026-11-01 00:00 UTC."), "{m}");
        assert!(m.contains("Deleting cards you no longer need"), "{m}");
        assert!(m.contains("Call get_subscription_usage"), "{m}");
        assert!(m.ends_with(PLAN_CHANGE_HINT), "{m}");
    }

    #[test]
    fn test_quota_message_daily_and_hourly_resets() {
        let m = quota(
            "ai_agent_messages",
            Some(20),
            Some(20),
            Some(1),
            Some("free"),
            Some("daily"),
        );
        assert!(
            m.contains("AI chat messages (ai_agent_messages) — daily limit reached"),
            "{m}"
        );
        assert!(m.contains("It resets 2026-10-27 00:00 UTC."), "{m}");
        let m = quota(
            "translation_chunks",
            Some(20),
            Some(20),
            None,
            None,
            Some("hourly"),
        );
        assert!(m.contains("hourly limit reached: 20 of 20 used."), "{m}");
        assert!(m.contains("It resets 2026-10-26 14:00 UTC."), "{m}");
    }

    #[test]
    fn test_quota_message_absolute_says_it_never_resets() {
        let m = quota(
            "catalogs_total",
            Some(5),
            Some(5),
            Some(1),
            Some("free"),
            Some("absolute"),
        );
        assert!(
            m.contains("lifetime limit reached: 5 of 5 used on your free plan."),
            "{m}"
        );
        assert!(m.contains("This limit doesn't reset."), "{m}");
        assert!(m.contains("Deleting catalogs you no longer need"), "{m}");
        assert!(!m.contains("resets 20"), "{m}");
    }

    #[test]
    fn test_quota_message_batch_larger_than_headroom() {
        let m = quota(
            "cards_total",
            Some(480),
            Some(500),
            Some(30),
            Some("free"),
            Some("absolute"),
        );
        assert!(
            m.contains("this request needed 30, but only 20 of the lifetime limit of 500 are left"),
            "{m}"
        );
        assert!(m.contains("Try a smaller batch (up to 20)."), "{m}");
        assert!(
            !m.contains("limit reached"),
            "used < limit must not read as 'reached': {m}"
        );
    }

    #[test]
    fn test_quota_message_limit_zero_is_not_included() {
        let m = quota(
            "tts_generations",
            Some(0),
            Some(0),
            Some(1),
            Some("free"),
            Some("monthly"),
        );
        assert!(
            m.contains(
                "Text-to-speech generations (tts_generations) isn't included in your free plan."
            ),
            "{m}"
        );
        assert!(!m.contains("0 of 0"), "{m}");
    }

    #[test]
    fn test_quota_message_cards_per_catalog_hint() {
        let m = quota(
            "cards_per_catalog",
            Some(100),
            Some(100),
            Some(1),
            None,
            Some("absolute"),
        );
        assert!(m.contains("applies to each catalog separately"), "{m}");
        assert!(m.contains("another catalog"), "{m}");
    }

    #[test]
    fn test_quota_message_collaborators_per_catalog_batch_and_hint() {
        let m = quota(
            "collaborators_per_catalog",
            Some(4),
            Some(5),
            Some(3),
            None,
            Some("absolute"),
        );
        assert!(m.contains("People per shared catalog"), "{m}");
        assert!(m.contains("smaller batch (up to 1)"), "{m}");
        assert!(
            m.contains("Remove people who no longer need access with unshare_catalog."),
            "{m}"
        );
        assert!(m.contains("doesn't reset"), "{m}");
        assert!(!m.contains("resets 20"), "{m}");
        assert!(m.ends_with(PLAN_CHANGE_HINT), "{m}");
    }

    #[test]
    fn test_quota_message_shared_catalogs_lifetime_and_hint() {
        let m = quota(
            "shared_catalogs",
            Some(1),
            Some(1),
            Some(1),
            None,
            Some("absolute"),
        );
        assert!(m.contains("Shared catalogs"), "{m}");
        assert!(m.contains("lifetime limit reached"), "{m}");
        assert!(m.contains("unshare_catalog removes everyone"), "{m}");
        assert!(m.ends_with(PLAN_CHANGE_HINT), "{m}");
    }

    #[test]
    fn test_quota_message_without_details_never_shows_zero_over_zero() {
        let m = quota("unknown", None, None, None, None, None);
        assert!(
            m.starts_with("Quota exceeded: unknown limit reached (limit details unavailable)."),
            "{m}"
        );
        assert!(!m.contains("0/0") && !m.contains("0 of 0"), "{m}");
        assert!(m.ends_with(PLAN_CHANGE_HINT), "{m}");
        // An old body with used/limit but no window: no reset sentence, still clear.
        let m = quota("future_thing", Some(3), Some(3), None, None, None);
        assert!(
            m.starts_with("Quota exceeded: future_thing — limit reached: 3 of 3 used."),
            "{m}"
        );
        assert!(!m.contains("resets"), "{m}");
    }

    #[test]
    fn test_feature_message_names_feature_and_plan() {
        let f = ApiError::FeatureNotEntitled {
            feature: "ai_chat".into(),
            plan_id: Some("free".into()),
        }
        .to_string();
        assert!(
            f.starts_with(
                "Not included in your plan: AI chat (ai_chat) isn't available on the free plan."
            ),
            "{f}"
        );
        assert!(f.contains("Call get_subscription_usage"), "{f}");
        assert!(f.ends_with(PLAN_CHANGE_HINT), "{f}");
        let f = ApiError::FeatureNotEntitled {
            feature: "new_feature".into(),
            plan_id: None,
        }
        .to_string();
        assert!(
            f.contains("new_feature isn't available on your current plan."),
            "{f}"
        );
    }

    #[test]
    fn test_stops_batch() {
        assert!(ApiError::Unauthorized.stops_batch());
        assert!(
            ApiError::FeatureNotEntitled {
                feature: "ai_chat".into(),
                plan_id: None
            }
            .stops_batch()
        );
        assert!(!ApiError::NotFound("x".into()).stops_batch());
        assert!(!ApiError::PermissionDenied("x".into()).stops_batch());
    }

    #[test]
    fn test_unauthorized_display() {
        let e = ApiError::Unauthorized;
        assert!(e.to_string().contains("invalid, expired, or revoked"));
    }

    #[test]
    fn test_bad_request_display() {
        let e = ApiError::BadRequest("'name' field is required".to_string());
        assert_eq!(e.to_string(), "Bad request: 'name' field is required");
    }

    async fn from_status_body(status: u16, body: Option<serde_json::Value>) -> ApiError {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let mut template = ResponseTemplate::new(status);
        if let Some(body) = body {
            template = template.set_body_json(body);
        }
        Mock::given(method("GET"))
            .respond_with(template)
            .mount(&server)
            .await;
        let resp = reqwest::get(server.uri()).await.unwrap();
        ApiError::from_response(resp).await
    }

    #[tokio::test]
    async fn test_from_response_quota_exceeded_with_body() {
        // The API's real 429 body (`routes/mod.rs` IntoResponse for QuotaExceeded).
        let err = from_status_body(
            429,
            Some(serde_json::json!({
                "error": "quota_exceeded",
                "resource_type": "cards_total",
                "used": 100,
                "limit": 100,
                "requested": 5,
                "plan_id": "free",
                "window": "absolute"
            })),
        )
        .await;
        match err {
            ApiError::QuotaExceeded {
                resource_type,
                used,
                limit,
                requested,
                plan_id,
                window,
                resets_at,
            } => {
                assert_eq!(resets_at, None, "absolute never resets");
                assert_eq!(resource_type, "cards_total");
                assert_eq!((used, limit, requested), (Some(100), Some(100), Some(5)));
                assert_eq!(plan_id.as_deref(), Some("free"));
                assert_eq!(window.as_deref(), Some("absolute"));
            }
            other => panic!("expected QuotaExceeded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_from_response_quota_exceeded_old_body_and_no_body() {
        let err = from_status_body(
            429,
            Some(serde_json::json!({"resource_type": "cards_total", "used": 1, "limit": 1})),
        )
        .await;
        assert!(
            matches!(
                &err,
                ApiError::QuotaExceeded {
                    used: Some(1),
                    window: None,
                    plan_id: None,
                    ..
                }
            ),
            "{err:?}"
        );
        let err = from_status_body(402, None).await;
        assert!(
            matches!(
                &err,
                ApiError::QuotaExceeded {
                    used: None,
                    limit: None,
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(
            err.to_string().contains("limit details unavailable"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn test_from_response_quota_exceeded_drops_non_identifier_strings() {
        let err = from_status_body(
            429,
            Some(serde_json::json!({
                "resource_type": "<b>x</b>",
                "used": 1, "limit": 1,
                "plan_id": "free\nINJECTED",
                "window": "monthly"
            })),
        )
        .await;
        match err {
            ApiError::QuotaExceeded {
                resource_type,
                plan_id,
                window,
                resets_at,
                ..
            } => {
                assert_eq!(resource_type, "unknown");
                assert_eq!(plan_id, None);
                assert_eq!(window.as_deref(), Some("monthly"));
                assert!(resets_at.is_some(), "monthly window has a reset");
            }
            other => panic!("expected QuotaExceeded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_from_response_feature_not_entitled() {
        let err = from_status_body(
            403,
            Some(serde_json::json!({
                "error": "Forbidden",
                "code": "FEATURE_NOT_ENTITLED",
                "feature": "ai_chat",
                "plan_id": "free"
            })),
        )
        .await;
        match &err {
            ApiError::FeatureNotEntitled { feature, plan_id } => {
                assert_eq!(feature, "ai_chat");
                assert_eq!(plan_id.as_deref(), Some("free"));
            }
            other => panic!("expected FeatureNotEntitled, got {other:?}"),
        }
        assert!(err.stops_batch());
    }

    #[tokio::test]
    async fn test_from_response_feature_not_entitled_drops_non_identifier_strings() {
        let err = from_status_body(
            403,
            Some(serde_json::json!({
                "error": "Forbidden", "code": "FEATURE_NOT_ENTITLED",
                "feature": "<img src=x>", "plan_id": "free\nINJECTED"
            })),
        )
        .await;
        match &err {
            ApiError::FeatureNotEntitled { feature, plan_id } => {
                assert_eq!(feature, "unknown");
                assert_eq!(plan_id, &None);
            }
            other => panic!("expected FeatureNotEntitled, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(!msg.contains('<') && !msg.contains('\n'), "{msg}");
        assert!(msg.contains("your current plan"), "{msg}");
        let err = from_status_body(
            403,
            Some(serde_json::json!({"code": "FEATURE_NOT_ENTITLED"})),
        )
        .await;
        assert!(
            matches!(&err, ApiError::FeatureNotEntitled { feature, plan_id: None } if feature == "unknown"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn test_from_response_403_other_code_or_text_body_stays_permission_denied() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let err = from_status_body(
            403,
            Some(serde_json::json!({
                "error": "Forbidden", "code": "SOMETHING_ELSE", "feature": "ai_chat"
            })),
        )
        .await;
        assert!(matches!(err, ApiError::PermissionDenied(_)), "{err:?}");
        assert!(!err.stops_batch());

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403).set_body_string("nope"))
            .mount(&server)
            .await;
        let resp = reqwest::get(server.uri()).await.unwrap();
        let err = ApiError::from_response(resp).await;
        assert!(matches!(err, ApiError::PermissionDenied(_)), "{err:?}");
        assert!(!err.stops_batch());
    }

    #[test]
    fn test_quota_message_batch_with_one_left_uses_singular() {
        let m = quota(
            "cards_total",
            Some(499),
            Some(500),
            Some(5),
            None,
            Some("monthly"),
        );
        assert!(
            m.contains("only 1 of the monthly limit of 500 is left (499 used)."),
            "{m}"
        );
        assert!(m.contains("Try a smaller batch (up to 1)."), "{m}");
        assert!(m.contains("It resets 2026-11-01 00:00 UTC."), "{m}");
    }

    #[test]
    fn test_quota_message_unknown_window_and_partial_counts() {
        let m = quota("cards_total", Some(3), Some(3), None, None, Some("weekly"));
        assert!(
            m.contains("Cards (total) (cards_total) — limit reached: 3 of 3 used."),
            "{m}"
        );
        assert!(!m.contains("weekly") && !m.contains("resets"), "{m}");
        for (used, limit) in [(Some(3), None), (None, Some(5))] {
            let m = quota("cards_total", used, limit, None, None, None);
            assert!(
                m.contains("limit reached (limit details unavailable)."),
                "{used:?}/{limit:?}: {m}"
            );
        }
    }

    #[test]
    fn test_ident_rejects_empty_and_overlong() {
        assert_eq!(ident(Some(String::new())), None);
        assert_eq!(ident(Some("a".repeat(65))), None);
        assert_eq!(
            ident(Some("a".repeat(64))).as_deref(),
            Some("a".repeat(64).as_str())
        );
        assert_eq!(ident(Some("Free".into())), None, "uppercase rejected");
        assert_eq!(
            ident(Some("cards_total_2".into())).as_deref(),
            Some("cards_total_2")
        );
        assert_eq!(ident(None), None);
    }

    #[tokio::test]
    async fn test_from_response_bare_forbidden_stays_permission_denied() {
        let err = from_status_body(403, Some(serde_json::json!({"error": "Forbidden"}))).await;
        match err {
            ApiError::PermissionDenied(msg) => {
                assert!(msg.contains("may belong to another user"), "{msg}")
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_from_response_not_found() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(serde_json::json!({"error": "catalog not found"})),
            )
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        let err = ApiError::from_response(resp).await;

        assert!(matches!(err, ApiError::NotFound(_)));
        assert!(err.to_string().contains("catalog not found"));
    }

    #[tokio::test]
    async fn test_from_response_401() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        let err = ApiError::from_response(resp).await;

        assert!(matches!(err, ApiError::Unauthorized));
    }

    #[tokio::test]
    async fn test_from_response_409() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        let err = ApiError::from_response(resp).await;

        assert!(matches!(err, ApiError::Conflict(_)));
    }

    #[tokio::test]
    async fn test_from_response_500() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        let err = ApiError::from_response(resp).await;

        assert!(matches!(err, ApiError::Internal));
    }

    #[tokio::test]
    async fn test_from_response_redirect_is_bad_request_with_actionable_message() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // This test doesn't capture logs itself, but it hits the exact same `tracing::warn!`
        // callsite (the 300..=399 branch of `ApiError::from_response`) as the capturing test
        // `test_from_response_redirect_logs_redacted_location_without_token` below. Without a
        // thread-local subscriber, that callsite's *first-ever* touch is evaluated against the
        // process's `NoSubscriber` default, which caches `Interest::never()` for it globally —
        // permanently disabling the event for every thread, including a concurrently-running
        // capturing test, unless something rebuilds the cache afterward. Taking the same lock
        // here just serializes it against that test (observed to be required, see the
        // `TRACING_CAPTURE_LOCK` doc); it does not need to install a subscriber of its own.
        let _lock = test_support::TRACING_CAPTURE_LOCK.lock().await;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(301)
                    .insert_header("Location", "https://api.example.com/catalogs?token=secret"),
            )
            .mount(&server)
            .await;

        let resp = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
            .get(server.uri())
            .send()
            .await
            .unwrap();
        let err = ApiError::from_response(resp).await;

        match err {
            ApiError::BadRequest(msg) => {
                assert!(msg.contains("redirect"), "{msg}");
                assert!(msg.contains("ENGRAMO_API_URL"), "{msg}");
                assert!(msg.contains("https://api.example.com/catalogs"), "{msg}");
                assert!(!msg.contains("token=secret"), "{msg}");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn test_redact_location_strips_query_and_fragment() {
        assert_eq!(
            redact_location("https://x.example.com/path?token=abc"),
            "https://x.example.com/path"
        );
        assert_eq!(
            redact_location("https://x.example.com/path#frag"),
            "https://x.example.com/path"
        );
        assert_eq!(
            redact_location("https://x.example.com/path"),
            "https://x.example.com/path"
        );
    }

    #[test]
    fn test_redact_location_strips_userinfo() {
        assert_eq!(
            redact_location("https://user:pass@h/p?token=s#f"),
            "https://h/p"
        );
    }

    #[test]
    fn test_redact_location_relative_falls_back_to_cutting_at_query_or_fragment() {
        assert_eq!(redact_location("/catalogs?token=secret"), "/catalogs");
        assert_eq!(redact_location("/catalogs#frag"), "/catalogs");
        assert_eq!(redact_location("/catalogs"), "/catalogs");
    }

    #[test]
    fn test_redact_location_network_path_reference_strips_userinfo() {
        assert_eq!(redact_location("//user:pass@h/p?t=s"), "https://h/p");
    }

    #[test]
    fn test_redact_location_backslash_network_path_strips_userinfo() {
        assert_eq!(redact_location("\\\\user:pass@h/p"), "https://h/p");
    }

    #[test]
    fn test_redact_location_invalid_port_strips_userinfo() {
        assert_eq!(
            redact_location("https://user:pass@h:99999/p"),
            "<unparseable Location>"
        );
    }

    #[test]
    fn test_redact_location_tab_bypass_still_strips_userinfo() {
        let redacted = redact_location("/\t/user:pass@h/p?t=1");
        assert!(!redacted.contains("user:pass"), "{redacted}");
        assert_eq!(redacted, "https://h/p");
    }

    #[test]
    fn test_redact_location_relative_path_without_leading_slash() {
        assert_eq!(redact_location("catalogs?token=x"), "/catalogs");
    }

    #[test]
    fn test_redact_location_caps_length() {
        let long = format!("https://h/{}", "a".repeat(1000));
        let redacted = redact_location(&long);
        assert_eq!(redacted.chars().count(), 256);
    }

    #[tokio::test]
    async fn test_from_response_redirect_logs_redacted_location_without_token() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // See `test_support::TRACING_CAPTURE_LOCK`: held for the capturing subscriber's
        // entire lifetime, including across the `.await`s below — a `tokio::sync::Mutex`
        // guard is fine to hold across an await (unlike a std one, which clippy rejects).
        let _lock = test_support::TRACING_CAPTURE_LOCK.lock().await;

        let buf = BufWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        // Rebuild so the thread-local subscriber above actually sees the event, even if a
        // callsite was previously cached as "no subscriber interested" by another test. Safe
        // now that TRACING_CAPTURE_LOCK guarantees no other capturing subscriber is active
        // concurrently.
        tracing::callsite::rebuild_interest_cache();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(301).insert_header(
                "Location",
                "https://user:pass@api.example.com/catalogs?token=secret",
            ))
            .mount(&server)
            .await;

        let resp = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
            .get(server.uri())
            .send()
            .await
            .unwrap();
        let _ = ApiError::from_response(resp).await;

        drop(_guard);
        let logged = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(!logged.contains("token=secret"), "{logged}");
        assert!(!logged.contains("user:pass"), "{logged}");
        assert!(
            logged.contains("https://api.example.com/catalogs"),
            "{logged}"
        );
    }

    #[test]
    fn test_strip_duplicate_prefix_dedups_not_found() {
        let msg = strip_duplicate_prefix(
            "Not found",
            "Not found: Learning path X not found".to_string(),
        );
        assert_eq!(msg, "Learning path X not found");
    }

    #[test]
    fn test_strip_duplicate_prefix_dedups_bad_request_case_insensitive() {
        let msg = strip_duplicate_prefix(
            "Bad request",
            "bad request: 'name' field is required".to_string(),
        );
        assert_eq!(msg, "'name' field is required");
    }

    #[test]
    fn test_strip_duplicate_prefix_passthrough_when_no_prefix() {
        let msg = strip_duplicate_prefix("Not found", "catalog abc-123 does not exist".to_string());
        assert_eq!(msg, "catalog abc-123 does not exist");
    }

    #[test]
    fn test_strip_duplicate_prefix_keeps_label_without_colon() {
        // exact label, no trailing colon -> unchanged
        assert_eq!(
            strip_duplicate_prefix("Not found", "Not found".to_string()),
            "Not found"
        );
        // label followed by a space, not a colon -> unchanged
        assert_eq!(
            strip_duplicate_prefix("Not found", "Not found anywhere in catalog".to_string()),
            "Not found anywhere in catalog"
        );
        // label is a prefix of a longer word -> unchanged
        assert_eq!(
            strip_duplicate_prefix("Bad request", "Bad requests are throttled".to_string()),
            "Bad requests are throttled"
        );
        // message shorter than the label -> unchanged
        assert_eq!(
            strip_duplicate_prefix("Not found", "Not".to_string()),
            "Not"
        );
    }

    #[test]
    fn test_strip_duplicate_prefix_trims_leading_whitespace_before_matching() {
        // `extract_error_message` replaces control characters (e.g. a leading "\n") with
        // spaces, so the label match must tolerate leading whitespace instead of doubling up.
        let msg = strip_duplicate_prefix("Not found", " Not found: X".to_string());
        assert_eq!(msg, "X");
    }

    #[test]
    fn test_dedup_or_raw_falls_back_when_stripping_leaves_nothing() {
        assert_eq!(
            dedup_or_raw("Not found", "Not found:".to_string()),
            "Not found:"
        );
        assert_eq!(
            dedup_or_raw("Bad request", "Bad request: ".to_string()),
            "Bad request: "
        );
    }

    #[test]
    fn test_dedup_or_raw_still_dedups_normal_case() {
        assert_eq!(
            dedup_or_raw("Not found", "Not found: catalog X".to_string()),
            "catalog X"
        );
    }

    #[tokio::test]
    async fn test_from_response_404_body_with_only_label_falls_back_to_raw() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(serde_json::json!({"error": "Not found:"})),
            )
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        match ApiError::from_response(resp).await {
            ApiError::NotFound(msg) => assert_eq!(msg, "Not found:"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn test_forbidden_hint_expands_bare_forbidden() {
        let msg = forbidden_hint("Forbidden".to_string());
        assert!(msg.starts_with("Forbidden ("), "{msg}");
        assert!(msg.contains("may not exist"), "{msg}");
        assert!(msg.contains("public"), "{msg}");
        assert!(msg.contains("unlisted"), "{msg}");
    }

    #[test]
    fn test_forbidden_hint_expands_empty_body() {
        let msg = forbidden_hint(String::new());
        assert!(msg.starts_with("Forbidden ("), "{msg}");
    }

    #[test]
    fn test_forbidden_hint_passes_through_non_bare_message() {
        let msg = forbidden_hint("you don't have edit access to this catalog".to_string());
        assert_eq!(msg, "you don't have edit access to this catalog");
    }

    #[test]
    fn test_forbidden_hint_case_insensitive_and_trimmed() {
        for input in [
            "forbidden",
            "FORBIDDEN",
            "  Forbidden  ",
            "FORBIDDEN ",
            "   ",
        ] {
            let msg = forbidden_hint(input.to_string());
            assert!(msg.starts_with("Forbidden ("), "input={input:?} got={msg}");
        }
    }

    #[tokio::test]
    async fn test_from_response_404_dedups_backend_prefix() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": "Not found: Learning path X not found"
            })))
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        let err = ApiError::from_response(resp).await;
        assert_eq!(
            err.to_string(),
            "Not found: Learning path X not found",
            "must not double up the prefix"
        );
        match err {
            ApiError::NotFound(msg) => assert_eq!(msg, "Learning path X not found"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_from_response_400_dedups_backend_prefix() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "Bad request: 'name' field is required"
            })))
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        let err = ApiError::from_response(resp).await;
        assert_eq!(
            err.to_string(),
            "Bad request: 'name' field is required",
            "must not double up the prefix"
        );
        match err {
            ApiError::BadRequest(msg) => assert_eq!(msg, "'name' field is required"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_from_response_403_bare_forbidden_is_actionable() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(serde_json::json!({"error": "Forbidden"})),
            )
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        let err = ApiError::from_response(resp).await;
        let msg = err.to_string();
        assert!(msg.starts_with("Permission denied: Forbidden ("), "{msg}");
        assert!(msg.contains("may not exist"), "{msg}");
    }

    #[tokio::test]
    async fn test_from_response_403_non_bare_message_passes_through() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "error": "you don't have edit access to this catalog"
            })))
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        let err = ApiError::from_response(resp).await;
        assert_eq!(
            err.to_string(),
            "Permission denied: you don't have edit access to this catalog"
        );
    }

    #[tokio::test]
    async fn test_from_response_403_dedups_prefix_then_expands_bare_forbidden() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Case 1: a backend body already prefixed with "Permission denied: Forbidden" must be
        // de-duplicated by `strip_duplicate_prefix` *before* `forbidden_hint` sees the bare
        // "Forbidden" and expands it — not left as a doubled, unexpanded prefix.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "error": "Permission denied: Forbidden"
            })))
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        let err = ApiError::from_response(resp).await;
        let msg = err.to_string();
        assert!(msg.starts_with("Permission denied: Forbidden ("), "{msg}");
        assert!(
            !msg.contains("Permission denied: Permission denied"),
            "{msg}"
        );
        assert!(msg.contains("may not exist"), "{msg}");

        // Case 2: a case-insensitive prefix on a specific message is stripped, and the
        // specific message (not bare "Forbidden") passes through `forbidden_hint` unchanged.
        let server2 = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "error": "permission denied: you lack the editor role"
            })))
            .mount(&server2)
            .await;

        let resp2 = reqwest::get(server2.uri()).await.unwrap();
        let err2 = ApiError::from_response(resp2).await;
        assert_eq!(
            err2.to_string(),
            "Permission denied: you lack the editor role"
        );
        match err2 {
            ApiError::PermissionDenied(m) => assert_eq!(m, "you lack the editor role"),
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_from_response_422_surfaces_deserialize_message() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Axum's real rejection body for an invalid enum variant is plain text, not
        // `{"error": "..."}` — extract_error_message must fall back to the raw body.
        let body = "Failed to deserialize the JSON body into the target type: \
            face.style.textAlign: unknown variant `diagonal`, expected one of \
            `left`, `center`, `right` at line 1 column 54";
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(422).set_body_string(body))
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        let err = ApiError::from_response(resp).await;

        match err {
            ApiError::BadRequest(msg) => {
                assert!(msg.contains("textAlign"), "{msg}");
                assert!(msg.contains("diagonal"), "{msg}");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }
}
