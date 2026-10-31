//! Feature-flagged tools that call EngrAmo's **paid, server-side AI** (TTS, translation,
//! dictionary generation, AI-agent chat). Only registered when `ENGRAMO_ENABLE_PAID_AI`
//! is on — see `EngramoMcpServer::new`. The always-on `generate_*` tools in
//! `tools/generate.rs` are bring-your-own-AI: the *caller* model does the generation and
//! only persists the result, incurring no cost to EngrAmo.

use base64::Engine;
use rmcp::{
    ErrorData, handler::server::wrapper::Parameters, model::CallToolResult, schemars, tool,
    tool_router,
};
use serde::Deserialize;

use crate::catalog_filters::{normalize_lang, with_filter_hint};
use crate::dto::{AiChatRequest, CardAiRequest, CreateCatalogRequest};
use crate::server::EngramoMcpServer;
use crate::tools::catalogs::{ClassificationWrite, err_result, ok_json, parse_uuid};
use crate::tools::media::MAX_UPLOAD_BASE64_LEN;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CardAiToolParams {
    #[schemars(
        description = "UUID of the catalog to operate on. Provide exactly one of catalog_id or card_id."
    )]
    pub catalog_id: Option<String>,
    #[schemars(
        description = "UUID of a single card to operate on. Provide exactly one of catalog_id or card_id."
    )]
    pub card_id: Option<String>,
    #[schemars(description = "BCP-47 language code, e.g. 'en', 'uk', 'es'.")]
    pub lang: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AiAgentChatParams {
    #[schemars(
        description = "UUID of an existing chat session to continue. Omit to start a new session."
    )]
    pub session_id: Option<String>,
    #[schemars(description = "Message to send to the EngrAmo AI agent.")]
    pub message: String,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct TranslateBatchImportParams {
    #[schemars(description = "Target BCP-47 language code for auto-translation, e.g. 'es'.")]
    pub target_lang: String,
    #[schemars(description = "Name for the new catalog.")]
    pub name: String,
    #[schemars(description = "Optional catalog description.")]
    pub description: Option<String>,
    #[schemars(description = "Optional catalog tags.")]
    pub tags: Option<Vec<String>>,
    #[schemars(
        description = "New catalog visibility: 'public', 'private', or 'unlisted' (hidden from listings/search, but reachable by anyone who has its exact short_id). Setting 'public' or 'unlisted' may require publish permission — non-moderators can get a permission error."
    )]
    pub visibility: Option<String>,
    #[schemars(
        description = "Optional. Languages the card FRONTS are in (the language being learned), as ISO 639-1 codes, e.g. ['es']. front_languages and topics together: at most 3 values. Not defaulted."
    )]
    pub front_languages: Option<Vec<String>>,
    #[schemars(
        description = "Optional. Languages of the card BACKS, as ISO 639-1 codes, at most 2; 'zxx' = no language needed. Defaults to [target_lang]; pass [] to leave the catalog without back languages."
    )]
    pub back_languages: Option<Vec<String>>,
    #[schemars(
        description = "Optional. Topic keys from get_catalog_filter_options (the list changes over time; don't guess). More specific subjects belong in tags."
    )]
    pub topics: Option<Vec<String>>,
    #[schemars(
        description = "Optional. CEFR levels A1-C2, or 'beginner' (A1,A2), 'intermediate' (B1,B2), 'advanced' (C1,C2)."
    )]
    pub levels: Option<Vec<crate::catalog_filters::LevelParam>>,
    #[schemars(
        description = "Base64-encoded content file: either a ZIP archive containing index.json \
        (an array of {original_text, audio_file}) plus referenced media files, or a raw JSON file \
        with the same shape."
    )]
    pub content_file_base64: String,
    #[schemars(description = "Filename for the uploaded content file, e.g. 'import.zip'.")]
    pub content_file_name: Option<String>,
}

fn parse_optional_uuid(s: Option<&str>) -> Result<Option<uuid::Uuid>, String> {
    s.map(parse_uuid).transpose()
}

