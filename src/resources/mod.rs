//! MCP Resources — static URI handles that expose live Engram data as context.
//!
//! Resources are read by URI; the server fetches the relevant data from the API
//! and returns it as plain JSON text so the LLM can inject it into context.
//!
//! Available resources:
//!   engram://card-schema           — CardContent JSON schema, validation rules, 3 examples
//!   engram://catalogs              — all catalogs (id, name, card_count)
//!   engram://learning/due          — due cards for today (card_id, face_text)
//!   engram://learning/stats        — learning stats (due_count, total_count)
//!   engram://learning-paths        — all learning paths (id, name)
//!   engram://subscription          — user subscription / plan info

use rmcp::model::{
    AnnotateAble, ListResourcesResult, RawResource, ReadResourceRequestParams, ReadResourceResult,
    Resource, ResourceContents,
};

use crate::client::EngramClient;
use crate::dto::{CatalogSummary, DueCardSummary, LearningPathSummary, LearningStats};
use crate::error::ApiError;

// ── URI constants ─────────────────────────────────────────────────────────────

pub const URI_CARD_SCHEMA: &str = "engram://card-schema";
pub const URI_CATALOGS: &str = "engram://catalogs";
pub const URI_DUE: &str = "engram://learning/due";
pub const URI_STATS: &str = "engram://learning/stats";
pub const URI_LEARNING_PATHS: &str = "engram://learning-paths";
pub const URI_SUBSCRIPTION: &str = "engram://subscription";

// ── Card schema document ──────────────────────────────────────────────────────

const CARD_SCHEMA_DOC: &str = "\
=== Engram CardContent — Schema & Examples ===

CardContent fields:
  text         String     REQUIRED. The full plain text. This is the validation anchor.
  rich_text    Span[]     Optional. Styled segments. MUST concatenate to `text` exactly — the
                          server validates this and silently discards rich_text on mismatch.
  dictionary   {str:str}  Optional. Word-level translation map (face only). Keys: lowercase
                          source word. Values: primary translation; append \"; (here) X\" if
                          contextual meaning differs from the most common meaning.
  style        CardStyle  Optional. Card-level default style (font, color, alignment).
  audio_id     UUID       Optional. TTS audio asset. Set by the server; do not fabricate.

RichTextSpan fields:
  text         String     REQUIRED. A VERBATIM contiguous slice of the parent `text`.
                          Never insert extra characters, markers, or spaces not in the original.
  style        SpanStyle  Optional.

SpanStyle fields:
  fontColor    String     CSS color. \"#27AE60\" = green (correct/highlight), \"#E74C3C\" = red (warning).
  bold         bool
  italic       bool
  underline    bool
  fontFamily   String     Use \"monospace\" for code.

Rich-text validation rules (CRITICAL):
  R1. Set `text` to the complete original sentence first.
  R2. Every span.text must be a verbatim substring of `text`.
  R3. Spans must cover ALL of `text` — no gaps, no extra characters.
  R4. Concatenation of all span.text values must equal `text` exactly (character-for-character).
  R5. If no styling is needed, omit `rich_text` entirely.

=== Example 1: Simple card (no rich_text) ===
{
  \"face\": { \"text\": \"What is the capital of France?\" },
  \"back\": { \"text\": \"Paris.\" }
}

=== Example 2: Card with styled spans ===
Sentence: \"The quick brown fox jumps over the lazy dog.\"
Highlight \"quick brown fox\" in green:

{
  \"face\": {
    \"text\": \"The quick brown fox jumps over the lazy dog.\",
    \"rich_text\": [
      { \"text\": \"The \" },
      { \"text\": \"quick brown fox\", \"style\": { \"fontColor\": \"#27AE60\", \"bold\": true } },
      { \"text\": \" jumps over the lazy dog.\" }
    ]
  },
  \"back\": { \"text\": \"An English pangram.\" }
}

Span check: \"The \" + \"quick brown fox\" + \" jumps over the lazy dog.\" = face.text ✓

=== Example 3: Language-learning card with dictionary and back-side verb reference ===
Source sentence (es): \"He hablado con mi madre esta mañana por teléfono.\"
Grammar highlight: pretérito perfecto compuesto form \"He hablado\" in green.
Back: Ukrainian translation + (infinitive - participle) in green.

{
  \"face\": {
    \"text\": \"He hablado con mi madre esta mañana por teléfono.\",
    \"rich_text\": [
      { \"text\": \"He hablado\", \"style\": { \"fontColor\": \"#27AE60\", \"bold\": true } },
      { \"text\": \" con mi madre esta mañana por teléfono.\" }
    ],
    \"dictionary\": {
      \"hablado\":   \"розмовляв/розмовляла; (тут) поговорив/поговорила\",
      \"madre\":     \"мати\",
      \"mañana\":    \"ранок; (тут) сьогодні вранці\",
      \"teléfono\":  \"телефон\"
    }
  },
  \"back\": {
    \"text\": \"Я поговорив з мамою цього ранку по телефону. (hablar - hablado)\",
    \"rich_text\": [
      { \"text\": \"Я поговорив з мамою цього ранку по телефону. (\" },
      { \"text\": \"hablar - hablado\", \"style\": { \"fontColor\": \"#27AE60\" } },
      { \"text\": \")\" }
    ]
  }
}

Span checks:
  face: \"He hablado\" + \" con mi madre esta mañana por teléfono.\" = face.text ✓
  back: \"Я поговорив з мамою цього ранку по телефону. (\" + \"hablar - hablado\" + \")\" = back.text ✓\
";

// ── Resource list ─────────────────────────────────────────────────────────────

fn make_resource(uri: &str, name: &str, description: &str, mime_type: &str) -> Resource {
    RawResource {
        uri: uri.to_string(),
        name: name.to_string(),
        title: None,
        description: Some(description.to_string()),
        mime_type: Some(mime_type.to_string()),
        size: None,
        icons: None,
        meta: None,
    }
    .no_annotation()
}

pub fn list_all() -> ListResourcesResult {
    ListResourcesResult::with_all_items(vec![
        make_resource(
            URI_CARD_SCHEMA,
            "Card Schema & Examples",
            "CardContent JSON schema with validation rules and 3 annotated examples \
             (simple, rich-text styled, language-learning with dictionary). \
             Read this before creating cards to avoid rich_text validation failures.",
            "text/plain",
        ),
        make_resource(
            URI_CATALOGS,
            "My Catalogs",
            "All flashcard catalogs (id, name, card_count).",
            "application/json",
        ),
        make_resource(
            URI_DUE,
            "Due Cards",
            "Cards due for review today (card_id, face_text).",
            "application/json",
        ),
        make_resource(
            URI_STATS,
            "Learning Stats",
            "Spaced-repetition stats: due_count and total_count.",
            "application/json",
        ),
        make_resource(
            URI_LEARNING_PATHS,
            "Learning Paths",
            "All learning paths (id, name).",
            "application/json",
        ),
        make_resource(
            URI_SUBSCRIPTION,
            "Subscription",
            "User subscription plan information.",
            "application/json",
        ),
    ])
}

// ── Read dispatch ─────────────────────────────────────────────────────────────

pub async fn read(
    client: &EngramClient,
    params: ReadResourceRequestParams,
) -> Result<ReadResourceResult, rmcp::model::ErrorData> {
    match params.uri.as_str() {
        URI_CARD_SCHEMA => Ok(read_card_schema()),
        URI_CATALOGS => fetch_as_result(read_catalogs(client).await, params.uri),
        URI_DUE => fetch_as_result(read_due(client).await, params.uri),
        URI_STATS => fetch_as_result(read_stats(client).await, params.uri),
        URI_LEARNING_PATHS => fetch_as_result(read_learning_paths(client).await, params.uri),
        URI_SUBSCRIPTION => fetch_as_result(read_subscription(client).await, params.uri),
        other => Err(rmcp::model::ErrorData::invalid_params(
            format!("Unknown resource URI: {other}"),
            None,
        )),
    }
}

fn fetch_as_result(
    result: Result<String, ApiError>,
    uri: String,
) -> Result<ReadResourceResult, rmcp::model::ErrorData> {
    let text = result.map_err(|e| rmcp::model::ErrorData::internal_error(e.to_string(), None))?;
    Ok(ReadResourceResult::new(vec![ResourceContents::text(
        text, uri,
    )]))
}

// ── Per-resource fetchers ─────────────────────────────────────────────────────

async fn read_catalogs(client: &EngramClient) -> Result<String, ApiError> {
    let resp = client.list_catalogs(Some(100), None).await?;
    let summaries: Vec<CatalogSummary> = resp.data.into_iter().map(Into::into).collect();
    Ok(serde_json::to_string(&summaries).unwrap_or_default())
}

async fn read_due(client: &EngramClient) -> Result<String, ApiError> {
    let resp = client.get_due_cards(Some(50), None).await?;
    let summaries: Vec<DueCardSummary> = resp.data.into_iter().map(Into::into).collect();
    Ok(serde_json::to_string(&summaries).unwrap_or_default())
}

async fn read_stats(client: &EngramClient) -> Result<String, ApiError> {
    let (due, total) =
        tokio::try_join!(client.learning_cards_count(), client.learning_cards_total())?;
    let stats = LearningStats {
        due_count: due,
        total_count: total,
    };
    Ok(serde_json::to_string(&stats).unwrap_or_default())
}

async fn read_learning_paths(client: &EngramClient) -> Result<String, ApiError> {
    let resp = client.list_learning_paths(Some(100), None).await?;
    let summaries: Vec<LearningPathSummary> = resp.data.into_iter().map(Into::into).collect();
    Ok(serde_json::to_string(&summaries).unwrap_or_default())
}

async fn read_subscription(client: &EngramClient) -> Result<String, ApiError> {
    let sub = client.get_subscription().await?;
    Ok(serde_json::to_string(&sub).unwrap_or_default())
}

fn read_card_schema() -> ReadResourceResult {
    ReadResourceResult::new(vec![ResourceContents::text(
        CARD_SCHEMA_DOC,
        URI_CARD_SCHEMA,
    )])
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_all_returns_six_resources() {
        let result = list_all();
        assert_eq!(result.resources.len(), 6);
        let uris: Vec<&str> = result.resources.iter().map(|r| r.uri.as_str()).collect();
        assert!(uris.contains(&URI_CARD_SCHEMA));
        assert!(uris.contains(&URI_CATALOGS));
        assert!(uris.contains(&URI_DUE));
        assert!(uris.contains(&URI_STATS));
        assert!(uris.contains(&URI_LEARNING_PATHS));
        assert!(uris.contains(&URI_SUBSCRIPTION));
    }

    #[test]
    fn test_list_all_data_resources_have_json_mime_type() {
        let result = list_all();
        for r in &result.resources {
            let expected = if r.uri == URI_CARD_SCHEMA {
                "text/plain"
            } else {
                "application/json"
            };
            assert_eq!(
                r.mime_type.as_deref(),
                Some(expected),
                "resource {} has wrong mime type",
                r.uri
            );
        }
    }

    #[tokio::test]
    async fn test_read_card_schema_contains_examples() {
        let client = EngramClient::new("http://localhost:9999", "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_CARD_SCHEMA))
            .await
            .unwrap();
        assert_eq!(result.contents.len(), 1);
        if let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] {
            assert!(text.contains("rich_text"), "{text}");
            assert!(text.contains("dictionary"), "{text}");
            assert!(text.contains("#27AE60"), "{text}");
            assert!(text.contains("R4"), "{text}");
        } else {
            panic!("expected text resource contents");
        }
    }

    #[tokio::test]
    async fn test_read_unknown_uri_returns_invalid_params() {
        let client = EngramClient::new("http://localhost:9999", "tok");
        let params = ReadResourceRequestParams::new("engram://unknown");
        let err = read(&client, params).await.unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_read_catalogs_ok() {
        use serde_json::json;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": "00000000-0000-0000-0000-000000000001", "name": "Rust", "version": 1, "card_count": 5}],
                "cursor": null
            })))
            .mount(&server)
            .await;

        let client = EngramClient::new(&server.uri(), "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_CATALOGS))
            .await
            .unwrap();
        assert_eq!(result.contents.len(), 1);
        if let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] {
            assert!(text.contains("Rust"), "{text}");
        } else {
            panic!("expected text resource contents");
        }
    }

    #[tokio::test]
    async fn test_read_stats_ok() {
        use serde_json::json;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning/cards/count"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(7)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/learning/cards/total"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(42)))
            .mount(&server)
            .await;

        let client = EngramClient::new(&server.uri(), "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_STATS))
            .await
            .unwrap();
        if let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] {
            assert!(text.contains("7"), "{text}");
            assert!(text.contains("42"), "{text}");
        } else {
            panic!("expected text resource contents");
        }
    }

    #[tokio::test]
    async fn test_read_due_api_error_returns_internal_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning/cards"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = EngramClient::new(&server.uri(), "tok");
        let err = read(&client, ReadResourceRequestParams::new(URI_DUE))
            .await
            .unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
    }
}
