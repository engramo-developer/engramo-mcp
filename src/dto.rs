/// Lightweight DTOs for the Engramo API responses.
/// These mirror the API's JSON shapes but only include fields the MCP server actually uses.
/// They have NO dependency on the `db` crate.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// ── Pagination ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct PagedResponse<T> {
    pub data: Vec<T>,
    /// The API sends this field as `nextCursor`, not `cursor` (issue #34) — only that key
    /// populates `Some(_)`. Serialized output keeps the key `cursor`, matching the tools'
    /// `cursor` input parameter. Note this is `Option<String>`, so an old mock still using
    /// the wrong key `cursor` won't error — it just deserializes to `None`, the same silent
    /// failure mode that caused #34 in the first place; correctness here relies on every
    /// mock actually using `nextCursor`, not on serde rejecting the old key.
    #[serde(rename(deserialize = "nextCursor"))]
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PagedResponseWithCount<T> {
    pub data: Vec<T>,
    /// See `PagedResponse::cursor` — same rename from the wire's `nextCursor`, same caveat
    /// about `Option` fields not turning a stale key into a hard error.
    #[serde(rename(deserialize = "nextCursor"))]
    pub cursor: Option<String>,
    /// The API sends this field as `total`; strict rename so a stale mock fails loudly
    /// instead of a confusing "missing field `total_count`" deserialize error (see
    /// issue #35). Serialized output keeps the key `total_count`, matching the stats
    /// resource's field name.
    #[serde(rename(deserialize = "total"))]
    pub total_count: i64,
}

// ── Catalog ───────────────────────────────────────────────────────────────────

/// Full catalog returned by GET /catalogs/{id} or POST /catalogs.
#[derive(Debug, Deserialize, Serialize)]
pub struct CatalogDto {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    #[serde(rename = "cardCount")]
    pub card_count: Option<i64>,
    pub visibility: Option<String>,
    pub version: i32,
}

/// Minimal catalog for MCP resources (context injection).
/// Only essential fields to prevent token bloat.
#[derive(Debug, Serialize)]
pub struct CatalogSummary {
    pub id: Uuid,
    pub name: String,
    pub card_count: Option<i64>,
}

impl From<CatalogDto> for CatalogSummary {
    fn from(c: CatalogDto) -> Self {
        Self {
            id: c.id,
            name: c.name,
            card_count: c.card_count,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CreateCatalogRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdateCatalogRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    pub version: i32,
}

// ── Card Content ──────────────────────────────────────────────────────────────

/// Styling for a single rich-text span.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone, schemars::JsonSchema)]
pub struct RichTextSpanStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strikethrough: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superscript: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscript: Option<bool>,
    /// Font size in points.
    #[serde(rename = "fontSize", skip_serializing_if = "Option::is_none")]
    pub font_size: Option<i32>,
    /// CSS color string, e.g. "#E74C3C".
    #[serde(rename = "fontColor", skip_serializing_if = "Option::is_none")]
    pub font_color: Option<String>,
    /// Font family. Use "monospace" for code snippets.
    #[serde(rename = "fontFamily", skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
}

/// A single span of styled text inside a `CardContent`.
///
/// Deserialization is lenient (issue #37): some LLMs put styling fields (`bold`,
/// `fontColor`, ...) directly on the span instead of nesting them under `style`, matching
/// how the sentence-level style reads. Both shapes are accepted here — flat fields are
/// folded into `style`, with an explicit nested `style` field winning on conflict — but
/// serialization only ever emits the nested `style` form, which is the canonical shape
/// matching the upstream API.
///
/// Because of `#[serde(from = "RawRichTextSpan")]`, schemars derives the JSON schema exposed
/// to MCP clients from `RawRichTextSpan`, not from this struct — its flat fields are hidden
/// via `#[schemars(skip)]` so the input schema shows only `text` and `style`. Keep
/// `RawRichTextSpan::text`'s `#[schemars(description = ...)]` in sync with `text` below;
/// it is the one that actually reaches the model.
#[derive(Debug, Serialize, Deserialize, Clone, schemars::JsonSchema)]
#[serde(from = "RawRichTextSpan")]
pub struct RichTextSpan {
    #[schemars(description = "Verbatim contiguous segment of the original text. \
        Never insert separator characters, markers, or non-original characters \
        (do NOT use CJK ideographs, bullet points, pipes, or any Unicode symbol as a span delimiter). \
        Consecutive spans must join to reproduce the original text exactly.")]
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<RichTextSpanStyle>,
}

/// Deserialization (and, since `RichTextSpan` derives its schema from this via
/// `#[serde(from = ...)]`, JSON-schema) helper for [`RichTextSpan`]: accepts either a nested
/// `style` object or the same fields flattened directly onto the span, or both — see
/// `RichTextSpan`'s doc comment. `style` is the only styling shape shown in the schema sent
/// to MCP clients; the flat fields below are `#[schemars(skip)]` (serde still reads them) so
/// they stay lenient-input-only and are never advertised as a valid shape.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RawRichTextSpan {
    #[schemars(description = "Verbatim contiguous segment of the original text. \
        Never insert separator characters, markers, or non-original characters \
        (do NOT use CJK ideographs, bullet points, pipes, or any Unicode symbol as a span delimiter). \
        Consecutive spans must join to reproduce the original text exactly.")]
    text: String,
    #[schemars(description = "Styling for this span, e.g. \
        {\"bold\":true,\"fontColor\":\"#27AE60\"}. Nest fields here — this is the only \
        styling shape accepted.")]
    #[serde(default)]
    style: Option<RichTextSpanStyle>,
    #[schemars(skip)]
    #[serde(default)]
    bold: Option<bool>,
    #[schemars(skip)]
    #[serde(default)]
    italic: Option<bool>,
    #[schemars(skip)]
    #[serde(default)]
    underline: Option<bool>,
    #[schemars(skip)]
    #[serde(default)]
    strikethrough: Option<bool>,
    #[schemars(skip)]
    #[serde(default)]
    superscript: Option<bool>,
    #[schemars(skip)]
    #[serde(default)]
    subscript: Option<bool>,
    #[schemars(skip)]
    #[serde(rename = "fontSize", default)]
    font_size: Option<i32>,
    #[schemars(skip)]
    #[serde(rename = "fontColor", default)]
    font_color: Option<String>,
    #[schemars(skip)]
    #[serde(rename = "fontFamily", default)]
    font_family: Option<String>,
}

