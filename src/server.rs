use std::sync::Arc;

use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, GetPromptRequestParams,
        GetPromptResponse, Implementation, ListPromptsResult, ListResourcesResult, ListToolsResult,
        PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
        ServerCapabilities, ServerInfo,
    },
    tool, tool_router,
};
use tracing::{debug, warn};

use base64::Engine;

use crate::client::EngramoClient;
use crate::dto::{
    CardInput, CreateCardRequest, CreateCatalogWithCardsApiRequest, CreateLearningPathRequest,
    UpdateCardRequest, UpdateCatalogRequest, UploadMediaResult,
};
use crate::tools::cards::{DeleteCardParams, GetCardParams, ListCardsParams, UpdateCardParams};
use crate::tools::catalogs::{
    DeleteCatalogParams, GetCatalogParams, ListCatalogsParams, UpdateCatalogParams, err_result,
    ok_json, ok_text, parse_uuid,
};
use crate::tools::generate::{
    GenerateCardParams, GenerateCardsParams, GenerateCatalogWithCardsParams,
};
use crate::tools::learning::{AddCardToLearningParams, AddCatalogToLearningParams, DueCardsParams};
use crate::tools::learning_paths::{
    ActivateLearningPathParams, CreateLearningPathParams, GetLearningPathParams,
    ListLearningPathsParams,
};
use crate::tools::media::{
    ListMediaParams, MAX_UPLOAD_BASE64_LEN, MAX_UPLOAD_BYTES, UploadMediaParams,
};
use crate::tools::search::SearchParams;
use crate::tts::TtsEngine;
use uuid::Uuid;

pub struct EngramoMcpServer {
    pub(crate) client: EngramoClient,
    /// Local, bring-your-own-key TTS engine (`tools/tts.rs`). `None` unless `with_tts` was
    /// called — which only `run_stdio` (`main.rs`) ever does. `http` mode's session factory
    /// (`build_session_server`, below) never calls it, so a session built there can never
    /// reach a TTS engine or the user's Gemini key, regardless of what's in the process
    /// environment. See `tts` module doc comment for the full structural argument.
    pub(crate) tts: Option<Arc<dyn TtsEngine>>,
    tool_router: ToolRouter<Self>,
}

impl EngramoMcpServer {
    /// `paid_ai_enabled` gates whether the paid-AI tool router (TTS, translation,
    /// dictionary, AI-agent chat — see `tools/ai.rs`) is registered at all. When
    /// `false`, those tools are entirely absent from `tools/list` — the public,
    /// bring-your-own-AI deployment default (`ENGRAMO_ENABLE_PAID_AI` unset).
    ///
    /// Local TTS (`tools/tts.rs`) is never registered here — call [`Self::with_tts`]
    /// afterwards to add it. This signature is unchanged on purpose: `http` mode's session
    /// factory calls this (via [`build_session_server`]) and must have no way to end up with
    /// a TTS engine.
    pub fn new(client: EngramoClient, paid_ai_enabled: bool) -> Self {
        let mut tool_router = Self::server_info_tools_router()
            + Self::catalog_tools_router()
            + Self::card_tools_router()
            + Self::learning_tools_router()
            + Self::learning_path_tools_router()
            + Self::search_tools_router()
            + Self::media_tools_router()
            + Self::generate_tools_router();
        if paid_ai_enabled {
            tool_router += Self::paid_ai_tools_router();
        }
        Self {
            tool_router,
            client,
            tts: None,
        }
    }

    /// Registers the local TTS tools (`list_tts_voices`, `generate_card_audio`) and stores
    /// `engine` for their handlers to use. Only ever called from `attach_tts` (`main.rs`,
    /// reached only via `run_stdio`) — see the `tts` module doc comment for why that's a
    /// structural guarantee, not just a convention.
    pub fn with_tts(mut self, engine: Arc<dyn TtsEngine>) -> Self {
        self.tool_router += Self::local_tts_tools_router();
        self.tts = Some(engine);
        self
    }
}

/// Builds the `EngramoMcpServer` for one `http`-mode session from that session's own bearer
/// token. Extracted out of the session factory closure in `build_app` (`main.rs`) so it can be unit
/// tested directly without going through axum/rmcp plumbing (rollout plan R8). Deliberately
/// has **no** TTS parameter and never calls [`EngramoMcpServer::with_tts`] — see the `tts`
/// module doc comment. Behaviour is otherwise identical to what the closure did before.
pub fn build_session_server(
    http: &crate::client::HardenedClient,
    api_url: &str,
    token: &str,
    paid_ai_enabled: bool,
) -> EngramoMcpServer {
    let client = EngramoClient::with_http(http.clone(), api_url, token);
    EngramoMcpServer::new(client, paid_ai_enabled)
}

// ── Server info tools ─────────────────────────────────────────────────────────

#[tool_router(router = server_info_tools_router)]
impl EngramoMcpServer {
    #[tool(
        description = "Get the version of this EngrAmo MCP server binary itself (its build/release \
        version, e.g. from Cargo.toml) — NOT a catalog's or card's `version` field (an unrelated \
        per-entity optimistic-locking counter used by update_catalog/update_card). Useful for \
        diagnosing which server build is deployed or installed; makes no backend API call."
    )]
    pub async fn get_server_version(&self) -> Result<CallToolResult, ErrorData> {
        Ok(ok_json(&crate::version::server_version()))
    }
}

// ── Catalog tools ─────────────────────────────────────────────────────────────

#[tool_router(router = catalog_tools_router)]
impl EngramoMcpServer {
    #[tool(
        description = "List the user's flashcard catalogs with cursor-based pagination. Returns id, name, and card_count for each catalog. Use get_catalog to fetch full details. Returns at most 50 items per call; pass the returned `cursor` back to fetch the next page (`cursor: null` means this is the last page)."
    )]
    pub async fn list_catalogs(
        &self,
        Parameters(p): Parameters<ListCatalogsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(
            match self
                .client
                .list_catalogs(p.limit, p.cursor.as_deref())
                .await
            {
                Ok(resp) => ok_json(&resp),
                Err(e) => err_result(e),
            },
        )
    }

    #[tool(
        description = "Get full details of a single catalog by its UUID, including description and tags."
    )]
    pub async fn get_catalog(
        &self,
        Parameters(p): Parameters<GetCatalogParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match parse_uuid(&p.catalog_id) {
            Ok(id) => match self.client.get_catalog(id).await {
                Ok(catalog) => ok_json(&catalog),
                Err(e) => err_result(e),
            },
            Err(e) => err_result(e),
        })
    }

    #[tool(
        description = "Update an existing catalog's name, description, tags, or visibility. Requires the current version for optimistic locking — fetch the catalog first to get the version. If you get a Conflict error, re-fetch and retry."
    )]
    pub async fn update_catalog(
        &self,
        Parameters(p): Parameters<UpdateCatalogParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match parse_uuid(&p.catalog_id) {
            Ok(id) => {
                let req = UpdateCatalogRequest {
                    name: p.name,
                    description: p.description,
                    tags: p.tags,
                    visibility: p.visibility,
                    version: p.version,
                };
                match self.client.update_catalog(id, &req).await {
                    Ok(catalog) => ok_json(&catalog),
                    Err(e) => err_result(e),
                }
            }
            Err(e) => err_result(e),
        })
    }

    #[tool(
        description = "Delete a catalog. Catalogs with no cards are hard-deleted; catalogs with cards are archived. Quota decrements automatically."
    )]
    pub async fn delete_catalog(
        &self,
        Parameters(p): Parameters<DeleteCatalogParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match parse_uuid(&p.catalog_id) {
            Ok(id) => match self.client.delete_catalog(id).await {
                Ok(()) => ok_text("Catalog deleted successfully."),
                Err(e) => err_result(e),
            },
            Err(e) => err_result(e),
        })
    }
}

