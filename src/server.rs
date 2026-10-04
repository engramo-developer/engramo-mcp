use std::sync::Arc;

use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, GetPromptRequestParams,
        GetPromptResponse, Implementation, ListPromptsResult, ListResourcesResult, ListToolsResult,
        PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
        ServerCapabilities, ServerConfig,
    },
    tool, tool_router,
};
use tracing::{debug, warn};

use base64::Engine;

use crate::client::EngramoClient;
use crate::dto::{
    CardInput, CreateCardRequest, CreateCatalogWithCardsApiRequest, CreateLearningPathRequest,
    UpdateCardRequest, UpdateCatalogRequest, UpdateLearningPathRequest, UploadMediaResult,
};
use crate::error::ApiError;
use crate::tools::cards::{
    DeleteCardParams, GetCardParams, ListCardsParams, UpdateCardParams, parse_catalog_ids,
};
use crate::tools::catalogs::{
    DeleteCatalogParams, GetCatalogParams, ListCatalogsParams, UpdateCatalogParams, err_result,
    ok_json, ok_text, parse_uuid,
};
use crate::tools::generate::{
    GenerateCardParams, GenerateCardsParams, GenerateCatalogWithCardsParams, check_batch_size,
};
use crate::tools::learning::{AddCardToLearningParams, AddCatalogToLearningParams, DueCardsParams};
use crate::tools::learning_paths::{
    ActivateLearningPathParams, AddCatalogToLearningPathParams, CatalogAddFailure,
    CreateLearningPathParams, CreatedLearningPathWithCatalogs, GetLearningPathParams,
    ListLearningPathsParams, MAX_PATH_CATALOG_IDS, RemoveCatalogFromLearningPathParams,
    UpdateLearningPathParams,
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
        description = "Delete a catalog. It is archived (not hard-deleted) if it has cards, a cover image, or is in an active learning path; otherwise it is hard-deleted. There is no restore/unarchive tool. Cards also in another live catalog are unaffected (they only lose this membership); cards only in this catalog stay reachable by id via get_card but show `catalogs: []`, disappear from search and list_cards, and are archived later by backend cleanup. To keep them, move them to another catalog with update_card (catalog_ids) BEFORE deleting. Catalog quota decrements automatically."
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
        description = "List flashcards in a catalog with cursor-based pagination. Returns at most 50 items per call; pass the returned `cursor` back to fetch the next page (`cursor: null` means this is the last page). Each returned card includes its `catalogs` memberships (id, name), permission-filtered the same as `get_card` — omitted when the API didn't report memberships."
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
        description = "Get a single flashcard by UUID, including its face, back, and catalog memberships (`catalogs` is omitted when the API does not report memberships). A card_id that doesn't exist is reported as \"Not found\"; one that belongs to another user is reported as a permission error (the API may also report a missing id that way)."
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
        `catalog_ids` REPLACES the memberships you can see, so build it from the \
        `catalogs[].id` values in `get_card` or `list_cards`'s response (which lists only \
        catalogs visible to you) plus/minus any intended changes. Must contain at least one \
        UUID — an empty list is rejected, since the API would otherwise silently move the card \
        to the user's default catalog. If the card JSON has no `catalogs` key, its memberships \
        are unknown — call `get_card` first. If you get a Conflict error, re-fetch and retry. \
        rich_text styling goes under a nested `style` object, e.g. \
        {\"text\":\"gracias.\",\"style\":{\"bold\":true,\"fontColor\":\"#27AE60\"}}. \
        A card_id that doesn't exist is reported as \"Not found\"; one that belongs to another user is reported as a permission error (the API may also report a missing id that way)."
    )]
    pub async fn update_card(
        &self,
        Parameters(mut p): Parameters<UpdateCardParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let card_id = match parse_uuid(&p.card_id) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let catalog_ids = match parse_catalog_ids(&p.catalog_ids) {
            Ok(ids) => ids,
            Err(e) => return Ok(err_result(e)),
        };
        for content in [&p.face, &p.back].into_iter().flatten() {
            if let Err(e) = check_card_content(content) {
                return Ok(err_result(e));
            }
        }
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
        // Preserve server-managed fields (audio_id, visual, dictionary) the LLM cannot know about.
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
                        if face.visual_id.is_none() && face.visual_type.is_none() {
                            face.visual_id = existing.face.visual_id;
                            face.visual_type = existing.face.visual_type;
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
                        if back.visual_id.is_none() && back.visual_type.is_none() {
                            back.visual_id = existing.back.visual_id;
                            back.visual_type = existing.back.visual_type;
                        }
                        if back.dictionary.is_none() {
                            back.dictionary = existing.back.dictionary;
                        }
                    }
                }
                Err(e) => return Ok(err_result(e)),
            }
        }
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
        description = "Delete a flashcard from a catalog. If the card has learning progress it is archived; otherwise hard-deleted. A card_id that doesn't exist is reported as \"Not found\"; one that belongs to another user is reported as a permission error (the API may also report a missing id that way)."
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

    #[tool(
        description = "Add a single card to the learning queue. It will appear in due reviews. A card_id that doesn't exist is reported as \"Not found\"; one that belongs to another user is reported as a permission error (the API may also report a missing id that way)."
    )]
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

    #[tool(
        description = "Get full details of a learning path, including its catalogs, tags, \
        visibility, and current version (needed by update_learning_path)."
    )]
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

    #[tool(
        description = "Create a new learning path. It starts empty — use add_catalog_to_learning_path \
        (or pass `catalog_ids` here) to populate it with catalogs. Passing `catalog_ids` adds each \
        one in its own request right after the path is created (non-atomic): the path is never \
        rolled back if a catalog fails to add, and the response then includes \
        `catalogs_added`/`catalogs_failed` so failed ids can be retried."
    )]
    pub async fn create_learning_path(
        &self,
        Parameters(p): Parameters<CreateLearningPathParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let raw_ids = p.catalog_ids.unwrap_or_default();
        if raw_ids.len() > MAX_PATH_CATALOG_IDS {
            return Ok(err_result(format!(
                "Too many catalog_ids: {} (max {MAX_PATH_CATALOG_IDS}); add the rest with \
                 add_catalog_to_learning_path",
                raw_ids.len()
            )));
        }
        // Validate every catalog id before creating anything, so a typo can't leave behind a
        // half-populated path. Duplicates are dropped (first occurrence wins) so the same id
        // can't show up twice in `catalogs_added`, or in both `catalogs_added` and
        // `catalogs_failed`.
        let mut seen = std::collections::HashSet::new();
        let mut catalog_ids = Vec::new();
        for raw_id in raw_ids {
            match parse_uuid(&raw_id) {
                Ok(id) => {
                    if seen.insert(id) {
                        catalog_ids.push((raw_id, id));
                    }
                }
                Err(e) => return Ok(err_result(e)),
            }
        }

        let req = CreateLearningPathRequest {
            name: p.name,
            description: p.description,
        };
        let path = match self.client.create_learning_path(&req).await {
            Ok(path) => path,
            Err(e) => return Ok(err_result(e)),
        };

        if catalog_ids.is_empty() {
            return Ok(ok_json(&path));
        }

        let mut added: Vec<String> = Vec::new();
        let mut failed: Vec<CatalogAddFailure> = Vec::new();
        let mut ids_iter = catalog_ids.into_iter();
        for (raw_id, id) in ids_iter.by_ref() {
            match self.client.add_catalog_to_learning_path(path.id, id).await {
                Ok(()) => added.push(raw_id),
                Err(e) => {
                    let e = disambiguate_catalog_forbidden(&self.client, id, e).await;
                    // Unauthorized/QuotaExceeded won't clear up on the next id in this same
                    // batch — stop instead of firing the rest of the requests under a token
                    // that's already known to be rejected/rate-limited.
                    let fatal =
                        matches!(e, ApiError::Unauthorized | ApiError::QuotaExceeded { .. });
                    failed.push(CatalogAddFailure {
                        catalog_id: raw_id,
                        error: e.to_string(),
                    });
                    if fatal {
                        for (raw_id, _) in ids_iter.by_ref() {
                            failed.push(CatalogAddFailure {
                                catalog_id: raw_id,
                                error: "skipped: a previous catalog add failed with a fatal \
                                        error"
                                    .to_string(),
                            });
                        }
                        break;
                    }
                }
            }
        }

        Ok(ok_json(&CreatedLearningPathWithCatalogs {
            path: &path,
            catalogs_added: added,
            catalogs_failed: failed,
        }))
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

    #[tool(
        description = "Add a catalog to a learning path so its cards become part of the path. \
        Idempotent: adding a catalog that is already in the path succeeds (no error). A missing \
        path or catalog returns Not found."
    )]
    pub async fn add_catalog_to_learning_path(
        &self,
        Parameters(p): Parameters<AddCatalogToLearningPathParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let path_id = match parse_uuid(&p.path_id) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let catalog_id = match parse_uuid(&p.catalog_id) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        Ok(
            match self
                .client
                .add_catalog_to_learning_path(path_id, catalog_id)
                .await
            {
                Ok(()) => ok_text("Catalog added to learning path."),
                Err(e) => {
                    err_result(disambiguate_catalog_forbidden(&self.client, catalog_id, e).await)
                }
            },
        )
    }

    #[tool(
        description = "Remove a catalog from a learning path. Idempotent: removing a catalog \
        that is not in the path succeeds (with a hint) instead of erroring, so a retried remove \
        is safe. A missing path returns Not found."
    )]
    pub async fn remove_catalog_from_learning_path(
        &self,
        Parameters(p): Parameters<RemoveCatalogFromLearningPathParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let path_id = match parse_uuid(&p.path_id) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let catalog_id = match parse_uuid(&p.catalog_id) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        Ok(
            match self
                .client
                .remove_catalog_from_learning_path(path_id, catalog_id)
                .await
            {
                Ok(()) => ok_text("Catalog removed from learning path."),
                // The backend 404s both for a missing path and for a catalog that isn't in
                // the path, with no reliable way to tell them apart from the body. Probe the
                // path: only if it is readable AND does not list the catalog was the catalog
                // simply not a member (idempotent no-op); otherwise surface the original
                // error (#72, #62).
                Err(ApiError::NotFound(msg)) => {
                    match self.client.get_learning_path(path_id).await {
                        Ok(lp)
                            if !lp
                                .catalogs
                                .as_deref()
                                .unwrap_or_default()
                                .iter()
                                .any(|c| c.id == catalog_id) =>
                        {
                            ok_text("Catalog was not in learning path.")
                        }
                        _ => err_result(ApiError::NotFound(msg)),
                    }
                }
                Err(e) => err_result(e),
            },
        )
    }

    #[tool(
        description = "Update a learning path's name, description, tags, or visibility. \
        Requires the current version for optimistic locking — fetch the path first \
        (get_learning_path) to get the version. If you get a Conflict error, re-fetch and retry. \
        `visibility: 'unlisted'` is catalog-only and is rejected for learning paths."
    )]
    pub async fn update_learning_path(
        &self,
        Parameters(p): Parameters<UpdateLearningPathParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match parse_uuid(&p.path_id) {
            Ok(id) => {
                let req = UpdateLearningPathRequest {
                    name: p.name,
                    description: p.description,
                    tags: p.tags,
                    visibility: p.visibility,
                    version: p.version,
                };
                match self.client.update_learning_path(id, &req).await {
                    Ok(path) => ok_json(&path),
                    Err(e) => err_result(e),
                }
            }
            Err(e) => err_result(e),
        })
    }
}

