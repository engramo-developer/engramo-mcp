use serde::Deserialize;

use crate::dto::CardContent;

// ── Params ────────────────────────────────────────────────────────────────────

/// Params for `generate_card`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GenerateCardParams {
    /// UUID of an existing catalog to add the card to.
    /// Omit to use the user's default catalog.
    #[schemars(description = "UUID of an existing catalog. Omit to use the default catalog.")]
    pub catalog_id: Option<String>,

    /// Front of the card.
    ///
    /// Read `engramo://card-schema` for the complete schema, validation rules, and 4 annotated examples.
    ///
    /// Rich-text span rules (IMPORTANT):
    /// - Each span.text must be a VERBATIM contiguous slice of the original sentence.
    ///   NEVER insert separator characters, markers, or non-original characters.
    /// - Spans partition the original text with no gaps and no extra characters.
    /// - ALWAYS set `text` to the full plain sentence — it's the validation anchor.
    ///   If richText spans are also provided, their concatenation must equal `text` exactly.
    ///   The server validates this and discards richText if they disagree.
    /// - Styling goes under a nested `style` object, never as flat fields on the span:
    ///   `{"text":"gracias.","style":{"bold":true,"fontColor":"#27AE60"}}`.
    ///   fontFamily="monospace" for code; bold=true for key terms;
    ///   fontColor="#E74C3C" for warnings, "#27AE60" for correct answers.
    /// - Omit richText entirely if no styling is needed.
    ///
    /// For language-learning cards, set `face.dictionary` yourself (word → translation map).
    ///
    /// For audio/images: EngrAmo does NOT generate these for you. If the user already has an
    /// audio recording or image (their own voice, a self-generated file, a picture they gave
    /// you), call `upload_media` first and set `audioId`/`visualId` (+ `visualType`) to the
    /// returned `media_id`. Never fabricate a UUID for either field.
    pub face: CardContent,

    /// Back of the card (answer / explanation).
    ///
    /// Read `engramo://card-schema` for the complete schema, validation rules, and 4 annotated examples.
    ///
    /// Same rich-text rules as `face`: always set `text`; spans must match it exactly.
    /// For language-learning cards, translate `face.text` yourself and set `back.text` —
    /// do not leave it empty. Same `upload_media` rule as `face` for `audioId`/`visualId`.
    pub back: CardContent,
}

/// A single card entry inside [`GenerateCatalogWithCardsParams`].
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CardInputParams {
    /// Front of the card. Read `engramo://card-schema` for the complete schema, validation rules,
    /// and 4 annotated examples. Always set `text` to the full sentence; spans must concatenate
    /// to match `text` exactly. Styling goes under a nested `style` object, e.g.
    /// `{"text":"gracias.","style":{"bold":true,"fontColor":"#27AE60"}}`.
    pub face: CardContent,
    /// Back of the card. Read `engramo://card-schema` for the complete schema, validation rules,
    /// and 4 annotated examples. Same rich-text rules as `face`.
    pub back: CardContent,
}

/// Params for `generate_catalog_with_cards`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GenerateCatalogWithCardsParams {
    #[schemars(description = "Catalog name")]
    pub name: String,
    #[schemars(description = "Optional description")]
    pub description: Option<String>,
    /// UUID of a stored image asset for the catalog's cover. Set this to a `media_id`
    /// returned by `upload_media` (e.g. an image the user provided). Omit if no cover
    /// image was requested — never fabricate a UUID here.
    #[schemars(description = "Optional cover image media_id, from upload_media")]
    pub image_id: Option<String>,
    #[schemars(description = "Optional list of tags")]
    pub tags: Option<Vec<String>>,
    #[schemars(description = "Visibility: 'public' or 'private' (default: 'private')")]
    pub visibility: Option<String>,
    /// All cards to create. Each card must follow the same rich-text rules as `generate_card`:
    /// always set `text`; span concatenation must equal `text`.
    /// For language-learning cards, set `face.dictionary` (word → translation map) and
    /// `back.text` (translation) on each card yourself before calling this tool.
    pub cards: Vec<CardInputParams>,
}