// ── Card tools ────────────────────────────────────────────────────────────────

#[tool_router(router = card_tools_router)]
impl EngramoMcpServer {
    #[tool(
        description = "List flashcards in a catalog with cursor-based pagination. Returns at most 50 items per call; pass the returned `cursor` back to fetch the next page (`cursor: null` means this is the last page)."
    )]
    pub async fn list_cards(
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
    pub async fn get_card(
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
    pub async fn update_card(
        &self,
        Parameters(mut p): Parameters<UpdateCardParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let card_id = match parse_uuid(&p.card_id) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        // Normalize richText spans so text is derived before any further processing.
        if let Some(ref mut face) = p.face {
            normalize_card_content(face);
        }
        if let Some(ref mut back) = p.back {
            normalize_card_content(back);
        }
        debug!(
            card_id = %card_id,
            updating_face = p.face.is_some(),
            updating_back = p.back.is_some(),
            "update_card: normalized"
        );
        // Preserve server-managed fields (audio_id, dictionary) the LLM cannot know about.
        // Fetch the current card and merge them into the update if not explicitly set.
        if p.face.is_some() || p.back.is_some() {
            match self.client.get_card(card_id).await {
                Ok(existing) => {
                    debug!(
                        existing_face_audio = existing.face.audio_id.is_some(),
                        existing_face_dict_entries = existing
                            .face
                            .dictionary
                            .as_ref()
                            .map(|d| d.len())
                            .unwrap_or(0),
                        existing_back_audio = existing.back.audio_id.is_some(),
                        "update_card: fetched existing card for merge"
                    );
                    if let Some(ref mut face) = p.face {
                        if face.audio_id.is_none() {
                            face.audio_id = existing.face.audio_id;
                            debug!(audio_id = ?face.audio_id, "update_card: merged face.audio_id from existing");
                        }
                        if face.dictionary.is_none() {
                            face.dictionary = existing.face.dictionary;
                            debug!(
                                dict_entries =
                                    face.dictionary.as_ref().map(|d| d.len()).unwrap_or(0),
                                "update_card: merged face.dictionary from existing"
                            );
                        }
                    }
                    if let Some(ref mut back) = p.back {
                        if back.audio_id.is_none() {
                            back.audio_id = existing.back.audio_id;
                            debug!(audio_id = ?back.audio_id, "update_card: merged back.audio_id from existing");
                        }
                        if back.dictionary.is_none() {
                            back.dictionary = existing.back.dictionary;
                        }
                    }
                }
                Err(e) => return Ok(err_result(e)),
            }
        }
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
    pub async fn delete_card(
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

// ── Learning tools ────────────────────────────────────────────────────────────

#[tool_router(router = learning_tools_router)]
impl EngramoMcpServer {
    #[tool(
        description = "Get flashcards due for review today, sorted by priority. Use this to start a study session. Returns at most 50 items per call; pass the returned `cursor` back to fetch the next page (`cursor: null` means this is the last page)."
    )]
    pub async fn get_due_cards(
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
        description = "Get all cards currently in the learning queue (due and future), with pagination. Returns at most 50 items per call; pass the returned `cursor` back to fetch the next page (`cursor: null` means this is the last page)."
    )]
    pub async fn get_all_learning_cards(
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
    pub async fn add_card_to_learning(
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
    pub async fn add_catalog_to_learning(
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

// ── Learning path tools ───────────────────────────────────────────────────────

#[tool_router(router = learning_path_tools_router)]
impl EngramoMcpServer {
    #[tool(
        description = "List all learning paths with cursor-based pagination. Returns at most 50 items per call; pass the returned `cursor` back to fetch the next page (`cursor: null` means this is the last page)."
    )]
    pub async fn list_learning_paths(
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
    pub async fn get_learning_path(
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
    pub async fn create_learning_path(
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
    pub async fn activate_learning_path(
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
    pub async fn deactivate_learning_path(
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

// ── Search tools ──────────────────────────────────────────────────────────────

#[tool_router(router = search_tools_router)]
impl EngramoMcpServer {
    #[tool(
        description = "Search across cards and catalogs (learning paths are NOT included — use \
        list_learning_paths/get_learning_path for those). Each hit has item_type ('catalog' or \
        'card'), title (the catalog's name, or the card's face text), subtitle (the catalog's \
        description, or the card's back text), and parent_id, which for a card hit is its \
        catalog's UUID (use it directly with get_catalog/list_cards). \
        If the user gives you a catalog's short ID (the ~8-character code shown in the app/URL, e.g. \
        \"A7KX9QM2\" — NOT a UUID), search for that exact code here (or with search_catalogs) instead \
        of paginating through list_catalogs — the short ID is indexed for search and matches fast, \
        usually returning a single unique result with the real UUID you need for other tools."
    )]
    pub async fn search_global(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match self.client.search_global(&p.query).await {
            Ok(results) => ok_json(&results),
            Err(e) => err_result(e),
        })
    }

    #[tool(
        description = "Search catalogs by name, description, or short ID. If the user gives you a \
        catalog's short ID (the ~8-character code shown in the app/URL, e.g. \"A7KX9QM2\" — NOT a \
        UUID) rather than a name, search for that exact code here instead of paginating through \
        list_catalogs — it's indexed for search and resolves fast to the real UUID other tools need."
    )]
    pub async fn search_catalogs(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match self.client.search_catalogs(&p.query).await {
            Ok(results) => ok_json(&results),
            Err(e) => err_result(e),
        })
    }
}

// ── Media tools ───────────────────────────────────────────────────────────────

#[tool_router(router = media_tools_router)]
impl EngramoMcpServer {
    #[tool(
        description = "List uploaded media files. Optionally filter by media type ('image', 'audio', etc.). Returns at most 50 items per call; pass the returned `cursor` back to fetch the next page (`cursor: null` means this is the last page)."
    )]
    pub async fn list_media(
        &self,
        Parameters(p): Parameters<ListMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(
            match self
                .client
                .list_media(p.media_type.as_deref(), p.limit, p.cursor.as_deref())
                .await
            {
                Ok(resp) => ok_json(&resp),
                Err(e) => err_result(e),
            },
        )
    }

    #[tool(
        description = "Upload a media file you already have — a user's own voice recording, a \
        self-generated audio clip, or an image — and get back a media_id. Use that id as \
        `audio_id`/`visual_id` on a card's face/back, or as `image_id` for a catalog cover, via \
        generate_card/generate_catalog_with_cards/generate_cards/update_card. This does NOT \
        generate audio or images itself — EngrAmo has no server-side TTS/image generation in this \
        flow; the caller must already have the file. For generated speech, use \
        generate_card_audio instead if it's available (stdio mode with your own TTS key). \
        Max ~10MB after decoding. \
        If you use a shell/code tool to prepare content_base64: prefer standard line-wrapped \
        `base64` output over `-w 0`/`--wrap=0` (a single unbroken multi-KB line can break some \
        tool-output pipelines), and encode directly to stdout in one step rather than writing to \
        a file and reading it back in a second command."
    )]
    pub async fn upload_media(
        &self,
        Parameters(p): Parameters<UploadMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Base64 expands data by 4/3 — reject on the encoded length before decoding so an
        // oversized payload doesn't get allocated in full just to be rejected afterward.
        if p.content_base64.len() > MAX_UPLOAD_BASE64_LEN {
            return Ok(err_result(
                "Encoded content exceeds the 10MB upload limit".to_string(),
            ));
        }
        let content = match base64::engine::general_purpose::STANDARD.decode(&p.content_base64) {
            Ok(bytes) => bytes,
            Err(e) => return Ok(err_result(format!("Invalid base64 content_base64: {e}"))),
        };
        if content.len() > MAX_UPLOAD_BYTES {
            return Ok(err_result(format!(
                "File is {} bytes, which exceeds the 10MB limit",
                content.len()
            )));
        }
        let filename = p.filename.unwrap_or_else(|| "upload".to_string());
        Ok(
            match self
                .client
                .upload_media(content, &filename, &p.content_type)
                .await
            {
                Ok(media_id) => ok_json(&UploadMediaResult { media_id }),
                Err(e) => err_result(e),
            },
        )
    }
}

