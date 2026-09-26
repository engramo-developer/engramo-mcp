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
use crate::error::{ApiError, read_bounded};

/// True for Unicode formatting characters that have no visible glyph of their
/// own but can still change how a filename *displays* — e.g. RIGHT-TO-LEFT
/// OVERRIDE can make a name ending `cod.exe` render as `exe.doc`. Neither
/// `char::is_control()` nor reqwest's header escaping catches these, since
/// they aren't CR/LF or C0 control bytes.
fn is_unicode_format_char(c: char) -> bool {
    matches!(c,
        '\u{061C}' // ARABIC LETTER MARK
        | '\u{200B}'..='\u{200F}' // ZERO WIDTH SPACE .. RIGHT-TO-LEFT MARK
        | '\u{202A}'..='\u{202E}' // bidi embedding/override controls (incl. RTLO)
        | '\u{2060}'..='\u{2069}' // WORD JOINER .. bidi isolates
        | '\u{FEFF}' // BOM / ZERO WIDTH NO-BREAK SPACE
    )
}

/// The per-page cap for every list endpoint. The server caps it, and `page_params` clamps
/// `limit` to `1..=MAX_PAGE_LIMIT` before sending.
pub(crate) const MAX_PAGE_LIMIT: i64 = 50;
/// Real cursors, search queries and media-type filters are short; anything longer is a
/// malformed or abusive value that would only produce an oversized upstream URL (and a noisy
/// 414/431 error log).
const MAX_QUERY_PARAM_LEN: usize = 512;

/// Validates one free-form, LLM-supplied query value and returns it as a query pair. Rejects
/// an over-long value before any request is sent, so there is a single limit for `cursor`,
/// `q` and `media_type`.
fn bounded_param(name: &'static str, value: &str) -> Result<(&'static str, String), ApiError> {
    if value.len() > MAX_QUERY_PARAM_LEN {
        let hint = if name == "cursor" {
            "; pass back the exact value from the previous response"
        } else {
            ""
        };
        return Err(ApiError::BadRequest(format!(
            "{name} is too long (max {MAX_QUERY_PARAM_LEN} bytes){hint}"
        )));
    }
    Ok((name, value.to_string()))
}

/// Builds the `limit`/`cursor` query parameters shared by every paginated list endpoint. The
/// values come from the calling LLM, so `limit` is clamped to `1..=MAX_PAGE_LIMIT` and an
/// over-long `cursor` is rejected before any request is sent.
fn page_params(
    limit: Option<i64>,
    cursor: Option<&str>,
) -> Result<Vec<(&'static str, String)>, ApiError> {
    let mut params = Vec::new();
    if let Some(l) = limit {
        params.push(("limit", l.clamp(1, MAX_PAGE_LIMIT).to_string()));
    }
    if let Some(c) = cursor {
        params.push(bounded_param("cursor", c)?);
    }
    Ok(params)
}

/// Upper bound on a 2xx response body read into memory. A misbehaving upstream or proxy must
/// not be able to make one session allocate without bound (the 30 s timeout limits time, not
/// size).
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Typed HTTP client for the Engramo REST API.
/// Every request automatically attaches the user's API token via `X-Api-Key`.
/// All methods return `Result<T, ApiError>` — callers convert errors to `CallToolResult`
/// with `is_error: true` (never propagate as raw `Err()`).
#[derive(Clone)]
pub struct EngramoClient {
    http: Client,
    base_url: String,
    api_token: String,
}

/// Connect timeout for requests to the Engramo REST API. Kept shorter than the overall
/// request timeout so a dead/unreachable host fails fast instead of tying up the
/// connection budget.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Overall per-request timeout (connect + send + receive). Without this, a slow or
/// hung upstream stalls the tool call indefinitely — in `http` mode that pins the
/// session task and its connection until the client gives up.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A `reqwest::Client` built by [`EngramoClient::build_http_client`] — the only way to
/// construct one outside this module, so [`EngramoClient::with_http`] can never be handed a
/// client that's missing `redirect::Policy::none()`. Without this wrapper, the no-redirect
/// guarantee for the `X-Api-Key` header (see `build_http_client`'s doc comment) would only be
/// kept by convention at each call site, and a future refactor could quietly pass
/// `reqwest::Client::new()` (which follows redirects and forwards the header unchanged)
/// instead.
#[derive(Clone)]
pub struct HardenedClient(Client);

