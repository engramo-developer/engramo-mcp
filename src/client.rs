use std::time::Duration;

use reqwest::Client;
use uuid::Uuid;

use crate::dto::{
    AiChatRequest, AiChatResponseDto, CardAiBatchResultDto, CardAiRequest, CardDto, CatalogDto,
    CatalogWithCardsCreatedDto, CatalogWithCardsResponse, CreateCardRequest, CreateCatalogRequest,
    CreateCatalogWithCardsApiRequest, CreateLearningPathRequest, GlobalSearchResult,
    LearningCardDto, LearningPathDetailDto, LearningPathDto, MediaDto, PagedResponse,
    PagedResponseWithCount, UpdateCardRequest, UpdateCatalogRequest, UploadMediaResponseDto,
    UsageSummaryDto, UserSubscriptionDto,
};
use crate::error::ApiError;

/// Typed HTTP client for the Engram REST API.
/// Every request automatically attaches the user's API token via `X-Api-Key`.
/// All methods return `Result<T, ApiError>` — callers convert errors to `CallToolResult`
/// with `is_error: true` (never propagate as raw `Err()`).
#[derive(Clone)]
pub struct EngramClient {
    http: Client,
    base_url: String,
    api_token: String,
}

/// Connect timeout for requests to the Engram REST API. Kept shorter than the overall
/// request timeout so a dead/unreachable host fails fast instead of tying up the
/// connection budget.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Overall per-request timeout (connect + send + receive). Without this, a slow or
/// hung upstream stalls the tool call indefinitely — in `http` mode that pins the
/// session task and its connection until the client gives up.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

impl EngramClient {
    /// Builds an `EngramClient` with its own dedicated `reqwest::Client`. Prefer
    /// [`Self::with_http`] when serving multiple sessions from one process (`http`
    /// mode) so they share a single connection pool instead of each paying for its
    /// own TLS handshakes.
    pub fn new(base_url: impl Into<String>, api_token: impl Into<String>) -> Self {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("reqwest client with static, well-formed config must build");
        Self::with_http(http, base_url, api_token)
    }

