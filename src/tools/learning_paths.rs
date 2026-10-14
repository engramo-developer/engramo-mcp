use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::CallToolResult,
    model::{ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};

use crate::client::EngramoClient;
use crate::dto::{CreateLearningPathRequest, LearningPathDto};
use crate::tools::catalogs::{err_result, ok_json, ok_text, parse_uuid};

/// Upper bound on `catalog_ids` accepted by `create_learning_path` in a single call. Keeps one
/// MCP call from turning into an unbounded run of sequential upstream
/// `POST /learning-paths/{id}/catalogs/{cid}` requests (mirrors `generate::MAX_BATCH_CARDS`).
pub const MAX_PATH_CATALOG_IDS: usize = 50;

/// One `catalog_ids` entry that failed to add in `create_learning_path`.
#[derive(Debug, Serialize)]
pub struct CatalogAddFailure {
    pub catalog_id: String,
    pub error: String,
}

/// `create_learning_path`'s response when `catalog_ids` is set: the created path's own fields,
/// flattened, plus which catalogs were added/failed.
#[derive(Debug, Serialize)]
pub struct CreatedLearningPathWithCatalogs<'a> {
    #[serde(flatten)]
    pub path: &'a LearningPathDto,
    pub catalogs_added: Vec<String>,
    pub catalogs_failed: Vec<CatalogAddFailure>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListLearningPathsParams {
    #[schemars(
        description = "Maximum number of paths to return (default 20; values are clamped to 1..=50)"
    )]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination cursor from a previous response")]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetLearningPathParams {
    #[schemars(description = "UUID of the learning path")]
    pub path_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateLearningPathParams {
    #[schemars(description = "Name of the learning path")]
    pub name: String,
    #[schemars(description = "Optional description")]
    pub description: Option<String>,
    #[schemars(
        length(max = MAX_PATH_CATALOG_IDS),
        description = "Optional UUIDs of catalogs to add to the new path right after it's created. Duplicates are ignored; the per-call maximum is given by maxItems. Each catalog is added in its own request after the path exists (non-atomic) — the response includes `catalogs_added`/`catalogs_failed` when this is set, so failed ids can be retried with add_catalog_to_learning_path."
    )]
    pub catalog_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ActivateLearningPathParams {
    #[schemars(description = "UUID of the learning path to activate")]
    pub path_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddCatalogToLearningPathParams {
    #[schemars(description = "UUID of the learning path")]
    pub path_id: String,
    #[schemars(description = "UUID of the catalog to add to the path")]
    pub catalog_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RemoveCatalogFromLearningPathParams {
    #[schemars(description = "UUID of the learning path")]
    pub path_id: String,
    #[schemars(description = "UUID of the catalog to remove from the path")]
    pub catalog_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateLearningPathParams {
    #[schemars(description = "UUID of the learning path to update")]
    pub path_id: String,
    #[schemars(description = "New name")]
    pub name: Option<String>,
    #[schemars(description = "New description")]
    pub description: Option<String>,
    #[schemars(description = "New tags")]
    pub tags: Option<Vec<String>>,
    #[schemars(
        description = "New visibility: 'public' or 'private'. 'unlisted' is catalog-only and is rejected for learning paths."
    )]
    pub visibility: Option<String>,
    #[schemars(
        description = "Current version of the learning path (required for optimistic locking)"
    )]
    pub version: i32,
}

#[derive(Clone)]
pub struct LearningPathTools {
    pub client: EngramoClient,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl LearningPathTools {
    pub fn new(client: EngramoClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List all learning paths with cursor-based pagination. Returns at most 50 items per call; pass the returned `cursor` back to fetch the next page (`cursor: null` means this is the last page)."
    )]
    async fn list_learning_paths(
        &self,
        Parameters(p): Parameters<ListLearningPathsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(
            match self
                .client
                .list_learning_paths(p.limit, p.cursor.as_deref())
                .await
            {
                Ok(resp) => ok_json(&resp),
                Err(e) => err_result(e),
            },
        )
    }

    #[tool(description = "Get full details of a learning path, including its catalogs.")]
    async fn get_learning_path(
        &self,
        Parameters(p): Parameters<GetLearningPathParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match parse_uuid(&p.path_id) {
            Ok(id) => match self.client.get_learning_path(id).await {
                Ok(path) => ok_json(&path),
                Err(e) => err_result(e),
            },
            Err(e) => err_result(e),
        })
    }

    #[tool(description = "Create a new learning path.")]
    async fn create_learning_path(
        &self,
        Parameters(p): Parameters<CreateLearningPathParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let req = CreateLearningPathRequest {
            name: p.name,
            description: p.description,
        };
        Ok(match self.client.create_learning_path(&req).await {
            Ok(path) => ok_json(&path),
            Err(e) => err_result(e),
        })
    }

    #[tool(description = "Activate a learning path so its catalogs are included in daily reviews.")]
    async fn activate_learning_path(
        &self,
        Parameters(p): Parameters<ActivateLearningPathParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match parse_uuid(&p.path_id) {
            Ok(id) => match self.client.activate_learning_path(id).await {
                Ok(()) => ok_text("Learning path activated."),
                Err(e) => err_result(e),
            },
            Err(e) => err_result(e),
        })
    }

    #[tool(
        description = "Deactivate a learning path so its catalogs are excluded from daily reviews."
    )]
    async fn deactivate_learning_path(
        &self,
        Parameters(p): Parameters<ActivateLearningPathParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match parse_uuid(&p.path_id) {
            Ok(id) => match self.client.deactivate_learning_path(id).await {
                Ok(()) => ok_text("Learning path deactivated."),
                Err(e) => err_result(e),
            },
            Err(e) => err_result(e),
        })
    }
}

#[tool_handler]
impl ServerHandler for LearningPathTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mock_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn make_tools(base_url: &str) -> LearningPathTools {
        LearningPathTools::new(EngramoClient::new(base_url, "engramo_test"))
    }

    #[tokio::test]
    async fn test_list_learning_paths_ok() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning-paths"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .list_learning_paths(Parameters(ListLearningPathsParams {
                limit: None,
                cursor: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    /// Regression test for issue #34/#35: the `list_learning_paths` tool output's `cursor`
    /// field must reflect the API's real `nextCursor` value, not always be `null` (mirrors
    /// `catalogs::tests::test_list_catalogs_cursor_reaches_tool_output`). A reporter
    /// confirmed this with `list_learning_paths({limit:1})` on an account with 3 paths.
    #[tokio::test]
    async fn test_list_learning_paths_cursor_reaches_tool_output() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning-paths"))
            .and(wiremock::matchers::query_param("limit", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "name": "Path 1", "version": 1}],
                "nextCursor": "abc"
            })))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .list_learning_paths(Parameters(ListLearningPathsParams {
                limit: Some(1),
                cursor: None,
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
    async fn test_get_learning_path_invalid_uuid_returns_error() {
        let server = MockServer::start().await;
        let result = make_tools(&server.uri())
            .get_learning_path(Parameters(GetLearningPathParams {
                path_id: "not-a-uuid".to_string(),
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
    async fn test_activate_learning_path_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/learning-paths/{}/activate", mock_id())))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .activate_learning_path(Parameters(ActivateLearningPathParams {
                path_id: mock_id().to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }
}
