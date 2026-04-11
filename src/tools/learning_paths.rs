use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::CallToolResult,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

use crate::client::EngramClient;
use crate::dto::CreateLearningPathRequest;
use crate::tools::catalogs::{err_result, ok_json, ok_text, parse_uuid};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListLearningPathsParams {
    #[schemars(description = "Maximum number of paths to return (default: 20)")]
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
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ActivateLearningPathParams {
    #[schemars(description = "UUID of the learning path to activate")]
    pub path_id: String,
}

#[derive(Clone)]
pub struct LearningPathTools {
    pub client: EngramClient,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl LearningPathTools {
    pub fn new(client: EngramClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "List all learning paths with cursor-based pagination.")]
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
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
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
        LearningPathTools::new(EngramClient::new(base_url, "engram_test"))
    }

    #[tokio::test]
    async fn test_list_learning_paths_ok() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning-paths"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "cursor": null
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
