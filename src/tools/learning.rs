use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::CallToolResult,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

use crate::client::EngramClient;
use crate::tools::catalogs::{err_result, ok_json, ok_text, parse_uuid};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DueCardsParams {
    #[schemars(description = "Maximum number of cards to return (default: 20)")]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination cursor from a previous response")]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddCardToLearningParams {
    #[schemars(description = "UUID of the card to add to learning")]
    pub card_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddCatalogToLearningParams {
    #[schemars(description = "UUID of the catalog — adds all its cards to learning")]
    pub catalog_id: String,
}

#[derive(Clone)]
pub struct LearningTools {
    pub client: EngramClient,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl LearningTools {
    pub fn new(client: EngramClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Get flashcards due for review today, sorted by priority. Use this to start a study session."
    )]
    async fn get_due_cards(
        &self,
        Parameters(p): Parameters<DueCardsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(
            match self
                .client
                .get_due_cards(p.limit, p.cursor.as_deref())
                .await
            {
                Ok(resp) => ok_json(&resp),
                Err(e) => err_result(e),
            },
        )
    }

    #[tool(
        description = "Get all cards currently in the learning queue (due and future), with pagination."
    )]
    async fn get_all_learning_cards(
        &self,
        Parameters(p): Parameters<DueCardsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(
            match self
                .client
                .get_all_learning_cards(p.limit, p.cursor.as_deref())
                .await
            {
                Ok(resp) => ok_json(&resp),
                Err(e) => err_result(e),
            },
        )
    }

    #[tool(description = "Add a single card to the learning queue. It will appear in due reviews.")]
    async fn add_card_to_learning(
        &self,
        Parameters(p): Parameters<AddCardToLearningParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match parse_uuid(&p.card_id) {
            Ok(id) => match self.client.add_card_to_learning(id).await {
                Ok(()) => ok_text("Card added to learning."),
                Err(e) => err_result(e),
            },
            Err(e) => err_result(e),
        })
    }

    #[tool(description = "Add all cards in a catalog to the learning queue at once.")]
    async fn add_catalog_to_learning(
        &self,
        Parameters(p): Parameters<AddCatalogToLearningParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match parse_uuid(&p.catalog_id) {
            Ok(id) => match self.client.add_catalog_to_learning(id).await {
                Ok(()) => ok_text("Catalog added to learning."),
                Err(e) => err_result(e),
            },
            Err(e) => err_result(e),
        })
    }
}

#[tool_handler]
impl ServerHandler for LearningTools {
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

    fn make_tools(base_url: &str) -> LearningTools {
        LearningTools::new(EngramClient::new(base_url, "engram_test"))
    }

    #[tokio::test]
    async fn test_get_due_cards_ok() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning/cards"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "cursor": null,
                "total_count": 0
            })))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .get_due_cards(Parameters(DueCardsParams {
                limit: None,
                cursor: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_add_card_invalid_uuid_returns_error() {
        let server = MockServer::start().await;
        let result = make_tools(&server.uri())
            .add_card_to_learning(Parameters(AddCardToLearningParams {
                card_id: "not-a-uuid".to_string(),
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
}
