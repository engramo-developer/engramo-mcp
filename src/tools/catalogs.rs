use rmcp::{model::CallToolResult, schemars};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::preview;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListCatalogsParams {
    #[schemars(
        description = "Maximum number of catalogs to return (default 20; values are clamped to 1..=50)"
    )]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination cursor from a previous response")]
    pub cursor: Option<String>,
    #[schemars(
        description = "If true, return only catalogs with shadowing turned on. Keep the same value for every page: a cursor from one value is rejected with the other."
    )]
    pub speaking_only: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetCatalogParams {
    #[schemars(description = "UUID of the catalog")]
    pub catalog_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateCatalogParams {
    #[schemars(description = "UUID of the catalog to update")]
    pub catalog_id: String,
    #[schemars(description = "New name")]
    pub name: Option<String>,
    #[schemars(description = "New description")]
    pub description: Option<String>,
    #[schemars(description = "New tags")]
    pub tags: Option<Vec<String>>,
    #[schemars(
        description = "New visibility: 'public', 'private', or 'unlisted' (hidden from listings/search, but reachable by anyone who has its exact short_id). Setting 'public' or 'unlisted' may require publish permission — non-moderators can get a permission error."
    )]
    pub visibility: Option<String>,
    #[schemars(description = "Current version of the catalog (required for optimistic locking)")]
    pub version: i32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteCatalogParams {
    #[schemars(description = "UUID of the catalog to delete")]
    pub catalog_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetCatalogShadowingParams {
    #[schemars(description = "UUID of the catalog (must be one you own)")]
    pub catalog_id: String,
    #[schemars(
        description = "true turns shadowing on (or changes its language); false turns it off"
    )]
    pub enabled: bool,
    #[schemars(
        description = "Language of the card FRONTS (faces) as a BCP-47 tag, e.g. 'es', 'es-ES', 'pt-BR', 'sr-Latn'. Required when enabled is true; ignored when enabled is false."
    )]
    pub language: Option<String>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub(crate) fn ok_json<T: serde::Serialize>(value: &T) -> CallToolResult {
    match serde_json::to_string_pretty(value) {
        Ok(json) => CallToolResult::success(vec![rmcp::model::ContentBlock::text(json)]),
        Err(e) => CallToolResult::error(vec![rmcp::model::ContentBlock::text(format!(
            "Serialization error: {e}"
        ))]),
    }
}

pub(crate) fn ok_text(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![rmcp::model::ContentBlock::text(text.into())])
}

pub(crate) fn err_result(e: impl std::fmt::Display) -> CallToolResult {
    let message = e.to_string();
    // WARN, not ERROR: this is the single choke point for every failed tool call
    // (~87 call sites), most of which are routine 4xx-shaped outcomes (not found,
    // bad input, quota, conflict) rather than bugs. Genuine system failures (backend
    // 5xx, network errors) are logged at ERROR at their own point of origin (see
    // `ApiError::from_response` and `impl From<reqwest::Error> for ApiError` in
    // `error.rs`), which is what feeds GCP Error Reporting.
    tracing::warn!(error = %message, "tool call returned an error");
    CallToolResult::error(vec![rmcp::model::ContentBlock::text(message)])
}

pub(crate) fn parse_uuid(s: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|_| format!("Invalid UUID: '{}'", preview(s)))
}

/// Fixes the case and separator of a BCP-47-like tag (`es_es` -> `es-ES`, `sr-latn` ->
/// `sr-Latn`) and nothing else: validation stays on the backend so the rule can't drift. The
/// region/script part is only re-cased when it is exactly 2 (region) or 4 (script) ASCII
/// characters; anything else, and anything non-ASCII, passes through for the backend to reject.
pub(crate) fn normalize_language_tag(raw: &str) -> String {
    let raw = raw.trim();
    let (lang, rest) = match raw.split_once(['-', '_']) {
        Some((lang, rest)) => (lang, Some(rest)),
        None => (raw, None),
    };
    let lang = lang.to_ascii_lowercase();
    match rest {
        None => lang,
        Some(rest) if rest.is_ascii() && rest.len() == 2 => {
            format!("{lang}-{}", rest.to_ascii_uppercase())
        }
        Some(rest) if rest.is_ascii() && rest.len() == 4 => {
            let (head, tail) = rest.split_at(1);
            format!(
                "{lang}-{}{}",
                head.to_ascii_uppercase(),
                tail.to_ascii_lowercase()
            )
        }
        Some(rest) => format!("{lang}-{rest}"),
    }
}

/// Generous upper bound for a BCP-47 tag (RFC 5646 recommends supporting at least 35 chars).
const MAX_LANGUAGE_TAG_LEN: usize = 64;

