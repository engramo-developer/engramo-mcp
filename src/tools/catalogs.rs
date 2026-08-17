use rmcp::{model::CallToolResult, schemars};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListCatalogsParams {
    #[schemars(description = "Maximum number of catalogs to return (default: 20)")]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination cursor from a previous response")]
    pub cursor: Option<String>,
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
    #[schemars(description = "New visibility: 'public' or 'private'")]
    pub visibility: Option<String>,
    #[schemars(description = "Current version of the catalog (required for optimistic locking)")]
    pub version: i32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteCatalogParams {
    #[schemars(description = "UUID of the catalog to delete")]
    pub catalog_id: String,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub(crate) fn ok_json<T: serde::Serialize>(value: &T) -> CallToolResult {
    match serde_json::to_string_pretty(value) {
        Ok(json) => CallToolResult::success(vec![rmcp::model::Content::text(json)]),
        Err(e) => CallToolResult::error(vec![rmcp::model::Content::text(format!(
            "Serialization error: {e}"
        ))]),
    }
}

pub(crate) fn ok_text(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![rmcp::model::Content::text(text.into())])
}

pub(crate) fn err_result(e: impl std::fmt::Display) -> CallToolResult {
    CallToolResult::error(vec![rmcp::model::Content::text(e.to_string())])
}

pub(crate) fn parse_uuid(s: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|_| format!("Invalid UUID: '{s}'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::EngramClient;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mock_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn make_server(base_url: &str) -> crate::server::EngramMcpServer {
        crate::server::EngramMcpServer::new(EngramClient::new(base_url, "engram_test"), false)
    }

    #[tokio::test]
    async fn test_list_catalogs_ok() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "name": "Rust", "version": 1}],
                "cursor": null
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
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
}