    /// Builds an `EngramClient` from a pre-built, shared `reqwest::Client` — used by
    /// `http` mode so every session's requests flow through one connection pool.
    pub fn with_http(
        http: Client,
        base_url: impl Into<String>,
        api_token: impl Into<String>,
    ) -> Self {
        Self {
            http,
            base_url: base_url.into(),
            api_token: api_token.into(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let resp = self
            .http
            .get(self.url(path))
            .header("X-Api-Key", &self.api_token)
            .send()
            .await?;
        self.deserialize(resp).await
    }

    async fn get_with_query<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<T, ApiError> {
        let resp = self
            .http
            .get(self.url(path))
            .header("X-Api-Key", &self.api_token)
            .query(params)
            .send()
            .await?;
        self.deserialize(resp).await
    }

    async fn post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        let resp = self
            .http
            .post(self.url(path))
            .header("X-Api-Key", &self.api_token)
            .json(body)
            .send()
            .await?;
        self.deserialize(resp).await
    }

    async fn post_void(&self, path: &str) -> Result<(), ApiError> {
        let resp = self
            .http
            .post(self.url(path))
            .header("X-Api-Key", &self.api_token)
            .send()
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ApiError::from_response(resp).await)
        }
    }

    async fn patch<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        let resp = self
            .http
            .patch(self.url(path))
            .header("X-Api-Key", &self.api_token)
            .json(body)
            .send()
            .await?;
        self.deserialize(resp).await
    }

    async fn delete_ok(&self, path: &str) -> Result<(), ApiError> {
        let resp = self
            .http
            .delete(self.url(path))
            .header("X-Api-Key", &self.api_token)
            .send()
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ApiError::from_response(resp).await)
        }
    }

    async fn deserialize<T: serde::de::DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T, ApiError> {
        if resp.status().is_success() {
            resp.json::<T>().await.map_err(ApiError::from)
        } else {
            Err(ApiError::from_response(resp).await)
        }
    }

    // ── Catalogs ──────────────────────────────────────────────────────────────

    pub async fn list_catalogs(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<PagedResponse<CatalogDto>, ApiError> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(l) = limit {
            params.push(("limit", l.to_string()));
        }
        if let Some(c) = cursor {
            params.push(("cursor", c.to_string()));
        }
        self.get_with_query("/catalogs", &params).await
    }

    pub async fn get_catalog(&self, catalog_id: Uuid) -> Result<CatalogDto, ApiError> {
        self.get(&format!("/catalogs/{catalog_id}")).await
    }

    pub async fn create_catalog(&self, req: &CreateCatalogRequest) -> Result<CatalogDto, ApiError> {
        self.post("/catalogs", req).await
    }

    pub async fn update_catalog(
        &self,
        catalog_id: Uuid,
        req: &UpdateCatalogRequest,
    ) -> Result<CatalogDto, ApiError> {
        self.patch(&format!("/catalogs/{catalog_id}"), req).await
    }

    pub async fn delete_catalog(&self, catalog_id: Uuid) -> Result<(), ApiError> {
        self.delete_ok(&format!("/catalogs/{catalog_id}")).await
    }

    // ── Cards ─────────────────────────────────────────────────────────────────

    pub async fn list_cards(
        &self,
        catalog_id: Uuid,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<CatalogWithCardsResponse, ApiError> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(l) = limit {
            params.push(("limit", l.to_string()));
        }
        if let Some(c) = cursor {
            params.push(("cursor", c.to_string()));
        }
        self.get_with_query(&format!("/catalogs/{catalog_id}/cards"), &params)
            .await
    }

    pub async fn get_card(&self, card_id: Uuid) -> Result<CardDto, ApiError> {
        self.get(&format!("/cards/{card_id}")).await
    }

    pub async fn create_card(&self, req: &CreateCardRequest) -> Result<CardDto, ApiError> {
        self.post("/cards", req).await
    }

    pub async fn create_catalog_with_cards(
        &self,
        req: &CreateCatalogWithCardsApiRequest,
    ) -> Result<CatalogWithCardsCreatedDto, ApiError> {
        self.post("/catalogs/with-cards", req).await
    }

    pub async fn update_card(
        &self,
        card_id: Uuid,
        req: &UpdateCardRequest,
    ) -> Result<CardDto, ApiError> {
        self.patch(&format!("/cards/{card_id}"), req).await
    }

    pub async fn delete_card(&self, catalog_id: Uuid, card_id: Uuid) -> Result<(), ApiError> {
        self.delete_ok(&format!("/catalogs/{catalog_id}/cards/{card_id}"))
            .await
    }

    // ── Learning ──────────────────────────────────────────────────────────────

    pub async fn get_due_cards(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<PagedResponseWithCount<LearningCardDto>, ApiError> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(l) = limit {
            params.push(("limit", l.to_string()));
        }
        if let Some(c) = cursor {
            params.push(("cursor", c.to_string()));
        }
        self.get_with_query("/learning/cards", &params).await
    }

    pub async fn get_all_learning_cards(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<PagedResponseWithCount<LearningCardDto>, ApiError> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(l) = limit {
            params.push(("limit", l.to_string()));
        }
        if let Some(c) = cursor {
            params.push(("cursor", c.to_string()));
        }
        self.get_with_query("/learning/cards/all", &params).await
    }

    pub async fn add_card_to_learning(&self, card_id: Uuid) -> Result<(), ApiError> {
        self.post_void(&format!("/learning/cards/{card_id}")).await
    }

    pub async fn add_catalog_to_learning(&self, catalog_id: Uuid) -> Result<(), ApiError> {
        self.post_void(&format!("/learning/catalogs/{catalog_id}"))
            .await
    }

    /// Grade a card. `grade` must be one of: "again", "hard", "good", "easy".
    pub async fn grade_card(&self, card_id: Uuid, grade: &str) -> Result<(), ApiError> {
        if !matches!(grade, "again" | "hard" | "good" | "easy") {
            return Err(ApiError::BadRequest(format!(
                "invalid grade '{grade}': must be one of \"again\", \"hard\", \"good\", \"easy\""
            )));
        }
        self.post_void(&format!("/learning/cards/{card_id}/{grade}"))
            .await
    }

    pub async fn learning_cards_count(&self) -> Result<i64, ApiError> {
        self.get("/learning/cards/count").await
    }

    pub async fn learning_cards_total(&self) -> Result<i64, ApiError> {
        self.get("/learning/cards/total").await
    }

    // ── Learning Paths ────────────────────────────────────────────────────────

    pub async fn list_learning_paths(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<PagedResponse<LearningPathDto>, ApiError> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(l) = limit {
            params.push(("limit", l.to_string()));
        }
        if let Some(c) = cursor {
            params.push(("cursor", c.to_string()));
        }
        self.get_with_query("/learning-paths", &params).await
    }

    pub async fn get_learning_path(
        &self,
        path_id: Uuid,
    ) -> Result<LearningPathDetailDto, ApiError> {
        self.get(&format!("/learning-paths/{path_id}")).await
    }

    pub async fn create_learning_path(
        &self,
        req: &CreateLearningPathRequest,
    ) -> Result<LearningPathDto, ApiError> {
        self.post("/learning-paths", req).await
    }

    pub async fn activate_learning_path(&self, path_id: Uuid) -> Result<(), ApiError> {
        self.post_void(&format!("/learning-paths/{path_id}/activate"))
            .await
    }

    pub async fn deactivate_learning_path(&self, path_id: Uuid) -> Result<(), ApiError> {
        self.post_void(&format!("/learning-paths/{path_id}/deactivate"))
            .await
    }

    // ── Search ────────────────────────────────────────────────────────────────

    pub async fn search_global(&self, query: &str) -> Result<Vec<GlobalSearchResult>, ApiError> {
        self.get_with_query("/search", &[("q", query.to_string())])
            .await
    }

    pub async fn search_catalogs(&self, query: &str) -> Result<Vec<CatalogDto>, ApiError> {
        self.get_with_query("/search/catalogs", &[("q", query.to_string())])
            .await
    }

    // ── Media ─────────────────────────────────────────────────────────────────

    pub async fn list_media(
        &self,
        media_type: Option<&str>,
        limit: Option<i64>,
    ) -> Result<PagedResponse<MediaDto>, ApiError> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(mt) = media_type {
            params.push(("media_type", mt.to_string()));
        }
        if let Some(l) = limit {
            params.push(("limit", l.to_string()));
        }
        self.get_with_query("/media", &params).await
    }

    /// Uploads a single media file (image or audio, ≤10MB) and returns its new media id,
    /// ready to attach via a card's `audio_id`/`visual_id` or a catalog's `image_id`.
    pub async fn upload_media(
        &self,
        content: Vec<u8>,
        filename: &str,
        content_type: &str,
    ) -> Result<Uuid, ApiError> {
        let part = reqwest::multipart::Part::bytes(content)
            .file_name(filename.to_string())
            .mime_str(content_type)
            .map_err(|e| ApiError::BadRequest(format!("Invalid content type: {e}")))?;
        let form = reqwest::multipart::Form::new().part("file", part);

        let resp = self
            .http
            .post(self.url("/media"))
            .header("X-Api-Key", &self.api_token)
            .multipart(form)
            .send()
            .await?;
        let parsed: UploadMediaResponseDto = self.deserialize(resp).await?;
        parsed
            .media_ids
            .ids
            .into_iter()
            .next()
            .ok_or(ApiError::Internal)
    }

    // ── Subscription ──────────────────────────────────────────────────────────

    pub async fn get_subscription(&self) -> Result<UserSubscriptionDto, ApiError> {
        self.get("/me/subscription").await
    }

    pub async fn get_usage(&self) -> Result<UsageSummaryDto, ApiError> {
        self.get("/me/usage").await
    }

    // ── Paid AI (feature-flagged) ────────────────────────────────────────────

    /// Fires server-side TTS generation for cards missing face audio.
    /// Uses EngrAmo's paid AI — only registered when `ENGRAM_ENABLE_PAID_AI` is on.
    /// The API accepts the request (202) and delivers audio asynchronously via
    /// callback; the response only confirms which cards were queued.
    pub async fn generate_tts_for_cards(
        &self,
        req: &CardAiRequest,
    ) -> Result<CardAiBatchResultDto, ApiError> {
        self.post("/catalogs/tts", req).await
    }

    /// Translates cards whose back text is empty, using EngrAmo's paid AI.
    pub async fn translate_cards(
        &self,
        req: &CardAiRequest,
    ) -> Result<CardAiBatchResultDto, ApiError> {
        self.post("/catalogs/translate", req).await
    }

    /// Fills in missing word-level dictionary entries on cards, using EngrAmo's paid AI.
    pub async fn generate_dictionary_for_cards(
        &self,
        req: &CardAiRequest,
    ) -> Result<CardAiBatchResultDto, ApiError> {
        self.post("/catalogs/dictionary", req).await
    }

    /// Sends a message to the EngrAmo AI agent (server-side LLM chat), using EngrAmo's paid AI.
    pub async fn ai_agent_chat(&self, req: &AiChatRequest) -> Result<AiChatResponseDto, ApiError> {
        self.post("/ai-agent", req).await
    }

    /// Creates a new catalog from an uploaded ZIP/JSON content file, auto-translating
    /// every card into `target_lang`. Uses EngrAmo's paid AI.
    pub async fn translate_batch_import(
        &self,
        target_lang: &str,
        catalog_metadata: &CreateCatalogRequest,
        content_file_name: &str,
        content_file: Vec<u8>,
    ) -> Result<CatalogDto, ApiError> {
        let metadata_json = serde_json::to_string(catalog_metadata).map_err(|e| {
            ApiError::BadRequest(format!("failed to serialize catalog metadata: {e}"))
        })?;
        let content_part =
            reqwest::multipart::Part::bytes(content_file).file_name(content_file_name.to_string());
        let form = reqwest::multipart::Form::new()
            .text("catalog_metadata", metadata_json)
            .part("content_file", content_part);

        let resp = self
            .http
            .post(self.url(&format!("/catalogs/translate-batch-import/{target_lang}")))
            .header("X-Api-Key", &self.api_token)
            .multipart(form)
            .send()
            .await?;
        self.deserialize(resp).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mock_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn client(base_url: &str) -> EngramClient {
        EngramClient::new(base_url, "engram_test_token")
    }

    async fn assert_auth_header(server: &MockServer) {
        // Every request must include the X-Api-Key header.
        // wiremock 0.6: received_requests() returns Option<Vec<Request>>
        let requests = server.received_requests().await.unwrap_or_default();
        assert!(!requests.is_empty(), "no requests recorded");
        for r in &requests {
            let val = r
                .headers
                .get("x-api-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            assert_eq!(
                val, "engram_test_token",
                "X-Api-Key header missing or wrong"
            );
        }
    }

    // ── Catalogs ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_catalogs_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .and(header("x-api-key", "engram_test_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "name": "Rust", "version": 1}],
                "cursor": null
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_catalogs(None, None)
            .await
            .unwrap();
        assert_eq!(result.data.len(), 1);
        assert_eq!(result.data[0].name, "Rust");
        assert_auth_header(&server).await;
    }

    #[tokio::test]
    async fn test_list_catalogs_with_pagination() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .and(query_param("limit", "10"))
            .and(query_param("cursor", "abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "cursor": null
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_catalogs(Some(10), Some("abc"))
            .await
            .unwrap();
        assert!(result.data.is_empty());
    }

    #[tokio::test]
    async fn test_list_catalogs_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let err = client(&server.uri())
            .list_catalogs(None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Unauthorized));
    }

    #[tokio::test]
    async fn test_get_catalog_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{}", mock_id())))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({"error": "catalog not found"})),
            )
            .mount(&server)
            .await;

        let err = client(&server.uri())
            .get_catalog(mock_id())
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
        assert!(err.to_string().contains("catalog not found"));
    }

    #[tokio::test]
    async fn test_create_catalog_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": mock_id(),
                "name": "New Catalog",
                "version": 1
            })))
            .mount(&server)
            .await;

        let req = CreateCatalogRequest {
            name: "New Catalog".to_string(),
            description: None,
            tags: None,
            visibility: None,
        };
        let result = client(&server.uri()).create_catalog(&req).await.unwrap();
        assert_eq!(result.name, "New Catalog");
    }

    #[tokio::test]
    async fn test_create_catalog_quota_exceeded() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "error": "quota_exceeded",
                "resource_type": "catalogs_total",
                "used": 10,
                "limit": 10
            })))
            .mount(&server)
            .await;

        let req = CreateCatalogRequest {
            name: "Too Many".to_string(),
            description: None,
            tags: None,
            visibility: None,
        };
        let err = client(&server.uri())
            .create_catalog(&req)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ApiError::QuotaExceeded { resource_type, .. } if resource_type == "catalogs_total"
        ));
    }

    #[tokio::test]
    async fn test_update_catalog_conflict() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/catalogs/{}", mock_id())))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let req = UpdateCatalogRequest {
            name: Some("New Name".to_string()),
            description: None,
            tags: None,
            visibility: None,
            version: 1,
        };
        let err = client(&server.uri())
            .update_catalog(mock_id(), &req)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Conflict(_)));
        assert!(err.to_string().contains("Fetch the latest version"));
    }

    #[tokio::test]
    async fn test_delete_catalog_success() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!("/catalogs/{}", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "deleted"})))
            .mount(&server)
            .await;

        client(&server.uri())
            .delete_catalog(mock_id())
            .await
            .unwrap();
    }

    // ── Cards ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_card_success() {
        use crate::dto::CardContent;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": mock_id(),
                "version": 1,
                "face": {"text": "Q?"},
                "back": {"text": "A."}
            })))
            .mount(&server)
            .await;

        let req = CreateCardRequest {
            catalog_id: None,
            face: CardContent::plain("Q?"),
            back: CardContent::plain("A."),
        };
        let result = client(&server.uri()).create_card(&req).await.unwrap();
        assert_eq!(result.face.text, "Q?");
    }

    #[tokio::test]
    async fn test_delete_card_permission_denied() {
        let server = MockServer::start().await;
        let catalog_id = mock_id();
        let card_id = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        Mock::given(method("DELETE"))
            .and(path(format!("/catalogs/{catalog_id}/cards/{card_id}")))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({"error": "you don't have edit access"})),
            )
            .mount(&server)
            .await;

        let err = client(&server.uri())
            .delete_card(catalog_id, card_id)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::PermissionDenied(_)));
    }

    // ── Learning ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_learning_cards_count() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning/cards/count"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(42)))
            .mount(&server)
            .await;

        let count = client(&server.uri()).learning_cards_count().await.unwrap();
        assert_eq!(count, 42);
    }

    #[tokio::test]
    async fn test_grade_card_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/learning/cards/{}/good", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "updated"})))
            .mount(&server)
            .await;

        client(&server.uri())
            .grade_card(mock_id(), "good")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_add_catalog_to_learning() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/learning/catalogs/{}", mock_id())))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"status": "ok"})))
            .mount(&server)
            .await;

        client(&server.uri())
            .add_catalog_to_learning(mock_id())
            .await
            .unwrap();
    }

    // ── Search ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_search_global() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "rust"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": mock_id(), "name": "Rust catalog", "item_type": "catalog"}
            ])))
            .mount(&server)
            .await;

        let results = client(&server.uri()).search_global("rust").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name.as_deref(), Some("Rust catalog"));
    }

    #[tokio::test]
    async fn test_search_catalogs() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/catalogs"))
            .and(query_param("q", "rust"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": mock_id(), "name": "Rust Basics", "version": 1}
            ])))
            .mount(&server)
            .await;

        let results = client(&server.uri()).search_catalogs("rust").await.unwrap();
        assert_eq!(results.len(), 1);
    }

    // ── Learning Paths ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_learning_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/learning-paths"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": mock_id(), "name": "My Path", "version": 1
            })))
            .mount(&server)
            .await;

        let req = CreateLearningPathRequest {
            name: "My Path".to_string(),
            description: None,
        };
        let result = client(&server.uri())
            .create_learning_path(&req)
            .await
            .unwrap();
        assert_eq!(result.name, "My Path");
    }

    #[tokio::test]
    async fn test_activate_learning_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/learning-paths/{}/activate", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "activated"})))
            .mount(&server)
            .await;

        client(&server.uri())
            .activate_learning_path(mock_id())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_deactivate_learning_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/learning-paths/{}/deactivate", mock_id())))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"status": "deactivated"})),
            )
            .mount(&server)
            .await;

        client(&server.uri())
            .deactivate_learning_path(mock_id())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_get_learning_path_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/learning-paths/{}", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": mock_id(), "name": "My Path", "version": 1,
                "catalogs": []
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .get_learning_path(mock_id())
            .await
            .unwrap();
        assert_eq!(result.name, "My Path");
    }

    #[tokio::test]
    async fn test_list_learning_paths_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning-paths"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "name": "Path 1", "version": 1}],
                "cursor": null
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_learning_paths(None, None)
            .await
            .unwrap();
        assert_eq!(result.data.len(), 1);
        assert_eq!(result.data[0].name, "Path 1");
    }

    // ── Cards ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_card_success() {
        use crate::dto::CardContent;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{}", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": mock_id(), "version": 1,
                "face": {"text": "Q?"}, "back": {"text": "A."}
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri()).get_card(mock_id()).await.unwrap();
        assert_eq!(result.face.text, "Q?");
        let _: CardContent = result.back; // type check
    }

    #[tokio::test]
    async fn test_get_card_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{}", mock_id())))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({"error": "card not found"})),
            )
            .mount(&server)
            .await;

        let err = client(&server.uri()).get_card(mock_id()).await.unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_list_cards_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{}/cards", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "version": 1, "face": {"text": "Q"}, "back": {"text": "A"}}],
                "cursor": null
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_cards(mock_id(), None, None)
            .await
            .unwrap();
        assert_eq!(result.data.len(), 1);
    }

    #[tokio::test]
    async fn test_update_card_success() {
        use crate::dto::{CardContent, UpdateCardRequest};
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{}", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": mock_id(), "version": 2,
                "face": {"text": "Updated"}, "back": {"text": "A."}
            })))
            .mount(&server)
            .await;

        let req = UpdateCardRequest {
            catalog_ids: vec![mock_id()],
            order_number: 1,
            face: Some(CardContent::plain("Updated")),
            back: None,
            version: 1,
        };
        let result = client(&server.uri())
            .update_card(mock_id(), &req)
            .await
            .unwrap();
        assert_eq!(result.version, 2);
    }

    #[tokio::test]
    async fn test_update_card_conflict() {
        use crate::dto::{CardContent, UpdateCardRequest};
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{}", mock_id())))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let req = UpdateCardRequest {
            catalog_ids: vec![mock_id()],
            order_number: 1,
            face: Some(CardContent::plain("X")),
            back: None,
            version: 1,
        };
        let err = client(&server.uri())
            .update_card(mock_id(), &req)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Conflict(_)));
    }

    #[tokio::test]
    async fn test_create_catalog_with_cards_success() {
        use crate::dto::{CardContent, CardInput, CreateCatalogWithCardsApiRequest};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/with-cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "catalog": {"id": mock_id(), "name": "Batch", "version": 1},
                "cardsCreated": 2
            })))
            .mount(&server)
            .await;

        let req = CreateCatalogWithCardsApiRequest {
            name: "Batch".to_string(),
            description: None,
            image_id: None,
            tags: None,
            visibility: None,
            cards: vec![
                CardInput {
                    face: CardContent::plain("Q1"),
                    back: CardContent::plain("A1"),
                },
                CardInput {
                    face: CardContent::plain("Q2"),
                    back: CardContent::plain("A2"),
                },
            ],
        };
        let result = client(&server.uri())
            .create_catalog_with_cards(&req)
            .await
            .unwrap();
        assert_eq!(result.cards_created, 2);
        assert_eq!(result.catalog.name, "Batch");
    }

    // ── Learning ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_due_cards_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning/cards"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "face": {"text": "Q"}, "back": {"text": "A"}}],
                "cursor": null,
                "total_count": 1
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .get_due_cards(None, None)
            .await
            .unwrap();
        assert_eq!(result.data.len(), 1);
        assert_eq!(result.total_count, 1);
    }

    #[tokio::test]
    async fn test_get_all_learning_cards_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning/cards/all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "cursor": null,
                "total_count": 0
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .get_all_learning_cards(None, None)
            .await
            .unwrap();
        assert!(result.data.is_empty());
    }

    #[tokio::test]
    async fn test_add_card_to_learning_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/learning/cards/{}", mock_id())))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"status": "added"})))
            .mount(&server)
            .await;

        client(&server.uri())
            .add_card_to_learning(mock_id())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_learning_cards_total_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning/cards/total"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(100)))
            .mount(&server)
            .await;

        let total = client(&server.uri()).learning_cards_total().await.unwrap();
        assert_eq!(total, 100);
    }

    // ── Media ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_media_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "media_type": "image", "fileName": "photo.jpg", "size": 1024}],
                "cursor": null
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri()).list_media(None, None).await.unwrap();
        assert_eq!(result.data.len(), 1);
        assert_eq!(result.data[0].media_type.as_deref(), Some("image"));
    }

    #[tokio::test]
    async fn test_list_media_with_type_filter() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .and(query_param("media_type", "audio"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "cursor": null
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_media(Some("audio"), None)
            .await
            .unwrap();
        assert!(result.data.is_empty());
    }

    // ── Subscription ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_subscription_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me/subscription"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"plan_id": "pro"})))
            .mount(&server)
            .await;

        let result = client(&server.uri()).get_subscription().await.unwrap();
        assert_eq!(result.plan_id.as_deref(), Some("pro"));
    }

    #[tokio::test]
    async fn test_get_usage_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me/usage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "resources": [{"resource_type": "cards_total", "used": 42, "limit": 500}]
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri()).get_usage().await.unwrap();
        let resources = result.resources.unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].used, 42);
    }

    #[tokio::test]
    async fn test_get_usage_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me/usage"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let err = client(&server.uri()).get_usage().await.unwrap_err();
        assert!(matches!(err, ApiError::Unauthorized));
    }

    // ── Paid AI ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_generate_tts_for_cards_success() {
        use crate::dto::CardAiRequest;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/tts"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({
                "updated": 1,
                "card_ids": [mock_id()]
            })))
            .mount(&server)
            .await;

        let req = CardAiRequest {
            catalog_id: Some(mock_id()),
            card_id: None,
            lang: "es".to_string(),
        };
        let result = client(&server.uri())
            .generate_tts_for_cards(&req)
            .await
            .unwrap();
        assert_eq!(result.updated, 1);
        assert_eq!(result.card_ids, vec![mock_id()]);
    }

    #[tokio::test]
    async fn test_generate_tts_for_cards_quota_exceeded() {
        use crate::dto::CardAiRequest;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/tts"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "error": "quota_exceeded",
                "resource_type": "tts_generations",
                "used": 10,
                "limit": 10
            })))
            .mount(&server)
            .await;

        let req = CardAiRequest {
            catalog_id: Some(mock_id()),
            card_id: None,
            lang: "es".to_string(),
        };
        let err = client(&server.uri())
            .generate_tts_for_cards(&req)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::QuotaExceeded { .. }));
    }

    #[tokio::test]
    async fn test_translate_cards_success() {
        use crate::dto::CardAiRequest;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/translate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "updated": 3,
                "card_ids": []
            })))
            .mount(&server)
            .await;

        let req = CardAiRequest {
            catalog_id: None,
            card_id: Some(mock_id()),
            lang: "uk".to_string(),
        };
        let result = client(&server.uri()).translate_cards(&req).await.unwrap();
        assert_eq!(result.updated, 3);
    }

    #[tokio::test]
    async fn test_generate_dictionary_for_cards_success() {
        use crate::dto::CardAiRequest;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/dictionary"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "updated": 0,
                "card_ids": []
            })))
            .mount(&server)
            .await;

        let req = CardAiRequest {
            catalog_id: Some(mock_id()),
            card_id: None,
            lang: "de".to_string(),
        };
        let result = client(&server.uri())
            .generate_dictionary_for_cards(&req)
            .await
            .unwrap();
        assert_eq!(result.updated, 0);
    }

    #[tokio::test]
    async fn test_ai_agent_chat_success() {
        use crate::dto::AiChatRequest;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/ai-agent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sessionId": mock_id(),
                "text": "Here's your deck idea..."
            })))
            .mount(&server)
            .await;

        let req = AiChatRequest {
            session_id: None,
            message: "Make me a Spanish restaurant deck".to_string(),
        };
        let result = client(&server.uri()).ai_agent_chat(&req).await.unwrap();
        assert_eq!(result.session_id, mock_id());
        assert_eq!(result.text, "Here's your deck idea...");
    }

    #[tokio::test]
    async fn test_ai_agent_chat_unauthorized() {
        use crate::dto::AiChatRequest;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/ai-agent"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let req = AiChatRequest {
            session_id: None,
            message: "hi".to_string(),
        };
        let err = client(&server.uri()).ai_agent_chat(&req).await.unwrap_err();
        assert!(matches!(err, ApiError::Unauthorized));
    }

    #[tokio::test]
    async fn test_translate_batch_import_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/catalogs/translate-batch-import/{}", "es")))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": mock_id(),
                "name": "Imported",
                "version": 1
            })))
            .mount(&server)
            .await;

        let metadata = CreateCatalogRequest {
            name: "Imported".to_string(),
            description: None,
            tags: None,
            visibility: None,
        };
        let result = client(&server.uri())
            .translate_batch_import("es", &metadata, "import.json", b"[]".to_vec())
            .await
            .unwrap();
        assert_eq!(result.name, "Imported");
    }

    #[tokio::test]
    async fn test_translate_batch_import_bad_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/translate-batch-import/es"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(json!({"error": "Content file must be provided"})),
            )
            .mount(&server)
            .await;

        let metadata = CreateCatalogRequest {
            name: "Imported".to_string(),
            description: None,
            tags: None,
            visibility: None,
        };
        let err = client(&server.uri())
            .translate_batch_import("es", &metadata, "import.json", vec![])
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[tokio::test]
    async fn test_upload_media_success() {
        let server = MockServer::start().await;
        let id = mock_id();
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": { "ids": [id] }
            })))
            .mount(&server)
            .await;

        let media_id = client(&server.uri())
            .upload_media(b"fake audio bytes".to_vec(), "card1_face.mp3", "audio/mpeg")
            .await
            .unwrap();
        assert_eq!(media_id, id);
    }

    #[tokio::test]
    async fn test_upload_media_bad_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(
                ResponseTemplate::new(400).set_body_json(json!({"error": "File too large"})),
            )
            .mount(&server)
            .await;

        let err = client(&server.uri())
            .upload_media(vec![0u8; 10], "big.mp3", "audio/mpeg")
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[tokio::test]
    async fn test_upload_media_empty_ids_returns_internal_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": { "ids": [] }
            })))
            .mount(&server)
            .await;

        let err = client(&server.uri())
            .upload_media(b"x".to_vec(), "f.png", "image/png")
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Internal));
    }
}
