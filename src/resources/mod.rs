//! MCP Resources — static URI handles that expose live Engramo data as context.
//!
//! Resources are read by URI; the server fetches the relevant data from the API
//! and returns it as plain JSON text so the LLM can inject it into context.
//!
//! Available resources:
//!   engramo://card-schema           — CardContent JSON schema, validation rules, 4 examples
//!   engramo://catalogs              — all catalogs (id, name, card_count)
//!   engramo://learning/due          — due cards for today (card_id, face_text)
//!   engramo://learning/stats        — learning stats (due_count, total_count)
//!   engramo://learning-paths        — all learning paths (id, name)
//!   engramo://subscription          — user subscription / plan info

use rmcp::model::{
    ListResourcesResult, ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents,
};

use std::time::Duration;

use crate::client::{EngramoClient, MAX_PAGE_LIMIT};
use crate::dto::{
    CatalogSummary, DueCardSummary, LearningPathSummary, LearningStats, PagedResponse,
};
use crate::error::ApiError;

// ── URI constants ─────────────────────────────────────────────────────────────

pub const URI_CARD_SCHEMA: &str = "engramo://card-schema";
pub const URI_CATALOGS: &str = "engramo://catalogs";
pub const URI_DUE: &str = "engramo://learning/due";
pub const URI_STATS: &str = "engramo://learning/stats";
pub const URI_LEARNING_PATHS: &str = "engramo://learning-paths";
pub const URI_SUBSCRIPTION: &str = "engramo://subscription";

// ── Card schema document ──────────────────────────────────────────────────────

const CARD_SCHEMA_DOC: &str = "\
=== Engramo CardContent — Schema & Examples ===

CardContent fields:
  text         String     REQUIRED. The full plain text. This is the validation anchor.
  richText     Span[]     Optional. Styled segments. MUST concatenate to `text` exactly — the
                          server validates this and silently discards richText on mismatch.
  dictionary   {str:str}  Optional. Word-level translation map (face only). Keys: lowercase
                          source word. Values: primary translation; append \"; (here) X\" if
                          contextual meaning differs from the most common meaning.
  style        CardStyle  Optional. Card-level default style (font, color, alignment).
  audioId      UUID       Optional. A media_id from the `upload_media` tool (e.g. the user's own
                          voice recording). Never fabricate a UUID here.
  visualId     UUID       Optional. A media_id from `upload_media` for an image/video. Requires
                          `visualType` alongside it. Never fabricate a UUID here.
  visualType   String     Required if `visualId` is set. \"image\" or \"video\".

RichTextSpan fields:
  text         String     REQUIRED. A VERBATIM contiguous slice of the parent `text`.
                          Never insert extra characters, markers, or spaces not in the original.
  style        SpanStyle  Optional.

SpanStyle fields:
  fontColor     String     CSS color. \"#27AE60\" = green (correct/highlight), \"#E74C3C\" = red (warning).
  fontSize      int        Font size in points.
  bold          bool
  italic        bool
  underline     bool
  strikethrough bool
  superscript   bool
  subscript     bool
  fontFamily    String     Use \"monospace\" for code.

Rich-text validation rules (CRITICAL):
  R1. Set `text` to the complete original sentence first.
  R2. Every span.text must be a verbatim substring of `text`.
  R3. Spans must cover ALL of `text` — no gaps, no extra characters.
  R4. Concatenation of all span.text values must equal `text` exactly (character-for-character).
  R5. If no styling is needed, omit `richText` entirely.

=== Example 1: Simple card (no richText) ===
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
    \"richText\": [
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
    \"richText\": [
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
    \"richText\": [
      { \"text\": \"Я поговорив з мамою цього ранку по телефону. (\" },
      { \"text\": \"hablar - hablado\", \"style\": { \"fontColor\": \"#27AE60\" } },
      { \"text\": \")\" }
    ]
  }
}