impl From<RawRichTextSpan> for RichTextSpan {
    fn from(raw: RawRichTextSpan) -> Self {
        let flat = RichTextSpanStyle {
            bold: raw.bold,
            italic: raw.italic,
            underline: raw.underline,
            strikethrough: raw.strikethrough,
            superscript: raw.superscript,
            subscript: raw.subscript,
            font_size: raw.font_size,
            font_color: raw.font_color,
            font_family: raw.font_family,
        };
        let nested = raw.style.unwrap_or_default();
        // Nested `style` wins over flat fields on conflict.
        let merged = RichTextSpanStyle {
            bold: nested.bold.or(flat.bold),
            italic: nested.italic.or(flat.italic),
            underline: nested.underline.or(flat.underline),
            strikethrough: nested.strikethrough.or(flat.strikethrough),
            superscript: nested.superscript.or(flat.superscript),
            subscript: nested.subscript.or(flat.subscript),
            font_size: nested.font_size.or(flat.font_size),
            font_color: nested.font_color.or(flat.font_color),
            font_family: nested.font_family.or(flat.font_family),
        };
        let style = if merged == RichTextSpanStyle::default() {
            None
        } else {
            Some(merged)
        };
        Self {
            text: raw.text,
            style,
        }
    }
}

/// Card-level text style defaults.
#[derive(Debug, Serialize, Deserialize, Clone, schemars::JsonSchema)]
pub struct CardStyle {
    #[serde(rename = "fontSize", skip_serializing_if = "Option::is_none")]
    pub font_size: Option<i32>,
    #[serde(rename = "fontColor", skip_serializing_if = "Option::is_none")]
    pub font_color: Option<String>,
    #[serde(rename = "fontFamily", skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(rename = "backgroundColor", skip_serializing_if = "Option::is_none")]
    pub background_color: Option<String>,
    #[serde(rename = "textAlign", skip_serializing_if = "Option::is_none")]
    pub text_align: Option<String>,
}

/// Image or video attached to a card face/back — mirrors `engram-api`'s `VisualType`.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VisualType {
    Image,
    Video,
}

/// Card face or back content.
/// LLMs should use `rich_text` spans for visual emphasis (code → monospace, keywords → bold).
/// If only `text` is provided, default styles are applied.
#[derive(Debug, Serialize, Deserialize, Clone, schemars::JsonSchema)]
pub struct CardContent {
    /// Full plain text of the card face/back. ALWAYS set this to the complete sentence.
    /// If `rich_text` spans are also provided, `text` must equal the exact concatenation of
    /// all span texts. The server validates this and discards `rich_text` if they disagree.
    #[serde(default)]
    pub text: String,
    /// Optional styled spans. Use for bold, colors, monospace code, etc.
    #[serde(rename = "richText", skip_serializing_if = "Option::is_none")]
    pub rich_text: Option<Vec<RichTextSpan>>,
    /// Card-level style overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<CardStyle>,
    /// Word-level translation dictionary for language-learning cards (set on the face).
    /// Keys are lowercased source words; values are their translations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dictionary: Option<HashMap<String, String>>,
    /// UUID of a stored audio asset for this card face/back. Optional — set this to a
    /// `media_id` returned by the `upload_media` tool (e.g. the user's own voice recording).
    /// Never fabricate a UUID here; only use one you actually got from `upload_media`.
    #[serde(rename = "audioId", skip_serializing_if = "Option::is_none")]
    pub audio_id: Option<String>,
    /// UUID of a stored image/video asset for this card face/back. Optional — same rule as
    /// `audio_id`: only set this to a `media_id` returned by `upload_media`.
    #[serde(rename = "visualId", skip_serializing_if = "Option::is_none")]
    pub visual_id: Option<String>,
    /// Required alongside `visual_id` — whether that asset is an image or a video.
    #[serde(rename = "visualType", skip_serializing_if = "Option::is_none")]
    pub visual_type: Option<VisualType>,
}