/// Disambiguates a `Forbidden` from `add_catalog_to_learning_path`: the backend's `INSERT ...
/// WHERE EXISTS` predicate can't tell "catalog doesn't exist" from "catalog exists but the
/// caller lacks read access" apart and reports both as a bare 403 (see #62 — the backend fix is
/// tracked separately). Only a `PermissionDenied` triggers the extra lookup; every other error
/// passes through untouched.
///
/// `get_catalog` is the ground truth: a `NotFound` there means the catalog truly doesn't exist,
/// so that (more actionable, and consistent with `get_catalog`'s own error) result replaces the
/// original Forbidden. Anything else — the catalog exists (so the 403 likely stems from
/// learning-path permissions), or the lookup itself fails (e.g. with its own 403) — returns the
/// original error unchanged, so a genuine permission error is never masked.
async fn disambiguate_catalog_forbidden(
    client: &EngramoClient,
    catalog_id: Uuid,
    original: ApiError,
) -> ApiError {
    if !matches!(original, ApiError::PermissionDenied(_)) {
        return original;
    }
    match client.get_catalog(catalog_id).await {
        Err(ApiError::NotFound(_)) => {
            ApiError::NotFound(format!("catalog {catalog_id} does not exist"))
        }
        _ => original,
    }
}

// ── Search tools ──────────────────────────────────────────────────────────────

#[tool_router(router = search_tools_router)]
impl EngramoMcpServer {
    #[tool(
        description = "Searches cards and catalogs (newer API versions also return learning paths \
        with item_type 'learning_path'). To search learning paths specifically, use \
        search_learning_paths. Each hit has item_type ('catalog', 'card', 'learning_path', or a \
        newer value not listed here), title (the catalog's name, or the card's face text), subtitle (the catalog's \
        description, or the card's back text), and parent_id, which for a card hit is its \
        catalog's UUID (use it directly with get_catalog/list_cards). For a 'learning_path' hit, \
        id is the path_id for get_learning_path (title = the path's name, subtitle = its \
        description). \
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

    #[tool(
        description = "Search learning paths by name or description. Returns an array of \
        {id, name, description, tags, visibility, version} (tags/visibility are omitted when the API \
        does not send them); pass `id` as path_id to get_learning_path (to see its \
        catalogs), activate_learning_path, or deactivate_learning_path. Use this instead of \
        search_global when you specifically want learning paths — search_global may not include \
        them depending on the API version. If this tool reports Not Found for the endpoint \
        itself (older API), fall back to list_learning_paths."
    )]
    pub async fn search_learning_paths(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match self.client.search_learning_paths(&p.query).await {
            Ok(results) => ok_json(&results),
            Err(e) => err_result(e),
        })
    }
}

// ── Media tools ───────────────────────────────────────────────────────────────