impl EngramoClient {
    /// Builds the `reqwest::Client` every `EngramoClient` — stdio's own instance
    /// ([`Self::new`]) and `http` mode's shared instance (`main.rs::run_http`) — is built
    /// from. Hardcodes `redirect::Policy::none()`: reqwest's default policy follows up to 10
    /// redirects and only strips `Authorization`/`Cookie`/`Proxy-Authorization`/
    /// `WWW-Authenticate` on a cross-host redirect — the custom `X-Api-Key` header used here
    /// is **not** stripped and would be forwarded verbatim to whatever host a 3xx `Location`
    /// names (e.g. a misconfigured proxy/CDN, or an open redirect on `ENGRAMO_API_URL`). One
    /// function so both call sites can't drift apart — mirrors
    /// `tts::gemini::build_http_client`'s reasoning for the Gemini key. Returns a
    /// [`HardenedClient`], not a plain `Client`, so [`Self::with_http`] can't be handed a
    /// client built some other way.
    pub fn build_http_client() -> HardenedClient {
        HardenedClient(
            Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client with static, well-formed config must build"),
        )
    }

    /// Builds an `EngramoClient` with its own dedicated `reqwest::Client`. Prefer
    /// [`Self::with_http`] when serving multiple sessions from one process (`http`
    /// mode) so they share a single connection pool instead of each paying for its
    /// own TLS handshakes.
    pub fn new(base_url: impl Into<String>, api_token: impl Into<String>) -> Self {
        Self::with_http(Self::build_http_client(), base_url, api_token)
    }