impl CardContent {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            rich_text: None,
            style: None,
            dictionary: None,
            audio_id: None,
            visual_id: None,
            visual_type: None,
        }
    }
}

// ── Card ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct CardDto {
    pub id: Uuid,
    pub version: i32,
    pub face: CardContent,
    pub back: CardContent,
    #[serde(rename = "orderNumber")]
    pub order_number: Option<i64>,
}

/// Minimal card for resource injection (context).
#[derive(Debug, Serialize)]
pub struct CardSummary {
    pub id: Uuid,
    pub face_text: String,
    pub back_text: String,
}

impl From<CardDto> for CardSummary {
    fn from(c: CardDto) -> Self {
        Self {
            id: c.id,
            face_text: c.face.text,
            back_text: c.back.text,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CreateCardRequest {
    #[serde(rename = "catalogId", skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<Uuid>,
    pub face: CardContent,
    pub back: CardContent,
}

#[derive(Debug, Serialize)]
pub struct UpdateCardRequest {
    /// All catalog IDs the card should belong to (replaces existing).
    #[serde(rename = "catalogIds")]
    pub catalog_ids: Vec<Uuid>,
    #[serde(rename = "orderNumber")]
    pub order_number: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face: Option<CardContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub back: Option<CardContent>,
    pub version: i32,
}

pub type CatalogWithCardsResponse = PagedResponse<CardDto>;

// ── Generate (catalog-with-cards batch) ──────────────────────────────────────

/// Single card input for [`CreateCatalogWithCardsApiRequest`].
#[derive(Debug, Serialize)]
pub struct CardInput {
    pub face: CardContent,
    pub back: CardContent,
}

/// Request body for `POST /catalogs/with-cards`.
#[derive(Debug, Serialize)]
pub struct CreateCatalogWithCardsApiRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// UUID of a stored image asset for the catalog's cover — set this to a `media_id`
    /// returned by the `upload_media` tool. Never fabricate a UUID here.
    #[serde(rename = "imageId", skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    pub cards: Vec<CardInput>,
}

/// Response from `POST /catalogs/with-cards`.
#[derive(Debug, Deserialize, Serialize)]
pub struct CatalogWithCardsCreatedDto {
    pub catalog: CatalogDto,
    #[serde(rename = "cardsCreated")]
    pub cards_created: usize,
}

// ── Learning ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct LearningCardDto {
    pub id: Uuid,
    pub face: CardContent,
    pub back: CardContent,
    #[serde(rename = "nextReview")]
    pub next_review: Option<String>,
}

/// Minimal due-card for resource injection.
#[derive(Debug, Serialize)]
pub struct DueCardSummary {
    pub card_id: Uuid,
    pub face_text: String,
}

impl From<LearningCardDto> for DueCardSummary {
    fn from(c: LearningCardDto) -> Self {
        Self {
            card_id: c.id,
            face_text: c.face.text,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LearningStats {
    pub due_count: i64,
    pub total_count: i64,
}

// ── Learning Paths ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct LearningPathDto {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub version: i32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LearningPathDetailDto {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub version: i32,
    pub catalogs: Option<Vec<CatalogDto>>,
}

/// Minimal learning path for resource injection.
#[derive(Debug, Serialize)]
pub struct LearningPathSummary {
    pub id: Uuid,
    pub name: String,
}

impl From<LearningPathDto> for LearningPathSummary {
    fn from(p: LearningPathDto) -> Self {
        Self {
            id: p.id,
            name: p.name,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CreateLearningPathRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ── Search ────────────────────────────────────────────────────────────────────

/// A search hit's kind. Only `catalog` and `card` are part of global search — learning
/// paths are NOT indexed by `/search` (issue #36 clarification). `Other` absorbs any
/// value the backend adds later (or a casing drift) so one unrecognized hit doesn't fail
/// the whole array, while keeping the raw wire value visible to the model instead of
/// collapsing it into an opaque `"unknown"`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchItemType {
    Catalog,
    Card,
    /// Any kind this crate doesn't model yet; the raw wire value is kept for the model.
    #[serde(untagged)]
    Other(String),
}

/// Wire shape is camelCase (`itemType`, `id`, `title`, `subtitle`, `parentId`, …).
/// `parent_id` is the catalog UUID for a card hit. `rank`, `imageId` and `imageUrl`
/// (a signed URL) are intentionally unmodeled. Don't add `#[serde(alias)]`: a payload
/// carrying both keys is a duplicate-field error that fails the whole `/search` array
/// (issue #36; pinned by tests below).
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all(deserialize = "camelCase"))]
pub struct GlobalSearchResult {
    pub id: Uuid,
    /// Hit kind: `Catalog` / `Card`, or `Other(raw)` for any value this crate doesn't model
    /// (see [`SearchItemType`]).
    pub item_type: Option<SearchItemType>,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub parent_id: Option<Uuid>,
}

// ── Media ─────────────────────────────────────────────────────────────────────

/// The API's `MediaDto` uses plain snake_case on the wire (no rename), so field names
/// here match it directly: `id`, `name`, `content_type`, `media_type`, `length`.
#[derive(Debug, Deserialize, Serialize)]
pub struct MediaDto {
    pub id: Uuid,
    pub name: Option<String>,
    pub content_type: Option<String>,
    pub media_type: Option<String>,
    pub length: Option<i64>,
}

/// Raw shape of `POST /media`'s response body: `{"media_ids": {"ids": [...]}}`.
/// We always upload exactly one file per call, so callers take the first id.
#[derive(Debug, Deserialize)]
pub struct UploadMediaResponseDto {
    pub media_ids: MediaIdsDto,
}

#[derive(Debug, Deserialize)]
pub struct MediaIdsDto {
    pub ids: Vec<Uuid>,
}

/// Result of the `upload_media` MCP tool — the new media's id, ready to attach via a
/// card's `audio_id`/`visual_id` or a catalog's `image_id`.
#[derive(Debug, Serialize)]
pub struct UploadMediaResult {
    pub media_id: Uuid,
}

// ── Paid AI (feature-flagged) ────────────────────────────────────────────────

/// Shared request body for card-scoped batch AI operations
/// (`POST /catalogs/tts`, `/catalogs/translate`, `/catalogs/dictionary`).
/// Exactly one of `catalog_id` / `card_id` must be set.
#[derive(Debug, Serialize)]
pub struct CardAiRequest {
    #[serde(rename = "catalogId", skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<Uuid>,
    #[serde(rename = "cardId", skip_serializing_if = "Option::is_none")]
    pub card_id: Option<Uuid>,
    /// BCP-47 language code, e.g. "en", "uk".
    pub lang: String,
}

/// Response body for card-scoped batch AI operations.
#[derive(Debug, Deserialize, Serialize)]
pub struct CardAiBatchResultDto {
    /// Number of cards that were updated (or, for TTS, accepted for async generation).
    pub updated: usize,
    /// IDs of the affected cards.
    pub card_ids: Vec<Uuid>,
}

/// Request body for `POST /ai-agent`.
#[derive(Debug, Serialize)]
pub struct AiChatRequest {
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    pub message: String,
}

/// Response body for `POST /ai-agent`.
#[derive(Debug, Deserialize, Serialize)]
pub struct AiChatResponseDto {
    #[serde(rename = "sessionId")]
    pub session_id: Uuid,
    pub text: String,
}

// ── Subscription ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct UserSubscriptionDto {
    pub plan_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UsageSummaryDto {
    pub resources: Option<Vec<UsageResourceDto>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UsageResourceDto {
    pub resource_type: String,
    pub used: i64,
    pub limit: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_summary_from_dto() {
        let dto = CatalogDto {
            id: Uuid::new_v4(),
            name: "Rust".to_string(),
            description: Some("Rust cards".to_string()),
            tags: Some(vec!["programming".to_string()]),
            card_count: Some(42),
            visibility: Some("private".to_string()),
            version: 1,
        };
        let summary: CatalogSummary = dto.into();
        assert_eq!(summary.name, "Rust");
        assert_eq!(summary.card_count, Some(42));
        // description and tags are NOT in the summary (no context bloat)
    }

    #[test]
    fn test_card_summary_strips_rich_text() {
        let dto = CardDto {
            id: Uuid::new_v4(),
            version: 1,
            face: CardContent {
                text: "What is ownership?".to_string(),
                rich_text: Some(vec![RichTextSpan {
                    text: "What is ownership?".to_string(),
                    style: Some(RichTextSpanStyle {
                        bold: Some(true),
                        ..Default::default()
                    }),
                }]),
                style: None,
                dictionary: None,
                audio_id: None,
                visual_id: None,
                visual_type: None,
            },
            back: CardContent::plain("Every value has one owner."),
            order_number: Some(1),
        };
        let summary: CardSummary = dto.into();
        assert_eq!(summary.face_text, "What is ownership?");
        assert_eq!(summary.back_text, "Every value has one owner.");
        // rich_text is NOT in the summary
    }

    #[test]
    fn test_card_content_plain() {
        let c = CardContent::plain("Hello");
        assert_eq!(c.text, "Hello");
        assert!(c.rich_text.is_none());
        assert!(c.style.is_none());
        assert!(c.visual_id.is_none());
        assert!(c.visual_type.is_none());
    }

    #[test]
    fn test_card_content_visual_fields_round_trip() {
        let mut c = CardContent::plain("A card with a picture");
        c.visual_id = Some("3fa85f64-5717-4562-b3fc-2c963f66afa6".to_string());
        c.visual_type = Some(VisualType::Image);
        c.audio_id = Some("3fa85f64-5717-4562-b3fc-2c963f66afa7".to_string());

        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"visualId\":\"3fa85f64-5717-4562-b3fc-2c963f66afa6\""));
        assert!(json.contains("\"visualType\":\"image\""));
        assert!(json.contains("\"audioId\":\"3fa85f64-5717-4562-b3fc-2c963f66afa7\""));

        let round_tripped: CardContent = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped.visual_id, c.visual_id);
        assert_eq!(round_tripped.visual_type, Some(VisualType::Image));
        assert_eq!(round_tripped.audio_id, c.audio_id);
    }