/// Params for `generate_cards` — add multiple cards to an existing catalog in one batch.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GenerateCardsParams {
    /// UUID of the existing catalog to add cards to.
    pub catalog_id: String,
    /// Cards to create. Same rich-text rules as `generate_catalog_with_cards.cards`.
    /// For language-learning cards, set `face.dictionary` (word → translation map) and
    /// `back.text` (translation) on each card yourself before calling this tool.
    pub cards: Vec<CardInputParams>,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::EngramoClient;
    use crate::server::EngramoMcpServer;
    use rmcp::handler::server::wrapper::Parameters;
    use serde_json::json;
    use uuid::Uuid;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mock_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn make_server(base_url: &str) -> EngramoMcpServer {
        EngramoMcpServer::new(EngramoClient::new(base_url, "engramo_test"), false)
    }

    fn card_dto_json() -> serde_json::Value {
        json!({
            "id": mock_id(),
            "version": 1,
            "face": { "text": "What is ownership?" },
            "back": { "text": "Every value has one owner." },
            "orderNumber": 1
        })
    }

    fn catalog_dto_json() -> serde_json::Value {
        json!({
            "id": mock_id(),
            "name": "Rust",
            "version": 1,
            "card_count": 0
        })
    }

    // ── sanitization ──────────────────────────────────────────────────────────

    async fn card_request_body(server: &MockServer) -> serde_json::Value {
        let requests = server.received_requests().await.unwrap();
        let req = requests
            .iter()
            .find(|r| r.url.path() == "/cards")
            .expect("POST /cards not called");
        serde_json::from_slice(&req.body).unwrap()
    }

    #[tokio::test]
    async fn test_generate_card_strips_tab_from_plain_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        make_server(&server.uri())
            .generate_card(Parameters(GenerateCardParams {
                catalog_id: None,
                face: CardContent::plain("Ellos \testán"),
                back: CardContent::plain("They\tare"),
            }))
            .await
            .unwrap();

        let body = card_request_body(&server).await;
        assert_eq!(body["face"]["text"].as_str(), Some("Ellos están"));
        assert_eq!(body["back"]["text"].as_str(), Some("Theyare"));
    }

    #[tokio::test]
    async fn test_generate_card_strips_tab_from_rich_text_span() {
        use crate::dto::RichTextSpan;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        let face = CardContent {
            text: String::new(), // derived from spans
            rich_text: Some(vec![
                RichTextSpan {
                    text: "Ellos \t".to_string(),
                    style: None,
                },
                RichTextSpan {
                    text: "están viviendo".to_string(),
                    style: Some(crate::dto::RichTextSpanStyle {
                        bold: Some(true),
                        ..Default::default()
                    }),
                },
                RichTextSpan {
                    text: " en Madrid.".to_string(),
                    style: None,
                },
            ]),
            style: None,
            dictionary: None,
            audio_id: None,
            visual_id: None,
            visual_type: None,
        };

        make_server(&server.uri())
            .generate_card(Parameters(GenerateCardParams {
                catalog_id: None,
                face,
                back: CardContent::plain("They are living in Madrid."),
            }))
            .await
            .unwrap();

        let body = card_request_body(&server).await;
        // Tab stripped from span → text derived from clean spans.
        assert_eq!(
            body["face"]["text"].as_str(),
            Some("Ellos están viviendo en Madrid."),
            "text must be derived from sanitized spans"
        );
        assert_eq!(
            body["face"]["richText"][0]["text"].as_str(),
            Some("Ellos "),
            "tab must be stripped from span text"
        );
    }

    #[tokio::test]
    async fn test_generate_card_strips_emoji_from_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        make_server(&server.uri())
            .generate_card(Parameters(GenerateCardParams {
                catalog_id: None,
                face: CardContent::plain("Hello 🌍 world"),
                back: CardContent::plain("Answer 👍"),
            }))
            .await
            .unwrap();

        let body = card_request_body(&server).await;
        assert_eq!(body["face"]["text"].as_str(), Some("Hello  world"));
        assert_eq!(body["back"]["text"].as_str(), Some("Answer "));
    }

    #[tokio::test]
    async fn test_generate_card_strips_control_chars_keeps_newline() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        // \r\n line ending → \r stripped, \n kept; \x00 (null) stripped.
        make_server(&server.uri())
            .generate_card(Parameters(GenerateCardParams {
                catalog_id: None,
                face: CardContent::plain("line1\r\nline2\x00end"),
                back: CardContent::plain("ok"),
            }))
            .await
            .unwrap();

        let body = card_request_body(&server).await;
        assert_eq!(body["face"]["text"].as_str(), Some("line1\nline2end"));
    }

    #[tokio::test]
    async fn test_generate_catalog_with_cards_sanitizes_tabs() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/with-cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "catalog": catalog_dto_json(),
                "cardsCreated": 1
            })))
            .mount(&server)
            .await;

        make_server(&server.uri())
            .generate_catalog_with_cards(Parameters(GenerateCatalogWithCardsParams {
                name: "Test".to_string(),
                description: None,
                image_id: None,
                tags: None,
                visibility: None,
                cards: vec![CardInputParams {
                    face: CardContent::plain("Ellos \testán"),
                    back: CardContent::plain("They\tare"),
                }],
            }))
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let req = requests
            .iter()
            .find(|r| r.url.path() == "/catalogs/with-cards")
            .expect("POST /catalogs/with-cards not called");
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(
            body["cards"][0]["face"]["text"].as_str(),
            Some("Ellos están")
        );
        assert_eq!(body["cards"][0]["back"]["text"].as_str(), Some("Theyare"));
    }

    #[tokio::test]
    async fn test_generate_catalog_with_cards_threads_image_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/with-cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "catalog": catalog_dto_json(),
                "cardsCreated": 1
            })))
            .mount(&server)
            .await;

        make_server(&server.uri())
            .generate_catalog_with_cards(Parameters(GenerateCatalogWithCardsParams {
                name: "Ordering Coffee".to_string(),
                description: None,
                image_id: Some("3fa85f64-5717-4562-b3fc-2c963f66afa6".to_string()),
                tags: None,
                visibility: None,
                cards: vec![CardInputParams {
                    face: CardContent::plain("Face"),
                    back: CardContent::plain("Back"),
                }],
            }))
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let req = requests
            .iter()
            .find(|r| r.url.path() == "/catalogs/with-cards")
            .expect("POST /catalogs/with-cards not called");
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(
            body["imageId"].as_str(),
            Some("3fa85f64-5717-4562-b3fc-2c963f66afa6")
        );
    }

    #[tokio::test]
    async fn test_generate_catalog_with_cards_omits_image_id_when_none() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/with-cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "catalog": catalog_dto_json(),
                "cardsCreated": 1
            })))
            .mount(&server)
            .await;

        make_server(&server.uri())
            .generate_catalog_with_cards(Parameters(GenerateCatalogWithCardsParams {
                name: "No Cover".to_string(),
                description: None,
                image_id: None,
                tags: None,
                visibility: None,
                cards: vec![CardInputParams {
                    face: CardContent::plain("Face"),
                    back: CardContent::plain("Back"),
                }],
            }))
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let req = requests
            .iter()
            .find(|r| r.url.path() == "/catalogs/with-cards")
            .expect("POST /catalogs/with-cards not called");
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert!(body.get("imageId").is_none());
    }

    // ── generate_card ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_generate_card_no_catalog_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .generate_card(Parameters(GenerateCardParams {
                catalog_id: None,
                face: CardContent::plain("What is ownership?"),
                back: CardContent::plain("Every value has one owner."),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_generate_card_with_catalog_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .generate_card(Parameters(GenerateCardParams {
                catalog_id: Some(mock_id().to_string()),
                face: CardContent::plain("What is ownership?"),
                back: CardContent::plain("Every value has one owner."),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_generate_card_invalid_catalog_uuid() {
        let server = MockServer::start().await;
        let result = make_server(&server.uri())
            .generate_card(Parameters(GenerateCardParams {
                catalog_id: Some("not-a-uuid".to_string()),
                face: CardContent::plain("Q"),
                back: CardContent::plain("A"),
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
        assert!(text.contains("Invalid UUID"), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_quota_exceeded_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(402).set_body_json(json!({
                "error": "quota_exceeded",
                "resource_type": "cards_total",
                "used": 100,
                "limit": 100
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .generate_card(Parameters(GenerateCardParams {
                catalog_id: None,
                face: CardContent::plain("Q"),
                back: CardContent::plain("A"),
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
            text.contains("402") || text.contains("quota") || text.contains("Quota"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn test_generate_card_unauthorized_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .generate_card(Parameters(GenerateCardParams {
                catalog_id: None,
                face: CardContent::plain("Q"),
                back: CardContent::plain("A"),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    // ── generate_catalog_with_cards ───────────────────────────────────────────

    #[tokio::test]
    async fn test_generate_catalog_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/with-cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "catalog": catalog_dto_json(),
                "cardsCreated": 2
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .generate_catalog_with_cards(Parameters(GenerateCatalogWithCardsParams {
                name: "Rust Basics".to_string(),
                description: None,
                image_id: None,
                tags: None,
                visibility: None,
                cards: vec![
                    CardInputParams {
                        face: CardContent::plain("What is ownership?"),
                        back: CardContent::plain("Every value has one owner."),
                    },
                    CardInputParams {
                        face: CardContent::plain("What is borrowing?"),
                        back: CardContent::plain("Temporary access without ownership."),
                    },
                ],
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_generate_catalog_quota_exceeded() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/with-cards"))
            .respond_with(ResponseTemplate::new(402).set_body_json(json!({
                "error": "quota_exceeded",
                "resource_type": "catalogs_total",
                "used": 10,
                "limit": 10
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .generate_catalog_with_cards(Parameters(GenerateCatalogWithCardsParams {
                name: "Overflow".to_string(),
                description: None,
                image_id: None,
                tags: None,
                visibility: None,
                cards: vec![CardInputParams {
                    face: CardContent::plain("Q"),
                    back: CardContent::plain("A"),
                }],
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
            text.contains("402") || text.contains("quota") || text.contains("Quota"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn test_generate_catalog_empty_cards_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/with-cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "catalog": catalog_dto_json(),
                "cardsCreated": 0
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .generate_catalog_with_cards(Parameters(GenerateCatalogWithCardsParams {
                name: "Empty Catalog".to_string(),
                description: None,
                image_id: None,
                tags: None,
                visibility: None,
                cards: vec![],
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_generate_card_passes_llm_dictionary_and_back_to_api() {
        // No /translate mock — the LLM pre-populates dictionary and back.text.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        let mut face = CardContent::plain("бігти");
        face.dictionary = Some([("бігти".to_string(), "to run".to_string())].into());

        make_server(&server.uri())
            .generate_card(Parameters(GenerateCardParams {
                catalog_id: None,
                face,
                back: CardContent::plain("to run"),
            }))
            .await
            .unwrap();

        let body = card_request_body(&server).await;
        assert_eq!(body["face"]["text"].as_str(), Some("бігти"));
        assert_eq!(body["back"]["text"].as_str(), Some("to run"));
        assert_eq!(
            body["face"]["dictionary"]["бігти"].as_str(),
            Some("to run"),
            "LLM-provided dictionary must pass through unchanged"
        );
    }

    #[tokio::test]
    async fn test_generate_catalog_passes_llm_dictionary_and_back_to_api() {
        // No /translate mock — the LLM pre-populates dictionary and back.text.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/with-cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "catalog": catalog_dto_json(),
                "cardsCreated": 1
            })))
            .mount(&server)
            .await;

        let mut face = CardContent::plain("бігти");
        face.dictionary = Some([("бігти".to_string(), "to run".to_string())].into());

        make_server(&server.uri())
            .generate_catalog_with_cards(Parameters(GenerateCatalogWithCardsParams {
                name: "UA Lang".to_string(),
                description: None,
                image_id: None,
                tags: None,
                visibility: None,
                cards: vec![CardInputParams {
                    face,
                    back: CardContent::plain("to run"),
                }],
            }))
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let req = requests
            .iter()
            .find(|r| r.url.path() == "/catalogs/with-cards")
            .expect("POST /catalogs/with-cards not called");
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["cards"][0]["back"]["text"].as_str(), Some("to run"));
        assert_eq!(
            body["cards"][0]["face"]["dictionary"]["бігти"].as_str(),
            Some("to run"),
            "LLM-provided dictionary must pass through unchanged"
        );
    }

    // ── update_card: preserves server-managed fields ───────────────────────────

    fn make_existing_card_json(audio_id: &str, dictionary: serde_json::Value) -> serde_json::Value {
        json!({
            "id": mock_id(),
            "version": 1,
            "face": {
                "text": "Quedo",
                "audioId": audio_id,
                "dictionary": dictionary
            },
            "back": {
                "text": "Чекаю",
                "audioId": "back-audio-id"
            },
            "orderNumber": 1
        })
    }

    #[tokio::test]
    async fn test_update_card_preserves_audio_id_from_existing_card() {
        use crate::tools::cards::UpdateCardParams;
        let server = MockServer::start().await;
        let card_id = mock_id();
        let audio_id = "00000000-0000-0000-0000-000000000099";

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(make_existing_card_json(audio_id, json!(null))),
            )
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: Some(CardContent::plain("Quedo actualizado")), // no audio_id set
                back: None,
                catalog_ids: vec![mock_id().to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));

        // The PATCH body must contain the preserved audioId.
        let patch_req = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.method == wiremock::http::Method::PATCH)
            .expect("PATCH request not made");
        let body: serde_json::Value = serde_json::from_slice(&patch_req.body).unwrap();
        assert_eq!(
            body["face"]["audioId"].as_str(),
            Some(audio_id),
            "audio_id must be preserved from existing card"
        );
    }

    #[tokio::test]
    async fn test_update_card_preserves_dictionary_from_existing_card() {
        use crate::tools::cards::UpdateCardParams;
        let server = MockServer::start().await;
        let card_id = mock_id();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(make_existing_card_json(
                    "audio-uuid",
                    json!({ "quedo": "I wait", "mañana": "tomorrow" }),
                )),
            )
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: Some(CardContent::plain("Quedo")), // no dictionary set
                back: None,
                catalog_ids: vec![mock_id().to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));

        let patch_req = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.method == wiremock::http::Method::PATCH)
            .expect("PATCH request not made");
        let body: serde_json::Value = serde_json::from_slice(&patch_req.body).unwrap();
        assert_eq!(
            body["face"]["dictionary"]["quedo"].as_str(),
            Some("I wait"),
            "dictionary must be preserved from existing card"
        );
    }

    #[tokio::test]
    async fn test_update_card_does_not_overwrite_explicitly_set_audio_id() {
        use crate::tools::cards::UpdateCardParams;
        let server = MockServer::start().await;
        let card_id = mock_id();
        let new_audio_id = "new-audio-id";

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(make_existing_card_json("old-audio-id", json!(null))),
            )
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        let mut face = CardContent::plain("Quedo");
        face.audio_id = Some(new_audio_id.to_string()); // explicitly set

        let result = make_server(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: Some(face),
                back: None,
                catalog_ids: vec![mock_id().to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));

        let patch_req = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.method == wiremock::http::Method::PATCH)
            .expect("PATCH request not made");
        let body: serde_json::Value = serde_json::from_slice(&patch_req.body).unwrap();
        assert_eq!(
            body["face"]["audioId"].as_str(),
            Some(new_audio_id),
            "explicitly set audio_id must not be overwritten by existing card's value"
        );
    }

    #[tokio::test]
    async fn test_update_card_preserves_back_audio_id_from_existing_card() {
        use crate::tools::cards::UpdateCardParams;
        let server = MockServer::start().await;
        let card_id = mock_id();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(make_existing_card_json("face-audio", json!(null))),
            )
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: None,
                back: Some(CardContent::plain("Nuevo")), // no audio_id set
                catalog_ids: vec![mock_id().to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));

        let patch_req = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.method == wiremock::http::Method::PATCH)
            .expect("PATCH request not made");
        let body: serde_json::Value = serde_json::from_slice(&patch_req.body).unwrap();
        assert_eq!(
            body["back"]["audioId"].as_str(),
            Some("back-audio-id"),
            "back.audio_id must be preserved from existing card"
        );
        assert_eq!(body["back"]["text"].as_str(), Some("Nuevo"));
    }

    #[tokio::test]
    async fn test_update_card_does_not_overwrite_explicitly_set_back_audio_id() {
        use crate::tools::cards::UpdateCardParams;
        let server = MockServer::start().await;
        let card_id = mock_id();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(make_existing_card_json("face-audio", json!(null))),
            )
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        let mut back = CardContent::plain("Nuevo");
        back.audio_id = Some("explicit".to_string());

        let result = make_server(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: None,
                back: Some(back),
                catalog_ids: vec![mock_id().to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));

        let patch_req = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.method == wiremock::http::Method::PATCH)
            .expect("PATCH request not made");
        let body: serde_json::Value = serde_json::from_slice(&patch_req.body).unwrap();
        assert_eq!(body["back"]["audioId"].as_str(), Some("explicit"));
    }

    #[tokio::test]
    async fn test_update_card_get_card_error_returns_error_result() {
        use crate::tools::cards::UpdateCardParams;
        let server = MockServer::start().await;
        let card_id = mock_id();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "not found"})))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: Some(CardContent::plain("Quedo")),
                back: None,
                catalog_ids: vec![mock_id().to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_update_card_skips_get_when_no_face_or_back_updated() {
        use crate::tools::cards::UpdateCardParams;
        let server = MockServer::start().await;
        let card_id = mock_id();
        // No GET mock — if it were called the test would fail (unmocked request).
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: None,
                back: None,
                catalog_ids: vec![mock_id().to_string()],
                order_number: 2,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_update_card_invalid_card_id_makes_no_request() {
        use crate::tools::cards::UpdateCardParams;
        let server = MockServer::start().await;

        let result = make_server(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: "not-a-uuid".to_string(),
                face: None,
                back: None,
                catalog_ids: vec![],
                order_number: 1,
                version: 1,
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
    async fn test_update_card_invalid_catalog_id_returns_error_without_patch() {
        use crate::tools::cards::UpdateCardParams;
        let server = MockServer::start().await;
        let card_id = mock_id();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(make_existing_card_json("face-audio", json!(null))),
            )
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: Some(CardContent::plain("x")),
                back: None,
                catalog_ids: vec!["bad".to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        let requests = server.received_requests().await.unwrap_or_default();
        assert!(
            !requests
                .iter()
                .any(|r| r.method == wiremock::http::Method::PATCH),
            "no PATCH must be sent when catalog_ids fails validation"
        );
    }

    #[tokio::test]
    async fn test_update_card_patch_conflict_after_merge_returns_error() {
        use crate::tools::cards::UpdateCardParams;
        let server = MockServer::start().await;
        let card_id = mock_id();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(make_existing_card_json("face-audio", json!(null))),
            )
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: Some(CardContent::plain("x")),
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

    // ── generate_cards ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_generate_cards_empty_list() {
        let server = MockServer::start().await;
        // No mocks needed — no translate/TTS/card calls expected.
        let result = make_server(&server.uri())
            .generate_cards(Parameters(GenerateCardsParams {
                catalog_id: mock_id().to_string(),
                cards: vec![],
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 0);
    }

    #[tokio::test]
    async fn test_generate_cards_invalid_catalog_uuid() {
        let server = MockServer::start().await;
        let result = make_server(&server.uri())
            .generate_cards(Parameters(GenerateCardsParams {
                catalog_id: "not-a-uuid".to_string(),
                cards: vec![],
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
        assert!(text.contains("Invalid UUID"), "{text}");
    }

    #[tokio::test]
    async fn test_generate_cards_create_card_failure_returns_error() {
        // First card creation succeeds; second fails (404). The handler must
        // return an error immediately. The first card is already persisted
        // (no transaction) — this is expected and documented behaviour.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "not found"})))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .generate_cards(Parameters(GenerateCardsParams {
                catalog_id: mock_id().to_string(),
                cards: vec![
                    CardInputParams {
                        face: CardContent::plain("cat"),
                        back: CardContent::plain("кіт"),
                    },
                    CardInputParams {
                        face: CardContent::plain("dog"),
                        back: CardContent::plain("собака"),
                    },
                ],
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_generate_cards_two_cards_ok_returns_array() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(card_dto_json()))
            .expect(2)
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .generate_cards(Parameters(GenerateCardsParams {
                catalog_id: mock_id().to_string(),
                cards: vec![
                    CardInputParams {
                        face: CardContent::plain("a\t"),
                        back: CardContent::plain("A"),
                    },
                    CardInputParams {
                        face: CardContent::plain("b"),
                        back: CardContent::plain("B"),
                    },
                ],
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
        let cards: Vec<serde_json::Value> = serde_json::from_str(text).unwrap();
        assert_eq!(cards.len(), 2, "{text}");

        let requests = server.received_requests().await.unwrap();
        let post_bodies: Vec<serde_json::Value> = requests
            .iter()
            .filter(|r| r.url.path() == "/cards")
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect();
        assert_eq!(post_bodies.len(), 2);
        for body in &post_bodies {
            assert_eq!(
                body["catalogId"].as_str(),
                Some(mock_id().to_string().as_str())
            );
        }
        assert_eq!(post_bodies[0]["face"]["text"].as_str(), Some("a"));
    }

    #[tokio::test]
    async fn test_update_card_normalizes_rich_text_before_merge() {
        use crate::dto::RichTextSpan;
        use crate::tools::cards::UpdateCardParams;
        let server = MockServer::start().await;
        let card_id = mock_id();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(make_existing_card_json("existing-audio", json!(null))),
            )
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        let mut face = CardContent::plain(""); // LLM leaves text empty
        face.rich_text = Some(vec![
            RichTextSpan {
                text: "Quedo".to_string(),
                style: None,
            },
            RichTextSpan {
                text: " actualizado".to_string(),
                style: None,
            },
        ]);

        let result = make_server(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: Some(face),
                back: None,
                catalog_ids: vec![mock_id().to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));

        let patch_req = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.method == wiremock::http::Method::PATCH)
            .expect("PATCH request not made");
        let body: serde_json::Value = serde_json::from_slice(&patch_req.body).unwrap();
        assert_eq!(
            body["face"]["text"].as_str(),
            Some("Quedo actualizado"),
            "text must be derived from richText spans"
        );
        assert_eq!(
            body["face"]["audioId"].as_str(),
            Some("existing-audio"),
            "audio_id must be preserved after normalization"
        );
    }

    // ── Lenient rich-text style deserialization (issue #37) ──────────────────

    #[tokio::test]
    async fn test_generate_card_flat_style_fields_produce_nested_style_on_wire() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        // Exact issue #37 repro: LLM sends flat `bold`/`fontColor` directly on the span.
        let raw = json!({
            "catalog_id": null,
            "face": {
                "text": "Hola, gracias.",
                "richText": [
                    { "text": "Hola, " },
                    { "text": "gracias.", "bold": true, "fontColor": "#27AE60" }
                ]
            },
            "back": { "text": "Hello, thank you." }
        });
        let params: GenerateCardParams = serde_json::from_value(raw).unwrap();

        make_server(&server.uri())
            .generate_card(Parameters(params))
            .await
            .unwrap();

        let body = card_request_body(&server).await;
        assert_eq!(
            body["face"]["richText"][1]["style"],
            json!({"bold": true, "fontColor": "#27AE60"}),
            "flat span fields must be folded into a nested style object on the wire: {body}"
        );
        assert!(
            body["face"]["richText"][1].get("bold").is_none(),
            "no flat `bold` field must reach the wire: {body}"
        );
    }

    #[tokio::test]
    async fn test_generate_catalog_with_cards_flat_style_fields_produce_nested_style_on_wire() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/with-cards"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "catalog": catalog_dto_json(),
                "cardsCreated": 1
            })))
            .mount(&server)
            .await;

        let raw = json!({
            "name": "Spanish",
            "description": null,
            "image_id": null,
            "tags": null,
            "visibility": null,
            "cards": [{
                "face": {
                    "text": "Hola, gracias.",
                    "richText": [
                        { "text": "Hola, " },
                        { "text": "gracias.", "bold": true, "fontColor": "#27AE60" }
                    ]
                },
                "back": { "text": "Hello, thank you." }
            }]
        });
        let params: GenerateCatalogWithCardsParams = serde_json::from_value(raw).unwrap();

        make_server(&server.uri())
            .generate_catalog_with_cards(Parameters(params))
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let req = requests
            .iter()
            .find(|r| r.url.path() == "/catalogs/with-cards")
            .expect("POST /catalogs/with-cards not called");
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(
            body["cards"][0]["face"]["richText"][1]["style"],
            json!({"bold": true, "fontColor": "#27AE60"}),
            "flat span fields must be folded into a nested style object on the wire: {body}"
        );
    }

    #[tokio::test]
    async fn test_update_card_outgoing_patch_carries_nested_style() {
        use crate::dto::{RichTextSpan, RichTextSpanStyle};
        use crate::tools::cards::UpdateCardParams;
        let server = MockServer::start().await;
        let card_id = mock_id();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(make_existing_card_json("existing-audio", json!(null))),
            )
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_dto_json()))
            .mount(&server)
            .await;

        let mut face = CardContent::plain("");
        face.rich_text = Some(vec![
            RichTextSpan {
                text: "Quedo ".to_string(),
                style: None,
            },
            RichTextSpan {
                text: "actualizado".to_string(),
                style: Some(RichTextSpanStyle {
                    bold: Some(true),
                    font_color: Some("#27AE60".to_string()),
                    ..Default::default()
                }),
            },
        ]);

        make_server(&server.uri())
            .update_card(Parameters(UpdateCardParams {
                card_id: card_id.to_string(),
                face: Some(face),
                back: None,
                catalog_ids: vec![mock_id().to_string()],
                order_number: 1,
                version: 1,
            }))
            .await
            .unwrap();

        let patch_req = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.method == wiremock::http::Method::PATCH)
            .expect("PATCH request not made");
        let body: serde_json::Value = serde_json::from_slice(&patch_req.body).unwrap();
        assert_eq!(
            body["face"]["richText"][1]["style"],
            json!({"bold": true, "fontColor": "#27AE60"}),
            "outgoing PATCH must carry nested style: {body}"
        );
    }

    #[tokio::test]
    async fn test_get_card_round_trips_nested_style_including_superscript() {
        let server = MockServer::start().await;
        let card_id = mock_id();
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": card_id,
                "version": 1,
                "face": {
                    "text": "E=mc2",
                    "richText": [
                        { "text": "E=mc" },
                        { "text": "2", "style": { "superscript": true, "bold": true } }
                    ]
                },
                "back": { "text": "Mass-energy equivalence" },
                "orderNumber": 1
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .get_card(Parameters(crate::tools::cards::GetCardParams {
                card_id: card_id.to_string(),
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
        assert_eq!(
            v["face"]["richText"][1]["style"],
            json!({"superscript": true, "bold": true}),
            "nested style (including superscript) must round-trip into tool output: {text}"
        );
    }

    #[tokio::test]
    async fn test_list_cards_round_trips_nested_style() {
        let server = MockServer::start().await;
        let catalog_id = mock_id();
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{catalog_id}/cards")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": mock_id(),
                    "version": 1,
                    "face": {
                        "text": "gracias.",
                        "richText": [
                            { "text": "gracias.", "style": { "bold": true, "fontColor": "#27AE60" } }
                        ]
                    },
                    "back": { "text": "thank you" },
                    "orderNumber": 1
                }],
                "nextCursor": null,
                "permissions": {"canEdit": true, "canDelete": true, "isOwner": true}
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .list_cards(Parameters(crate::tools::cards::ListCardsParams {
                catalog_id: catalog_id.to_string(),
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
        assert_eq!(
            v["data"][0]["face"]["richText"][0]["style"],
            json!({"bold": true, "fontColor": "#27AE60"}),
            "nested style must round-trip into list_cards tool output: {text}"
        );
    }

    // ── real server: get_card / delete_card error and invalid-UUID paths ──────

    #[tokio::test]
    async fn test_server_get_card_not_found_returns_error() {
        let server = MockServer::start().await;
        let card_id = mock_id();
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "not found"})))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .get_card(Parameters(crate::tools::cards::GetCardParams {
                card_id: card_id.to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
    }

    #[tokio::test]
    async fn test_server_delete_card_ok_and_forbidden() {
        use crate::tools::cards::DeleteCardParams;
        let server = MockServer::start().await;
        let catalog_id = mock_id();
        let card_id = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();

        Mock::given(method("DELETE"))
            .and(path(format!("/catalogs/{catalog_id}/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .delete_card(Parameters(DeleteCardParams {
                catalog_id: catalog_id.to_string(),
                card_id: card_id.to_string(),
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
        assert_eq!(text, "Card deleted successfully.");

        let server2 = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!("/catalogs/{catalog_id}/cards/{card_id}")))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({"error": "you don't have edit access"})),
            )
            .mount(&server2)
            .await;

        let result = make_server(&server2.uri())
            .delete_card(Parameters(DeleteCardParams {
                catalog_id: catalog_id.to_string(),
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
    async fn test_server_card_tools_invalid_uuid_make_no_request() {
        use crate::tools::cards::{DeleteCardParams, GetCardParams, ListCardsParams};
        let server = MockServer::start().await;
        let s = make_server(&server.uri());

        let result = s
            .list_cards(Parameters(ListCardsParams {
                catalog_id: "bad".to_string(),
                limit: None,
                cursor: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));

        let result = s
            .get_card(Parameters(GetCardParams {
                card_id: "bad".to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));

        let result = s
            .delete_card(Parameters(DeleteCardParams {
                catalog_id: "bad".to_string(),
                card_id: mock_id().to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));

        let result = s
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
}