    /// Builds an `EngramoClient` from a pre-built, shared, hardened `reqwest::Client` — used by
    /// `http` mode so every session's requests flow through one connection pool. Takes a
    /// [`HardenedClient`] (only buildable via [`Self::build_http_client`]) rather than a plain
    /// `reqwest::Client`, so a caller can't accidentally pass one without the no-redirect
    /// policy the `X-Api-Key` header depends on.
    pub fn with_http(
        http: HardenedClient,
        base_url: impl Into<String>,
        api_token: impl Into<String>,
    ) -> Self {
        Self {
            http: http.0,
            base_url: base_url.into(),
            api_token: api_token.into(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Makes an LLM-supplied filename safe to put in a multipart
    /// `Content-Disposition` header. reqwest escapes `"` and `\` but still emits a
    /// raw CR/LF byte for a newline in the name (`\\r`/`\\n` quoted-pairs), which a
    /// lenient upstream parser can read as the start of another header. Drop
    /// control characters, bidi/format characters, and path separators, and cap
    /// the length.
    fn sanitize_filename(name: &str) -> String {
        const MAX_FILENAME_LEN: usize = 200;
        let cleaned: String = name
            .chars()
            .filter(|c| {
                !c.is_control() && !is_unicode_format_char(*c) && !matches!(c, '/' | '\\' | '"')
            })
            .take(MAX_FILENAME_LEN)
            .collect();
        let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
        if cleaned.is_empty() {
            "upload".to_string()
        } else {
            cleaned
        }
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

    /// Builds the size-cap error, logging it at ERROR: an oversized body is an abnormal
    /// upstream/proxy condition that should reach Error Reporting.
    fn response_too_large() -> ApiError {
        tracing::error!(
            limit = MAX_RESPONSE_BYTES,
            "Engramo API response body exceeds size limit"
        );
        ApiError::ResponseTooLarge(MAX_RESPONSE_BYTES)
    }

    async fn deserialize<T: serde::de::DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T, ApiError> {
        if resp.status().is_success() {
            // Read the body separately from parsing it: a transport failure while reading
            // (`?` via `From<reqwest::Error>`) is still a `Network` error, while a failure
            // to deserialize an otherwise-successful body is a `Decode` error — see
            // `ApiError::decode` for why these need different, non-vague messages (#35).
            if resp
                .content_length()
                .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
            {
                return Err(Self::response_too_large());
            }
            let (buf, truncated) = read_bounded(resp, MAX_RESPONSE_BYTES).await?;
            if truncated {
                return Err(Self::response_too_large());
            }
            serde_json::from_slice::<T>(&buf).map_err(ApiError::decode)
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
        let params = page_params(limit, cursor)?;
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
        let params = page_params(limit, cursor)?;
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

    /// Raw JSON `GET /cards/{id}`, for the local-TTS attach flow (`tools/tts.rs`). Bypasses
    /// `CardDto` on purpose: that type only models the fields this crate otherwise needs, and
    /// `face` on the wire also carries fields we don't model (e.g. `tts`, `richText`). Since
    /// `PATCH /cards/{id}` replaces `face` wholesale, round-tripping through the typed DTO
    /// would silently drop anything we don't model — see the local-TTS rollout plan §0 R1.
    pub async fn get_card_raw(&self, card_id: Uuid) -> Result<serde_json::Value, ApiError> {
        self.get(&format!("/cards/{card_id}")).await
    }

    /// Raw JSON `PATCH /cards/{id}`, the counterpart to [`Self::get_card_raw`]. Callers build
    /// `body` from a `get_card_raw` response so unmodelled fields survive the round trip.
    pub async fn patch_card_raw(
        &self,
        card_id: Uuid,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, ApiError> {
        self.patch(&format!("/cards/{card_id}"), body).await
    }

    // ── Learning ──────────────────────────────────────────────────────────────

    pub async fn get_due_cards(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<PagedResponseWithCount<LearningCardDto>, ApiError> {
        let params = page_params(limit, cursor)?;
        self.get_with_query("/learning/cards", &params).await
    }

    pub async fn get_all_learning_cards(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<PagedResponseWithCount<LearningCardDto>, ApiError> {
        let params = page_params(limit, cursor)?;
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
        let params = page_params(limit, cursor)?;
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
        self.get_with_query("/search", &[bounded_param("q", query)?])
            .await
    }

    pub async fn search_catalogs(&self, query: &str) -> Result<Vec<CatalogDto>, ApiError> {
        self.get_with_query("/search/catalogs", &[bounded_param("q", query)?])
            .await
    }

    // ── Media ─────────────────────────────────────────────────────────────────

    pub async fn list_media(
        &self,
        media_type: Option<&str>,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<PagedResponse<MediaDto>, ApiError> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(mt) = media_type {
            params.push(bounded_param("media_type", mt)?);
        }
        params.extend(page_params(limit, cursor)?);
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
            .file_name(Self::sanitize_filename(filename))
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
    /// Uses EngrAmo's paid AI — only registered when `ENGRAMO_ENABLE_PAID_AI` is on.
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
        // `target_lang` is interpolated into the request path below, so restrict it to
        // the shape of a real BCP-47 code (ASCII letters, digits, `-`). This keeps a
        // free-form LLM-supplied value from injecting extra path segments (`/`, `..`) or
        // URL delimiters (`?`, `#`) that would silently retarget the request — same
        // defensive stance as `grade_card`'s allowlist and the UUID parsing elsewhere.
        if target_lang.is_empty()
            || !target_lang
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(ApiError::BadRequest(format!(
                "invalid target_lang '{target_lang}': expected a BCP-47 code like 'es' or 'pt-BR'"
            )));
        }
        let metadata_json = serde_json::to_string(catalog_metadata).map_err(|e| {
            ApiError::BadRequest(format!("failed to serialize catalog metadata: {e}"))
        })?;
        let content_part = reqwest::multipart::Part::bytes(content_file)
            .file_name(Self::sanitize_filename(content_file_name));
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
    use crate::dto::SearchItemType;
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mock_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn client(base_url: &str) -> EngramoClient {
        EngramoClient::new(base_url, "engramo_test_token")
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
                val, "engramo_test_token",
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
            .and(header("x-api-key", "engramo_test_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "name": "Rust", "version": 1}],
                "nextCursor": null
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
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_catalogs(Some(10), Some("abc"))
            .await
            .unwrap();
        assert!(result.data.is_empty());
    }

    // Regression test for issue #34: the API sends the cursor as `nextCursor`, not
    // `cursor` — a stale mock (or a stale DTO) makes `cursor` silently deserialize
    // to `None`, so every list tool would always report "last page" even when more
    // data exists. This asserts the client actually surfaces a non-null cursor.
    #[tokio::test]
    async fn test_list_catalogs_maps_next_cursor_to_cursor() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "nextCursor": "abc"
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_catalogs(None, None)
            .await
            .unwrap();
        assert_eq!(result.cursor, Some("abc".to_string()));
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
                {"id": mock_id(), "itemType": "catalog", "title": "Rust catalog"}
            ])))
            .mount(&server)
            .await;

        let results = client(&server.uri()).search_global("rust").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title.as_deref(), Some("Rust catalog"));
        assert_eq!(results[0].item_type, Some(SearchItemType::Catalog));
    }

    #[tokio::test]
    async fn test_search_global_maps_full_camel_case_shape() {
        let server = MockServer::start().await;
        let card_id = mock_id();
        let catalog_id = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "ownership"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {
                    "itemType": "card",
                    "id": card_id,
                    "title": "What is ownership?",
                    "subtitle": "Rust Basics",
                    "parentId": catalog_id,
                    "rank": 0.5,
                    "imageId": null,
                    "imageUrl": null
                }
            ])))
            .mount(&server)
            .await;

        let results = client(&server.uri())
            .search_global("ownership")
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].item_type, Some(SearchItemType::Card));
        assert_eq!(results[0].id, card_id);
        assert_eq!(results[0].title.as_deref(), Some("What is ownership?"));
        assert_eq!(results[0].subtitle.as_deref(), Some("Rust Basics"));
        assert_eq!(results[0].parent_id, Some(catalog_id));
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
                "nextCursor": null
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