// ── ServerHandler ─────────────────────────────────────────────────────────────

impl ServerHandler for EngramoMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut instructions = String::from(
            "Engramo flashcard assistant. Use catalog and card tools to manage flashcards, \
             learning tools to track spaced-repetition progress, and search to find content. \
             Cards can carry more than plain text — a dictionary (word translations), rich_text/style \
             (font, color, per-side styling), and your own audio/images via upload_media (bring your \
             own recording or picture; no paid AI is used for any of this, ever). \
             Every tool taking a catalog_id needs the real UUID, never the ~8-character short ID \
             shown in the app/URL (e.g. \"A7KX9QM2\") — if the user gives you a short ID, resolve it \
             first with search_catalogs/search_global (it's indexed, so this is fast and usually \
             returns one exact match), don't page through list_catalogs guessing. \
             Resources expose live data (catalogs, due cards, stats) and engramo://card-schema documents \
             all of the above with worked examples. \
             Prompts guide you through review sessions, flashcard creation (including \
             create_language_deck for styled, translated, dictionary-annotated decks), and study planning.",
        );
        if self.tts.is_some() {
            // Only true when the user configured their own TTS key (ENGRAMO_TTS_GEMINI_API_KEYS,
            // stdio-only — see `with_tts`/`tts` module doc comment): still no paid AI, since this
            // spends the user's own Gemini quota, never EngrAmo's.
            instructions.push_str(
                " You also have your own TTS key configured: generate_card_audio can voice a \
                 card's face side (use list_tts_voices first to see the configured model and \
                 voice catalog) — this still spends only your own Gemini quota, not EngrAmo's.",
            );
        }
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        // `InitializeResult::new` defaults `server_info` to `Implementation::from_build_env()`,
        // which expands `CARGO_CRATE_NAME`/`CARGO_PKG_VERSION` at the time **rmcp itself** is
        // compiled — not at this crate's compile time. Left alone, every client (Claude
        // Desktop, Cursor, …) would display this server as "rmcp" / rmcp's own version instead
        // of "engramo-mcp". Override it explicitly with this crate's own build-env values so
        // the identity clients see is actually ours. Do not remove this — it looks redundant
        // with `ServerInfo::new` but isn't.
        .with_server_info(Implementation::new(
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(instructions)
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        let tools = self.tool_router.list_all();
        async move { Ok(ListToolsResult::with_all_items(tools)) }
    }

    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        ctx: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.tool_router
            .call(ToolCallContext::new(self, params, ctx))
            .await
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(crate::resources::list_all())
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
        _ctx: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        crate::resources::read(&self.client, params)
            .await
            .map(Into::into)
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(crate::prompts::list_all())
    }

    async fn get_prompt(
        &self,
        params: GetPromptRequestParams,
        _ctx: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        crate::prompts::get(params).map(Into::into)
    }
}

// ── Generate tools ────────────────────────────────────────────────────────────

/// Strip characters the LLM should not embed in card text:
/// - Control characters (tabs, carriage returns, …) — newline is kept intentionally.
/// - Emoji Unicode blocks.
///
/// `pub(crate)` so `tools/tts.rs`'s `generate_card_audio` can sanitize `face.text` before
/// synthesis, the same way `normalize_card_content` does for card creation/update.
pub(crate) fn sanitize_text(s: &str) -> String {
    s.chars()
        .filter(|&c| (!c.is_control() || c == '\n') && !is_emoji_char(c))
        .collect()
}

fn is_emoji_char(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF | // All emoji/symbol blocks (Mahjong through Extended-A)
        0x2600..=0x27BF     // Misc Symbols (☀…) + Dingbats (✈ at U+2708, …)
    )
}

fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x4E00..=0x9FFF   // CJK Unified Ideographs
        | 0x3400..=0x4DBF // CJK Extension A
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
    )
}

