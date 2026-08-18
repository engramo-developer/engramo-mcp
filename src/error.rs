use serde::Deserialize;
use thiserror::Error;

/// Structured error body returned by the Engram API for quota exceeded (HTTP 429).
#[derive(Debug, Deserialize)]
pub struct QuotaExceededBody {
    pub resource_type: Option<String>,
    pub used: Option<i64>,
    pub limit: Option<i64>,
}

/// Errors produced by the Engram HTTP client.
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

    #[error("Unauthorized: API token is invalid or expired. Check your ENGRAM_API_TOKEN.")]
    Unauthorized,

    #[error(
        "Server error: the Engram API returned an internal error. Please try again in a moment."
    )]
    Internal,

    #[error("Network error: {0}")]
    Network(reqwest::Error),
}

impl From<reqwest::Error> for ApiError {
    /// Connectivity failures to the Engram API (DNS, TLS, timeout, connection
    /// refused) are a genuine system failure, not a routine tool-call outcome —
    /// log at ERROR here so it reaches GCP Error Reporting (see `error_reporting`).
    fn from(e: reqwest::Error) -> Self {
        tracing::error!(error = %e, "network error calling Engram API");
        Self::Network(e)
    }
}

impl ApiError {
    /// Parse an API error from an HTTP response status + body.
    /// This is the single place that converts HTTP semantics into typed errors.
    pub async fn from_response(response: reqwest::Response) -> Self {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        match status.as_u16() {
            401 => Self::Unauthorized,
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
                    "Engram API returned an unexpected status"
                );
                Self::Internal
            }
        }
    }
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
