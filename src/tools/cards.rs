use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::CallToolResult,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::client::EngramClient;
use crate::dto::{CardContent, UpdateCardRequest};
use crate::tools::catalogs::{err_result, ok_json, ok_text, parse_uuid};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListCardsParams {
    #[schemars(description = "UUID of the catalog to list cards from")]
    pub catalog_id: String,
    #[schemars(description = "Maximum number of cards to return (default: 20)")]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination cursor from a previous response")]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetCardParams {
    #[schemars(description = "UUID of the card")]
    pub card_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateCardParams {
    #[schemars(description = "UUID of the card to update")]
    pub card_id: String,
    #[schemars(description = "Updated face content (optional)")]
    pub face: Option<CardContent>,
    #[schemars(description = "Updated back content (optional)")]
    pub back: Option<CardContent>,
    #[schemars(description = "All catalog UUIDs this card should belong to")]
    pub catalog_ids: Vec<String>,
    #[schemars(description = "Card order number within the catalog")]
    pub order_number: i64,
    #[schemars(description = "Current version for optimistic locking — fetch the card first")]
    pub version: i32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteCardParams {
    #[schemars(description = "UUID of the catalog the card belongs to")]
    pub catalog_id: String,
    #[schemars(description = "UUID of the card to delete")]
    pub card_id: String,
}

#[derive(Clone)]
pub struct CardTools {
    pub client: EngramClient,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl CardTools {
    pub fn new(client: EngramClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "List flashcards in a catalog with cursor-based pagination.")]
    async fn list_cards(
        &self,
        Parameters(p): Parameters<ListCardsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match parse_uuid(&p.catalog_id) {
            Ok(id) => match self
                .client
                .list_cards(id, p.limit, p.cursor.as_deref())
                .await
            {
                Ok(resp) => ok_json(&resp),
                Err(e) => err_result(e),
            },
            Err(e) => err_result(e),
        })
    }

    #[tool(
        description = "Get a single flashcard by UUID, including its face, back, and catalog memberships."
    )]
    async fn get_card(
        &self,
        Parameters(p): Parameters<GetCardParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match parse_uuid(&p.card_id) {
            Ok(id) => match self.client.get_card(id).await {
                Ok(card) => ok_json(&card),
                Err(e) => err_result(e),
            },
            Err(e) => err_result(e),
        })
    }

    #[tool(
        description = "Update an existing flashcard's face, back, or catalog memberships. \
        Requires the current version for optimistic locking — fetch the card first. \
        If you get a Conflict error, re-fetch and retry."
    )]
    async fn update_card(
        &self,
        Parameters(p): Parameters<UpdateCardParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let card_id = match parse_uuid(&p.card_id) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let catalog_ids: Result<Vec<Uuid>, _> =
            p.catalog_ids.iter().map(|s| parse_uuid(s)).collect();
        let catalog_ids = match catalog_ids {
            Ok(ids) => ids,
            Err(e) => return Ok(err_result(e)),
        };
        let req = UpdateCardRequest {
            catalog_ids,
            order_number: p.order_number,
            face: p.face,
            back: p.back,
            version: p.version,
        };
        Ok(match self.client.update_card(card_id, &req).await {
            Ok(card) => ok_json(&card),
            Err(e) => err_result(e),
        })
    }

    #[tool(
        description = "Delete a flashcard from a catalog. If the card has learning progress it is archived; otherwise hard-deleted."
    )]
    async fn delete_card(
        &self,
        Parameters(p): Parameters<DeleteCardParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let catalog_id = match parse_uuid(&p.catalog_id) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let card_id = match parse_uuid(&p.card_id) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        Ok(match self.client.delete_card(catalog_id, card_id).await {
            Ok(()) => ok_text("Card deleted successfully."),
            Err(e) => err_result(e),
        })
    }
}

#[tool_handler]
impl ServerHandler for CardTools {
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

    fn mock_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn make_tools(base_url: &str) -> CardTools {
        CardTools::new(EngramClient::new(base_url, "engram_test"))
    }

    #[tokio::test]
    async fn test_delete_card_permission_denied_returns_error_content() {
        let server = MockServer::start().await;
        let card_id = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        Mock::given(method("DELETE"))
            .and(path(format!("/catalogs/{}/cards/{}", mock_id(), card_id)))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({"error": "you don't have edit access"})),
            )
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .delete_card(Parameters(DeleteCardParams {
                catalog_id: mock_id().to_string(),
                card_id: card_id.to_string(),
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
        assert!(
            text.contains("ermission") || text.contains("access"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn test_update_card_conflict_message_is_actionable() {
        let server = MockServer::start().await;
        let card_id = mock_id();
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: None,
                back: None,
                catalog_ids: vec![mock_id().to_string()],
                order_number: 1,
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
}