Span checks:
  face: \"He hablado\" + \" con mi madre esta mañana por teléfono.\" = face.text ✓
  back: \"Я поговорив з мамою цього ранку по телефону. (\" + \"hablar - hablado\" + \")\" = back.text ✓

=== Example 4: Card with user-supplied audio (BYO audio, no server-side TTS) ===
The user already recorded their own voice for this card's face and gave you the file. You called
`upload_media` first (content_base64 of the recording, content_type=\"audio/mpeg\") and got back
media_id \"3fa85f64-5717-4562-b3fc-2c963f66afa6\" — use that value for `audioId` below. Never
invent a UUID; only use one actually returned by `upload_media`.
(A face's `audioId` can also be set this same way by the generate_card_audio tool, when it's
available — it synthesizes speech locally with the user's own TTS key, uploads it, and attaches it
for you, so you don't need to call upload_media yourself in that case.)

{
  \"face\": {
    \"text\": \"¿Me puede traer un café con leche, por favor?\",
    \"audioId\": \"3fa85f64-5717-4562-b3fc-2c963f66afa6\"
  },
  \"back\": { \"text\": \"Could you bring me a coffee with milk, please?\" }
}\
";

// ── Resource list ─────────────────────────────────────────────────────────────

fn make_resource(uri: &str, name: &str, description: &str, mime_type: &str) -> Resource {
    Resource::new(uri, name)
        .with_description(description)
        .with_mime_type(mime_type)
}

pub fn list_all() -> ListResourcesResult {
    ListResourcesResult::with_all_items(vec![
        make_resource(
            URI_CARD_SCHEMA,
            "Card Schema & Examples",
            "CardContent JSON schema with validation rules and 4 annotated examples \
             (simple, rich-text styled, language-learning with dictionary, user-supplied audio). \
             Read this before creating cards to avoid richText validation failures.",
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
    client: &EngramoClient,
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

/// Page cap so a misbehaving upstream that keeps returning a cursor can't loop forever.
const RESOURCE_MAX_PAGES: usize = 20;
/// Hard cap on items one resource read aggregates (20 pages x 50 is the honest maximum). The
/// upstream's per-page limit isn't enforced on our side, so this bounds memory too.
const RESOURCE_MAX_ITEMS: usize = RESOURCE_MAX_PAGES * MAX_PAGE_LIMIT as usize;
/// Overall deadline for one paginated resource read; the page cap bounds iterations, not time.
const RESOURCE_READ_DEADLINE: Duration = Duration::from_secs(60);

/// Follows `nextCursor` until the data ends, aggregating every page into `S` summaries.
/// Stops (and logs a warning) when the page cap, the item cap or a non-advancing cursor is
/// hit, and fails the whole read if the overall deadline passes or any page errors — a later
/// page's error must not be papered over by returning a partial result as if complete.
async fn collect_pages<D, S, F, Fut>(mut fetch: F) -> Result<Vec<S>, ApiError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<PagedResponse<D>, ApiError>>,
    S: From<D>,
{
    tokio::time::timeout(RESOURCE_READ_DEADLINE, async {
        let mut out: Vec<S> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..RESOURCE_MAX_PAGES {
            let resp = fetch(cursor.clone()).await?;
            let room = RESOURCE_MAX_ITEMS.saturating_sub(out.len());
            let over_item_cap = resp.data.len() > room;
            out.extend(resp.data.into_iter().take(room).map(S::from));
            if over_item_cap || out.len() >= RESOURCE_MAX_ITEMS {
                if resp.cursor.is_some() || over_item_cap {
                    tracing::warn!(
                        max_items = RESOURCE_MAX_ITEMS,
                        "resource item cap reached; result truncated"
                    );
                }
                return Ok(out);
            }
            let Some(next) = resp.cursor else {
                return Ok(out);
            };
            if cursor.as_deref() == Some(next.as_str()) {
                tracing::warn!("upstream returned a non-advancing cursor; result truncated");
                return Ok(out);
            }
            cursor = Some(next);
        }
        tracing::warn!(
            max_pages = RESOURCE_MAX_PAGES,
            "resource pagination cap reached; result truncated"
        );
        Ok(out)
    })
    .await
    .map_err(|_| {
        tracing::error!(
            deadline_secs = RESOURCE_READ_DEADLINE.as_secs(),
            "resource read deadline exceeded"
        );
        ApiError::Timeout("resource read; use the list tools with a cursor instead")
    })?
}

async fn read_catalogs(client: &EngramoClient) -> Result<String, ApiError> {
    let summaries: Vec<CatalogSummary> = collect_pages(|c| async move {
        client
            .list_catalogs(Some(MAX_PAGE_LIMIT), c.as_deref())
            .await
    })
    .await?;
    Ok(serde_json::to_string(&summaries).unwrap_or_default())
}

async fn read_due(client: &EngramoClient) -> Result<String, ApiError> {
    let resp = client.get_due_cards(Some(MAX_PAGE_LIMIT), None).await?;
    let summaries: Vec<DueCardSummary> = resp.data.into_iter().map(Into::into).collect();
    Ok(serde_json::to_string(&summaries).unwrap_or_default())
}

async fn read_stats(client: &EngramoClient) -> Result<String, ApiError> {
    let (due, total) =
        tokio::try_join!(client.learning_cards_count(), client.learning_cards_total())?;
    let stats = LearningStats {
        due_count: due,
        total_count: total,
    };
    Ok(serde_json::to_string(&stats).unwrap_or_default())
}

async fn read_learning_paths(client: &EngramoClient) -> Result<String, ApiError> {
    let summaries: Vec<LearningPathSummary> = collect_pages(|c| async move {
        client
            .list_learning_paths(Some(MAX_PAGE_LIMIT), c.as_deref())
            .await
    })
    .await?;
    Ok(serde_json::to_string(&summaries).unwrap_or_default())
}

async fn read_subscription(client: &EngramoClient) -> Result<String, ApiError> {
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

    #[tokio::test(start_paused = true)]
    async fn test_collect_pages_deadline_returns_timeout() {
        let result: Result<Vec<CatalogSummary>, ApiError> = collect_pages(|_c| async {
            std::future::pending::<Result<PagedResponse<crate::dto::CatalogDto>, ApiError>>().await
        })
        .await;
        match result {
            Err(ApiError::Timeout(msg)) => assert!(msg.contains("resource read"), "{msg}"),
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

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
        let client = EngramoClient::new("http://localhost:9999", "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_CARD_SCHEMA))
            .await
            .unwrap();
        assert_eq!(result.contents.len(), 1);
        if let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] {
            assert!(text.contains("richText"), "{text}");
            assert!(text.contains("dictionary"), "{text}");
            assert!(text.contains("#27AE60"), "{text}");
            assert!(text.contains("R4"), "{text}");
            assert!(text.contains("upload_media"), "{text}");
            assert!(text.contains("visualId"), "{text}");
            assert!(!text.contains("audio_id"), "{text}");
            assert!(!text.contains("rich_text"), "{text}");
        } else {
            panic!("expected text resource contents");
        }
    }

    #[tokio::test]
    async fn test_read_card_schema_lists_all_span_style_fields() {
        let client = EngramoClient::new("http://localhost:9999", "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_CARD_SCHEMA))
            .await
            .unwrap();
        if let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] {
            for f in [
                "bold",
                "italic",
                "underline",
                "strikethrough",
                "superscript",
                "subscript",
                "fontSize",
                "fontColor",
                "fontFamily",
            ] {
                assert!(text.contains(f), "{f} missing from card-schema doc: {text}");
            }
            // Nested-`style` guidance (issue #37): styling shown nested under `style`, not flat.
            assert!(text.contains("\"style\": { \"fontColor\""), "{text}");
        } else {
            panic!("expected text resource contents");
        }
    }

    #[tokio::test]
    async fn test_read_unknown_uri_returns_invalid_params() {
        let client = EngramoClient::new("http://localhost:9999", "tok");
        let params = ReadResourceRequestParams::new("engramo://unknown");
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
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let client = EngramoClient::new(server.uri(), "tok");
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

        let client = EngramoClient::new(server.uri(), "tok");
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

        let client = EngramoClient::new(server.uri(), "tok");
        let err = read(&client, ReadResourceRequestParams::new(URI_DUE))
            .await
            .unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
    }

    fn resource_text(result: &ReadResourceResult) -> &str {
        match &result.contents[0] {
            ResourceContents::TextResourceContents { text, .. } => text.as_str(),
            _ => panic!("expected text resource contents"),
        }
    }

    #[tokio::test]
    async fn test_read_catalogs_follows_cursor_across_pages() {
        use serde_json::json;
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .and(query_param("limit", "50"))
            .and(query_param("cursor", "c1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": "00000000-0000-0000-0000-000000000002", "name": "SecondPage", "version": 1}],
                "nextCursor": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .and(query_param("limit", "50"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": "00000000-0000-0000-0000-000000000001", "name": "FirstPage", "version": 1}],
                "nextCursor": "c1"
            })))
            .mount(&server)
            .await;

        let client = EngramoClient::new(server.uri(), "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_CATALOGS))
            .await
            .unwrap();
        let text = resource_text(&result);
        assert!(text.contains("FirstPage"), "{text}");
        assert!(text.contains("SecondPage"), "{text}");
    }

    /// Responds with an empty page and a fresh cursor every time (`c0`, `c1`, ...), i.e. an
    /// upstream that never ends but whose cursor does advance.
    struct EndlessPages(std::sync::atomic::AtomicUsize);

    impl wiremock::Respond for EndlessPages {
        fn respond(&self, _: &wiremock::Request) -> wiremock::ResponseTemplate {
            let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [], "nextCursor": format!("c{n}")
            }))
        }
    }

    #[tokio::test]
    async fn test_read_catalogs_stops_at_page_cap_when_cursor_never_ends() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(EndlessPages(Default::default()))
            .mount(&server)
            .await;

        let client = EngramoClient::new(server.uri(), "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_CATALOGS))
            .await
            .unwrap();
        assert_eq!(resource_text(&result), "[]");
        let requests = server.received_requests().await.unwrap_or_default();
        assert_eq!(requests.len(), RESOURCE_MAX_PAGES);
    }

    #[tokio::test]
    async fn test_read_catalogs_stops_when_cursor_does_not_advance() {
        use serde_json::json;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [], "nextCursor": "again"
            })))
            .mount(&server)
            .await;

        let client = EngramoClient::new(server.uri(), "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_CATALOGS))
            .await
            .unwrap();
        assert_eq!(resource_text(&result), "[]");
        // Page 1 (no cursor) yields "again"; page 2 sends it and gets "again" back -> stop.
        let requests = server.received_requests().await.unwrap_or_default();
        assert_eq!(requests.len(), 2);
    }

    #[tokio::test]
    async fn test_read_catalogs_caps_total_items() {
        use serde_json::json;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let data: Vec<_> = (0..RESOURCE_MAX_ITEMS + 1)
            .map(|i| {
                json!({
                    "id": format!("00000000-0000-0000-0000-{i:012}"),
                    "name": format!("Cat{i}"),
                    "version": 1
                })
            })
            .collect();
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": data, "nextCursor": "more"
            })))
            .mount(&server)
            .await;

        let client = EngramoClient::new(server.uri(), "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_CATALOGS))
            .await
            .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(resource_text(&result)).unwrap();
        assert_eq!(items.len(), RESOURCE_MAX_ITEMS);
        let requests = server.received_requests().await.unwrap_or_default();
        assert_eq!(requests.len(), 1, "must stop once the item cap is reached");
    }

    #[tokio::test]
    async fn test_read_catalogs_error_on_second_page_returns_internal_error() {
        use serde_json::json;
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .and(query_param("cursor", "c1"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": "00000000-0000-0000-0000-000000000001", "name": "FirstPage", "version": 1}],
                "nextCursor": "c1"
            })))
            .mount(&server)
            .await;

        let client = EngramoClient::new(server.uri(), "tok");
        let err = read(&client, ReadResourceRequestParams::new(URI_CATALOGS))
            .await
            .unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
    }

    #[tokio::test]
    async fn test_read_learning_paths_follows_cursor_across_pages() {
        use serde_json::json;
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning-paths"))
            .and(query_param("limit", "50"))
            .and(query_param("cursor", "c1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": "00000000-0000-0000-0000-000000000002", "name": "SecondPath", "version": 1}],
                "nextCursor": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/learning-paths"))
            .and(query_param("limit", "50"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": "00000000-0000-0000-0000-000000000001", "name": "FirstPath", "version": 1}],
                "nextCursor": "c1"
            })))
            .mount(&server)
            .await;

        let client = EngramoClient::new(server.uri(), "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_LEARNING_PATHS))
            .await
            .unwrap();
        let text = resource_text(&result);
        assert!(text.contains("FirstPath"), "{text}");
        assert!(text.contains("SecondPath"), "{text}");
        let requests = server.received_requests().await.unwrap_or_default();
        assert_eq!(requests.len(), 2);
    }

    #[tokio::test]
    async fn test_read_learning_paths_stops_at_page_cap() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning-paths"))
            .respond_with(EndlessPages(Default::default()))
            .mount(&server)
            .await;

        let client = EngramoClient::new(server.uri(), "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_LEARNING_PATHS))
            .await
            .unwrap();
        assert_eq!(resource_text(&result), "[]");
        let requests = server.received_requests().await.unwrap_or_default();
        assert_eq!(requests.len(), RESOURCE_MAX_PAGES);
    }

    #[tokio::test]
    async fn test_read_due_ok_with_real_wire_shape() {
        use serde_json::json;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning/cards"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": "00000000-0000-0000-0000-000000000001",
                    "face": {"text": "Q"},
                    "back": {"text": "A"},
                    "nextReview": "2026-09-26T00:00:00Z"
                }],
                "nextCursor": null,
                "total": 1
            })))
            .mount(&server)
            .await;

        let client = EngramoClient::new(server.uri(), "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_DUE))
            .await
            .unwrap();
        let text = resource_text(&result);
        assert!(text.contains("\"face_text\":\"Q\""), "{text}");
        assert!(
            text.contains("00000000-0000-0000-0000-000000000001"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn test_read_due_decode_error_returns_internal_error_with_field_name() {
        use serde_json::json;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning/cards"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let client = EngramoClient::new(server.uri(), "tok");
        let err = read(&client, ReadResourceRequestParams::new(URI_DUE))
            .await
            .unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
        assert!(err.message.contains("total"), "{}", err.message);
    }

    #[tokio::test]
    async fn test_read_learning_paths_ok() {
        use serde_json::json;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learning-paths"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": "00000000-0000-0000-0000-000000000001", "name": "Path 1", "version": 1}],
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let client = EngramoClient::new(server.uri(), "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_LEARNING_PATHS))
            .await
            .unwrap();
        assert_eq!(result.contents.len(), 1);
        let text = resource_text(&result);
        assert!(text.contains("Path 1"), "{text}");
    }

    #[tokio::test]
    async fn test_read_subscription_ok() {
        use serde_json::json;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me/subscription"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"plan_id": "pro"})))
            .mount(&server)
            .await;

        let client = EngramoClient::new(server.uri(), "tok");
        let result = read(&client, ReadResourceRequestParams::new(URI_SUBSCRIPTION))
            .await
            .unwrap();
        assert_eq!(result.contents.len(), 1);
        let text = resource_text(&result);
        assert!(text.contains("pro"), "{text}");
    }
}
