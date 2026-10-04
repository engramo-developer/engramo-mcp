use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::CallToolResult,
    model::{ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::client::EngramoClient;
use crate::dto::{CardContent, UpdateCardRequest};
use crate::tools::catalogs::{err_result, ok_json, ok_text, parse_uuid};

/// `update_card`'s `catalog_ids` is REQUIRED and must be non-empty: the underlying API accepts
/// `catalogIds: []` without error and silently moves the card into the user's default catalog
/// ("My Catalog") instead of removing it from every catalog (see engramo-mcp#64). Reject an
/// empty list here, before any HTTP call, rather than forwarding it.
pub(crate) const EMPTY_CATALOG_IDS_ERROR: &str = "catalog_ids must contain at least one catalog \
    UUID — a card must belong to at least one catalog. Build the list from the `catalogs[].id` \
    values in `get_card`/`list_cards`, plus/minus any intended changes; an empty list would \
    silently move the card to your default catalog instead of removing it from every catalog.";

/// Validates `update_card`'s `catalog_ids`: non-empty and every entry a valid UUID.
pub(crate) fn parse_catalog_ids(raw: &[String]) -> Result<Vec<Uuid>, String> {
    if raw.is_empty() {
        return Err(EMPTY_CATALOG_IDS_ERROR.to_string());
    }
    raw.iter().map(|s| parse_uuid(s)).collect()
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListCardsParams {
    #[schemars(description = "UUID of the catalog to list cards from")]
    pub catalog_id: String,
    #[schemars(
        description = "Maximum number of cards to return (default 20; values are clamped to 1..=50)"
    )]
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
    #[schemars(
        description = "Updated face content (optional). rich_text styling goes under a nested \
        `style` object, e.g. {\"text\":\"gracias.\",\"style\":{\"bold\":true,\"fontColor\":\"#27AE60\"}}."
    )]
    pub face: Option<CardContent>,
    #[schemars(description = "Updated back content (optional). Same rich_text rules as `face`.")]
    pub back: Option<CardContent>,
    #[schemars(
        description = "Catalog UUIDs this card should belong to — REPLACES the memberships you \
        can see. Start from `catalogs[].id` in `get_card`/`list_cards` (which lists only \
        catalogs visible to you), then add/remove as intended. Must contain at least one UUID — \
        a card must belong to at least one catalog, so an empty list is rejected (it is NOT a \
        way to remove the card from all catalogs; the API would otherwise silently move it to \
        your default catalog). If the card JSON has no `catalogs` key, its memberships are \
        unknown — call `get_card` first."
    )]
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
    pub client: EngramoClient,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl CardTools {
    pub fn new(client: EngramoClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List flashcards in a catalog with cursor-based pagination. Returns at most 50 items per call; pass the returned `cursor` back to fetch the next page (`cursor: null` means this is the last page). Each returned card includes its `catalogs` memberships (id, name), permission-filtered the same as `get_card` — omitted when the API didn't report memberships."
    )]
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
        description = "Get a single flashcard by UUID, including its face, back, and catalog memberships (`catalogs` is omitted when the API does not report memberships). A card that was archived (e.g. deleted after learning started) is reported as Not found even if it still appears in get_due_cards."
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
        `catalog_ids` REPLACES the memberships you can see, so build it from the \
        `catalogs[].id` values in `get_card` or `list_cards`'s response (which lists only \
        catalogs visible to you) plus/minus any intended changes. Must contain at least one \
        UUID — an empty list is rejected, since the API would otherwise silently move the card \
        to the user's default catalog. If the card JSON has no `catalogs` key, its memberships \
        are unknown — call `get_card` first. If you get a Conflict error, re-fetch and retry. \
        rich_text styling goes under a nested `style` object, e.g. \
        {\"text\":\"gracias.\",\"style\":{\"bold\":true,\"fontColor\":\"#27AE60\"}}."
    )]
    async fn update_card(
        &self,
        Parameters(p): Parameters<UpdateCardParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let card_id = match parse_uuid(&p.card_id) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let catalog_ids = match parse_catalog_ids(&p.catalog_ids) {
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
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
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
        CardTools::new(EngramoClient::new(base_url, "engramo_test"))
    }

    /// Regression test for issue #34/#35: the `list_cards` tool output's `cursor` field
    /// must reflect the API's real `nextCursor` value, not always be `null` (mirrors
    /// `catalogs::tests::test_list_catalogs_cursor_reaches_tool_output`).
    ///
    /// Also covers issue #39: `GET /catalogs/{id}/cards` now populates each card's
    /// `catalogs` memberships (same permission-filtered data as `get_card`), so the
    /// fixture includes it and the assertions check it reaches the tool output.
    #[tokio::test]
    async fn test_list_cards_cursor_reaches_tool_output() {
        let server = MockServer::start().await;
        // Distinct from the card id (F9) so a mapping bug that swapped the two would be caught.
        let catalog_id = Uuid::parse_str("00000000-0000-0000-0000-000000000099").unwrap();
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{}/cards", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": mock_id(),
                    "version": 1,
                    "face": {"text": "Q?"},
                    "back": {"text": "A."},
                    "orderNumber": 1,
                    "catalogs": [{"id": catalog_id, "name": "Spanish"}]
                }],
                "nextCursor": "abc",
                "permissions": {"canEdit": true, "canDelete": true, "isOwner": true}
            })))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .list_cards(Parameters(ListCardsParams {
                catalog_id: mock_id().to_string(),
                limit: None,
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
        assert_eq!(
            v["data"][0]["catalogs"][0]["id"],
            catalog_id.to_string(),
            "{text}"
        );
        assert_eq!(v["data"][0]["catalogs"][0]["name"], "Spanish", "{text}");
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
    async fn test_card_tools_get_card_ok_and_invalid_uuid() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{}", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": mock_id(),
                "version": 1,
                "face": {"text": "Q?"},
                "back": {"text": "A."},
                "orderNumber": 1
            })))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .get_card(Parameters(GetCardParams {
                card_id: mock_id().to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));

        let result = make_tools(&server.uri())
            .get_card(Parameters(GetCardParams {
                card_id: "bad".to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        assert_eq!(
            server.received_requests().await.unwrap_or_default().len(),
            1
        );
    }

    /// Regression test for issue #39: the `get_card` tool's JSON output must surface the
    /// `catalogs` array (previously dropped because `CardDto` didn't model it at all).
    #[tokio::test]
    async fn test_get_card_tool_output_includes_catalogs() {
        let server = MockServer::start().await;
        let catalog_id = Uuid::parse_str("00000000-0000-0000-0000-000000000099").unwrap();
        Mock::given(method("GET"))
            .and(path(format!("/cards/{}", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": mock_id(),
                "version": 1,
                "face": {"text": "Q?"},
                "back": {"text": "A."},
                "orderNumber": 1,
                "catalogs": [{"id": catalog_id, "name": "Spanish"}]
            })))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .get_card(Parameters(GetCardParams {
                card_id: mock_id().to_string(),
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
        assert_eq!(v["catalogs"][0]["id"], catalog_id.to_string(), "{text}");
        assert_eq!(v["catalogs"][0]["name"], "Spanish", "{text}");
    }

    /// Regression test for issue #39: `update_card`'s response also carries `catalogs` —
    /// assert the tool's JSON output round-trips it, not just `get_card`'s.
    #[tokio::test]
    async fn test_update_card_tool_output_includes_catalogs() {
        let server = MockServer::start().await;
        // Distinct from the card id (F9) so a mapping bug that swapped the two would be caught.
        let catalog_id = Uuid::parse_str("00000000-0000-0000-0000-000000000099").unwrap();
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{}", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": mock_id(),
                "version": 2,
                "face": {"text": "Updated"},
                "back": {"text": "A."},
                "catalogs": [{"id": catalog_id, "name": "Spanish"}]
            })))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: mock_id().to_string(),
                face: None,
                back: None,
                catalog_ids: vec![catalog_id.to_string()],
                order_number: 1,
                version: 1,
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
        assert_eq!(v["catalogs"][0]["id"], catalog_id.to_string(), "{text}");
        assert_eq!(v["catalogs"][0]["name"], "Spanish", "{text}");
    }

    #[tokio::test]
    async fn test_card_tools_invalid_uuids_return_error_without_request() {
        let server = MockServer::start().await;
        let tools = make_tools(&server.uri());

        let result = tools
            .list_cards(Parameters(ListCardsParams {
                catalog_id: "bad".to_string(),
                limit: None,
                cursor: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));

        let result = tools
            .update_card(Parameters(UpdateCardParams {
                card_id: "bad".to_string(),
                face: None,
                back: None,
                catalog_ids: vec![mock_id().to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));

        let result = tools
            .update_card(Parameters(UpdateCardParams {
                card_id: mock_id().to_string(),
                face: None,
                back: None,
                catalog_ids: vec!["bad".to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));

        let result = tools
            .delete_card(Parameters(DeleteCardParams {
                catalog_id: "bad".to_string(),
                card_id: mock_id().to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));

        let result = tools
            .delete_card(Parameters(DeleteCardParams {
                catalog_id: mock_id().to_string(),
                card_id: "bad".to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));

        assert!(
            server
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty()
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

    /// Regression for F13: the API-error arm of `list_cards`/`get_card` was only ever
    /// exercised for invalid UUIDs, never for an actual upstream error (e.g. 404).
    #[tokio::test]
    async fn test_card_tools_list_and_get_api_error_returns_error_content() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{}/cards", mock_id())))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{}", mock_id())))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let t = make_tools(&server.uri());
        let r1 = t
            .list_cards(Parameters(ListCardsParams {
                catalog_id: mock_id().to_string(),
                limit: None,
                cursor: None,
            }))
            .await
            .unwrap();
        let r2 = t
            .get_card(Parameters(GetCardParams {
                card_id: mock_id().to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(r1.is_error, Some(true));
        assert_eq!(r2.is_error, Some(true));
    }

    /// Regression for F14: `delete_card`'s success arm was only ever exercised via the
    /// 403/invalid-UUID paths, never the actual `Ok(())` -> "deleted" text.
    #[tokio::test]
    async fn test_card_tools_delete_card_ok() {
        let server = MockServer::start().await;
        let card_id = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        Mock::given(method("DELETE"))
            .and(path(format!("/catalogs/{}/cards/{}", mock_id(), card_id)))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let r = make_tools(&server.uri())
            .delete_card(Parameters(DeleteCardParams {
                catalog_id: mock_id().to_string(),
                card_id: card_id.to_string(),
            }))
            .await
            .unwrap();
        assert!(!r.is_error.unwrap_or(false));
        let text = r
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(text.contains("deleted"), "{text}");
    }

    /// Regression for issue #64: an empty `catalog_ids` list must be rejected with a clear
    /// validation error and NOT forwarded to the API — the API accepts `"catalogIds": []`
    /// silently and moves the card into the user's default catalog ("My Catalog") instead of
    /// removing it from every catalog.
    #[tokio::test]
    async fn test_card_tools_update_card_empty_catalog_ids_rejected_without_http_call() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{}", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": mock_id(), "version": 2, "face": {"text": "Q"}, "back": {"text": "A"},
                "catalogs": []
            })))
            .expect(0)
            .mount(&server)
            .await;
        let r = make_tools(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: mock_id().to_string(),
                face: None,
                back: None,
                catalog_ids: vec![],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(r.is_error.unwrap_or(false), "{r:?}");
        let text = r
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(text.contains("at least one catalog"), "{text}");
    }

    /// A non-empty `catalog_ids` list still updates memberships normally.
    #[tokio::test]
    async fn test_card_tools_update_card_nonempty_catalog_ids_still_works() {
        use wiremock::matchers::body_partial_json;
        let server = MockServer::start().await;
        let catalog_id = Uuid::parse_str("00000000-0000-0000-0000-000000000099").unwrap();
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{}", mock_id())))
            .and(body_partial_json(json!({"catalogIds": [catalog_id]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": mock_id(), "version": 2, "face": {"text": "Q"}, "back": {"text": "A"},
                "catalogs": [{"id": catalog_id, "name": "Spanish"}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let r = make_tools(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: mock_id().to_string(),
                face: None,
                back: None,
                catalog_ids: vec![catalog_id.to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(!r.is_error.unwrap_or(false), "{r:?}");
    }
}