#[tool_router(router = media_tools_router)]
impl EngramoMcpServer {
    #[tool(
        description = "List your own media library — files you uploaded with upload_media, \
        including audio produced by generate_card_audio. This does NOT include media attached \
        to cards uploaded by other accounts (e.g. in shared/subscribed catalogs); to find a \
        specific card's audio or image, use get_card and read its `audio_id`/`visual_id` instead. \
        Optionally filter by media type ('image', 'audio', etc.). Each item has `id`, `name` \
        (original filename), `content_type`, `media_type`, and `length` (size in bytes). Returns \
        at most 50 items per call; pass the returned `cursor` back to fetch the next page \
        (`cursor: null` means this is the last page)."
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
        // The raw bound is loose enough to admit CRLF line-wrapping (every 76 chars); the
        // exact bound is re-checked below once whitespace is stripped.
        if p.content_base64.len() > MAX_UPLOAD_BASE64_LEN + MAX_UPLOAD_BASE64_LEN / 38 {
            return Ok(err_result(
                "Encoded content exceeds the 10MB upload limit".to_string(),
            ));
        }
        // Line-wrapped `base64` output is explicitly recommended above, so drop ASCII
        // whitespace before checking the length and decoding.
        let encoded: String = p
            .content_base64
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        if encoded.len() > MAX_UPLOAD_BASE64_LEN {
            return Ok(err_result(
                "Encoded content exceeds the 10MB upload limit".to_string(),
            ));
        }
        let content = match base64::engine::general_purpose::STANDARD.decode(&encoded) {
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
    fn get_info(&self) -> ServerConfig {
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
        ServerConfig::new(
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
        // with `ServerConfig::new` but isn't.
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
        .filter(|&c| (!c.is_control() || c == '\n') && c != '\u{FE0F}' && !is_emoji_char(c))
        .collect()
}

fn is_emoji_char(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF | // All emoji/symbol blocks (Mahjong through Extended-A)
        0x2600..=0x2653 | // Misc Symbols (☀…) up to the zodiac signs
        0x2670..=0x2712 | // rest of Misc Symbols + Dingbats (✈ at U+2708, …)
        0x2718..=0x27BF   // rest of Dingbats
    )
    // Deliberately kept: chess pieces/card suits/music signs (U+2654–266F) and check/ballot
    // marks (U+2713–2717) carry meaning in flashcard text (e.g. "C♯ major").
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
        // Arrows through Dingbats (✈ is U+2708), minus the ranges `is_emoji_char`
        // preserves as real content (music/chess/suits U+2654–266F, check marks U+2713–2717).
        0x2190..=0x2653 | 0x2670..=0x2712 | 0x2718..=0x27BF |
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

    // One pass: every char that appears in an interior (non-boundary) position of any span.
    let mut interior = std::collections::HashSet::new();
    for s in spans.iter() {
        let n = s.text.chars().count();
        for (i, c) in s.text.chars().enumerate() {
            if i != 0 && i + 1 != n {
                interior.insert(c);
            }
        }
    }
    // Accept a marker only if it never appears in the interior of any span.
    let markers: Vec<char> = candidates
        .into_iter()
        .filter(|c| !interior.contains(c))
        .collect();

    for span in spans.iter_mut() {
        for &marker in &markers {
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

const MAX_SPANS_PER_CONTENT: usize = 1_000;
const MAX_CONTENT_CHARS: usize = 20_000;

/// Reject oversized or malformed card content before `normalize_card_content` does any
/// work on it (bounds the cost of normalization) and before untrusted media ids go upstream.
fn check_card_content(content: &crate::dto::CardContent) -> Result<(), String> {
    let span_count = content.rich_text.as_ref().map_or(0, Vec::len);
    let span_chars: usize = content
        .rich_text
        .iter()
        .flatten()
        .map(|s| s.text.chars().count())
        .sum();
    if span_count > MAX_SPANS_PER_CONTENT
        || content.text.chars().count() > MAX_CONTENT_CHARS
        || span_chars > MAX_CONTENT_CHARS
    {
        return Err(format!(
            "card content too large (max {MAX_SPANS_PER_CONTENT} spans / {MAX_CONTENT_CHARS} chars)"
        ));
    }
    for (name, id) in [
        ("audio_id", &content.audio_id),
        ("visual_id", &content.visual_id),
    ] {
        if let Some(id) = id
            && let Err(e) = parse_uuid(id)
        {
            return Err(format!("Invalid {name}: {e}"));
        }
    }
    Ok(())
}

fn is_safe_color(s: &str) -> bool {
    let h = s.strip_prefix('#').unwrap_or("");
    matches!(h.len(), 3 | 6 | 8) && h.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_safe_font_family(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | ',' | '-'))
}

fn is_safe_text_align(s: &str) -> bool {
    matches!(s, "left" | "right" | "center" | "justify")
}

/// Drop style values that are not plain colors / font names / alignments, so a
/// prompt-injected value can't reach a CSS renderer as an injection payload.
fn drop_unsafe(name: &'static str, field: &mut Option<String>, ok: fn(&str) -> bool) {
    if field.as_deref().is_some_and(|v| !ok(v)) {
        debug!(field = name, "sanitize_styles: dropped unsafe style value");
        *field = None;
    }
}

fn sanitize_styles(content: &mut crate::dto::CardContent) {
    if let Some(style) = &mut content.style {
        drop_unsafe("fontColor", &mut style.font_color, is_safe_color);
        drop_unsafe(
            "backgroundColor",
            &mut style.background_color,
            is_safe_color,
        );
        drop_unsafe("fontFamily", &mut style.font_family, is_safe_font_family);
        drop_unsafe("textAlign", &mut style.text_align, is_safe_text_align);
    }
    for span in content.rich_text.iter_mut().flatten() {
        if let Some(style) = &mut span.style {
            drop_unsafe("fontColor", &mut style.font_color, is_safe_color);
            drop_unsafe("fontFamily", &mut style.font_family, is_safe_font_family);
        }
    }
}

fn normalize_card_content(content: &mut crate::dto::CardContent) {
    // Save sanitized original text as a validation anchor. An empty anchor means the LLM
    // omitted `text`, so skip validation and derive from spans (backward-compatible path).
    let anchor = sanitize_text(&content.text).trim().to_owned();

    if let Some(spans) = &mut content.rich_text {
        for span in spans.iter_mut() {
            span.text = sanitize_text(&span.text);
        }
        strip_span_boundary_markers(spans);
        spans.retain(|s| !s.text.is_empty());
        if spans.is_empty() {
            content.rich_text = None;
        }
    }
    sanitize_styles(content);

    if let Some(spans) = &content.rich_text
        && !spans.is_empty()
    {
        let derived: String = spans.iter().map(|s| s.text.as_str()).collect();
        if !anchor.is_empty() && derived.trim() != anchor {
            // Span concatenation doesn't match the provided text — LLM corrupted them.
            // Discard richText and fall back to plain text.
            warn!(
                anchor_chars = anchor.chars().count(),
                derived_chars = derived.chars().count(),
                span_count = spans.len(),
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
            Styling goes under a nested `style` object on the span, never as flat fields, e.g. \
            {\"text\":\"gracias.\",\"style\":{\"bold\":true,\"fontColor\":\"#27AE60\"}}. \
            Use fontFamily='monospace' for code, bold=true for key terms, \
            fontColor='#E74C3C' for warnings, '#27AE60' for correct answers. \
            If no styling needed, omit rich_text entirely and just set text.")]
    pub async fn generate_card(
        &self,
        Parameters(mut p): Parameters<GenerateCardParams>,
    ) -> Result<CallToolResult, ErrorData> {
        for content in [&p.face, &p.back] {
            if let Err(e) = check_card_content(content) {
                return Ok(err_result(e));
            }
        }
        normalize_card_content(&mut p.face);
        normalize_card_content(&mut p.back);
        debug!(
            face_chars = p.face.text.chars().count(),
            back_chars = p.back.text.chars().count(),
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
            The server validates this and discards richText if spans disagree with text. \
            Styling goes under a nested `style` object on the span, e.g. \
            {\"text\":\"gracias.\",\"style\":{\"bold\":true,\"fontColor\":\"#27AE60\"}}. \
            The returned catalog's cardCount reflects the cards created in this call. \
            At most 200 cards per call; split larger sets into several calls."
    )]
    pub async fn generate_catalog_with_cards(
        &self,
        Parameters(mut p): Parameters<GenerateCatalogWithCardsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(e) = check_batch_size(p.cards.len()) {
            return Ok(err_result(e));
        }
        if let Some(ref image_id) = p.image_id
            && let Err(e) = parse_uuid(image_id)
        {
            return Ok(err_result(format!("Invalid image_id: {e}")));
        }
        for card in &p.cards {
            for content in [&card.face, &card.back] {
                if let Err(e) = check_card_content(content) {
                    return Ok(err_result(e));
                }
            }
        }
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
            Ok(mut resp) => {
                // POST /catalogs/with-cards snapshots the catalog before inserting cards (#40);
                // a freshly created catalog holds exactly the cards just created.
                resp.catalog.card_count =
                    Some(i64::try_from(resp.cards_created).unwrap_or(i64::MAX));
                ok_json(&resp)
            }
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
            The server validates this and discards richText if spans disagree with text. \
            Styling goes under a nested `style` object on the span, e.g. \
            {\"text\":\"gracias.\",\"style\":{\"bold\":true,\"fontColor\":\"#27AE60\"}}. \
            At most 200 cards per call; split larger sets into several calls."
    )]
    pub async fn generate_cards(
        &self,
        Parameters(mut p): Parameters<GenerateCardsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(e) = check_batch_size(p.cards.len()) {
            return Ok(err_result(e));
        }
        let catalog_id = match parse_uuid(&p.catalog_id) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        for card in &p.cards {
            for content in [&card.face, &card.back] {
                if let Err(e) = check_card_content(content) {
                    return Ok(err_result(e));
                }
            }
        }
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
        let total = p.cards.len();
        let mut created_cards = Vec::with_capacity(total);
        for card in p.cards {
            let req = CreateCardRequest {
                catalog_id: Some(catalog_id),
                face: card.face,
                back: card.back,
            };
            match self.client.create_card(&req).await {
                Ok(created) => created_cards.push(created),
                Err(e) => {
                    if created_cards.is_empty() {
                        return Ok(err_result(e));
                    }
                    let ids: Vec<String> = created_cards.iter().map(|c| c.id.to_string()).collect();
                    let sep = if e.to_string().ends_with('.') {
                        " "
                    } else {
                        ". "
                    };
                    let n = created_cards.len();
                    let advice =
                        if matches!(e, ApiError::Unauthorized | ApiError::QuotaExceeded { .. }) {
                            "do not retry until the error above is resolved".to_string()
                        } else {
                            format!("retry only the remaining cards starting at index {n}")
                        };
                    return Ok(err_result(format!(
                        "{e}{sep}{n} of {total} cards were created before this failure and \
                         were NOT rolled back (ids: {}); {advice}.",
                        ids.join(", ")
                    )));
                }
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
    fn test_batch_tool_descriptions_state_max_batch_cards() {
        use crate::tools::generate::MAX_BATCH_CARDS;
        let server = EngramoMcpServer::new(
            EngramoClient::new("http://localhost", "engramo_test"),
            false,
        );
        let needle = format!("At most {MAX_BATCH_CARDS} cards per call");
        for name in ["generate_cards", "generate_catalog_with_cards"] {
            let tool = server
                .tool_router
                .list_all()
                .into_iter()
                .find(|t| t.name == name)
                .expect(name);
            let desc = tool.description.as_deref().unwrap_or("");
            assert!(
                desc.contains(&needle),
                "{name} description out of sync with MAX_BATCH_CARDS"
            );
        }
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
    fn test_learning_path_catalog_tools_are_registered_on_the_real_server() {
        // Same regression class as `test_upload_media_is_registered_on_the_real_server`:
        // `tools/learning_paths.rs`'s `LearningPathTools` struct is a separate, unused
        // scaffold — the tools actually served to clients are the ones registered directly on
        // `EngramoMcpServer` below.
        for flag in [false, true] {
            let server =
                EngramoMcpServer::new(EngramoClient::new("http://localhost", "engramo_test"), flag);
            let names = tool_names(&server);
            for name in [
                "add_catalog_to_learning_path",
                "remove_catalog_from_learning_path",
                "update_learning_path",
            ] {
                assert!(
                    names.contains(&name.to_string()),
                    "{name} missing: {names:?}"
                );
            }
        }
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
    fn test_update_card_and_list_media_descriptions_document_catalogs_and_get_card_on_the_real_server()
     {
        // Regression guard for F1/F2: `update_card`'s and `list_media`'s docs must reach the
        // real, registered tool — not just their `tools/*.rs` scaffold — so the wording can't
        // silently drift back to the undocumented scaffold text.
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let tools = server.tool_router.list_all();

        let update_card = tools
            .iter()
            .find(|t| t.name == "update_card")
            .expect("update_card not registered");
        let update_card_description = update_card.description.clone().unwrap_or_default();
        assert!(
            update_card_description.contains("REPLACES"),
            "{update_card_description}"
        );
        assert!(
            update_card_description.contains("has no `catalogs` key")
                && update_card_description.contains("call `get_card` first"),
            "{update_card_description}"
        );
        let catalog_ids_schema = serde_json::to_string(&update_card.input_schema)
            .expect("update_card input schema serializes");
        assert!(
            catalog_ids_schema.contains("has no `catalogs` key"),
            "{catalog_ids_schema}"
        );

        let get_card = tools
            .iter()
            .find(|t| t.name == "get_card")
            .expect("get_card not registered");
        let get_card_description = get_card.description.clone().unwrap_or_default();
        assert!(
            get_card_description.contains("omitted when the API does not report"),
            "{get_card_description}"
        );

        let list_media = tools
            .iter()
            .find(|t| t.name == "list_media")
            .expect("list_media not registered");
        let list_media_description = list_media.description.clone().unwrap_or_default();
        assert!(
            list_media_description.contains("get_card"),
            "{list_media_description}"
        );
    }

    #[test]
    fn test_catalog_visibility_params_document_unlisted_on_the_real_server() {
        // Regression guard for #61: `update_catalog`'s and `generate_catalog_with_cards`'s
        // `visibility` params must list all three values the API accepts — including
        // `unlisted` — and note the role limit, not just say "'public' or 'private'".
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let tools = server.tool_router.list_all();

        for name in ["update_catalog", "generate_catalog_with_cards"] {
            let tool = tools
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} not registered"));
            let description = tool
                .input_schema
                .get("properties")
                .and_then(|p| p.get("visibility"))
                .and_then(|v| v.get("description"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("{name}: visibility param has no description"));
            for value in ["'public'", "'private'", "'unlisted'"] {
                assert!(description.contains(value), "{name}: {description}");
            }
            assert!(description.contains("permission"), "{name}: {description}");
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

    #[test]
    fn test_search_learning_paths_is_registered_on_the_real_server() {
        // Regression guard for issue #49: the handler tests call the method directly, so assert the
        // tool is actually reachable through the router a real MCP client sees.
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let tools = server.tool_router.list_all();
        let tool = tools
            .iter()
            .find(|t| t.name == "search_learning_paths")
            .expect("search_learning_paths not registered");
        let description = tool.description.clone().unwrap_or_default();
        assert!(description.contains("learning paths"), "{description}");
        assert!(description.contains("search_global"), "{description}");
        // Input schema is SearchParams — the query field must be exposed.
        let schema = serde_json::to_string(&tool.input_schema).unwrap();
        assert!(schema.contains("query"), "{schema}");
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

    /// Regression for F2 (issue #38 follow-up): `tools/media.rs`'s `MediaTools` is a
    /// separate, unused scaffold — assert the `name`/`length` mapping reaches the tool
    /// output through the real, registered `list_media` handler, not just the scaffold.
    #[tokio::test]
    async fn test_list_media_output_includes_name_and_length_on_real_server() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "id": "22516154-0000-0000-0000-000000000001",
                    "name": "mcp-test.png",
                    "created_at": "2026-09-01T00:00:00Z",
                    "content_type": "image/png",
                    "media_type": "image",
                    "length": 2048
                }],
                "nextCursor": null
            })))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .list_media(Parameters(ListMediaParams {
                media_type: None,
                limit: None,
                cursor: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(first_text(&result)).unwrap();
        let item = &v["data"][0];
        assert_eq!(item["name"], "mcp-test.png", "{v}");
        assert_eq!(item["length"], 2048, "{v}");
    }

    /// Regression for F1 (issue #39 follow-up): `tools/cards.rs`'s `CardTools` is a
    /// separate, unused scaffold — assert `catalogs` memberships reach the tool output
    /// through the real, registered `list_cards`/`get_card`/`update_card` handlers.
    #[tokio::test]
    async fn test_list_cards_catalogs_reach_tool_output_on_real_server() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let catalog_id = "00000000-0000-0000-0000-000000000099";
        let card_id = "00000000-0000-0000-0000-000000000001";
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{catalog_id}/cards")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "id": card_id,
                    "version": 1,
                    "face": {"text": "Q?"},
                    "back": {"text": "A."},
                    "orderNumber": 1,
                    "catalogs": [{"id": catalog_id, "name": "Spanish"}]
                }],
                "nextCursor": null
            })))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .list_cards(Parameters(ListCardsParams {
                catalog_id: catalog_id.to_string(),
                limit: None,
                cursor: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(first_text(&result)).unwrap();
        assert_eq!(v["data"][0]["catalogs"][0]["id"], catalog_id, "{v}");
        assert_eq!(v["data"][0]["catalogs"][0]["name"], "Spanish", "{v}");
    }

    #[tokio::test]
    async fn test_get_card_tool_output_includes_catalogs_on_real_server() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let catalog_id = "00000000-0000-0000-0000-000000000099";
        let card_id = "00000000-0000-0000-0000-000000000001";
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": card_id,
                "version": 1,
                "face": {"text": "Q?"},
                "back": {"text": "A."},
                "orderNumber": 1,
                "catalogs": [{"id": catalog_id, "name": "Spanish"}]
            })))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .get_card(Parameters(GetCardParams {
                card_id: card_id.to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(first_text(&result)).unwrap();
        assert_eq!(v["catalogs"][0]["id"], catalog_id, "{v}");
        assert_eq!(v["catalogs"][0]["name"], "Spanish", "{v}");
    }

    #[tokio::test]
    async fn test_update_card_tool_output_includes_catalogs_on_real_server() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let catalog_id = "00000000-0000-0000-0000-000000000099";
        let card_id = "00000000-0000-0000-0000-000000000001";
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": card_id,
                "version": 2,
                "face": {"text": "Updated"},
                "back": {"text": "A."},
                "catalogs": [{"id": catalog_id, "name": "Spanish"}]
            })))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: None,
                back: None,
                catalog_ids: vec![catalog_id.to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(first_text(&result)).unwrap();
        assert_eq!(v["catalogs"][0]["id"], catalog_id, "{v}");
        assert_eq!(v["catalogs"][0]["name"], "Spanish", "{v}");
    }

    /// Regression for issue #64: `update_card` with `catalog_ids: []` must be rejected with a
    /// clear validation error and make NO HTTP call at all — not even the `get_card` merge
    /// fetch — because the API accepts an empty list silently and moves the card into the
    /// user's default catalog ("My Catalog") instead of removing it from every catalog.
    #[tokio::test]
    async fn test_update_card_empty_catalog_ids_rejected_without_http_call_on_real_server() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let card_id = "00000000-0000-0000-0000-000000000001";
        let card_json = serde_json::json!({
            "id": card_id,
            "version": 1,
            "face": {"text": "Q?"},
            "back": {"text": "A."},
            "orderNumber": 1
        });
        // Mounted but must receive zero requests: neither the GET merge fetch nor the PATCH
        // may fire once `catalog_ids` fails validation.
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json.clone()))
            .expect(0)
            .mount(&mock_server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json))
            .expect(0)
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: Some(CardContent::plain("x")),
                back: None,
                catalog_ids: vec![],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false), "{result:?}");
        assert!(
            first_text(&result).contains("at least one catalog"),
            "{result:?}"
        );
    }

    /// `update_card` with an invalid catalog UUID and `face` set must fail validation before the
    /// `get_card` merge fetch, so the model sees the actionable UUID error.
    #[tokio::test]
    async fn test_update_card_invalid_catalog_uuid_rejected_without_http_call() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let card_id = "00000000-0000-0000-0000-000000000001";
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: Some(CardContent::plain("x")),
                back: None,
                catalog_ids: vec!["bad".into()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false), "{result:?}");
        assert!(
            first_text(&result).to_lowercase().contains("uuid"),
            "{result:?}"
        );
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

    /// Regression guard for issue #49's description change: `search_global` now says
    /// learning paths *may* be included (with item_type 'learning_path') depending on
    /// API version, and points callers who want them specifically at
    /// `search_learning_paths`. Nothing else pins this wording, so that pointer could
    /// silently disappear.
    #[test]
    fn test_search_global_description_points_at_search_learning_paths() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let tools = server.tool_router.list_all();
        let tool = tools.iter().find(|t| t.name == "search_global").unwrap();
        let description = tool.description.clone().unwrap_or_default();
        assert!(
            description.contains("search_learning_paths"),
            "{description}"
        );
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

    /// issue #49: drives the `search_learning_paths` tool handler's success arm against
    /// `GET /search/learning-paths`.
    #[tokio::test]
    async fn test_search_learning_paths_ok_returns_learning_path_json() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let path_id = "00000000-0000-0000-0000-000000000001";
        Mock::given(method("GET"))
            .and(path("/search/learning-paths"))
            .and(query_param("q", "spanish"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "id": path_id,
                    "name": "Spanish Basics",
                    "description": "Beginner path",
                    "version": 1
                }])),
            )
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .search_learning_paths(Parameters(SearchParams {
                query: "spanish".to_string(),
            }))
            .await
            .unwrap();
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(first_text(&result)).unwrap();
        assert_eq!(v[0]["id"], path_id);
        assert_eq!(v.as_array().unwrap().len(), 1);
    }

    /// issue #49: drives the `search_learning_paths` tool handler's error arm — mirrors
    /// `test_search_global_unauthorized_returns_error` / `test_search_catalogs_unauthorized_returns_error`.
    #[tokio::test]
    async fn test_search_learning_paths_unauthorized_returns_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/learning-paths"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .search_learning_paths(Parameters(SearchParams {
                query: "spanish".to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("Unauthorized"), "{result:?}");
    }

    /// issue #49: the same oversized-query contract as `search_global` (finding F2) — an
    /// over-long `q` must surface as `is_error: true`, never a raw `Err`, and never reach the
    /// backend.
    #[tokio::test]
    async fn test_search_learning_paths_oversized_query_returns_is_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/learning-paths"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let long_query = "a".repeat(513);
        let result = server
            .search_learning_paths(Parameters(SearchParams { query: long_query }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("too long"), "{result:?}");
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

    #[tokio::test]
    async fn test_add_catalog_to_learning_ok_and_invalid_uuid() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let catalog_id = "00000000-0000-0000-0000-000000000001";
        Mock::given(method("POST"))
            .and(path(format!("/learning/catalogs/{catalog_id}")))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .add_catalog_to_learning(Parameters(AddCatalogToLearningParams {
                catalog_id: catalog_id.to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        assert_eq!(first_text(&result), "Catalog added to learning.");

        let before = mock_server
            .received_requests()
            .await
            .unwrap_or_default()
            .len();
        let result = server
            .add_catalog_to_learning(Parameters(AddCatalogToLearningParams {
                catalog_id: "bad".to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        assert_eq!(
            mock_server
                .received_requests()
                .await
                .unwrap_or_default()
                .len(),
            before
        );
    }

    #[tokio::test]
    async fn test_create_learning_path_ok() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/learning-paths"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "name": "Spanish A1",
                "description": null,
                "version": 1
            })))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .create_learning_path(Parameters(CreateLearningPathParams {
                name: "Spanish A1".to_string(),
                description: None,
                catalog_ids: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        assert!(first_text(&result).contains("Spanish A1"), "{result:?}");
    }

    #[tokio::test]
    async fn test_create_learning_path_forbidden_returns_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/learning-paths"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .create_learning_path(Parameters(CreateLearningPathParams {
                name: "Spanish A1".to_string(),
                description: None,
                catalog_ids: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_deactivate_learning_path_ok_and_invalid_uuid() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let path_id = "00000000-0000-0000-0000-000000000001";
        Mock::given(method("POST"))
            .and(path(format!("/learning-paths/{path_id}/deactivate")))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .deactivate_learning_path(Parameters(ActivateLearningPathParams {
                path_id: path_id.to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        assert_eq!(first_text(&result), "Learning path deactivated.");

        let before = mock_server
            .received_requests()
            .await
            .unwrap_or_default()
            .len();
        let result = server
            .deactivate_learning_path(Parameters(ActivateLearningPathParams {
                path_id: "bad".to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        assert_eq!(
            mock_server
                .received_requests()
                .await
                .unwrap_or_default()
                .len(),
            before
        );
    }

    #[tokio::test]
    async fn test_deactivate_learning_path_not_found_returns_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let path_id = "00000000-0000-0000-0000-000000000001";
        Mock::given(method("POST"))
            .and(path(format!("/learning-paths/{path_id}/deactivate")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .deactivate_learning_path(Parameters(ActivateLearningPathParams {
                path_id: path_id.to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_add_catalog_to_learning_path_ok_and_invalid_uuid() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let path_id = "00000000-0000-0000-0000-000000000001";
        let catalog_id = "00000000-0000-0000-0000-000000000002";
        Mock::given(method("POST"))
            .and(path(format!(
                "/learning-paths/{path_id}/catalogs/{catalog_id}"
            )))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"status": "catalog added"})),
            )
            .mount(&mock_server)
            .await;

        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let result = server
            .add_catalog_to_learning_path(Parameters(AddCatalogToLearningPathParams {
                path_id: path_id.to_string(),
                catalog_id: catalog_id.to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        assert_eq!(first_text(&result), "Catalog added to learning path.");

        // An invalid catalog UUID must never reach the HTTP client.
        let before = mock_server.received_requests().await.unwrap().len();
        let result = server
            .add_catalog_to_learning_path(Parameters(AddCatalogToLearningPathParams {
                path_id: path_id.to_string(),
                catalog_id: "not-a-uuid".to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        assert!(first_text(&result).contains("Invalid UUID"), "{result:?}");
        assert_eq!(mock_server.received_requests().await.unwrap().len(), before);
    }

    /// Only the invalid-`catalog_id` branch was covered above; `path_id` is validated first
    /// and must be exercised too.
    #[tokio::test]
    async fn test_add_catalog_to_learning_path_invalid_path_id_makes_no_request() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&mock_server)
            .await;

        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .add_catalog_to_learning_path(Parameters(AddCatalogToLearningPathParams {
                    path_id: "not-a-uuid".to_string(),
                    catalog_id: "00000000-0000-0000-0000-000000000002".to_string(),
                }))
                .await
                .unwrap();
        assert!(result.is_error.unwrap_or(false), "{result:?}");
        assert!(first_text(&result).contains("Invalid UUID"), "{result:?}");
        // `.expect(0)` on the mock is verified when mock_server drops.
    }

    #[tokio::test]
    async fn test_add_catalog_to_learning_path_non_forbidden_error_skips_catalog_lookup() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let catalog_id = "00000000-0000-0000-0000-000000000002";
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/learning-paths/{path_id}/catalogs/{catalog_id}"
            )))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": format!("Learning path with id {path_id} not found")
            })))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{catalog_id}")))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&mock_server)
            .await;

        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .add_catalog_to_learning_path(Parameters(AddCatalogToLearningPathParams {
                    path_id: path_id.to_string(),
                    catalog_id: catalog_id.to_string(),
                }))
                .await
                .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        let text = first_text(&result);
        assert!(text.contains("Not found"), "{text}");
        assert!(text.contains(path_id), "{text}");
    }

    #[tokio::test]
    async fn test_add_catalog_to_learning_path_forbidden_missing_catalog_empty_body_names_catalog()
    {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let catalog_id = "00000000-0000-0000-0000-000000000002";
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/learning-paths/{path_id}/catalogs/{catalog_id}"
            )))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{catalog_id}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .add_catalog_to_learning_path(Parameters(AddCatalogToLearningPathParams {
                    path_id: path_id.to_string(),
                    catalog_id: catalog_id.to_string(),
                }))
                .await
                .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        let text = first_text(&result);
        assert!(text.contains("Not found"), "{text}");
        assert!(text.contains(catalog_id), "{text}");
    }

    #[tokio::test]
    async fn test_create_learning_path_forbidden_missing_catalog_reported_not_found() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let bad_catalog = "00000000-0000-0000-0000-000000000003";
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/learning-paths"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": path_id, "name": "Spanish A1", "version": 1
            })))
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/learning-paths/{path_id}/catalogs/{bad_catalog}"
            )))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{bad_catalog}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .create_learning_path(Parameters(CreateLearningPathParams {
                    name: "Spanish A1".to_string(),
                    description: None,
                    catalog_ids: Some(vec![bad_catalog.to_string()]),
                }))
                .await
                .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(first_text(&result)).unwrap();
        let err = v["catalogs_failed"][0]["error"].as_str().unwrap();
        assert!(err.contains("Not found"), "{err}");
    }

    #[tokio::test]
    async fn test_add_catalog_to_learning_path_forbidden_with_existing_catalog_stays_forbidden() {
        // #62: a 403 on the add itself, but the disambiguating `get_catalog` lookup succeeds
        // (the catalog exists and is readable, so the 403 is about the path or catalog-level
        // policy) — the original Forbidden must pass through unchanged.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let catalog_id = "00000000-0000-0000-0000-000000000002";
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/learning-paths/{path_id}/catalogs/{catalog_id}"
            )))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{catalog_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": catalog_id, "name": "Spanish", "version": 1
            })))
            .mount(&mock_server)
            .await;

        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .add_catalog_to_learning_path(Parameters(AddCatalogToLearningPathParams {
                    path_id: path_id.to_string(),
                    catalog_id: catalog_id.to_string(),
                }))
                .await
                .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(
            first_text(&result).contains("Permission denied"),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn test_add_catalog_to_learning_path_forbidden_with_missing_catalog_becomes_not_found() {
        // #62: a 403 on the add, and the disambiguating `get_catalog` lookup reports the
        // catalog doesn't exist — the caller should see `Not found`, not a bare `Forbidden`.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let catalog_id = "00000000-0000-0000-0000-000000000002";
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/learning-paths/{path_id}/catalogs/{catalog_id}"
            )))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{catalog_id}")))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": format!("Catalog with id {catalog_id} not found")
            })))
            .mount(&mock_server)
            .await;

        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .add_catalog_to_learning_path(Parameters(AddCatalogToLearningPathParams {
                    path_id: path_id.to_string(),
                    catalog_id: catalog_id.to_string(),
                }))
                .await
                .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        let text = first_text(&result);
        assert!(text.contains("Not found"), "{text}");
        assert!(!text.contains("Permission denied"), "{text}");
        assert!(text.contains(catalog_id), "{text}");
    }

    #[tokio::test]
    async fn test_add_catalog_to_learning_path_forbidden_stays_forbidden_when_catalog_lookup_also_forbidden()
     {
        // #62: the disambiguating `get_catalog` lookup itself comes back 403 (e.g. a catalog
        // the caller can't even see exists) rather than 404 — the original Forbidden from the
        // add must still be what's reported, never a fabricated Not found.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let catalog_id = "00000000-0000-0000-0000-000000000002";
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/learning-paths/{path_id}/catalogs/{catalog_id}"
            )))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{catalog_id}")))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;

        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .add_catalog_to_learning_path(Parameters(AddCatalogToLearningPathParams {
                    path_id: path_id.to_string(),
                    catalog_id: catalog_id.to_string(),
                }))
                .await
                .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(
            first_text(&result).contains("Permission denied"),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn test_remove_catalog_from_learning_path_ok_and_not_found() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let catalog_id = "00000000-0000-0000-0000-000000000002";

        let mock_server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!(
                "/learning-paths/{path_id}/catalogs/{catalog_id}"
            )))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"status": "catalog removed"})),
            )
            .mount(&mock_server)
            .await;
        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .remove_catalog_from_learning_path(Parameters(
                    RemoveCatalogFromLearningPathParams {
                        path_id: path_id.to_string(),
                        catalog_id: catalog_id.to_string(),
                    },
                ))
                .await
                .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        assert_eq!(first_text(&result), "Catalog removed from learning path.");

        let missing = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!(
                "/learning-paths/{path_id}/catalogs/{catalog_id}"
            )))
            .respond_with(ResponseTemplate::new(404))
            .mount(&missing)
            .await;
        // The path probe also 404s, so the original Not found is surfaced.
        Mock::given(method("GET"))
            .and(path(format!("/learning-paths/{path_id}")))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&missing)
            .await;
        let result =
            EngramoMcpServer::new(EngramoClient::new(missing.uri(), "engramo_test"), false)
                .remove_catalog_from_learning_path(Parameters(
                    RemoveCatalogFromLearningPathParams {
                        path_id: path_id.to_string(),
                        catalog_id: catalog_id.to_string(),
                    },
                ))
                .await
                .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("Not found"), "{result:?}");
    }

    #[tokio::test]
    async fn test_remove_catalog_from_learning_path_not_member_succeeds_with_hint() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let catalog_id = "00000000-0000-0000-0000-000000000002";
        let mock_server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!(
                "/learning-paths/{path_id}/catalogs/{catalog_id}"
            )))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": format!("Catalog {catalog_id} is not in learning path {path_id}")
            })))
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/learning-paths/{path_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": path_id, "name": "P", "version": 1
            })))
            .expect(2)
            .mount(&mock_server)
            .await;
        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);

        for _ in 0..2 {
            let result = server
                .remove_catalog_from_learning_path(Parameters(
                    RemoveCatalogFromLearningPathParams {
                        path_id: path_id.to_string(),
                        catalog_id: catalog_id.to_string(),
                    },
                ))
                .await
                .unwrap();
            assert!(!result.is_error.unwrap_or(false), "{result:?}");
            assert_eq!(first_text(&result), "Catalog was not in learning path.");
        }
    }

    #[tokio::test]
    async fn test_remove_catalog_from_learning_path_missing_path_stays_not_found() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": "Learning path not found"
            })))
            .expect(1)
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&mock_server)
            .await;
        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .remove_catalog_from_learning_path(Parameters(
                    RemoveCatalogFromLearningPathParams {
                        path_id: "00000000-0000-0000-0000-000000000001".to_string(),
                        catalog_id: "00000000-0000-0000-0000-000000000002".to_string(),
                    },
                ))
                .await
                .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        let text = first_text(&result);
        assert!(text.contains("Not found"), "{text}");
        assert!(text.contains("Learning path not found"), "{text}");
    }

    #[tokio::test]
    async fn test_remove_catalog_from_learning_path_still_member_stays_not_found() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let catalog_id = "00000000-0000-0000-0000-000000000002";
        let mock_server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&mock_server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": path_id, "name": "P", "version": 1,
                "catalogs": [{"id": catalog_id, "name": "C", "version": 1}]
            })))
            .expect(1)
            .mount(&mock_server)
            .await;
        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .remove_catalog_from_learning_path(Parameters(
                    RemoveCatalogFromLearningPathParams {
                        path_id: path_id.to_string(),
                        catalog_id: catalog_id.to_string(),
                    },
                ))
                .await
                .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("Not found"), "{result:?}");
    }

    #[tokio::test]
    async fn test_remove_catalog_from_learning_path_non_not_found_error_skips_probe() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        for (status, needle) in [(403u16, "Permission denied"), (500, "")] {
            let mock_server = MockServer::start().await;
            Mock::given(method("DELETE"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&mock_server)
                .await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "00000000-0000-0000-0000-000000000001", "name": "P", "version": 1
                })))
                .expect(0)
                .mount(&mock_server)
                .await;
            let result =
                EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                    .remove_catalog_from_learning_path(Parameters(
                        RemoveCatalogFromLearningPathParams {
                            path_id: "00000000-0000-0000-0000-000000000001".to_string(),
                            catalog_id: "00000000-0000-0000-0000-000000000002".to_string(),
                        },
                    ))
                    .await
                    .unwrap();
            assert_eq!(result.is_error, Some(true), "{status}: {result:?}");
            assert!(first_text(&result).contains(needle), "{status}: {result:?}");
        }
    }

    #[tokio::test]
    async fn test_remove_catalog_from_learning_path_probe_error_surfaces_original_not_found() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        for probe_status in [403u16, 500] {
            let mock_server = MockServer::start().await;
            Mock::given(method("DELETE"))
                .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                    "error": "Learning path not found"
                })))
                .mount(&mock_server)
                .await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(probe_status))
                .expect(1)
                .mount(&mock_server)
                .await;
            let result =
                EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                    .remove_catalog_from_learning_path(Parameters(
                        RemoveCatalogFromLearningPathParams {
                            path_id: "00000000-0000-0000-0000-000000000001".to_string(),
                            catalog_id: "00000000-0000-0000-0000-000000000002".to_string(),
                        },
                    ))
                    .await
                    .unwrap();
            assert_eq!(result.is_error, Some(true), "{probe_status}: {result:?}");
            let text = first_text(&result);
            assert!(text.contains("Not found"), "{probe_status}: {text}");
            assert!(
                text.contains("Learning path not found"),
                "{probe_status}: {text}"
            );
            assert!(
                !text.contains("Permission denied"),
                "{probe_status}: {text}"
            );
        }
    }

    #[test]
    fn test_learning_path_catalog_tool_descriptions_state_idempotency() {
        let server = EngramoMcpServer::new(EngramoClient::new("http://unused", "t"), false);
        for name in [
            "add_catalog_to_learning_path",
            "remove_catalog_from_learning_path",
        ] {
            let tool = server
                .tool_router
                .list_all()
                .into_iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} not registered"));
            let desc = tool.description.as_deref().unwrap_or_default();
            assert!(desc.contains("Idempotent"), "{name}: {desc}");
        }
    }

    #[tokio::test]
    async fn test_remove_catalog_from_learning_path_invalid_uuid_makes_no_request() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&mock_server)
            .await;
        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let valid = "00000000-0000-0000-0000-000000000001";

        for (p, c) in [("not-a-uuid", valid), (valid, "not-a-uuid")] {
            let result = server
                .remove_catalog_from_learning_path(Parameters(
                    RemoveCatalogFromLearningPathParams {
                        path_id: p.to_string(),
                        catalog_id: c.to_string(),
                    },
                ))
                .await
                .unwrap();
            assert_eq!(result.is_error, Some(true), "{result:?}");
            assert!(first_text(&result).contains("Invalid UUID"), "{result:?}");
        }
        assert!(mock_server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_update_learning_path_ok_and_invalid_uuid_makes_no_request() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let mock_server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/learning-paths/{path_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": path_id, "name": "Renamed", "version": 2
            })))
            .mount(&mock_server)
            .await;
        let server =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false);
        let params = |path_id: &str| UpdateLearningPathParams {
            path_id: path_id.to_string(),
            name: Some("Renamed".to_string()),
            description: None,
            tags: None,
            visibility: None,
            version: 1,
        };
        let result = server
            .update_learning_path(Parameters(params(path_id)))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        assert!(first_text(&result).contains("Renamed"), "{result:?}");

        let before = mock_server.received_requests().await.unwrap().len();
        let result = server
            .update_learning_path(Parameters(params("not-a-uuid")))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("Invalid UUID"), "{result:?}");
        assert_eq!(mock_server.received_requests().await.unwrap().len(), before);
    }

    #[tokio::test]
    async fn test_update_learning_path_conflict_returns_actionable_message() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let mock_server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/learning-paths/{path_id}")))
            .respond_with(ResponseTemplate::new(409))
            .mount(&mock_server)
            .await;

        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .update_learning_path(Parameters(UpdateLearningPathParams {
                    path_id: path_id.to_string(),
                    name: Some("New".to_string()),
                    description: None,
                    tags: None,
                    visibility: None,
                    version: 1,
                }))
                .await
                .unwrap();
        assert!(result.is_error.unwrap_or(false));
        assert!(
            first_text(&result).contains("Fetch the latest version"),
            "{result:?}"
        );
    }

    /// Regression for issue #53: `create_learning_path` with `catalog_ids` must add each
    /// catalog in its own request (non-atomic) and report per-id failures rather than
    /// silently dropping them or failing the whole call.
    #[tokio::test]
    async fn test_create_learning_path_with_catalog_ids_reports_partial_failure() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let ok_catalog = "00000000-0000-0000-0000-000000000002";
        let bad_catalog = "00000000-0000-0000-0000-000000000003";

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/learning-paths"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": path_id, "name": "Spanish A1", "version": 1
            })))
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/learning-paths/{path_id}/catalogs/{ok_catalog}"
            )))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"status": "catalog added"})),
            )
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/learning-paths/{path_id}/catalogs/{bad_catalog}"
            )))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .create_learning_path(Parameters(CreateLearningPathParams {
                    name: "Spanish A1".to_string(),
                    description: None,
                    catalog_ids: Some(vec![ok_catalog.to_string(), bad_catalog.to_string()]),
                }))
                .await
                .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let text = first_text(&result);
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["catalogs_added"], serde_json::json!([ok_catalog]));
        assert_eq!(v["catalogs_failed"][0]["catalog_id"], bad_catalog);
    }

    /// When every catalog add succeeds, `catalogs_failed` must be empty, every id must be in
    /// `catalogs_added` in input order, and the created path's own fields must survive being
    /// flattened into the response.
    #[tokio::test]
    async fn test_create_learning_path_with_catalog_ids_all_succeed() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let path_id = "00000000-0000-0000-0000-000000000001";
        let catalog_1 = "00000000-0000-0000-0000-000000000002";
        let catalog_2 = "00000000-0000-0000-0000-000000000003";

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/learning-paths"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": path_id, "name": "Spanish A1", "version": 1
            })))
            .mount(&mock_server)
            .await;
        for catalog_id in [catalog_1, catalog_2] {
            Mock::given(method("POST"))
                .and(path(format!(
                    "/learning-paths/{path_id}/catalogs/{catalog_id}"
                )))
                .respond_with(
                    ResponseTemplate::new(201)
                        .set_body_json(serde_json::json!({"status": "catalog added"})),
                )
                .mount(&mock_server)
                .await;
        }

        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .create_learning_path(Parameters(CreateLearningPathParams {
                    name: "Spanish A1".to_string(),
                    description: None,
                    catalog_ids: Some(vec![catalog_1.to_string(), catalog_2.to_string()]),
                }))
                .await
                .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let text = first_text(&result);
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            v["catalogs_added"],
            serde_json::json!([catalog_1, catalog_2])
        );
        assert_eq!(v["catalogs_failed"], serde_json::json!([]));
        assert_eq!(v["id"], path_id);
        assert_eq!(v["name"], "Spanish A1");
        assert_eq!(v["version"], 1);
    }

    /// Regression guard: `create_learning_path`'s early return after path creation fails must
    /// keep the add-catalogs loop from ever running — otherwise it would fire
    /// `POST /learning-paths/{id}/catalogs/{cid}` against a path that doesn't exist.
    #[tokio::test]
    async fn test_create_learning_path_with_catalog_ids_creation_failure_adds_nothing() {
        use wiremock::matchers::{method, path, path_regex};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/learning-paths"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/learning-paths/.+/catalogs/.+$"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&mock_server)
            .await;

        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .create_learning_path(Parameters(CreateLearningPathParams {
                    name: "Spanish A1".to_string(),
                    description: None,
                    catalog_ids: Some(vec!["00000000-0000-0000-0000-000000000002".to_string()]),
                }))
                .await
                .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(
            first_text(&result).contains("Permission denied"),
            "{result:?}"
        );
        // `.expect(0)` on the catalogs mock is verified when mock_server drops.
    }

    /// An invalid catalog id is rejected before the path is created, so no empty path leaks.
    #[tokio::test]
    async fn test_create_learning_path_invalid_catalog_id_creates_nothing() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&mock_server)
            .await;

        let result =
            EngramoMcpServer::new(EngramoClient::new(mock_server.uri(), "engramo_test"), false)
                .create_learning_path(Parameters(CreateLearningPathParams {
                    name: "Spanish A1".to_string(),
                    description: None,
                    catalog_ids: Some(vec!["not-a-uuid".to_string()]),
                }))
                .await
                .unwrap();
        assert!(result.is_error.unwrap_or(false), "{result:?}");
        assert!(first_text(&result).contains("not-a-uuid"));
    }

    const TEST_ID: &str = "00000000-0000-0000-0000-000000000001";

    fn mock_server_for(uri: &str) -> EngramoMcpServer {
        EngramoMcpServer::new(EngramoClient::new(uri, "engramo_test"), false)
    }

    #[tokio::test]
    async fn test_update_catalog_forwards_unlisted_visibility_on_wire() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/catalogs/{TEST_ID}")))
            .and(body_partial_json(
                serde_json::json!({"visibility": "unlisted", "version": 1}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": TEST_ID, "name": "X", "version": 2, "visibility": "unlisted"
            })))
            .expect(1)
            .mount(&mock_server)
            .await;
        let server = mock_server_for(&mock_server.uri());
        let result = server
            .update_catalog(Parameters(UpdateCatalogParams {
                catalog_id: TEST_ID.to_string(),
                name: None,
                description: None,
                tags: None,
                visibility: Some("unlisted".to_string()),
                version: 1,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
    }

    #[tokio::test]
    async fn test_get_catalog_ok_and_not_found() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{TEST_ID}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": TEST_ID, "name": "Rust", "version": 1
            })))
            .mount(&mock_server)
            .await;
        let result = mock_server_for(&mock_server.uri())
            .get_catalog(Parameters(GetCatalogParams {
                catalog_id: TEST_ID.to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        assert!(first_text(&result).contains("Rust"), "{result:?}");

        let missing = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{TEST_ID}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&missing)
            .await;
        let result = mock_server_for(&missing.uri())
            .get_catalog(Parameters(GetCatalogParams {
                catalog_id: TEST_ID.to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("Not found"), "{result:?}");
    }

    #[tokio::test]
    async fn test_update_catalog_ok_and_invalid_uuid_makes_no_request() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/catalogs/{TEST_ID}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": TEST_ID, "name": "Renamed", "version": 2
            })))
            .mount(&mock_server)
            .await;
        let server = mock_server_for(&mock_server.uri());
        let params = |catalog_id: &str| UpdateCatalogParams {
            catalog_id: catalog_id.to_string(),
            name: Some("Renamed".to_string()),
            description: None,
            tags: None,
            visibility: None,
            version: 1,
        };
        let result = server
            .update_catalog(Parameters(params(TEST_ID)))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        assert!(first_text(&result).contains("Renamed"), "{result:?}");

        let before = mock_server.received_requests().await.unwrap().len();
        let result = server
            .update_catalog(Parameters(params("not-a-uuid")))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("Invalid UUID"), "{result:?}");
        assert_eq!(mock_server.received_requests().await.unwrap().len(), before);
    }

    #[tokio::test]
    async fn test_update_catalog_forbidden_visibility_returns_actionable_hint() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/catalogs/{TEST_ID}")))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(serde_json::json!({"error": "Forbidden"})),
            )
            .mount(&mock_server)
            .await;
        let server = mock_server_for(&mock_server.uri());
        let result = server
            .update_catalog(Parameters(UpdateCatalogParams {
                catalog_id: TEST_ID.to_string(),
                name: None,
                description: None,
                tags: None,
                visibility: Some("public".to_string()),
                version: 1,
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        let text = first_text(&result);
        assert!(text.contains("Permission denied: Forbidden ("), "{text}");
        assert!(text.contains("unlisted"), "{text}");
    }

    #[tokio::test]
    async fn test_delete_catalog_forbidden_and_invalid_uuid() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!("/catalogs/{TEST_ID}")))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;
        let server = mock_server_for(&mock_server.uri());
        let result = server
            .delete_catalog(Parameters(DeleteCatalogParams {
                catalog_id: TEST_ID.to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(
            first_text(&result).contains("Permission denied"),
            "{result:?}"
        );

        let before = mock_server.received_requests().await.unwrap().len();
        let result = server
            .delete_catalog(Parameters(DeleteCatalogParams {
                catalog_id: "bad".to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert_eq!(mock_server.received_requests().await.unwrap().len(), before);
    }

    #[tokio::test]
    async fn test_add_card_to_learning_ok_and_not_found() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/learning/cards/{TEST_ID}")))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;
        let result = mock_server_for(&mock_server.uri())
            .add_card_to_learning(Parameters(AddCardToLearningParams {
                card_id: TEST_ID.to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        assert_eq!(first_text(&result), "Card added to learning.");

        let missing = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/learning/cards/{TEST_ID}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&missing)
            .await;
        let result = mock_server_for(&missing.uri())
            .add_card_to_learning(Parameters(AddCardToLearningParams {
                card_id: TEST_ID.to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("Not found"), "{result:?}");
    }

    #[tokio::test]
    async fn test_card_tools_bare_forbidden_returns_actionable_hint() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let get_card_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{TEST_ID}")))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(serde_json::json!({"error": "Forbidden"})),
            )
            .mount(&get_card_server)
            .await;
        let result = mock_server_for(&get_card_server.uri())
            .get_card(Parameters(GetCardParams {
                card_id: TEST_ID.to_string(),
            }))
            .await
            .unwrap(); // must be Ok, never Err
        assert_eq!(result.is_error, Some(true), "{result:?}");
        let text = first_text(&result);
        assert!(text.starts_with("Permission denied: Forbidden ("), "{text}");
        assert!(text.contains("may not exist"), "{text}");

        let add_to_learning_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/learning/cards/{TEST_ID}")))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(serde_json::json!({"error": "Forbidden"})),
            )
            .mount(&add_to_learning_server)
            .await;
        let result = mock_server_for(&add_to_learning_server.uri())
            .add_card_to_learning(Parameters(AddCardToLearningParams {
                card_id: TEST_ID.to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        let text = first_text(&result);
        assert!(text.starts_with("Permission denied: Forbidden ("), "{text}");
        assert!(text.contains("may not exist"), "{text}");
    }

    #[test]
    fn test_card_tool_descriptions_report_not_found_and_permission_on_the_real_server() {
        // Regression guard for #73: the stale claim that a missing card is reported as a
        // permission error "not \"not found\"" must be gone; a 404 is "Not found".
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let tools = server.tool_router.list_all();
        for name in [
            "get_card",
            "update_card",
            "delete_card",
            "add_card_to_learning",
        ] {
            let tool = tools
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} not registered"));
            let description = tool.description.as_deref().unwrap_or("");
            assert!(
                !description.contains("not \"not found\""),
                "{name}: {description}"
            );
            assert!(
                description.contains("\"Not found\""),
                "{name}: {description}"
            );
            assert!(
                description.contains("permission error"),
                "{name}: {description}"
            );
        }
    }

    #[tokio::test]
    async fn test_card_and_catalog_mutators_map_404_to_not_found_and_403_to_permission() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        for status in [404u16, 403] {
            let mock = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(format!("/cards/{TEST_ID}")))
                .respond_with(ResponseTemplate::new(status))
                .expect(2..) // get_card + update_card's merge fetch
                .mount(&mock)
                .await;
            Mock::given(method("PATCH"))
                .and(path(format!("/cards/{TEST_ID}")))
                .respond_with(ResponseTemplate::new(status))
                .expect(1) // only the face-less update_card(patch) call reaches it
                .mount(&mock)
                .await;
            Mock::given(method("DELETE"))
                .and(path(format!("/catalogs/{TEST_ID}/cards/{TEST_ID}")))
                .respond_with(ResponseTemplate::new(status))
                .mount(&mock)
                .await;
            Mock::given(method("POST"))
                .and(path(format!("/learning/cards/{TEST_ID}")))
                .respond_with(ResponseTemplate::new(status))
                .mount(&mock)
                .await;
            Mock::given(method("DELETE"))
                .and(path(format!("/catalogs/{TEST_ID}")))
                .respond_with(ResponseTemplate::new(status))
                .mount(&mock)
                .await;
            Mock::given(method("PATCH"))
                .and(path(format!("/catalogs/{TEST_ID}")))
                .respond_with(ResponseTemplate::new(status))
                .mount(&mock)
                .await;
            let server = mock_server_for(&mock.uri());
            let results = vec![
                (
                    "get_card",
                    server
                        .get_card(Parameters(GetCardParams {
                            card_id: TEST_ID.to_string(),
                        }))
                        .await
                        .unwrap(),
                ),
                (
                    "update_card",
                    server
                        .update_card(Parameters(UpdateCardParams {
                            card_id: TEST_ID.to_string(),
                            // face set so the GET-before-PATCH merge fetch runs
                            face: Some(
                                serde_json::from_value(serde_json::json!({"text": "hello"}))
                                    .unwrap(),
                            ),
                            back: None,
                            catalog_ids: vec![TEST_ID.to_string()],
                            order_number: 1,
                            version: 1,
                        }))
                        .await
                        .unwrap(),
                ),
                (
                    "update_card(patch)",
                    server
                        .update_card(Parameters(UpdateCardParams {
                            card_id: TEST_ID.to_string(),
                            // no face/back: skips the merge fetch, PATCH runs
                            face: None,
                            back: None,
                            catalog_ids: vec![TEST_ID.to_string()],
                            order_number: 1,
                            version: 1,
                        }))
                        .await
                        .unwrap(),
                ),
                (
                    "delete_card",
                    server
                        .delete_card(Parameters(DeleteCardParams {
                            catalog_id: TEST_ID.to_string(),
                            card_id: TEST_ID.to_string(),
                        }))
                        .await
                        .unwrap(),
                ),
                (
                    "add_card_to_learning",
                    server
                        .add_card_to_learning(Parameters(AddCardToLearningParams {
                            card_id: TEST_ID.to_string(),
                        }))
                        .await
                        .unwrap(),
                ),
                (
                    "delete_catalog",
                    server
                        .delete_catalog(Parameters(DeleteCatalogParams {
                            catalog_id: TEST_ID.to_string(),
                        }))
                        .await
                        .unwrap(),
                ),
                (
                    "update_catalog",
                    server
                        .update_catalog(Parameters(UpdateCatalogParams {
                            catalog_id: TEST_ID.to_string(),
                            name: None,
                            description: None,
                            tags: None,
                            visibility: None,
                            version: 1,
                        }))
                        .await
                        .unwrap(),
                ),
            ];
            let expected = if status == 404 {
                "Not found"
            } else {
                "Permission denied"
            };
            for (name, result) in &results {
                assert_eq!(result.is_error, Some(true), "{name} {status}: {result:?}");
                assert!(
                    first_text(result).starts_with(expected),
                    "{name} {status}: {}",
                    first_text(result)
                );
            }
        }
    }

    #[tokio::test]
    async fn test_get_learning_path_ok_and_not_found() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/learning-paths/{TEST_ID}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": TEST_ID, "name": "Spanish A1", "description": null, "version": 1
            })))
            .mount(&mock_server)
            .await;
        let result = mock_server_for(&mock_server.uri())
            .get_learning_path(Parameters(GetLearningPathParams {
                path_id: TEST_ID.to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        assert!(first_text(&result).contains("Spanish A1"), "{result:?}");

        let missing = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/learning-paths/{TEST_ID}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&missing)
            .await;
        let result = mock_server_for(&missing.uri())
            .get_learning_path(Parameters(GetLearningPathParams {
                path_id: TEST_ID.to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("Not found"), "{result:?}");
    }

    /// Regression test for issue #63: `get_learning_path` must return `tags` and `visibility`
    /// like `list_learning_paths`/`search_learning_paths`/`update_learning_path` already do,
    /// so an agent can see the path's current values before calling `update_learning_path`.
    #[tokio::test]
    async fn test_get_learning_path_includes_tags_and_visibility() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/learning-paths/{TEST_ID}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": TEST_ID, "name": "Spanish A1", "description": null, "version": 1,
                "tags": ["mcp-test"], "visibility": "private", "catalogs": []
            })))
            .mount(&mock_server)
            .await;
        let result = mock_server_for(&mock_server.uri())
            .get_learning_path(Parameters(GetLearningPathParams {
                path_id: TEST_ID.to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let text = first_text(&result);
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["tags"], serde_json::json!(["mcp-test"]), "{text}");
        assert_eq!(v["visibility"], "private", "{text}");
    }

    /// `tags`/`visibility` must stay optional: an older backend (or any response that omits
    /// them) still decodes and the tool call still succeeds.
    #[tokio::test]
    async fn test_get_learning_path_without_tags_and_visibility_still_decodes() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/learning-paths/{TEST_ID}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": TEST_ID, "name": "Spanish A1", "description": null, "version": 1
            })))
            .mount(&mock_server)
            .await;
        let result = mock_server_for(&mock_server.uri())
            .get_learning_path(Parameters(GetLearningPathParams {
                path_id: TEST_ID.to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let text = first_text(&result);
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(v.get("tags").is_none(), "{text}");
        assert!(v.get("visibility").is_none(), "{text}");
    }

    #[tokio::test]
    async fn test_activate_learning_path_ok_and_invalid_uuid() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/learning-paths/{TEST_ID}/activate")))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;
        let server = mock_server_for(&mock_server.uri());
        let result = server
            .activate_learning_path(Parameters(ActivateLearningPathParams {
                path_id: TEST_ID.to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        assert_eq!(first_text(&result), "Learning path activated.");

        let before = mock_server.received_requests().await.unwrap().len();
        let result = server
            .activate_learning_path(Parameters(ActivateLearningPathParams {
                path_id: "bad".to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert_eq!(mock_server.received_requests().await.unwrap().len(), before);
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
        // Leading markers are only detected on non-first spans, and span[1] starts with ASCII,
        // so no boundary rule fires here. `sanitize_text` (is_emoji_char) strips '✈' from real
        // input before this function runs — see test_sanitize_strips_airplane_emoji.
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

    #[test]
    fn test_normalize_empty_rich_text_vec_keeps_plain_text() {
        let mut c = CardContent::plain("Hola\tamigo");
        c.rich_text = Some(vec![]);
        normalize_card_content(&mut c);
        assert_eq!(c.text, "Holaamigo");
        assert!(c.rich_text.is_none());
    }

    // ── review follow-ups ────────────────────────────────────────────────────

    fn content_with(text: &str, spans: Vec<RichTextSpan>) -> CardContent {
        let mut c = CardContent::plain(text);
        c.rich_text = Some(spans);
        c
    }

    #[test]
    fn test_trailing_emoji_keeps_rich_text() {
        let mut c = content_with(
            "Hola amigo 😀",
            vec![span("Hola "), bold_span("amigo"), span(" 😀")],
        );
        normalize_card_content(&mut c);
        let spans = c.rich_text.expect("rich_text kept");
        assert_eq!(spans[1].style.as_ref().unwrap().bold, Some(true));
    }

    #[test]
    fn test_music_sharp_and_checkmark_spans_keep_rich_text() {
        let mut c = content_with("C♯ major", vec![bold_span("C♯"), span(" major")]);
        normalize_card_content(&mut c);
        let spans = c.rich_text.expect("rich_text kept");
        assert_eq!(spans[0].text, "C♯");

        let mut c = content_with("Answer: ✓", vec![span("Answer: "), bold_span("✓")]);
        normalize_card_content(&mut c);
        let spans = c.rich_text.expect("rich_text kept");
        assert_eq!(spans[1].text, "✓");
    }

    #[test]
    fn test_strip_cjk_leading_marker_from_non_first_spans() {
        let mut spans = vec![
            span("Me "),
            bold_span("极estoy muriendo"),
            span("极 de ganas."),
        ];
        strip_span_boundary_markers(&mut spans);
        assert_eq!(spans[0].text, "Me ");
        assert_eq!(spans[1].text, "estoy muriendo");
        assert_eq!(spans[2].text, " de ganas.");
    }

    #[test]
    fn test_strip_preserves_leading_cjk_word_on_non_first_span() {
        let mut spans = vec![span("hello "), bold_span("力量 world")];
        strip_span_boundary_markers(&mut spans);
        assert_eq!(spans[1].text, "力量 world");
    }

    #[test]
    fn test_strip_trailing_arrow_marker_from_non_final_spans() {
        let mut spans = vec![span("Me →"), bold_span("gusta→"), span(" el café.")];
        strip_span_boundary_markers(&mut spans);
        assert_eq!(spans[0].text, "Me ");
        assert_eq!(spans[1].text, "gusta");
        assert_eq!(spans[2].text, " el café.");
    }

    #[test]
    fn test_normalize_strips_marker_that_survives_sanitize() {
        let mut content = content_with(
            "Me gusta el café.",
            vec![span("Me "), bold_span("⬛gusta"), span("⬛ el café.")],
        );
        normalize_card_content(&mut content);
        assert_eq!(content.text, "Me gusta el café.");
        let spans = content.rich_text.as_ref().expect("rich_text kept");
        assert_eq!(spans[1].text, "gusta");
    }

    #[test]
    fn test_strip_boundary_candidate_kept_when_also_in_interior() {
        let mut spans = vec![span("A →"), bold_span("B → C"), span(" D")];
        strip_span_boundary_markers(&mut spans);
        assert_eq!(spans[0].text, "A →");
        assert_eq!(spans[1].text, "B → C");
        assert_eq!(spans[2].text, " D");
    }

    #[test]
    fn test_strip_many_markers_many_spans_finishes_quickly() {
        let markers: Vec<char> = ('\u{2190}'..='\u{21FF}').collect();
        let mut spans: Vec<RichTextSpan> = (0..MAX_SPANS_PER_CONTENT)
            .map(|i| span(&format!("ab{}", markers[i % markers.len()])))
            .collect();
        spans.push(span("end"));
        let start = std::time::Instant::now();
        strip_span_boundary_markers(&mut spans);
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(spans[0].text, "ab");
    }

    #[test]
    fn test_check_card_content_rejects_too_many_spans() {
        let spans = (0..=MAX_SPANS_PER_CONTENT).map(|_| span("a")).collect();
        let err = check_card_content(&content_with("a", spans)).unwrap_err();
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn test_check_card_content_rejects_too_many_chars() {
        let c = CardContent::plain("a".repeat(MAX_CONTENT_CHARS + 1));
        assert!(check_card_content(&c).is_err());
        assert!(check_card_content(&CardContent::plain("ok")).is_ok());
    }

    #[test]
    fn test_check_card_content_rejects_non_uuid_media_ids() {
        let mut c = CardContent::plain("x");
        c.audio_id = Some("../etc/passwd".to_string());
        assert!(check_card_content(&c).unwrap_err().contains("audio_id"));
        let mut c = CardContent::plain("x");
        c.visual_id = Some("nope".to_string());
        assert!(check_card_content(&c).unwrap_err().contains("visual_id"));
        let mut c = CardContent::plain("x");
        c.audio_id = Some("3fa85f64-5717-4562-b3fc-2c963f66afa7".to_string());
        assert!(check_card_content(&c).is_ok());
    }

    #[test]
    fn test_normalize_ignores_surrounding_whitespace_in_anchor() {
        let mut c = content_with("Hola amigo ", vec![span("Hola "), bold_span("amigo ")]);
        normalize_card_content(&mut c);
        assert!(c.rich_text.is_some());
    }

    #[test]
    fn test_normalize_clears_rich_text_when_all_spans_empty() {
        let mut c = content_with("Hola", vec![span("\t"), span("")]);
        normalize_card_content(&mut c);
        assert!(c.rich_text.is_none());
        assert_eq!(c.text, "Hola");
    }

    #[test]
    fn test_sanitize_preserves_music_and_chess_symbols() {
        assert_eq!(sanitize_text("C♯ major ♞ ✓"), "C♯ major ♞ ✓");
        assert_eq!(sanitize_text("\u{2764}\u{FE0F}"), "");
        assert_eq!(sanitize_text("a ✈ b"), "a  b");
    }

    #[test]
    fn test_normalize_drops_unsafe_style_values() {
        let mut c = content_with(
            "Hi",
            vec![RichTextSpan {
                text: "Hi".to_string(),
                style: Some(RichTextSpanStyle {
                    font_color: Some("red;background:url(https://x)".to_string()),
                    font_family: Some("monospace".to_string()),
                    ..Default::default()
                }),
            }],
        );
        c.style = Some(crate::dto::CardStyle {
            font_size: None,
            font_color: Some("#27AE60".to_string()),
            font_family: Some("a;b".to_string()),
            background_color: Some("javascript:1".to_string()),
            text_align: Some("center".to_string()),
        });
        normalize_card_content(&mut c);
        let st = c.style.as_ref().unwrap();
        assert_eq!(st.font_color.as_deref(), Some("#27AE60"));
        assert!(st.font_family.is_none());
        assert!(st.background_color.is_none());
        assert_eq!(st.text_align.as_deref(), Some("center"));
        let sp = c.rich_text.as_ref().unwrap()[0].style.as_ref().unwrap();
        assert!(sp.font_color.is_none());
        assert_eq!(sp.font_family.as_deref(), Some("monospace"));
    }

    #[tokio::test]
    async fn test_upload_media_accepts_line_wrapped_base64() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let ms = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "media_ids": { "ids": ["00000000-0000-0000-0000-000000000001"] }
            })))
            .expect(1)
            .mount(&ms)
            .await;
        let raw = base64::engine::general_purpose::STANDARD.encode(vec![7u8; 300]);
        let wrapped: Vec<&str> = raw
            .as_bytes()
            .chunks(76)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        let result = mock_server_for(&ms.uri())
            .upload_media(Parameters(UploadMediaParams {
                content_base64: wrapped.join("\n") + "\n",
                content_type: "audio/mpeg".to_string(),
                filename: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
    }

    async fn mount_path_create(ms: &wiremock::MockServer, path_id: &str) {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        Mock::given(method("POST"))
            .and(path("/learning-paths"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": path_id, "name": "P", "version": 1
            })))
            .mount(ms)
            .await;
    }

    async fn assert_fatal_status_skips_remaining(status: u16, expect_text: &str) {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let path_id = "00000000-0000-0000-0000-000000000001";
        let c1 = "00000000-0000-0000-0000-000000000002";
        let c2 = "00000000-0000-0000-0000-000000000003";
        let c3 = "00000000-0000-0000-0000-000000000004";
        let ms = MockServer::start().await;
        mount_path_create(&ms, path_id).await;
        Mock::given(method("POST"))
            .and(path(format!("/learning-paths/{path_id}/catalogs/{c1}")))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(&ms)
            .await;
        for c in [c2, c3] {
            Mock::given(method("POST"))
                .and(path(format!("/learning-paths/{path_id}/catalogs/{c}")))
                .respond_with(ResponseTemplate::new(201))
                .expect(0)
                .mount(&ms)
                .await;
        }
        let result = mock_server_for(&ms.uri())
            .create_learning_path(Parameters(CreateLearningPathParams {
                name: "P".into(),
                description: None,
                catalog_ids: Some(vec![c1.into(), c2.into(), c3.into()]),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(first_text(&result)).unwrap();
        assert_eq!(v["catalogs_added"], serde_json::json!([]));
        let failed = v["catalogs_failed"].as_array().unwrap();
        assert_eq!(failed.len(), 3);
        assert_eq!(failed[0]["catalog_id"], c1);
        assert!(
            failed[0]["error"].as_str().unwrap().contains(expect_text),
            "{failed:?}"
        );
        for (i, c) in [(1, c2), (2, c3)] {
            assert_eq!(failed[i]["catalog_id"], c);
            assert!(failed[i]["error"].as_str().unwrap().starts_with("skipped:"));
        }
    }

    #[tokio::test]
    async fn test_create_learning_path_unauthorized_catalog_add_skips_remaining_ids() {
        assert_fatal_status_skips_remaining(401, "nauthorized").await;
    }

    #[tokio::test]
    async fn test_create_learning_path_quota_exceeded_catalog_add_skips_remaining_ids() {
        assert_fatal_status_skips_remaining(429, "").await;
    }

    #[tokio::test]
    async fn test_create_learning_path_too_many_catalog_ids_rejected_without_request() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let ms = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&ms)
            .await;
        let ids: Vec<String> = (0..=MAX_PATH_CATALOG_IDS)
            .map(|i| format!("00000000-0000-0000-0000-{:012}", i + 1))
            .collect();
        let result = mock_server_for(&ms.uri())
            .create_learning_path(Parameters(CreateLearningPathParams {
                name: "P".into(),
                description: None,
                catalog_ids: Some(ids),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(first_text(&result).contains("Too many catalog_ids"));
        assert!(first_text(&result).contains(&format!("max {MAX_PATH_CATALOG_IDS}")));
    }

    #[tokio::test]
    async fn test_create_learning_path_duplicate_catalog_ids_added_once() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let path_id = "00000000-0000-0000-0000-000000000001";
        let c1 = "00000000-0000-0000-0000-000000000002";
        let ms = MockServer::start().await;
        mount_path_create(&ms, path_id).await;
        Mock::given(method("POST"))
            .and(path(format!("/learning-paths/{path_id}/catalogs/{c1}")))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&ms)
            .await;
        let result = mock_server_for(&ms.uri())
            .create_learning_path(Parameters(CreateLearningPathParams {
                name: "P".into(),
                description: None,
                catalog_ids: Some(vec![c1.into(), c1.into(), c1.into()]),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(first_text(&result)).unwrap();
        assert_eq!(v["catalogs_added"], serde_json::json!([c1]));
        assert_eq!(v["catalogs_failed"], serde_json::json!([]));
    }
}
