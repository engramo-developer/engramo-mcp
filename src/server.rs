use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResult, GetPromptRequestParams, GetPromptResult,
        ListPromptsResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        ReadResourceRequestParams, ReadResourceResult, ServerCapabilities, ServerInfo,
    },
    tool, tool_router,
};
use tracing::{debug, warn};

use crate::client::EngramClient;
use crate::dto::{
    CardInput, CreateCardRequest, CreateCatalogWithCardsApiRequest, CreateLearningPathRequest,
    UpdateCardRequest, UpdateCatalogRequest,
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
use crate::tools::media::ListMediaParams;
use crate::tools::search::SearchParams;
use uuid::Uuid;

pub struct EngramMcpServer {
    client: EngramClient,
    tool_router: ToolRouter<Self>,
}

impl EngramMcpServer {
    pub fn new(client: EngramClient) -> Self {
        Self {
            tool_router: Self::catalog_tools_router()
                + Self::card_tools_router()
                + Self::learning_tools_router()
                + Self::learning_path_tools_router()
                + Self::search_tools_router()
                + Self::media_tools_router()
                + Self::generate_tools_router(),
            client,
        }
    }
}

// ── Catalog tools ─────────────────────────────────────────────────────────────

#[tool_router(router = catalog_tools_router)]
impl EngramMcpServer {
    #[tool(
        description = "List the user's flashcard catalogs with cursor-based pagination. Returns id, name, and card_count for each catalog. Use get_catalog to fetch full details."
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
impl EngramMcpServer {
    #[tool(description = "List flashcards in a catalog with cursor-based pagination.")]
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
impl EngramMcpServer {
    #[tool(
        description = "Get flashcards due for review today, sorted by priority. Use this to start a study session."
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
        description = "Get all cards currently in the learning queue (due and future), with pagination."
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
impl EngramMcpServer {
    #[tool(description = "List all learning paths with cursor-based pagination.")]
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
impl EngramMcpServer {
    #[tool(
        description = "Search across all cards, catalogs, and learning paths. Returns ids and types."
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

    #[tool(description = "Search catalogs by name or description.")]
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
impl EngramMcpServer {
    #[tool(
        description = "List uploaded media files. Optionally filter by media type ('image', 'audio', etc.)."
    )]
    pub async fn list_media(
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

// ── ServerHandler ─────────────────────────────────────────────────────────────

impl ServerHandler for EngramMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_instructions(
            "Engram flashcard assistant. Use catalog and card tools to manage flashcards, \
             learning tools to track spaced-repetition progress, and search to find content. \
             Resources expose live data (catalogs, due cards, stats). \
             Prompts guide you through review sessions, flashcard creation, and study planning.",
        )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        let tools = self.tool_router.list_all();
        async move {
            Ok(ListToolsResult {
                tools,
                next_cursor: None,
                meta: None,
            })
        }
    }

    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        ctx: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
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
    ) -> Result<ReadResourceResult, ErrorData> {
        crate::resources::read(&self.client, params).await
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
    ) -> Result<GetPromptResult, ErrorData> {
        crate::prompts::get(params)
    }
}

// ── Generate tools ────────────────────────────────────────────────────────────

/// Strip characters the LLM should not embed in card text:
/// - Control characters (tabs, carriage returns, …) — newline is kept intentionally.
/// - Emoji Unicode blocks.
fn sanitize_text(s: &str) -> String {
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
impl EngramMcpServer {
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
        };
        normalize_card_content(&mut content);
        assert_eq!(content.text, "Me gusta el café.");
        assert!(content.rich_text.is_some());
    }
}
