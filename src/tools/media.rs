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
pub struct ListMediaParams {
    #[schemars(description = "Filter by media type (e.g., 'image', 'audio')")]
    pub media_type: Option<String>,
    #[schemars(description = "Maximum number of items to return (default: 20)")]
    pub limit: Option<i64>,
}

#[derive(Clone)]
pub struct MediaTools {
    pub client: EngramClient,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl MediaTools {
    pub fn new(client: EngramClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List uploaded media files. Optionally filter by media type ('image', 'audio', etc.)."
    )]
    async fn list_media(
        &self,
        Parameters(p): Parameters<ListMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(
            match self
                .client
                .list_media(p.media_type.as_deref(), p.limit)
                .await
            {
                Ok(resp) => ok_json(&resp),
                Err(e) => err_result(e),
            },
        )
    }
}

#[tool_handler]
impl ServerHandler for MediaTools {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_tools(base_url: &str) -> MediaTools {
        MediaTools::new(EngramClient::new(base_url, "engram_test"))
    }

    #[tokio::test]
    async fn test_list_media_ok() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "cursor": null
            })))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .list_media(Parameters(ListMediaParams {
                media_type: None,
                limit: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_list_media_unauthorized_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .list_media(Parameters(ListMediaParams {
                media_type: None,
                limit: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }
}