/// True for Unicode symbol/pictograph code points that are very unlikely to appear
/// in natural-language card text but are commonly inserted by LLMs as span boundary
/// markers (e.g. ✈ at U+2708 in the Dingbats block).
fn is_symbol_char(c: char) -> bool {
    matches!(c as u32,
        0x2190..=0x27BF | // Arrows through Dingbats (✈ is U+2708)
        0x2B00..=0x2BFF   // Miscellaneous Symbols and Arrows
    )
}

/// If `s` ends with an isolated CJK ideograph (not preceded by another CJK char),
/// return it. This detects a CJK char appended as a closing span delimiter to
/// Latin/mixed text while leaving genuine CJK words (consecutive ideographs) intact.
///
/// Returns `None` for single-char spans (too short to judge safely).
fn isolated_trailing_cjk(s: &str) -> Option<char> {
    let mut rev = s.chars().rev();
    let last = rev.next()?;
    if !is_cjk(last) {
        return None;
    }
    let prev = rev.next()?; // None → single char span → return None (safe)
    if is_cjk(prev) { None } else { Some(last) }
}

/// If `s` starts with an isolated CJK ideograph (not followed by another CJK char),
/// return it. Guards against stripping the first character of genuine CJK words.
///
/// Returns `None` for single-char spans.
fn isolated_leading_cjk(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let first = chars.next()?;
    if !is_cjk(first) {
        return None;
    }
    let next = chars.next()?; // None → single char span → return None (safe)
    if is_cjk(next) { None } else { Some(first) }
}

/// Detect and strip characters that an LLM may insert as span boundary markers.
///
/// LLMs sometimes append/prepend a distinctive non-ASCII character to each span as a
/// delimiter — e.g. a CJK ideograph ('极') at the end, or a symbol glyph ('✈') at the
/// start. We identify a marker as any non-ASCII character that:
///   1. Appears at the **trailing** position of a non-final span (isolated CJK or symbol)
///      OR at the **leading** position of a non-first span (isolated CJK or symbol).
///   2. Never appears in the **interior** (non-boundary position) of any span.
///
/// When such a character is found it is stripped from every span boundary that carries
/// it. Legitimate CJK content is unaffected because consecutive ideographs fail the
/// isolation guard, and natural-language extended-Latin chars (é, ñ, ¿ …) are below
/// the symbol ranges.
fn strip_span_boundary_markers(spans: &mut [crate::dto::RichTextSpan]) {
    if spans.len() < 2 {
        return;
    }

    let mut candidates = std::collections::HashSet::new();

    // Trailing symbol or isolated-CJK chars on non-final spans.
    for span in &spans[..spans.len() - 1] {
        if let Some(last) = span.text.chars().last() {
            if is_symbol_char(last) {
                candidates.insert(last);
            } else if let Some(c) = isolated_trailing_cjk(&span.text) {
                candidates.insert(c);
            }
        }
    }

    // Leading symbol or isolated-CJK chars on non-first spans.
    for span in &spans[1..] {
        if let Some(first) = span.text.chars().next() {
            if is_symbol_char(first) {
                candidates.insert(first);
            } else if let Some(c) = isolated_leading_cjk(&span.text) {
                candidates.insert(c);
            }
        }
    }

    for marker in candidates {
        // Accept only if the marker never appears in the interior of any span.
        let only_at_boundaries = spans.iter().all(|s| {
            let char_count = s.text.chars().count();
            s.text
                .chars()
                .enumerate()
                .all(|(i, c)| c != marker || i == 0 || i == char_count - 1)
        });

        if only_at_boundaries {
            for span in spans.iter_mut() {
                if span.text.starts_with(marker) {
                    span.text = span.text[marker.len_utf8()..].to_owned();
                }
                if span.text.ends_with(marker) {
                    let trim_len = span.text.len() - marker.len_utf8();
                    span.text.truncate(trim_len);
                }
            }
        }
    }
}

fn normalize_card_content(content: &mut crate::dto::CardContent) {
    // Save sanitized original text as a validation anchor. An empty anchor means the LLM
    // omitted `text`, so skip validation and derive from spans (backward-compatible path).
    let anchor = sanitize_text(content.text.trim());

    if let Some(spans) = &mut content.rich_text {
        for span in spans.iter_mut() {
            span.text = sanitize_text(&span.text);
        }
        strip_span_boundary_markers(spans);
    }

    if let Some(spans) = &content.rich_text
        && !spans.is_empty()
    {
        let derived: String = spans.iter().map(|s| s.text.as_str()).collect();
        if !anchor.is_empty() && derived != anchor {
            // Span concatenation doesn't match the provided text — LLM corrupted them.
            // Discard richText and fall back to plain text.
            warn!(
                anchor = %anchor,
                derived = %derived,
                "normalize_card_content: span mismatch — discarding richText"
            );
            content.text = anchor;
            content.rich_text = None;
        } else {
            content.text = derived;
        }
    } else {
        content.text = sanitize_text(&content.text);
    }
}

#[tool_router(router = generate_tools_router)]
impl EngramoMcpServer {
    #[tool(description = "Create a single styled flashcard. \
            Pass catalog_id to add to an existing catalog; omit for the default catalog. \
            For language-learning cards: translate face.text yourself and set back.text before calling; \
            build a word-level dictionary (word → translation map) yourself and set face.dictionary. \
            rich_text spans (IMPORTANT rules): \
            ALWAYS set face.text and back.text to the full plain sentence — it is the ground truth. \
            Each span's text must be a VERBATIM continuous segment of the original text \
            — never add separator characters, markers, or ANY characters not present in the original sentence. \
            Spans must partition face.text with no gaps; their concatenation must equal face.text exactly. \
            The server validates this and discards richText if spans disagree with text. \
            Use fontFamily='monospace' for code, bold=true for key terms, \
            fontColor='#E74C3C' for warnings, '#27AE60' for correct answers. \
            If no styling needed, omit rich_text entirely and just set text.")]
    pub async fn generate_card(
        &self,
        Parameters(mut p): Parameters<GenerateCardParams>,
    ) -> Result<CallToolResult, ErrorData> {
        normalize_card_content(&mut p.face);
        normalize_card_content(&mut p.back);
        debug!(
            face_text = %p.face.text,
            back_text = %p.back.text,
            has_rich_text = p.face.rich_text.is_some(),
            "generate_card: after normalization"
        );
        let catalog_id = match p.catalog_id.as_deref().map(parse_uuid) {
            Some(Err(e)) => return Ok(err_result(e)),
            Some(Ok(id)) => Some(id),
            None => None,
        };
        let req = CreateCardRequest {
            catalog_id,
            face: p.face,
            back: p.back,
        };
        Ok(match self.client.create_card(&req).await {
            Ok(card) => ok_json(&card),
            Err(e) => err_result(e),
        })
    }

