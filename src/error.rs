use serde::Deserialize;
use thiserror::Error;

/// Structured error body returned by the Engramo API for quota exceeded (HTTP 429).
#[derive(Debug, Deserialize)]
pub struct QuotaExceededBody {
    pub resource_type: Option<String>,
    pub used: Option<i64>,
    pub limit: Option<i64>,
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

    #[error(
        "Quota exceeded: {resource_type} limit reached ({used}/{limit}). Upgrade your plan or delete unused resources."
    )]
    QuotaExceeded {
        resource_type: String,
        used: i64,
        limit: i64,
    },

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Unauthorized: API token is invalid or expired. Check your ENGRAMO_API_TOKEN.")]
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
            403 => Self::PermissionDenied(extract_error_message(&body)),
            404 => Self::NotFound(extract_error_message(&body)),
            409 => Self::Conflict(
                "resource was modified by another client. Fetch the latest version and retry."
                    .to_string(),
            ),
            402 | 429 => {
                if let Ok(quota) = serde_json::from_str::<QuotaExceededBody>(&body) {
                    Self::QuotaExceeded {
                        resource_type: quota.resource_type.unwrap_or_else(|| "unknown".to_string()),
                        used: quota.used.unwrap_or(0),
                        limit: quota.limit.unwrap_or(0),
                    }
                } else {
                    Self::QuotaExceeded {
                        resource_type: "unknown".to_string(),
                        used: 0,
                        limit: 0,
                    }
                }
            }
            // 422 is axum's rejection status for a request body that parses as JSON but
            // fails to deserialize into the target type (e.g. an invalid enum variant like
            // `style.textAlign: "diagonal"`) — a client input mistake, same as 400, not a
            // transient server failure. The rejection text names the exact field and valid
            // values, which is directly actionable for the calling LLM to correct and retry.
            400 | 422 => Self::BadRequest(extract_error_message(&body)),
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

        let buf = BufWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
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

    #[test]
    fn test_quota_exceeded_display() {
        let e = ApiError::QuotaExceeded {
            resource_type: "cards_total".to_string(),
            used: 500,
            limit: 500,
        };
        let msg = e.to_string();
        assert!(msg.contains("cards_total"));
        assert!(msg.contains("500/500"));
    }

    #[test]
    fn test_unauthorized_display() {
        let e = ApiError::Unauthorized;
        assert!(e.to_string().contains("invalid or expired"));
    }

    #[test]
    fn test_bad_request_display() {
        let e = ApiError::BadRequest("'name' field is required".to_string());
        assert_eq!(e.to_string(), "Bad request: 'name' field is required");
    }

    #[tokio::test]
    async fn test_from_response_quota_exceeded_with_body() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
                "error": "quota_exceeded",
                "resource_type": "cards_total",
                "used": 100,
                "limit": 100
            })))
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        let err = ApiError::from_response(resp).await;

        match err {
            ApiError::QuotaExceeded {
                resource_type,
                used,
                limit,
            } => {
                assert_eq!(resource_type, "cards_total");
                assert_eq!(used, 100);
                assert_eq!(limit, 100);
            }
            other => panic!("expected QuotaExceeded, got {other:?}"),
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

        let buf = BufWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

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
