/// Lightweight DTOs for the Engram API responses.
/// These mirror the API's JSON shapes but only include fields the MCP server actually uses.
/// They have NO dependency on the `db` crate.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// ── Pagination ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct PagedResponse<T> {
    pub data: Vec<T>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PagedResponseWithCount<T> {
    pub data: Vec<T>,
    pub cursor: Option<String>,
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
#[derive(Debug, Default, Serialize, Deserialize, Clone, schemars::JsonSchema)]
pub struct RichTextSpanStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strikethrough: Option<bool>,
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
#[derive(Debug, Serialize, Deserialize, Clone, schemars::JsonSchema)]
pub struct RichTextSpan {
    #[schemars(description = "Verbatim contiguous segment of the original text. \
        Never insert separator characters, markers, or non-original characters \
        (do NOT use CJK ideographs, bullet points, pipes, or any Unicode symbol as a span delimiter). \
        Consecutive spans must join to reproduce the original text exactly.")]
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<RichTextSpanStyle>,
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

#[derive(Debug, Deserialize, Serialize)]
pub struct GlobalSearchResult {
    pub id: Uuid,
    pub name: Option<String>,
    pub item_type: Option<String>,
}

// ── Media ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct MediaDto {
    pub id: Uuid,
    pub media_type: Option<String>,
    #[serde(rename = "fileName")]
    pub file_name: Option<String>,
    pub size: Option<i64>,
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
        let json = r#"{"data":[{"id":"00000000-0000-0000-0000-000000000001","name":"Rust","version":1}],"cursor":"abc"}"#;
        let resp: PagedResponse<CatalogDto> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].name, "Rust");
        assert_eq!(resp.cursor, Some("abc".to_string()));
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
}