    /// Regression test: `list_learning_paths` has the same `nextCursor` bug as #34 (see
    /// `test_list_catalogs_maps_next_cursor_to_cursor`) — confirmed by a reporter calling
    /// `list_learning_paths({limit:1})` on an account with 3 paths and always getting
    /// `cursor: null`.
    #[tokio::test]
    async fn test_list_learning_paths_maps_next_cursor_to_cursor() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning-paths"))
            .and(query_param("limit", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "name": "Path 1", "version": 1}],
                "nextCursor": "abc"
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_learning_paths(Some(1), None)
            .await
            .unwrap();
        assert_eq!(result.cursor, Some("abc".to_string()));
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
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_cards(mock_id(), None, None)
            .await
            .unwrap();
        assert_eq!(result.data.len(), 1);
    }

    /// Regression test: `list_cards` has the same `nextCursor` bug as #34 (see
    /// `test_list_catalogs_maps_next_cursor_to_cursor`). The real API also sends a
    /// `permissions` object alongside `data`/`nextCursor` — assert it's harmlessly
    /// ignored and the cursor still surfaces.
    #[tokio::test]
    async fn test_list_cards_maps_next_cursor_to_cursor() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{}/cards", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": mock_id(),
                    "version": 1,
                    "face": {"text": "Q?"},
                    "back": {"text": "A."},
                    "orderNumber": 1
                }],
                "nextCursor": "abc",
                "permissions": {"canEdit": true, "canDelete": true, "isOwner": true}
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_cards(mock_id(), None, None)
            .await
            .unwrap();
        assert_eq!(result.cursor, Some("abc".to_string()));
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
    async fn test_get_card_raw_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{}", mock_id())))
            .and(header("x-api-key", "engramo_test_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": mock_id(),
                "version": 3,
                "orderNumber": 5,
                "face": {"text": "Q?", "tts": {"voice": "Puck"}},
                "back": {"text": "A."},
                "catalogs": [{"id": mock_id()}]
            })))
            .mount(&server)
            .await;

        let raw = client(&server.uri()).get_card_raw(mock_id()).await.unwrap();
        assert_eq!(raw["version"], json!(3));
        assert_eq!(raw["face"]["tts"]["voice"], json!("Puck"));
        assert_auth_header(&server).await;
    }

    #[tokio::test]
    async fn test_get_card_raw_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{}", mock_id())))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({"error": "card not found"})),
            )
            .mount(&server)
            .await;

        let err = client(&server.uri())
            .get_card_raw(mock_id())
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_patch_card_raw_success() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{}", mock_id())))
            .and(header("x-api-key", "engramo_test_token"))
            .and(wiremock::matchers::body_json(json!({
                "catalogIds": [mock_id()],
                "orderNumber": 5,
                "version": 3,
                "face": {"text": "Q?", "audioId": mock_id()}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": mock_id(),
                "version": 4,
                "face": {"text": "Q?", "audioId": mock_id()},
                "back": {"text": "A."}
            })))
            .mount(&server)
            .await;

        let body = json!({
            "catalogIds": [mock_id()],
            "orderNumber": 5,
            "version": 3,
            "face": {"text": "Q?", "audioId": mock_id()}
        });
        let updated = client(&server.uri())
            .patch_card_raw(mock_id(), &body)
            .await
            .unwrap();
        assert_eq!(updated["version"], json!(4));
        assert_auth_header(&server).await;
    }

    #[tokio::test]
    async fn test_patch_card_raw_conflict() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{}", mock_id())))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let err = client(&server.uri())
            .patch_card_raw(mock_id(), &json!({}))
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
                "nextCursor": null,
                "total": 1
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
                "nextCursor": null,
                "total": 0
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .get_all_learning_cards(None, None)
            .await
            .unwrap();
        assert!(result.data.is_empty());
    }

    /// Regression test for issue #35: a realistic `GET /learning/cards` body, including the
    /// full set of `LearningCardDto` fields this crate doesn't model (`version`,
    /// `orderNumber`, `status`, `mode`, `shadowingTurnIndex`, `speakingEnabled`) plus the
    /// real `nextCursor`/`total` field names, must decode successfully — these extra fields
    /// are ignored, not rejected.
    #[tokio::test]
    async fn test_get_due_cards_realistic_full_body_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning/cards"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": mock_id(),
                    "version": 3,
                    "face": {"text": "Q"},
                    "back": {"text": "A"},
                    "orderNumber": 1,
                    "status": "learning",
                    "nextReview": "2026-09-26T00:00:00Z",
                    "lastReviewed": "2026-09-20T00:00:00Z",
                    "mode": "flashcard",
                    "shadowingTurnIndex": 0,
                    "speakingEnabled": false
                }],
                "nextCursor": "page2",
                "total": 1
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .get_due_cards(None, None)
            .await
            .unwrap();
        assert_eq!(result.data.len(), 1);
        assert_eq!(
            result.data[0].next_review.as_deref(),
            Some("2026-09-26T00:00:00Z")
        );
        assert_eq!(result.cursor, Some("page2".to_string()));
        assert_eq!(result.total_count, 1);
    }

    /// Same as above but for `/learning/cards/all` — the `FullLearningCardDto` shape also
    /// carries `catalogs` and allows a null `nextReview`.
    #[tokio::test]
    async fn test_get_all_learning_cards_realistic_full_body_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning/cards/all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": mock_id(),
                    "version": 1,
                    "face": {"text": "Q"},
                    "back": {"text": "A"},
                    "orderNumber": 1,
                    "status": "new",
                    "nextReview": null,
                    "lastReviewed": null,
                    "mode": "flashcard",
                    "shadowingTurnIndex": 0,
                    "speakingEnabled": false,
                    "catalogs": [{"id": mock_id(), "name": "Rust", "version": 1}]
                }],
                "nextCursor": null,
                "total": 1
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .get_all_learning_cards(None, None)
            .await
            .unwrap();
        assert_eq!(result.data.len(), 1);
        assert_eq!(result.data[0].next_review, None);
        assert_eq!(result.cursor, None);
        assert_eq!(result.total_count, 1);
    }

    /// Regression test for issue #35: a 200 body missing the required `total` field must
    /// surface as `ApiError::Decode` whose message names the missing field, not the vague
    /// `ApiError::Network` reqwest previously produced by dropping the serde detail.
    #[tokio::test]
    async fn test_get_due_cards_missing_total_field_yields_decode_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning/cards"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let err = client(&server.uri())
            .get_due_cards(None, None)
            .await
            .unwrap_err();
        match err {
            ApiError::Decode(msg) => assert!(msg.contains("total"), "{msg}"),
            other => panic!("expected Decode, got {other:?}"),
        }
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
                "data": [{"id": mock_id(), "media_type": "image", "name": "photo.jpg", "length": 1024, "content_type": "image/jpeg"}],
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_media(None, None, None)
            .await
            .unwrap();
        assert_eq!(result.data.len(), 1);
        assert_eq!(result.data[0].media_type.as_deref(), Some("image"));
        assert_eq!(result.data[0].name.as_deref(), Some("photo.jpg"));
        assert_eq!(result.data[0].length, Some(1024));
        assert_eq!(result.data[0].content_type.as_deref(), Some("image/jpeg"));
    }

    #[tokio::test]
    async fn test_list_media_with_type_filter() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .and(query_param("media_type", "audio"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_media(Some("audio"), None, None)
            .await
            .unwrap();
        assert!(result.data.is_empty());
    }

    /// Regression test for #34: `list_media` had no cursor param at all, even though the
    /// backend `/media` endpoint (`MediaFilter`) accepts one. Assert it's forwarded as a
    /// query param and that the response's `nextCursor` reaches the caller.
    #[tokio::test]
    async fn test_list_media_forwards_cursor_and_surfaces_next_cursor() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .and(query_param("cursor", "abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "nextCursor": "def"
            })))
            .mount(&server)
            .await;

        let result = client(&server.uri())
            .list_media(None, None, Some("abc"))
            .await
            .unwrap();
        assert_eq!(result.cursor.as_deref(), Some("def"));
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

    #[test]
    fn test_sanitize_filename_strips_crlf_and_path_separators() {
        // A CR/LF in the name reaches the multipart Content-Disposition header;
        // reqwest only backslash-escapes it, leaving the raw byte in place.
        assert_eq!(
            EngramoClient::sanitize_filename("a.mp3\r\nContent-Type: text/html"),
            "a.mp3Content-Type: texthtml"
        );
        assert_eq!(
            EngramoClient::sanitize_filename("../../etc/passwd"),
            "etcpasswd"
        );
        assert_eq!(
            EngramoClient::sanitize_filename("say\"hi\".mp3"),
            "sayhi.mp3"
        );
    }

    #[test]
    fn test_sanitize_filename_strips_bidi_override_characters() {
        // U+202E (RTLO) has no glyph of its own but flips render order for
        // everything after it — a name ending "cod\u{202E}exe.mp3" would
        // display as "cod.mp3exe" in a lenient file picker. Not caught by
        // `is_control()` since it isn't a C0 control byte.
        assert_eq!(
            EngramoClient::sanitize_filename("safe\u{202E}exe.mp3"),
            "safeexe.mp3"
        );
        // Zero-width space / BOM: invisible but can hide extra characters.
        assert_eq!(
            EngramoClient::sanitize_filename("a\u{200B}b\u{FEFF}.mp3"),
            "ab.mp3"
        );
    }

    #[test]
    fn test_sanitize_filename_keeps_ordinary_names() {
        assert_eq!(
            EngramoClient::sanitize_filename("card1_face.mp3"),
            "card1_face.mp3"
        );
        assert_eq!(
            EngramoClient::sanitize_filename("ñandú — foto.png"),
            "ñandú — foto.png"
        );
    }

    #[test]
    fn test_sanitize_filename_falls_back_when_nothing_survives() {
        assert_eq!(EngramoClient::sanitize_filename(""), "upload");
        assert_eq!(EngramoClient::sanitize_filename("///"), "upload");
        assert_eq!(EngramoClient::sanitize_filename("\r\n"), "upload");
    }

    #[test]
    fn test_sanitize_filename_caps_length() {
        let long = "a".repeat(5000);
        assert_eq!(EngramoClient::sanitize_filename(&long).chars().count(), 200);
    }

    #[tokio::test]
    async fn test_translate_batch_import_rejects_path_injecting_lang() {
        let server = MockServer::start().await;
        // No mock mounted: a value that passed validation would hit the network and
        // fail with a connection error. Rejecting before any request is sent proves
        // the guard short-circuits path-injecting input.
        let metadata = CreateCatalogRequest {
            name: "Imported".to_string(),
            description: None,
            tags: None,
            visibility: None,
        };
        for bad in ["es/../../me/subscription", "es?x=1", "", "es#frag", "en US"] {
            let err = client(&server.uri())
                .translate_batch_import(bad, &metadata, "import.json", b"[]".to_vec())
                .await
                .unwrap_err();
            assert!(
                matches!(err, ApiError::BadRequest(_)),
                "expected BadRequest for target_lang {bad:?}, got {err:?}"
            );
        }
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
    async fn test_redirect_response_is_not_followed_and_api_key_never_reaches_redirect_target() {
        let primary = MockServer::start().await;
        let redirect_target = MockServer::start().await;

        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/catalogs", redirect_target.uri())),
            )
            .mount(&primary)
            .await;
        // The redirect target must never be contacted — proves the token doesn't follow the 3xx.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [], "nextCursor": null
            })))
            .mount(&redirect_target)
            .await;

        let err = client(&primary.uri())
            .list_catalogs(None, None)
            .await
            .unwrap_err();
        match err {
            ApiError::BadRequest(msg) => {
                assert!(msg.contains("redirect"), "{msg}");
                assert!(msg.contains("ENGRAMO_API_URL"), "{msg}");
            }
            other => panic!("expected redirect error, got {other:?}"),
        }

        let redirect_target_requests = redirect_target.received_requests().await.unwrap();
        assert_eq!(
            redirect_target_requests.len(),
            0,
            "the redirect target must never receive a request — X-Api-Key must not follow a \
            3xx to another host"
        );
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

    #[test]
    fn test_page_params_clamps_limit() {
        let p = page_params(Some(i64::MAX), None).unwrap();
        assert_eq!(p, vec![("limit", "50".to_string())]);
        let p = page_params(Some(-3), None).unwrap();
        assert_eq!(p, vec![("limit", "1".to_string())]);
        let p = page_params(Some(20), Some("abc")).unwrap();
        assert_eq!(
            p,
            vec![("limit", "20".to_string()), ("cursor", "abc".to_string())]
        );
    }

    #[tokio::test]
    async fn test_list_catalogs_rejects_oversized_cursor_without_sending_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let long = "a".repeat(MAX_QUERY_PARAM_LEN + 1);
        let err = client(&server.uri())
            .list_catalogs(None, Some(&long))
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)), "{err:?}");
    }

    #[tokio::test]
    async fn test_deserialize_rejects_body_over_size_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(vec![b' '; MAX_RESPONSE_BYTES + 1]),
            )
            .mount(&server)
            .await;

        let err = client(&server.uri())
            .list_catalogs(None, None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, ApiError::ResponseTooLarge(MAX_RESPONSE_BYTES)),
            "{err:?}"
        );
    }

    /// Serves one raw HTTP response (head + body bytes as given) on a loopback port, then
    /// closes the connection. Needed because wiremock always sends `Content-Length`, and the
    /// chunked / truncated-body paths can't be reached without controlling the wire bytes.
    async fn spawn_raw(response: Vec<u8>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await; // consume the request
            let _ = sock.write_all(&response).await;
            // dropping `sock` closes the connection
        });
        format!("http://{addr}")
    }

    const CHUNKED_HEAD: &str =
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n";

    #[tokio::test]
    async fn test_deserialize_rejects_chunked_body_over_size_limit() {
        // No Content-Length, so only the per-chunk cap can catch this.
        const MIB: usize = 1024 * 1024;
        let mut resp = CHUNKED_HEAD.as_bytes().to_vec();
        for _ in 0..(MAX_RESPONSE_BYTES / MIB + 1) {
            resp.extend_from_slice(format!("{MIB:x}\r\n").as_bytes());
            resp.extend(std::iter::repeat_n(b' ', MIB));
            resp.extend_from_slice(b"\r\n");
        }
        resp.extend_from_slice(b"0\r\n\r\n");
        let uri = spawn_raw(resp).await;

        let err = client(&uri).list_catalogs(None, None).await.unwrap_err();
        assert!(
            matches!(err, ApiError::ResponseTooLarge(MAX_RESPONSE_BYTES)),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn test_deserialize_truncated_chunked_body_is_network_not_decode() {
        // One partial chunk, then the socket closes without the terminating chunk.
        let mut resp = CHUNKED_HEAD.as_bytes().to_vec();
        resp.extend_from_slice(b"5\r\n{\"dat");
        let uri = spawn_raw(resp).await;

        let err = client(&uri).list_catalogs(None, None).await.unwrap_err();
        assert!(matches!(err, ApiError::Network(_)), "{err:?}");
    }

    #[tokio::test]
    async fn test_network_error_display_omits_url_and_query() {
        // Port 1 (tcpmux) has no listener, so the connection is refused deterministically. A
        // dropped MockServer's port would race with other tests' servers reusing it.
        let uri = "http://127.0.0.1:1".to_string();

        let err = client(&uri)
            .search_global("SECRET_QUERY")
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Network(_)), "{err:?}");
        let msg = err.to_string();
        assert!(!msg.contains("SECRET_QUERY"), "{msg}");
        assert!(!msg.contains(&uri), "{msg}");
        assert!(!msg.contains("127.0.0.1"), "{msg}");
    }

    #[tokio::test]
    async fn test_deserialize_non_json_200_yields_syntax_decode_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>SECRET</html>"))
            .mount(&server)
            .await;

        let err = client(&server.uri())
            .list_catalogs(None, None)
            .await
            .unwrap_err();
        match err {
            ApiError::Decode(msg) => {
                assert!(msg.contains("Syntax error at line 1"), "{msg}");
                assert!(!msg.contains("SECRET"), "{msg}");
                assert!(!msg.contains("update engramo-mcp"), "{msg}");
            }
            other => panic!("expected Decode, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_list_media_rejects_oversized_cursor_without_sending_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let long = "a".repeat(MAX_QUERY_PARAM_LEN + 1);
        let err = client(&server.uri())
            .list_media(Some("image"), None, Some(&long))
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)), "{err:?}");
    }

    #[tokio::test]
    async fn test_free_form_query_values_over_limit_are_rejected_without_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let long = "a".repeat(MAX_QUERY_PARAM_LEN + 1);
        let c = client(&server.uri());
        let errs = [
            c.search_global(&long).await.unwrap_err(),
            c.search_catalogs(&long).await.unwrap_err(),
            c.list_media(Some(&long), None, None).await.unwrap_err(),
        ];
        for err in errs {
            assert!(
                matches!(&err, ApiError::BadRequest(m) if m.contains("too long")),
                "{err:?}"
            );
        }
    }

    #[test]
    fn test_page_params_boundaries() {
        assert_eq!(
            page_params(Some(0), None).unwrap(),
            vec![("limit", "1".to_string())]
        );
        assert_eq!(
            page_params(Some(50), None).unwrap(),
            vec![("limit", "50".to_string())]
        );
        let exact = "a".repeat(MAX_QUERY_PARAM_LEN);
        assert_eq!(
            page_params(None, Some(&exact)).unwrap(),
            vec![("cursor", exact.clone())]
        );
        assert!(page_params(None, None).unwrap().is_empty());
        assert!(page_params(None, Some(&format!("{exact}a"))).is_err());
    }
}