    #[test]
    fn test_card_content_visual_fields_omitted_when_none() {
        let json = serde_json::to_string(&CardContent::plain("no media")).unwrap();
        assert!(!json.contains("visualId"));
        assert!(!json.contains("visualType"));
        assert!(!json.contains("audioId"));
    }

    #[test]
    fn test_create_catalog_request_serialization() {
        let req = CreateCatalogRequest {
            name: "Test".to_string(),
            description: None,
            tags: None,
            visibility: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        // None fields should be omitted
        assert!(!json.contains("description"));
        assert!(json.contains("\"name\":\"Test\""));
    }

    #[test]
    fn test_pagedresponse_deserialization() {
        let json = r#"{"data":[{"id":"00000000-0000-0000-0000-000000000001","name":"Rust","version":1}],"nextCursor":"abc"}"#;
        let resp: PagedResponse<CatalogDto> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].name, "Rust");
        assert_eq!(resp.cursor, Some("abc".to_string()));
    }

    #[test]
    fn test_pagedresponse_serialization_uses_cursor_key() {
        // The output key stays `cursor` even though the wire's deserialize key is
        // `nextCursor` — the tools' `cursor` input parameter expects this name back.
        let resp = PagedResponse::<CatalogDto> {
            data: vec![],
            cursor: Some("abc".to_string()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"cursor\":\"abc\""), "{json}");
        assert!(!json.contains("nextCursor"), "{json}");
    }