#[tool_router(router = paid_ai_tools_router, vis = "pub(crate)")]
impl EngramoMcpServer {
    #[tool(
        title = "Generate TTS for cards",
        annotations(
            title = "Generate TTS for cards",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        ),
        description = "Generate server-side TTS audio for cards that have no face audio yet. \
        USES ENGRAMO'S PAID AI — this consumes the account's TTS quota. \
        Provide exactly one of catalog_id or card_id. Audio is generated asynchronously; \
        this call only confirms which cards were queued."
    )]
    pub async fn generate_tts_for_cards(
        &self,
        Parameters(p): Parameters<CardAiToolParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let catalog_id = match parse_optional_uuid(p.catalog_id.as_deref()) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let card_id = match parse_optional_uuid(p.card_id.as_deref()) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let req = CardAiRequest {
            catalog_id,
            card_id,
            lang: p.lang,
        };
        Ok(match self.client.generate_tts_for_cards(&req).await {
            Ok(resp) => ok_json(&resp),
            Err(e) => err_result(e),
        })
    }

    #[tool(
        title = "Translate cards",
        annotations(
            title = "Translate cards",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        ),
        description = "Translate cards whose back text is empty, into the given language. \
        USES ENGRAMO'S PAID AI — this consumes the account's translation quota. \
        Provide exactly one of catalog_id or card_id. \
        Prefer the free generate_card / generate_catalog_with_cards tools (translate it yourself) \
        unless the caller explicitly wants EngrAmo to translate server-side."
    )]
    pub async fn translate_cards(
        &self,
        Parameters(p): Parameters<CardAiToolParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let catalog_id = match parse_optional_uuid(p.catalog_id.as_deref()) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let card_id = match parse_optional_uuid(p.card_id.as_deref()) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let req = CardAiRequest {
            catalog_id,
            card_id,
            lang: p.lang,
        };
        Ok(match self.client.translate_cards(&req).await {
            Ok(resp) => ok_json(&resp),
            Err(e) => err_result(e),
        })
    }

    #[tool(
        title = "Generate dictionary for cards",
        annotations(
            title = "Generate dictionary for cards",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        ),
        description = "Fill in missing word-level dictionary entries (word -> translation) on cards. \
        USES ENGRAMO'S PAID AI — this consumes the account's translation quota. \
        Provide exactly one of catalog_id or card_id."
    )]
    pub async fn generate_dictionary_for_cards(
        &self,
        Parameters(p): Parameters<CardAiToolParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let catalog_id = match parse_optional_uuid(p.catalog_id.as_deref()) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let card_id = match parse_optional_uuid(p.card_id.as_deref()) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let req = CardAiRequest {
            catalog_id,
            card_id,
            lang: p.lang,
        };
        Ok(
            match self.client.generate_dictionary_for_cards(&req).await {
                Ok(resp) => ok_json(&resp),
                Err(e) => err_result(e),
            },
        )
    }

    #[tool(
        title = "Chat with AI agent",
        annotations(
            title = "Chat with AI agent",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        ),
        description = "Chat with EngrAmo's server-side AI agent (e.g. to draft a song- or \
        vocabulary-based deck idea). USES ENGRAMO'S PAID AI — this consumes the account's \
        AI-agent message quota."
    )]
    pub async fn ai_agent_chat(
        &self,
        Parameters(p): Parameters<AiAgentChatParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let session_id = match parse_optional_uuid(p.session_id.as_deref()) {
            Ok(id) => id,
            Err(e) => return Ok(err_result(e)),
        };
        let req = AiChatRequest {
            session_id,
            message: p.message,
        };
        Ok(match self.client.ai_agent_chat(&req).await {
            Ok(resp) => ok_json(&resp),
            Err(e) => err_result(e),
        })
    }

    #[tool(
        title = "Translate batch import",
        annotations(
            title = "Translate batch import",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        ),
        description = "Create a new catalog from an uploaded ZIP/JSON content file, auto-translating \
        every card into target_lang. USES ENGRAMO'S PAID AI — this consumes the account's \
        translation quota. Intended for bulk imports of pre-recorded audio + source text; most \
        chat-driven deck creation should use generate_catalog_with_cards instead. The catalog's back \
        language defaults to target_lang; optional front_languages, back_languages, topics and \
        levels classify it (see get_catalog_filter_options)."
    )]
    pub async fn translate_batch_import(
        &self,
        Parameters(p): Parameters<TranslateBatchImportParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Same 10MB whole-request limit as `/media` (see `docs/bulk-import.md`), checked
        // on the encoded string so an oversized import is refused before it is decoded
        // and buffered again as a multipart body.
        if p.content_file_base64.len() > MAX_UPLOAD_BASE64_LEN {
            return Ok(err_result(
                "Encoded content_file exceeds the 10MB import limit".to_string(),
            ));
        }
        let mut class = match ClassificationWrite::new(
            p.front_languages.as_deref(),
            p.back_languages.as_deref(),
            p.topics.as_deref(),
            p.levels.as_deref(),
        ) {
            Ok(c) => c,
            Err(e) => return Ok(err_result(e)),
        };
        let learn = match class.learn(None) {
            Ok(l) => l,
            Err(e) => return Ok(err_result(e)),
        };
        // Only caller-supplied classification earns the filter hint on a 400.
        let has_filters = class.any();
        // An explicit back_languages (even []) wins; otherwise the translation language is the
        // back language. A target_lang that is not a 2-letter ISO 639-1 code (e.g. 'zxx', 'fil') is skipped.
        if p.back_languages.is_none()
            && let Ok(code) = normalize_lang(&p.target_lang, "back_languages", false)
            && code.len() == 2
        {
            class.speak_langs = Some(vec![code]);
        }
        let content_file =
            match base64::engine::general_purpose::STANDARD.decode(&p.content_file_base64) {
                Ok(bytes) => bytes,
                Err(e) => return Ok(err_result(format!("Invalid base64 content_file: {e}"))),
            };
        let metadata = CreateCatalogRequest {
            name: p.name,
            description: p.description,
            tags: p.tags,
            visibility: p.visibility,
            speak_langs: class.speak_langs,
            learn,
            levels: class.levels,
        };
        let file_name = p
            .content_file_name
            .unwrap_or_else(|| "import.zip".to_string());
        Ok(
            match self
                .client
                .translate_batch_import(&p.target_lang, &metadata, &file_name, content_file)
                .await
            {
                Ok(catalog) => ok_json(&catalog),
                Err(e) if has_filters => err_result(with_filter_hint(e)),
                Err(e) => err_result(e),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::EngramoClient;
    use rmcp::handler::server::wrapper::Parameters;
    use serde_json::json;
    use uuid::Uuid;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mock_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn make_server(base_url: &str) -> EngramoMcpServer {
        EngramoMcpServer::new(EngramoClient::new(base_url, "engramo_test"), true)
    }

    #[tokio::test]
    async fn test_generate_tts_for_cards_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/tts"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({
                "updated": 1,
                "card_ids": [mock_id()]
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .generate_tts_for_cards(Parameters(CardAiToolParams {
                catalog_id: Some(mock_id().to_string()),
                card_id: None,
                lang: "es".to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_generate_tts_for_cards_invalid_uuid_returns_error_not_panic() {
        let server = MockServer::start().await;
        let result = make_server(&server.uri())
            .generate_tts_for_cards(Parameters(CardAiToolParams {
                catalog_id: Some("not-a-uuid".to_string()),
                card_id: None,
                lang: "es".to_string(),
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
    async fn test_translate_cards_quota_exceeded_returns_error_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/translate"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "error": "quota_exceeded",
                "resource_type": "translation_chunks",
                "used": 5,
                "limit": 5
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .translate_cards(Parameters(CardAiToolParams {
                catalog_id: None,
                card_id: Some(mock_id().to_string()),
                lang: "uk".to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_generate_dictionary_for_cards_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/dictionary"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "updated": 2,
                "card_ids": [mock_id()]
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .generate_dictionary_for_cards(Parameters(CardAiToolParams {
                catalog_id: Some(mock_id().to_string()),
                card_id: None,
                lang: "de".to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_ai_agent_chat_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/ai-agent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sessionId": mock_id(),
                "text": "Sure, here's a deck idea..."
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .ai_agent_chat(Parameters(AiAgentChatParams {
                session_id: None,
                message: "Make me a Spanish restaurant deck".to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_ai_agent_chat_invalid_session_id() {
        let server = MockServer::start().await;
        let result = make_server(&server.uri())
            .ai_agent_chat(Parameters(AiAgentChatParams {
                session_id: Some("not-a-uuid".to_string()),
                message: "hi".to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_translate_batch_import_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/translate-batch-import/es"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": mock_id(),
                "name": "Imported",
                "version": 1
            })))
            .mount(&server)
            .await;

        let content_b64 = base64::engine::general_purpose::STANDARD.encode(b"[]");
        let result = make_server(&server.uri())
            .translate_batch_import(Parameters(TranslateBatchImportParams {
                target_lang: "es".to_string(),
                name: "Imported".to_string(),
                description: None,
                tags: None,
                visibility: None,
                content_file_base64: content_b64,
                content_file_name: Some("import.json".to_string()),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_translate_batch_import_rejects_oversized_file_without_network_call() {
        // No MockServer mounted: an oversized payload must be refused before the
        // request is built, let alone sent.
        let tools = EngramoMcpServer::new(EngramoClient::new("http://127.0.0.1:1", "t"), true);
        let result = tools
            .translate_batch_import(Parameters(TranslateBatchImportParams {
                target_lang: "es".to_string(),
                name: "Deck".to_string(),
                description: None,
                tags: None,
                visibility: None,
                content_file_base64: "A".repeat(MAX_UPLOAD_BASE64_LEN + 1),
                content_file_name: None,
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let text = format!("{:?}", result.content);
        assert!(text.contains("10MB"), "{text}");
    }

    #[tokio::test]
    async fn test_translate_batch_import_invalid_base64_returns_error_not_panic() {
        let server = MockServer::start().await;
        let result = make_server(&server.uri())
            .translate_batch_import(Parameters(TranslateBatchImportParams {
                target_lang: "es".to_string(),
                name: "Imported".to_string(),
                description: None,
                tags: None,
                visibility: None,
                content_file_base64: "not-valid-base64!!!".to_string(),
                content_file_name: None,
                ..Default::default()
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
        assert!(text.contains("base64"), "{text}");
    }

    /// Regression for F5: the second `parse_optional_uuid` early return (on `card_id`) in
    /// each paid-AI tool must also short-circuit before any request is sent — previously
    /// only a bad `catalog_id` was ever exercised.
    #[tokio::test]
    async fn test_paid_ai_tools_invalid_card_id_returns_error_without_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let s = make_server(&server.uri());

        let results = [
            s.generate_tts_for_cards(Parameters(CardAiToolParams {
                catalog_id: None,
                card_id: Some("bad".to_string()),
                lang: "es".to_string(),
            }))
            .await
            .unwrap(),
            s.translate_cards(Parameters(CardAiToolParams {
                catalog_id: None,
                card_id: Some("bad".to_string()),
                lang: "es".to_string(),
            }))
            .await
            .unwrap(),
            s.generate_dictionary_for_cards(Parameters(CardAiToolParams {
                catalog_id: None,
                card_id: Some("bad".to_string()),
                lang: "es".to_string(),
            }))
            .await
            .unwrap(),
        ];
        for r in results {
            assert_eq!(r.is_error, Some(true), "{r:?}");
            let text = r
                .content
                .first()
                .and_then(|c| c.as_text())
                .map(|t| t.text.as_str())
                .unwrap_or("");
            assert!(text.contains("Invalid UUID"), "{text}");
        }
    }

    /// Regression for F6: an upstream quota/auth failure must still reach the caller as
    /// `is_error` tool content, not a propagated `Err`, for every paid-AI tool that only
    /// had happy-path/pre-request-validation coverage so far.
    #[tokio::test]
    async fn test_paid_ai_tools_quota_exceeded_returns_error_content() {
        let server = MockServer::start().await;
        let quota = json!({"error":"quota_exceeded","resource_type":"x","used":1,"limit":1});
        for p in [
            "/catalogs/tts",
            "/catalogs/dictionary",
            "/ai-agent",
            "/catalogs/translate-batch-import/es",
        ] {
            Mock::given(method("POST"))
                .and(path(p))
                .respond_with(ResponseTemplate::new(429).set_body_json(quota.clone()))
                .expect(1)
                .mount(&server)
                .await;
        }
        let s = make_server(&server.uri());

        let results = [
            s.generate_tts_for_cards(Parameters(CardAiToolParams {
                catalog_id: Some(mock_id().to_string()),
                card_id: None,
                lang: "es".to_string(),
            }))
            .await
            .unwrap(),
            s.generate_dictionary_for_cards(Parameters(CardAiToolParams {
                catalog_id: Some(mock_id().to_string()),
                card_id: None,
                lang: "es".to_string(),
            }))
            .await
            .unwrap(),
            s.ai_agent_chat(Parameters(AiAgentChatParams {
                session_id: None,
                message: "hi".to_string(),
            }))
            .await
            .unwrap(),
            s.translate_batch_import(Parameters(TranslateBatchImportParams {
                target_lang: "es".to_string(),
                name: "D".to_string(),
                description: None,
                tags: None,
                visibility: None,
                content_file_base64: base64::engine::general_purpose::STANDARD.encode(b"[]"),
                content_file_name: None,
                ..Default::default()
            }))
            .await
            .unwrap(),
        ];
        for r in results {
            assert_eq!(r.is_error, Some(true), "{r:?}");
            let text = r
                .content
                .first()
                .and_then(|c| c.as_text())
                .map(|t| t.text.as_str())
                .unwrap_or("");
            assert!(text.contains("Quota exceeded"), "{text}");
        }
    }

    /// Regression for F12: `visibility` must reach the multipart `catalog_metadata` part,
    /// and an omitted `content_file_name` must default to `import.zip`.
    #[tokio::test]
    async fn test_translate_batch_import_forwards_unlisted_visibility_and_default_filename() {
        use wiremock::matchers::body_string_contains;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/catalogs/translate-batch-import/es"))
            .and(body_string_contains(r#""visibility":"unlisted""#))
            .and(body_string_contains(r#"filename="import.zip""#))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": mock_id(), "name": "D", "version": 1
            })))
            .expect(1)
            .mount(&server)
            .await;

        let r = make_server(&server.uri())
            .translate_batch_import(Parameters(TranslateBatchImportParams {
                target_lang: "es".to_string(),
                name: "D".to_string(),
                description: None,
                tags: None,
                visibility: Some("unlisted".to_string()),
                content_file_base64: base64::engine::general_purpose::STANDARD.encode(b"[]"),
                content_file_name: None,
                ..Default::default()
            }))
            .await
            .unwrap();
        assert!(!r.is_error.unwrap_or(false), "{r:?}");
    }

    fn svec(v: &[&str]) -> Option<Vec<String>> {
        Some(v.iter().map(|x| x.to_string()).collect())
    }

    fn import_params(target: &str) -> TranslateBatchImportParams {
        TranslateBatchImportParams {
            target_lang: target.to_string(),
            name: "D".to_string(),
            content_file_base64: base64::engine::general_purpose::STANDARD.encode(b"[]"),
            ..Default::default()
        }
    }

    async fn run_import(p: TranslateBatchImportParams, status: u16) -> (CallToolResult, String) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(if status == 201 {
                ResponseTemplate::new(201).set_body_json(json!({
                    "id": mock_id(), "name": "D", "version": 1
                }))
            } else {
                ResponseTemplate::new(status).set_body_json(json!({"error": "bad topic"}))
            })
            .mount(&server)
            .await;
        let r = make_server(&server.uri())
            .translate_batch_import(Parameters(p))
            .await
            .unwrap();
        let reqs = server.received_requests().await.unwrap();
        let body = reqs
            .first()
            .map(|r| String::from_utf8_lossy(&r.body).into_owned())
            .unwrap_or_default();
        (r, body)
    }

    #[tokio::test]
    async fn test_translate_batch_import_defaults_back_languages_to_target() {
        let (r, body) = run_import(import_params("ES-es"), 201).await;
        assert!(!r.is_error.unwrap_or(false), "{r:?}");
        assert!(body.contains(r#""speakLangs":["es"]"#), "{body}");
        assert!(
            !body.contains(r#""learn""#) && !body.contains(r#""levels""#),
            "{body}"
        );
    }

    #[tokio::test]
    async fn test_translate_batch_import_explicit_back_languages_wins() {
        let (r, body) = run_import(
            TranslateBatchImportParams {
                back_languages: svec(&["EN"]),
                front_languages: svec(&["es"]),
                topics: svec(&["medicine"]),
                levels: Some(vec![crate::catalog_filters::LevelParam::Beginner]),
                ..import_params("es")
            },
            201,
        )
        .await;
        assert!(!r.is_error.unwrap_or(false), "{r:?}");
        assert!(body.contains(r#""speakLangs":["en"]"#), "{body}");
        assert!(body.contains(r#""learn":["lang:es","medicine"]"#), "{body}");
        assert!(body.contains(r#""levels":["A1","A2"]"#), "{body}");
    }

    #[tokio::test]
    async fn test_translate_batch_import_explicit_empty_back_languages_is_kept() {
        let (r, body) = run_import(
            TranslateBatchImportParams {
                back_languages: Some(vec![]),
                ..import_params("es")
            },
            201,
        )
        .await;
        assert!(!r.is_error.unwrap_or(false), "{r:?}");
        assert!(body.contains(r#""speakLangs":[]"#), "{body}");
    }

    #[tokio::test]
    async fn test_translate_batch_import_skips_default_for_non_iso_target() {
        let (r, body) = run_import(import_params("zxx"), 201).await;
        assert!(!r.is_error.unwrap_or(false), "{r:?}");
        assert!(!body.contains("speakLangs"), "{body}");
    }

    #[tokio::test]
    async fn test_translate_batch_import_skips_default_for_three_letter_target() {
        let (r, body) = run_import(import_params("fil"), 201).await;
        assert!(!r.is_error.unwrap_or(false), "{r:?}");
        assert!(!body.contains("speakLangs"), "{body}");
    }

    #[tokio::test]
    async fn test_translate_batch_import_invalid_classification_makes_no_request() {
        for (p, expect) in [
            (
                TranslateBatchImportParams {
                    back_languages: svec(&["english!"]),
                    ..import_params("es")
                },
                "not a language code",
            ),
            (
                TranslateBatchImportParams {
                    front_languages: svec(&["zxx"]),
                    ..import_params("es")
                },
                "only valid in back_languages",
            ),
            (
                TranslateBatchImportParams {
                    topics: svec(&["lang:es"]),
                    ..import_params("es")
                },
                "is a language",
            ),
            (
                TranslateBatchImportParams {
                    front_languages: svec(&["es", "fr"]),
                    topics: svec(&["math", "law"]),
                    ..import_params("es")
                },
                "front_languages and topics together",
            ),
        ] {
            let server = MockServer::start().await;
            let r = make_server(&server.uri())
                .translate_batch_import(Parameters(p))
                .await
                .unwrap();
            assert!(r.is_error.unwrap_or(false), "{r:?}");
            let text = r
                .content
                .first()
                .and_then(|c| c.as_text())
                .map_or("", |t| t.text.as_str());
            assert!(text.contains(expect), "{text}");
            assert!(server.received_requests().await.unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn test_translate_batch_import_400_with_filters_adds_hint() {
        let (r, _) = run_import(
            TranslateBatchImportParams {
                topics: svec(&["nonsense"]),
                ..import_params("es")
            },
            400,
        )
        .await;
        assert_eq!(r.is_error, Some(true));
        let text = r.content[0].as_text().unwrap().text.clone();
        assert!(text.contains("get_catalog_filter_options"), "{text}");

        // The defaulted back language alone does not earn the hint.
        let (r, _) = run_import(import_params("es"), 400).await;
        let text = r.content[0].as_text().unwrap().text.clone();
        assert!(!text.contains("get_catalog_filter_options"), "{text}");
    }
}
