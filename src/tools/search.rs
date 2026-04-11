use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::CallToolResult,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

use crate::client::EngramClient;
use crate::tools::catalogs::{err_result, ok_json};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Search query string")]
    pub query: String,
}

#[derive(Clone)]
pub struct SearchTools {
    pub client: EngramClient,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl SearchTools {
    pub fn new(client: EngramClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Search across all cards, catalogs, and learning paths. Returns ids and types."
    )]
    async fn search_global(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match self.client.search_global(&p.query).await {
            Ok(results) => ok_json(&results),
            Err(e) => err_result(e),
        })
    }

    #[tool(description = "Search catalogs by name or description.")]
    async fn search_catalogs(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match self.client.search_catalogs(&p.query).await {
            Ok(results) => ok_json(&results),
            Err(e) => err_result(e),
        })
    }
}

#[tool_handler]
impl ServerHandler for SearchTools {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_tools(base_url: &str) -> SearchTools {
        SearchTools::new(EngramClient::new(base_url, "engram_test"))
    }

    #[tokio::test]
    async fn test_search_global_ok() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "rust"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .search_global(Parameters(SearchParams {
                query: "rust".to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_search_catalogs_unauthorized_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/catalogs"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .search_catalogs(Parameters(SearchParams {
                query: "rust".to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }
}