/// The language to store for `set_catalog_shadowing`: `None` (turn off) when `enabled` is false,
/// otherwise the normalized tag. `enabled: true` without a usable language is a local error, so
/// a model that forgets `language` can never silently turn shadowing off.
pub(crate) fn shadowing_target(
    enabled: bool,
    language: Option<&str>,
) -> Result<Option<String>, String> {
    if !enabled {
        return Ok(None);
    }
    match language.map(str::trim) {
        Some(l) if l.len() > MAX_LANGUAGE_TAG_LEN => Err(format!(
            "`language` is too long (max {MAX_LANGUAGE_TAG_LEN} bytes): '{}'",
            preview(l)
        )),
        Some(l) if !l.is_empty() => Ok(Some(normalize_language_tag(l))),
        _ => Err(
            "`language` is required when `enabled` is true (e.g. 'es', 'es-ES', 'pt-BR', \
                  'sr-Latn')"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::EngramoClient;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mock_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn make_server(base_url: &str) -> crate::server::EngramoMcpServer {
        crate::server::EngramoMcpServer::new(EngramoClient::new(base_url, "engramo_test"), false)
    }

    #[tokio::test]
    async fn test_list_catalogs_ok() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "name": "Rust", "version": 1}],
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: None,
                speaking_only: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    fn tool_text(result: &rmcp::model::CallToolResult) -> &str {
        result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("")
    }

    #[tokio::test]
    async fn test_list_catalogs_speaking_only_param_reaches_api() {
        use rmcp::handler::server::wrapper::Parameters;
        for (flag, expect_param) in [(Some(true), true), (Some(false), false), (None, false)] {
            let server = MockServer::start().await;
            let mock = Mock::given(method("GET")).and(path("/catalogs"));
            let mock = if expect_param {
                mock.and(query_param("speakingOnly", "true"))
            } else {
                mock.and(query_param_is_missing("speakingOnly"))
            };
            mock.respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [], "nextCursor": null
            })))
            .expect(1)
            .mount(&server)
            .await;

            let result = make_server(&server.uri())
                .list_catalogs(Parameters(ListCatalogsParams {
                    limit: None,
                    cursor: None,
                    speaking_only: flag,
                }))
                .await
                .unwrap();
            assert!(!result.is_error.unwrap_or(false), "{flag:?}");
        }
    }

    #[tokio::test]
    async fn test_list_catalogs_output_includes_speaking_settings_only_when_set() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    {"id": mock_id(), "name": "Shadow", "version": 1,
                     "speakingSettings": {"language": "es-ES"}},
                    {"id": mock_id(), "name": "Plain", "version": 1, "speakingSettings": {}}
                ],
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: None,
                speaking_only: None,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(tool_text(&result)).unwrap();
        assert_eq!(v["data"][0]["speakingSettings"]["language"], "es-ES");
        assert!(v["data"][1].get("speakingSettings").is_none(), "{v}");
    }

    #[tokio::test]
    async fn test_get_catalog_output_includes_speaking_settings_only_when_set() {
        use rmcp::handler::server::wrapper::Parameters;
        for (body, expected) in [
            (
                json!({"speakingSettings": {"language": "es-ES"}}),
                Some("es-ES"),
            ),
            (json!({}), None),
        ] {
            let server = MockServer::start().await;
            let mut catalog = json!({"id": mock_id(), "name": "Rust", "version": 1});
            catalog
                .as_object_mut()
                .unwrap()
                .extend(body.as_object().unwrap().clone());
            Mock::given(method("GET"))
                .and(path(format!("/catalogs/{}", mock_id())))
                .respond_with(ResponseTemplate::new(200).set_body_json(catalog))
                .mount(&server)
                .await;

            let result = make_server(&server.uri())
                .get_catalog(Parameters(GetCatalogParams {
                    catalog_id: mock_id().to_string(),
                }))
                .await
                .unwrap();
            let v: serde_json::Value = serde_json::from_str(tool_text(&result)).unwrap();
            match expected {
                Some(lang) => assert_eq!(v["speakingSettings"]["language"], lang),
                None => assert!(v.get("speakingSettings").is_none(), "{v}"),
            }
        }
    }

    /// Regression test for issue #34: the tool output's `cursor` field must reflect the
    /// API's real `nextCursor` value, not always be `null`.
    #[tokio::test]
    async fn test_list_catalogs_cursor_reaches_tool_output() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "nextCursor": "abc"
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: None,
                speaking_only: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["cursor"], "abc", "{text}");
    }

    #[tokio::test]
    async fn test_list_catalogs_unauthorized_returns_error_content_not_panic() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        // CRITICAL: must return Ok(CallToolResult) with is_error=true, NOT panic or Err()
        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: None,
                speaking_only: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false), "expected is_error=true");
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(
            text.contains("nauthorized") || text.contains("token"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn test_get_catalog_invalid_uuid_returns_error_content() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        // No mock needed — UUID parse fails before HTTP call
        let result = make_server(&server.uri())
            .get_catalog(Parameters(GetCatalogParams {
                catalog_id: "not-a-uuid".to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(text.contains("Invalid UUID"), "{text}");
    }

    #[tokio::test]
    async fn test_update_catalog_conflict_returns_actionable_message() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/catalogs/{}", mock_id())))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .update_catalog(Parameters(UpdateCatalogParams {
                catalog_id: mock_id().to_string(),
                name: Some("New".to_string()),
                description: None,
                tags: None,
                visibility: None,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(text.contains("Fetch the latest version"), "{text}");
    }

    #[tokio::test]
    async fn test_delete_catalog_ok() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!("/catalogs/{}", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "deleted"})))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .delete_catalog(Parameters(DeleteCatalogParams {
                catalog_id: mock_id().to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[test]
    fn test_parse_uuid_valid() {
        let id = parse_uuid("00000000-0000-0000-0000-000000000001");
        assert!(id.is_ok());
    }

    #[test]
    fn test_parse_uuid_invalid() {
        let id = parse_uuid("not-a-uuid");
        assert!(id.is_err());
        assert!(id.unwrap_err().contains("Invalid UUID"));
    }

    /// Regression for F10: a huge, malformed LLM-supplied value must not be echoed in full
    /// into the error text (and thus into the WARN log via `err_result`) — `preview` caps it.
    #[test]
    fn test_parse_uuid_huge_input_error_is_capped() {
        let huge = "x".repeat(1024 * 1024);
        let err = parse_uuid(&huge).unwrap_err();
        assert!(
            err.len() < 100,
            "error message not capped: {} bytes",
            err.len()
        );
    }

    #[tokio::test]
    async fn test_decode_error_body_values_do_not_reach_logs_via_err_result() {
        use crate::error::test_support::{BufWriter, TRACING_CAPTURE_LOCK};
        use rmcp::handler::server::wrapper::Parameters;

        // See `error::test_support::TRACING_CAPTURE_LOCK`: held for the capturing
        // subscriber's entire lifetime, including across the `.await`s below — a
        // `tokio::sync::Mutex` guard is fine to hold across an await (unlike a std one,
        // which clippy rejects).
        let _lock = TRACING_CAPTURE_LOCK.lock().await;

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

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "name": "x", "version": "SECRET_USER_TEXT"}],
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: None,
                speaking_only: None,
            }))
            .await
            .unwrap();
        drop(guard);

        assert_eq!(result.is_error, Some(true));
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(
            text.contains("Unexpected response from the Engramo API"),
            "{text}"
        );
        assert!(!text.contains("SECRET_USER_TEXT"), "{text}");
        let logged = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(!logged.contains("SECRET_USER_TEXT"), "{logged}");
    }

    #[tokio::test]
    async fn test_list_catalogs_network_failure_returns_error_without_url() {
        use rmcp::handler::server::wrapper::Parameters;
        // Port 1 (tcpmux) has no listener, so the connection is refused deterministically. A
        // dropped MockServer's port would race with other tests' servers reusing it.
        let uri = "http://127.0.0.1:1".to_string();

        let result = make_server(&uri)
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: Some("SECRET_CURSOR".to_string()),
                speaking_only: None,
            }))
            .await
            .unwrap(); // must NOT be Err — MCP contract
        assert_eq!(result.is_error, Some(true), "{result:?}");
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(text.contains("Network error"), "{text}");
        assert!(!text.contains("SECRET_CURSOR"), "{text}");
        assert!(!text.contains(&uri), "{text}");
    }

    #[test]
    fn test_normalize_language_tag() {
        for (input, expected) in [
            ("es_es", "es-ES"),
            (" PT-br ", "pt-BR"),
            ("sr-latn", "sr-Latn"),
            ("SR_LATN", "sr-Latn"),
            ("es", "es"),
            ("ES", "es"),
            ("spanish", "spanish"),
            ("zh-hans-cn", "zh-hans-cn"),
            ("es-é", "es-é"),
            ("ésp-es", "ésp-ES"),
        ] {
            assert_eq!(normalize_language_tag(input), expected, "{input}");
        }
    }

    #[test]
    fn test_shadowing_target() {
        assert_eq!(shadowing_target(false, None), Ok(None));
        assert_eq!(shadowing_target(false, Some("es_es")), Ok(None));
        assert_eq!(
            shadowing_target(true, Some("es_es")),
            Ok(Some("es-ES".to_string()))
        );
        for bad in [None, Some(""), Some("   ")] {
            assert!(shadowing_target(true, bad).is_err(), "{bad:?}");
        }
    }
}