    #[tool(
        description = "Create a new catalog together with all its flashcards in one atomic call. \
            More efficient than creating cards one by one. \
            For language-learning cards: translate each face.text yourself and set back.text; \
            build a word-level dictionary (word → translation map) yourself and set face.dictionary on each card. \
            rich_text spans (IMPORTANT rules): \
            ALWAYS set face.text and back.text to the full plain sentence — it is the ground truth. \
            Each span's text must be a VERBATIM continuous segment of the original text \
            — never add separator characters, markers, or ANY characters not present in the original sentence. \
            Spans must partition face.text with no gaps; their concatenation must equal face.text exactly. \
            The server validates this and discards richText if spans disagree with text."
    )]
    pub async fn generate_catalog_with_cards(
        &self,
        Parameters(mut p): Parameters<GenerateCatalogWithCardsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        for card in &mut p.cards {
            normalize_card_content(&mut card.face);
            normalize_card_content(&mut card.back);
        }
        debug!(
            card_count = p.cards.len(),
            "generate_catalog_with_cards: after normalization"
        );
        let req = CreateCatalogWithCardsApiRequest {
            name: p.name,
            description: p.description,
            image_id: p.image_id,
            tags: p.tags,
            visibility: p.visibility,
            cards: p
                .cards
                .into_iter()
                .map(|c| CardInput {
                    face: c.face,
                    back: c.back,
                })
                .collect(),
        };
        Ok(match self.client.create_catalog_with_cards(&req).await {
            Ok(resp) => ok_json(&resp),
            Err(e) => err_result(e),
        })
    }

    #[tool(
        description = "Add multiple flashcards to an EXISTING catalog in one batch. \
            Use this instead of calling generate_card N times. \
            WARNING: cards are created one at a time — if one fails, previously created cards \
            in this batch are NOT rolled back. Use generate_catalog_with_cards for atomic creation. \
            For language-learning cards: translate each face.text yourself and set back.text; \
            build a word-level dictionary (word → translation map) yourself and set face.dictionary on each card. \
            rich_text spans (IMPORTANT rules): \
            ALWAYS set face.text and back.text to the full plain sentence — it is the ground truth. \
            Each span's text must be a VERBATIM continuous segment of the original text \
            — never add separator characters, markers, or ANY characters not present in the original sentence. \
            Spans must partition face.text with no gaps; their concatenation must equal face.text exactly. \
            The server validates this and discards richText if spans disagree with text."
    )]
    pub async fn generate_cards(
        &self,
        Parameters(mut p): Parameters<GenerateCardsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let catalog_id = match parse_uuid(&p.catalog_id) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        for card in &mut p.cards {
            normalize_card_content(&mut card.face);
            normalize_card_content(&mut card.back);
        }
        debug!(
            catalog_id = %catalog_id,
            card_count = p.cards.len(),
            "generate_cards: after normalization"
        );
        // Create cards individually in the existing catalog.
        let mut created_cards = Vec::with_capacity(p.cards.len());
        for card in p.cards {
            let req = CreateCardRequest {
                catalog_id: Some(catalog_id),
                face: card.face,
                back: card.back,
            };
            match self.client.create_card(&req).await {
                Ok(created) => created_cards.push(created),
                Err(e) => return Ok(err_result(e)),
            }
        }
        Ok(ok_json(&created_cards))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{CardContent, RichTextSpan, RichTextSpanStyle};

    // ── Paid-AI flag gates tool registration ─────────────────────────────────

    const PAID_AI_TOOL_NAMES: [&str; 5] = [
        "generate_tts_for_cards",
        "translate_cards",
        "generate_dictionary_for_cards",
        "ai_agent_chat",
        "translate_batch_import",
    ];

    // Local, bring-your-own-key TTS tools (`tools/tts.rs`) — gated purely by whether
    // `with_tts` was called, never by `ENGRAMO_ENABLE_PAID_AI`.
    const LOCAL_TTS_TOOL_NAMES: [&str; 2] = ["list_tts_voices", "generate_card_audio"];

    fn fake_tts_engine() -> std::sync::Arc<dyn crate::tts::TtsEngine> {
        std::sync::Arc::new(crate::tts::gemini::GeminiTts::new(
            "gemini-2.5-flash-preview-tts".to_string(),
            "Puck".to_string(),
            vec![crate::config::Redacted::new("test-key".to_string())],
        ))
    }

    fn tool_names(server: &EngramoMcpServer) -> Vec<String> {
        server
            .tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }

    #[test]
    fn test_get_info_instructions_mention_rich_card_capabilities() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let instructions = server.get_info().instructions.unwrap_or_default();
        // These are the capabilities a plain "what can you do?" should surface without the
        // user needing to already know a tool/prompt name — see docs/prompt-examples.md.
        assert!(instructions.contains("dictionary"), "{instructions}");
        assert!(instructions.contains("upload_media"), "{instructions}");
        assert!(instructions.contains("short ID"), "{instructions}");
        assert!(instructions.contains("search_catalogs"), "{instructions}");
        assert!(
            instructions.contains("create_language_deck"),
            "{instructions}"
        );
        assert!(instructions.contains("card-schema"), "{instructions}");
    }

    #[test]
    fn test_get_info_instructions_omit_tts_mention_without_tts_engine() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let instructions = server.get_info().instructions.unwrap_or_default();
        assert!(
            !instructions.contains("generate_card_audio"),
            "{instructions}"
        );
        // The blanket "no paid AI" guarantee must still hold regardless of TTS configuration.
        assert!(
            instructions.contains("no paid AI is used for any of this, ever"),
            "{instructions}"
        );
    }

    #[test]
    fn test_get_info_instructions_mention_tts_when_engine_configured() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false).with_tts(fake_tts_engine());
        let instructions = server.get_info().instructions.unwrap_or_default();
        assert!(
            instructions.contains("generate_card_audio"),
            "{instructions}"
        );
        assert!(instructions.contains("list_tts_voices"), "{instructions}");
        assert!(
            instructions.contains("no paid AI is used for any of this, ever"),
            "{instructions}"
        );
        // Still true and worth restating: local TTS spends the user's own Gemini quota.
        assert!(
            instructions.contains("your own Gemini quota"),
            "{instructions}"
        );
    }

    #[test]
    fn test_get_info_reports_this_crate_as_server_info_not_rmcp() {
        // Regression guard: `InitializeResult::new` defaults `server_info` to
        // `Implementation::from_build_env()`, which resolves to rmcp's own crate name/version
        // (since those `env!` macros expand when rmcp is compiled) rather than ours. Assert
        // clients actually see "engramo-mcp" / this crate's version.
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let server_info = server.get_info().server_info;
        assert_eq!(server_info.name, "engramo-mcp");
        assert!(!server_info.version.is_empty());
        assert_eq!(server_info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn test_upload_media_is_registered_on_the_real_server() {
        // Regression guard: `tools/media.rs`'s `MediaTools` struct is a separate, unused
        // scaffold (like every other `tools/*.rs` XxxTools struct in this codebase) — the
        // tool actually served to clients is the one registered directly on
        // `EngramoMcpServer` below. A tool added only to `MediaTools` would pass its own
        // tests while being completely unreachable from a real MCP client.
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let names = tool_names(&server);
        assert!(names.contains(&"upload_media".to_string()), "{names:?}");
        assert!(names.contains(&"list_media".to_string()), "{names:?}");
    }

    #[test]
    fn test_cursor_paginated_list_tools_document_the_50_item_cap_on_the_real_server() {
        // Regression guard for #34: every cursor-paginated list tool must tell the caller
        // about the server-side 50-item cap and how to page past it, not just say
        // "cursor-based pagination" and leave the cap undocumented.
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let tools = server.tool_router.list_all();
        for name in [
            "list_catalogs",
            "list_cards",
            "get_due_cards",
            "get_all_learning_cards",
            "list_learning_paths",
            "list_media",
        ] {
            let tool = tools
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} not registered"));
            let description = tool.description.clone().unwrap_or_default();
            assert!(description.contains("50"), "{name}: {description}");
            assert!(description.contains("cursor"), "{name}: {description}");
        }
    }

    #[test]
    fn test_search_tools_describe_short_id_resolution_on_the_real_server() {
        // Assert against the tools actually registered on EngramoMcpServer (search_tools_router).
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let tools = server.tool_router.list_all();
        for name in ["search_global", "search_catalogs"] {
            let tool = tools
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} not registered"));
            let description = tool.description.clone().unwrap_or_default();
            assert!(description.contains("short ID"), "{name}: {description}");
            assert!(description.contains("UUID"), "{name}: {description}");
        }
    }

    #[tokio::test]
    async fn test_upload_media_end_to_end_on_real_server() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "media_ids": { "ids": ["00000000-0000-0000-0000-000000000001"] }
            })))
            .mount(&mock_server)
            .await;

        let client = EngramoClient::new(mock_server.uri(), "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let result = server
            .upload_media(Parameters(UploadMediaParams {
                content_base64: base64::engine::general_purpose::STANDARD.encode(b"test audio"),
                content_type: "audio/mpeg".to_string(),
                filename: Some("test.mp3".to_string()),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    fn first_text(result: &CallToolResult) -> &str {
        result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("")
    }

    #[tokio::test]
    async fn test_list_media_cursor_forwarded_and_reaches_tool_output_on_real_server() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .and(query_param("cursor", "abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [],
                "nextCursor": "def"
            })))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .list_media(Parameters(ListMediaParams {
                media_type: None,
                limit: None,
                cursor: Some("abc".to_string()),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(first_text(&result)).unwrap();
        assert_eq!(v["cursor"], "def");
    }

    #[tokio::test]
    async fn test_search_global_title_and_parent_id_reach_tool_output() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "x"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "itemType": "card",
                    "id": "00000000-0000-0000-0000-000000000001",
                    "title": "T",
                    "parentId": "00000000-0000-0000-0000-000000000002"
                }])),
            )
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .search_global(Parameters(SearchParams {
                query: "x".to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(first_text(&result)).unwrap();
        assert_eq!(v[0]["item_type"], "card");
        assert_eq!(v[0]["title"], "T");
        assert_eq!(v[0]["parent_id"], "00000000-0000-0000-0000-000000000002");
    }

    /// Repro for issue #36: a realistic `/search` payload — one catalog hit (with
    /// `imageId`/`imageUrl` set) and one card hit (with `parentId`), Cyrillic text
    /// included — driven end-to-end through the `search_global` tool handler. The
    /// bug report claims every field but `id` comes back null; this asserts
    /// `item_type` and `title` are non-null for both hits in the tool's JSON output.
    #[tokio::test]
    async fn test_search_global_realistic_backend_payload_reaches_tool_output() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let catalog_id = "dda5cf9a-84c3-463b-985f-df02b71a3a4b";
        let card_id = "00000000-0000-0000-0000-000000000099";
        let card_parent_id = "00000000-0000-0000-0000-000000000098";
        let image_id = "00000000-0000-0000-0000-0000000000aa";
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "іспанська"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "itemType": "catalog",
                    "id": catalog_id,
                    "title": "[Іспанська] Співбесіда: …",
                    "subtitle": "desc",
                    "parentId": null,
                    "rank": 0.42,
                    "imageId": image_id,
                    "imageUrl": "https://cdn.example.com/img.png"
                },
                {
                    "itemType": "card",
                    "id": card_id,
                    "title": "¿Cómo estás?",
                    "subtitle": "back text",
                    "parentId": card_parent_id,
                    "rank": 0.1,
                    "imageId": null
                }
            ])))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .search_global(Parameters(SearchParams {
                query: "іспанська".to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(first_text(&result)).unwrap();

        // Catalog hit.
        assert_eq!(v[0]["id"], catalog_id);
        assert_eq!(v[0]["item_type"], "catalog", "{v}");
        assert_eq!(v[0]["title"], "[Іспанська] Співбесіда: …", "{v}");

        // Card hit.
        assert_eq!(v[1]["id"], card_id);
        assert_eq!(v[1]["item_type"], "card", "{v}");
        assert_eq!(v[1]["title"], "¿Cómo estás?", "{v}");
        assert_eq!(v[1]["parent_id"], card_parent_id, "{v}");
    }

    #[test]
    fn test_search_global_description_documents_parent_id() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let tools = server.tool_router.list_all();
        let tool = tools.iter().find(|t| t.name == "search_global").unwrap();
        let description = tool.description.clone().unwrap_or_default();
        assert!(description.contains("parent_id"), "{description}");
    }

    /// Regression guard for issue #36's description change: `search_global` no longer
    /// claims to cover learning paths, and points callers at `list_learning_paths`
    /// instead. Nothing else pins this wording, so the old "cards, catalogs, and
    /// learning paths" claim could silently come back.
    #[test]
    fn test_search_global_description_excludes_learning_paths() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let tools = server.tool_router.list_all();
        let tool = tools.iter().find(|t| t.name == "search_global").unwrap();
        let description = tool.description.clone().unwrap_or_default();
        assert!(
            description.contains("learning paths are NOT included"),
            "{description}"
        );
        assert!(description.contains("list_learning_paths"), "{description}");
        assert!(
            !description.contains("cards, catalogs, and learning paths"),
            "{description}"
        );
    }

    /// Regression for the tool-handler contract (finding F2): an oversized `q` must
    /// surface as `is_error: true` output — never a raw `Err`, and never a request sent
    /// to the backend at all.
    #[tokio::test]
    async fn test_search_global_oversized_query_returns_is_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        // Over client.rs's private MAX_QUERY_PARAM_LEN (512); can't reference the
        // constant from here since it isn't `pub(crate)`.
        let long_query = "a".repeat(513);
        let result = server
            .search_global(Parameters(SearchParams { query: long_query }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("too long"), "{result:?}");
    }

    /// Moved from the now-deleted `tools/search.rs` scaffold (`SearchTools` was never
    /// constructed) — asserts the tool-handler error path for `search_catalogs`.
    #[tokio::test]
    async fn test_search_catalogs_unauthorized_returns_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/catalogs"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .search_catalogs(Parameters(SearchParams {
                query: "rust".to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("Unauthorized"), "{result:?}");
    }

    /// Drives the `search_global` tool handler's error arm — only `search_catalogs` had
    /// a tool-level error test before this.
    #[tokio::test]
    async fn test_search_global_unauthorized_returns_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .search_global(Parameters(SearchParams {
                query: "rust".to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("Unauthorized"), "{result:?}");
    }

    /// Drives the `search_catalogs` tool handler's success arm — previously covered
    /// only indirectly through the client-level `test_search_catalogs` in `client.rs`.
    #[tokio::test]
    async fn test_search_catalogs_ok_returns_catalog_json() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let catalog_id = "00000000-0000-0000-0000-000000000001";
        Mock::given(method("GET"))
            .and(path("/search/catalogs"))
            .and(query_param("q", "A7KX9QM2"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "id": catalog_id,
                    "name": "Rust Basics",
                    "version": 1
                }])),
            )
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .search_catalogs(Parameters(SearchParams {
                query: "A7KX9QM2".to_string(),
            }))
            .await
            .unwrap();
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(first_text(&result)).unwrap();
        assert_eq!(v[0]["id"], catalog_id);
        assert_eq!(v.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_get_server_version_returns_crate_name_and_version() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let result = server.get_server_version().await.unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
        assert!(text.contains(env!("CARGO_PKG_NAME")), "{text}");
    }

    #[test]
    fn test_paid_ai_tools_absent_when_flag_off() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let names = tool_names(&server);
        for tool in PAID_AI_TOOL_NAMES {
            assert!(
                !names.contains(&tool.to_string()),
                "expected {tool} to be ABSENT when ENGRAMO_ENABLE_PAID_AI is off"
            );
        }
    }

    #[test]
    fn test_paid_ai_tools_present_when_flag_on() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, true);
        let names = tool_names(&server);
        for tool in PAID_AI_TOOL_NAMES {
            assert!(
                names.contains(&tool.to_string()),
                "expected {tool} to be registered when ENGRAMO_ENABLE_PAID_AI is on"
            );
        }
    }

    #[test]
    fn test_local_tts_tools_absent_by_default() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let names = tool_names(&server);
        for tool in LOCAL_TTS_TOOL_NAMES {
            assert!(
                !names.contains(&tool.to_string()),
                "expected {tool} to be ABSENT until with_tts is called"
            );
        }
        assert!(server.tts.is_none());
    }

    #[test]
    fn test_local_tts_tools_present_after_with_tts() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false).with_tts(fake_tts_engine());
        let names = tool_names(&server);
        for tool in LOCAL_TTS_TOOL_NAMES {
            assert!(
                names.contains(&tool.to_string()),
                "expected {tool} to be registered after with_tts"
            );
        }
        assert!(server.tts.is_some());
    }

    #[test]
    fn test_local_tts_tools_absent_via_build_session_server_paid_ai_off() {
        let http = EngramoClient::build_http_client();
        let server = build_session_server(&http, "http://localhost", "session-token", false);
        let names = tool_names(&server);
        for tool in LOCAL_TTS_TOOL_NAMES {
            assert!(
                !names.contains(&tool.to_string()),
                "http-mode session must never see {tool}, regardless of process env"
            );
        }
        assert!(server.tts.is_none());
    }

    #[test]
    fn test_local_tts_tools_absent_via_build_session_server_paid_ai_on() {
        let http = EngramoClient::build_http_client();
        let server = build_session_server(&http, "http://localhost", "session-token", true);
        let names = tool_names(&server);
        for tool in LOCAL_TTS_TOOL_NAMES {
            assert!(
                !names.contains(&tool.to_string()),
                "http-mode session must never see {tool}, even with paid_ai_enabled on"
            );
        }
        assert!(server.tts.is_none());
    }

    #[test]
    fn test_always_on_tools_present_regardless_of_flag() {
        for flag in [false, true] {
            let client = EngramoClient::new("http://localhost", "engramo_test");
            let server = EngramoMcpServer::new(client, flag);
            let names = tool_names(&server);
            assert!(names.contains(&"list_catalogs".to_string()));
            assert!(names.contains(&"generate_card".to_string()));
            assert!(names.contains(&"get_server_version".to_string()));
        }
    }

    fn span(text: &str) -> RichTextSpan {
        RichTextSpan {
            text: text.to_owned(),
            style: None,
        }
    }

    fn bold_span(text: &str) -> RichTextSpan {
        RichTextSpan {
            text: text.to_owned(),
            style: Some(RichTextSpanStyle {
                bold: Some(true),
                ..Default::default()
            }),
        }
    }

    // ── strip_span_boundary_markers ──────────────────────────────────────────

    // ── trailing CJK marker (极) ─────────────────────────────────────────────

    #[test]
    fn test_strip_cjk_boundary_marker_from_span_ends() {
        // LLM appends '极' to close each span.
        let mut spans = vec![
            span("Me 极"),
            bold_span("estoy muriendo极"),
            span(" de ganas de verte pronto."),
        ];
        strip_span_boundary_markers(&mut spans);
        assert_eq!(spans[0].text, "Me ");
        assert_eq!(spans[1].text, "estoy muriendo");
        assert_eq!(spans[2].text, " de ganas de verte pronto.");
    }

    #[test]
    fn test_strip_does_not_affect_single_span() {
        let mut spans = vec![span("hello 极 world")];
        strip_span_boundary_markers(&mut spans);
        assert_eq!(spans[0].text, "hello 极 world");
    }

    #[test]
    fn test_strip_preserves_legitimate_cjk_word() {
        // Consecutive CJK chars — trailing CJK is part of a real word.
        let mut spans = vec![span("极大的"), bold_span("力量")];
        strip_span_boundary_markers(&mut spans);
        assert_eq!(spans[0].text, "极大的");
        assert_eq!(spans[1].text, "力量");
    }

    #[test]
    fn test_strip_preserves_single_cjk_char_span() {
        let mut spans = vec![span("大"), bold_span("力量")];
        strip_span_boundary_markers(&mut spans);
        assert_eq!(spans[0].text, "大");
        assert_eq!(spans[1].text, "力量");
    }

    #[test]
    fn test_strip_preserves_ascii_trailing_char() {
        let mut spans = vec![span("hello "), bold_span("world")];
        strip_span_boundary_markers(&mut spans);
        assert_eq!(spans[0].text, "hello ");
        assert_eq!(spans[1].text, "world");
    }

    #[test]
    fn test_strip_cjk_only_at_final_span_is_not_stripped() {
        // '极' only in the final span — never a non-final trailing marker.
        let mut spans = vec![span("hello"), bold_span("world极")];
        strip_span_boundary_markers(&mut spans);
        assert_eq!(spans[0].text, "hello");
        assert_eq!(spans[1].text, "world极");
    }

    // ── leading symbol marker (✈) ────────────────────────────────────────────

    #[test]
    fn test_strip_symbol_leading_marker_from_span_starts() {
        // LLM prepends '✈' (U+2708, Dingbats) to open each highlighted span.
        let mut spans = vec![
            span("Ahora mismo "),
            bold_span("✈estoy trabajando"),
            span(" en un proyecto."),
        ];
        strip_span_boundary_markers(&mut spans);
        assert_eq!(spans[0].text, "Ahora mismo ");
        assert_eq!(spans[1].text, "estoy trabajando");
        assert_eq!(spans[2].text, " en un proyecto.");
    }

    #[test]
    fn test_strip_symbol_leading_marker_on_first_span() {
        // '✈' at the very start of the first span (no preceding plain span).
        let mut spans = vec![bold_span("✈Estamos viendo"), span(" una película.")];
        strip_span_boundary_markers(&mut spans);
        // span[0] is the first span; '✈' is not at the END of a non-final span, but it
        // IS at the START of a non-first... wait: span[0] is the first span so its
        // leading char is checked against the "non-first spans" rule.
        // span[1] start = ' ' (ASCII) → no leading candidate from span[1].
        // So '✈' is at the START of span[0] which is the FIRST span → NOT detected as
        // a leading marker. It will be stripped only if it was a trailing marker on a
        // non-final span — which it isn't here.
        // However, sanitize_text (is_emoji_char) covers this case for plain text.
        // The spans here remain unchanged since no boundary rule fires.
        assert_eq!(spans[0].text, "✈Estamos viendo");
        assert_eq!(spans[1].text, " una película.");
    }

    #[test]
    fn test_strip_symbol_not_stripped_when_in_interior() {
        // '✈' appears in the middle of a span — must not be stripped.
        let mut spans = vec![span("fly ✈ here"), bold_span("world")];
        strip_span_boundary_markers(&mut spans);
        assert_eq!(spans[0].text, "fly ✈ here");
        assert_eq!(spans[1].text, "world");
    }

    // ── sanitize_text strips symbols from plain text ─────────────────────────

    #[test]
    fn test_sanitize_strips_airplane_emoji() {
        assert_eq!(sanitize_text("✈estoy trabajando"), "estoy trabajando");
    }

    #[test]
    fn test_sanitize_strips_misc_symbols() {
        assert_eq!(sanitize_text("☀ sunny day"), " sunny day");
    }

    #[test]
    fn test_sanitize_preserves_extended_latin() {
        // ¿, é, ñ are valid Spanish chars and must not be stripped.
        assert_eq!(sanitize_text("¿Cómo estás?"), "¿Cómo estás?");
    }

    // ── normalize_card_content ───────────────────────────────────────────────

    #[test]
    fn test_normalize_derives_text_from_spans_and_strips_markers() {
        let mut content = CardContent {
            text: String::new(),
            rich_text: Some(vec![
                span("Me 极"),
                bold_span("estoy muriendo极"),
                span(" de ganas de verte pronto."),
            ]),
            style: None,
            dictionary: None,
            audio_id: None,
            visual_id: None,
            visual_type: None,
        };
        normalize_card_content(&mut content);
        assert_eq!(content.text, "Me estoy muriendo de ganas de verte pronto.");
        let spans = content.rich_text.as_ref().unwrap();
        assert_eq!(spans[0].text, "Me ");
        assert_eq!(spans[1].text, "estoy muriendo");
    }

    #[test]
    fn test_normalize_plain_text_strips_control_chars() {
        let mut content = CardContent::plain("hello\tworld\r");
        normalize_card_content(&mut content);
        assert_eq!(content.text, "helloworld");
    }

    #[test]
    fn test_normalize_matching_anchor_preserves_rich_text() {
        let mut content = CardContent {
            text: "Ella está vistiendo a su hija.".to_string(),
            rich_text: Some(vec![
                span("Ella está "),
                bold_span("vistiendo"),
                span(" a su hija."),
            ]),
            style: None,
            dictionary: None,
            audio_id: None,
            visual_id: None,
            visual_type: None,
        };
        normalize_card_content(&mut content);
        assert_eq!(content.text, "Ella está vistiendo a su hija.");
        assert!(content.rich_text.is_some());
        assert_eq!(content.rich_text.unwrap().len(), 3);
    }

    #[test]
    fn test_normalize_corrupted_spans_discards_rich_text() {
        // Reproduces the real hallucination: span 3 has ",type:" appended, span 4 duplicates full sentence.
        let mut content = CardContent {
            text: "Ella está vistiendo a su hija.".to_string(),
            rich_text: Some(vec![
                span("Ella está "),
                bold_span("vistiendo"),
                span(" a su hija.,type:"),
                span("Ella está vistiendo a su hija."),
            ]),
            style: None,
            dictionary: None,
            audio_id: None,
            visual_id: None,
            visual_type: None,
        };
        normalize_card_content(&mut content);
        assert_eq!(content.text, "Ella está vistiendo a su hija.");
        assert!(content.rich_text.is_none()); // discarded
    }

    #[test]
    fn test_normalize_empty_anchor_derives_from_spans() {
        // When `text` is "" (omitted by LLM), skip validation — backward-compatible path.
        let mut content = CardContent {
            text: String::new(),
            rich_text: Some(vec![span("Me "), bold_span("gusta"), span(" el café.")]),
            style: None,
            dictionary: None,
            audio_id: None,
            visual_id: None,
            visual_type: None,
        };
        normalize_card_content(&mut content);
        assert_eq!(content.text, "Me gusta el café.");
        assert!(content.rich_text.is_some());
    }
}
