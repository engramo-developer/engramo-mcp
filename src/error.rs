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
}

impl From<reqwest::Error> for ApiError {
    /// Connectivity failures to the Engramo API (DNS, TLS, timeout, connection
    /// refused) are a genuine system failure, not a routine tool-call outcome —
    /// log at ERROR here so it reaches GCP Error Reporting (see `error_reporting`).
    fn from(e: reqwest::Error) -> Self {
        tracing::error!(error = %e, "network error calling Engramo API");
        Self::Network(e)
    }
}

impl ApiError {
    /// Parse an API error from an HTTP response status + body.
    /// This is the single place that converts HTTP semantics into typed errors.
    pub async fn from_response(response: reqwest::Response) -> Self {
        let status = response.status();
        // Must be read before `response.text()` consumes the response body below.
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = response.text().await.unwrap_or_default();

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

/// Extract a human-readable message from a JSON `{ "error": "..." }` body.
fn extract_error_message(body: &str) -> String {
    #[derive(Deserialize)]
    struct ErrorBody {
        error: Option<String>,
    }
    serde_json::from_str::<ErrorBody>(body)
        .ok()
        .and_then(|b| b.error)
        .unwrap_or_else(|| body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        use std::sync::{Arc, Mutex};
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Minimal in-memory tracing writer so we can assert on what the WARN log actually
        // contains, without pulling in a new dev-dependency for this one test.
        #[derive(Clone, Default)]
        struct BufWriter(Arc<Mutex<Vec<u8>>>);
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