    #[test]
    fn test_pagedresponse_stale_cursor_key_is_ignored_not_matched() {
        // `cursor` is `Option<String>`, so a strict (non-alias) rename to `nextCursor`
        // can't turn a wrong key into a hard deserialize error — serde treats a missing
        // Option field as `None`, same as before the fix. This is precisely why #34 went
        // undetected: the field silently defaulted instead of failing. The rename's value
        // is that only the *real* wire key (`nextCursor`) ever populates `cursor` with
        // `Some(_)` — asserted by `test_pagedresponse_deserialization` above — not that a
        // stale mock errors.
        let json = r#"{"data":[],"cursor":"abc"}"#;
        let resp: PagedResponse<CatalogDto> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.cursor, None);
    }

    #[test]
    fn test_pagedresponsewithcount_deserialization_and_serialization() {
        let json = r#"{"data":[],"nextCursor":"xyz","total":42}"#;
        let resp: PagedResponseWithCount<CatalogDto> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.cursor, Some("xyz".to_string()));
        assert_eq!(resp.total_count, 42);

        let out = serde_json::to_string(&resp).unwrap();
        assert!(out.contains("\"cursor\":\"xyz\""), "{out}");
        assert!(out.contains("\"total_count\":42"), "{out}");
    }

    #[test]
    fn test_global_search_result_deserializes_camel_case() {
        let json = r#"{"itemType":"card","id":"00000000-0000-0000-0000-000000000001",
            "title":"Ownership","subtitle":"Rust Basics",
            "parentId":"00000000-0000-0000-0000-000000000002"}"#;
        let result: GlobalSearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.item_type, Some(SearchItemType::Card));
        assert_eq!(result.title.as_deref(), Some("Ownership"));
        assert_eq!(result.subtitle.as_deref(), Some("Rust Basics"));
        assert_eq!(
            result.parent_id,
            Some(Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap())
        );
    }

    #[test]
    fn test_global_search_result_serializes_snake_case() {
        let r = GlobalSearchResult {
            id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
            item_type: Some(SearchItemType::Card),
            title: Some("T".to_string()),
            subtitle: None,
            parent_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap()),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"parent_id\""), "{json}");
        assert!(json.contains("\"item_type\""), "{json}");
        assert!(!json.contains("parentId"), "{json}");
        assert!(!json.contains("itemType"), "{json}");
    }

    /// An item type the backend adds later must not fail the whole array — it should
    /// decode to `Other` (preserving the raw wire value) instead of erroring.
    #[test]
    fn test_global_search_result_unknown_item_type_does_not_fail() {
        let json = r#"{"itemType":"learning_path","id":"00000000-0000-0000-0000-000000000001"}"#;
        let result: GlobalSearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(
            result.item_type,
            Some(SearchItemType::Other("learning_path".to_string()))
        );
        let out = serde_json::to_string(&result.item_type).unwrap();
        assert_eq!(out, "\"learning_path\"");
    }

    /// Extra, non-canonical keys alongside the canonical ones (e.g. a stray `name`/`type`
    /// from a different shape, or the signed `imageUrl` we never model) must be ignored by
    /// serde rather than overriding the canonical values — this is why we don't declare
    /// them as `#[serde(alias = ...)]`: an alias would make a payload carrying both keys
    /// a "duplicate field" error that fails the whole `/search` array.
    #[test]
    fn test_global_search_result_unknown_extra_keys_do_not_override_canonical_fields() {
        let json = r#"{"itemType":"card","id":"00000000-0000-0000-0000-000000000001",
            "title":"Ownership","name":"Something Else","type":"catalog",
            "parentId":"00000000-0000-0000-0000-000000000002",
            "imageUrl":"https://cdn.example.com/img.png"}"#;
        let result: GlobalSearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.item_type, Some(SearchItemType::Card));
        assert_eq!(result.title.as_deref(), Some("Ownership"));
        assert_eq!(
            result.parent_id,
            Some(Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap())
        );
    }

    #[test]
    fn test_search_item_type_serializes_lowercase_and_other() {
        assert_eq!(
            serde_json::to_string(&SearchItemType::Catalog).unwrap(),
            "\"catalog\""
        );
        assert_eq!(
            serde_json::to_string(&SearchItemType::Card).unwrap(),
            "\"card\""
        );
        assert_eq!(
            serde_json::to_string(&SearchItemType::Other("learning_path".to_string())).unwrap(),
            "\"learning_path\""
        );
    }

    /// `rename_all = "lowercase"` is case-sensitive: a backend casing drift (e.g. "Card"
    /// instead of "card") falls to `Other`, not `Card` — this pins that behavior so a
    /// casing skew is visible in tool output instead of silently misclassified.
    #[test]
    fn test_search_item_type_is_case_sensitive_mixed_case_is_other() {
        let json = r#"{"itemType":"Card","id":"00000000-0000-0000-0000-000000000001"}"#;
        let r: GlobalSearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(r.item_type, Some(SearchItemType::Other("Card".to_string())));
    }

    #[test]
    fn test_global_search_result_missing_title_is_none() {
        let json = r#"{"id":"00000000-0000-0000-0000-000000000001"}"#;
        let result: GlobalSearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.title, None);
        assert_eq!(result.item_type, None);
    }

    #[test]
    fn test_global_search_result_null_title_is_none() {
        let json = r#"{"id":"00000000-0000-0000-0000-000000000001","title":null}"#;
        let result: GlobalSearchResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.title, None);
    }

    /// `rank` and `imageId` are unmodeled extra keys, and `imageUrl` (a signed URL) is
    /// not modeled either — none of them can leak into serialized tool output even
    /// though the backend sends them.
    #[test]
    fn test_global_search_result_rank_and_image_url_never_reach_output() {
        let json = r#"{"itemType":"catalog","id":"00000000-0000-0000-0000-000000000001",
            "rank":0.42,"imageId":"00000000-0000-0000-0000-0000000000aa",
            "imageUrl":"https://cdn.example.com/img.png"}"#;
        let result: GlobalSearchResult = serde_json::from_str(json).unwrap();
        let out = serde_json::to_string(&result).unwrap();
        assert!(!out.contains("rank"), "{out}");
        assert!(!out.contains("image_id"), "{out}");
        assert!(!out.contains("imageId"), "{out}");
        assert!(!out.contains("image_url"), "{out}");
        assert!(!out.contains("imageUrl"), "{out}");
        assert!(!out.contains("cdn.example.com"), "{out}");
    }

    /// Pins current behavior: untagged `Other(String)` only absorbs strings — a
    /// non-string `itemType` (number, object, bool) fails the whole hit, and thus the
    /// whole `/search` array, even though the doc comment describes the fallback as
    /// tolerant. If this turns out to be the wrong tradeoff, that's a separate fix.
    #[test]
    fn test_global_search_result_non_string_item_type_is_rejected() {
        let json = r#"[{"itemType":5,"id":"00000000-0000-0000-0000-000000000001"}]"#;
        let r: Result<Vec<GlobalSearchResult>, _> = serde_json::from_str(json);
        assert!(
            r.is_err(),
            "non-string itemType must not silently decode: {r:?}"
        );
    }

    /// One `Other` hit alongside known `Catalog`/`Card` hits in the same array must not
    /// fail its neighbours.
    #[test]
    fn test_global_search_result_array_with_unknown_item_type_decodes_all_hits() {
        let json = r#"[
            {"itemType":"catalog","id":"00000000-0000-0000-0000-000000000001"},
            {"itemType":"learning_path","id":"00000000-0000-0000-0000-000000000002"},
            {"itemType":"card","id":"00000000-0000-0000-0000-000000000003"}]"#;
        let v: Vec<GlobalSearchResult> = serde_json::from_str(json).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].item_type, Some(SearchItemType::Catalog));
        assert_eq!(
            v[1].item_type,
            Some(SearchItemType::Other("learning_path".into()))
        );
        assert_eq!(v[2].item_type, Some(SearchItemType::Card));
    }

    #[test]
    fn test_media_dto_deserializes_snake_case() {
        let json = r#"{"id":"00000000-0000-0000-0000-000000000001","name":"photo.jpg",
            "content_type":"image/jpeg","media_type":"image","length":1024}"#;
        let media: MediaDto = serde_json::from_str(json).unwrap();
        assert_eq!(media.name.as_deref(), Some("photo.jpg"));
        assert_eq!(media.content_type.as_deref(), Some("image/jpeg"));
        assert_eq!(media.media_type.as_deref(), Some("image"));
        assert_eq!(media.length, Some(1024));
    }

    #[test]
    fn test_card_ai_request_serialization_omits_none() {
        let req = CardAiRequest {
            catalog_id: Some(Uuid::nil()),
            card_id: None,
            lang: "uk".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("cardId"));
        assert!(json.contains("\"catalogId\""));
        assert!(json.contains("\"lang\":\"uk\""));
    }

    #[test]
    fn test_card_ai_batch_result_deserialization() {
        let json = r#"{"updated":2,"card_ids":["00000000-0000-0000-0000-000000000001"]}"#;
        let resp: CardAiBatchResultDto = serde_json::from_str(json).unwrap();
        assert_eq!(resp.updated, 2);
        assert_eq!(resp.card_ids.len(), 1);
    }

    #[test]
    fn test_ai_chat_request_serialization_omits_session_id_when_none() {
        let req = AiChatRequest {
            session_id: None,
            message: "hello".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("sessionId"));
        assert!(json.contains("\"message\":\"hello\""));
    }

    #[test]
    fn test_ai_chat_response_deserialization() {
        let json = r#"{"sessionId":"00000000-0000-0000-0000-000000000001","text":"hi there"}"#;
        let resp: AiChatResponseDto = serde_json::from_str(json).unwrap();
        assert_eq!(resp.text, "hi there");
    }

    #[test]
    fn test_learning_stats_serialization() {
        let stats = LearningStats {
            due_count: 15,
            total_count: 200,
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("\"due_count\":15"));
        assert!(json.contains("\"total_count\":200"));
    }

    // ── RichTextSpan lenient deserialization (issue #37) ─────────────────────

    #[test]
    fn test_rich_text_span_flat_style_fields_deserialize_into_nested_style() {
        let json = r##"{"text":"gracias.","bold":true,"fontColor":"#27AE60"}"##;
        let span: RichTextSpan = serde_json::from_str(json).unwrap();
        assert_eq!(span.text, "gracias.");
        let style = span.style.expect("flat fields must populate style");
        assert_eq!(style.bold, Some(true));
        assert_eq!(style.font_color, Some("#27AE60".to_string()));
    }

    #[test]
    fn test_rich_text_span_nested_style_deserializes() {
        let json = r##"{"text":"gracias.","style":{"bold":true,"fontColor":"#27AE60"}}"##;
        let span: RichTextSpan = serde_json::from_str(json).unwrap();
        let style = span.style.expect("nested style must be preserved");
        assert_eq!(style.bold, Some(true));
        assert_eq!(style.font_color, Some("#27AE60".to_string()));
    }

    #[test]
    fn test_rich_text_span_nested_style_wins_over_flat_on_conflict() {
        let json = r#"{"text":"gracias.","bold":false,"style":{"bold":true}}"#;
        let span: RichTextSpan = serde_json::from_str(json).unwrap();
        let style = span.style.expect("style must be set");
        assert_eq!(
            style.bold,
            Some(true),
            "nested style.bold must win over the flat bold field"
        );
    }

    #[test]
    fn test_rich_text_span_no_styling_leaves_style_none() {
        let json = r#"{"text":"gracias."}"#;
        let span: RichTextSpan = serde_json::from_str(json).unwrap();
        assert!(span.style.is_none());
    }

    #[test]
    fn test_rich_text_span_superscript_subscript_round_trip() {
        let span = RichTextSpan {
            text: "x2".to_string(),
            style: Some(RichTextSpanStyle {
                superscript: Some(true),
                ..Default::default()
            }),
        };
        let json = serde_json::to_string(&span).unwrap();
        assert!(json.contains("\"superscript\":true"));
        let round_tripped: RichTextSpan = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped.style.unwrap().superscript, Some(true));
    }

    #[test]
    fn test_rich_text_span_serializes_nested_style_only_no_flat_fields() {
        let span = RichTextSpan {
            text: "gracias.".to_string(),
            style: Some(RichTextSpanStyle {
                bold: Some(true),
                font_color: Some("#27AE60".to_string()),
                ..Default::default()
            }),
        };
        let json = serde_json::to_value(&span).unwrap();
        assert_eq!(json["style"]["bold"], serde_json::json!(true));
        assert_eq!(json["style"]["fontColor"], serde_json::json!("#27AE60"));
        // No top-level flat styling fields on the wire.
        assert!(json.get("bold").is_none());
        assert!(json.get("fontColor").is_none());
    }

    #[test]
    fn test_rich_text_span_every_flat_style_field_maps_to_its_own_nested_field() {
        let json = r#"{"text":"x","italic":true,"underline":false,"strikethrough":true,
            "superscript":false,"subscript":true,"fontSize":18,"fontFamily":"monospace"}"#;
        let span: RichTextSpan = serde_json::from_str(json).unwrap();
        let s = span.style.expect("style");
        assert_eq!(s.bold, None);
        assert_eq!(s.italic, Some(true));
        assert_eq!(s.underline, Some(false));
        assert_eq!(s.strikethrough, Some(true));
        assert_eq!(s.superscript, Some(false));
        assert_eq!(s.subscript, Some(true));
        assert_eq!(s.font_size, Some(18));
        assert_eq!(s.font_color, None);
        assert_eq!(s.font_family.as_deref(), Some("monospace"));
        // Serialize back: nested only, camelCase keys, subscript present.
        let v = serde_json::to_value(RichTextSpan {
            text: span.text,
            style: Some(s),
        })
        .unwrap();
        assert_eq!(v["style"]["subscript"], serde_json::json!(true));
        assert_eq!(v["style"]["fontSize"], serde_json::json!(18));
        assert!(v.get("subscript").is_none());
    }

    #[test]
    fn test_rich_text_span_flat_and_nested_disjoint_fields_are_merged() {
        let json = r##"{"text":"x","bold":true,"fontSize":20,
            "style":{"fontColor":"#E74C3C","fontSize":14}}"##;
        let span: RichTextSpan = serde_json::from_str(json).unwrap();
        let s = span.style.unwrap();
        assert_eq!(
            s.bold,
            Some(true),
            "flat-only field must survive alongside nested style"
        );
        assert_eq!(s.font_color.as_deref(), Some("#E74C3C"));
        assert_eq!(
            s.font_size,
            Some(14),
            "nested fontSize must win over flat on conflict"
        );
    }

    #[test]
    fn test_rich_text_span_wrong_typed_flat_field_is_rejected() {
        // Pins the current contract: a typed flat field with the wrong JSON type fails
        // deserialization of the whole span (and thus the whole tool call).
        let json = r#"{"text":"x","bold":"true"}"#;
        assert!(serde_json::from_str::<RichTextSpan>(json).is_err());
    }

    #[test]
    fn test_rich_text_span_explicit_null_style_is_none() {
        let span: RichTextSpan = serde_json::from_str(r#"{"text":"x","style":null}"#).unwrap();
        assert!(span.style.is_none());
    }

    #[test]
    fn test_rich_text_span_flat_false_is_kept_not_collapsed_to_none() {
        let span: RichTextSpan = serde_json::from_str(r#"{"text":"x","bold":false}"#).unwrap();
        assert_eq!(span.style.unwrap().bold, Some(false));
    }

    #[test]
    fn test_rich_text_span_missing_text_is_rejected() {
        assert!(serde_json::from_str::<RichTextSpan>(r#"{"bold":true}"#).is_err());
    }

    #[test]
    fn test_rich_text_span_schema_documents_nested_style_and_hides_flat_fields() {
        // The tool input schema (draft2020-12, deserialize contract) is derived from
        // RawRichTextSpan via #[serde(from = ...)] — assert on that schema directly.
        let schema = schemars::schema_for!(RawRichTextSpan);
        let value = serde_json::to_value(&schema).unwrap();
        let s = value.to_string();
        // The `text` description must carry the full anti-marker guidance (F1).
        assert!(s.contains("Verbatim contiguous segment"), "{s}");
        assert!(s.contains("CJK ideographs"), "{s}");
        assert!(!s.contains("Deprecated shorthand"), "{s}");
        // Top-level properties must be exactly `text` and `style` — the flat shorthand
        // fields must not be advertised as span properties (F2).
        let props = value["properties"].as_object().expect("properties object");
        let mut keys: Vec<&str> = props.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["style", "text"], "{value}");

        // Confirm this schema is actually what GenerateCardParams exposes to MCP clients.
        let params_schema = schemars::schema_for!(crate::tools::generate::GenerateCardParams);
        let params_s = serde_json::to_string(&params_schema).unwrap();
        assert!(params_s.contains("CJK ideographs"), "{params_s}");
    }
}
