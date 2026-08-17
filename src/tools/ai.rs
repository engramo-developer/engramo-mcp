//! Feature-flagged tools that call EngrAmo's **paid, server-side AI** (TTS, translation,
//! dictionary generation, AI-agent chat). Only registered when `ENGRAM_ENABLE_PAID_AI`
//! is on — see `EngramMcpServer::new`. The always-on `generate_*` tools in
//! `tools/generate.rs` are bring-your-own-AI: the *caller* model does the generation and
//! only persists the result, incurring no cost to EngrAmo.

use base64::Engine;
use rmcp::{
    ErrorData, handler::server::wrapper::Parameters, model::CallToolResult, schemars, tool,
    tool_router,
};
use serde::Deserialize;

use crate::dto::{AiChatRequest, CardAiRequest, CreateCatalogRequest};
use crate::server::EngramMcpServer;
use crate::tools::catalogs::{err_result, ok_json, parse_uuid};

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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TranslateBatchImportParams {
    #[schemars(description = "Target BCP-47 language code for auto-translation, e.g. 'es'.")]
    pub target_lang: String,
    #[schemars(description = "Name for the new catalog.")]
    pub name: String,
    #[schemars(description = "Optional catalog description.")]
    pub description: Option<String>,
    #[schemars(description = "Optional catalog tags.")]
    pub tags: Option<Vec<String>>,
    #[schemars(description = "New catalog visibility: 'public' or 'private'.")]
    pub visibility: Option<String>,
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
impl EngramMcpServer {
    #[tool(
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
        description = "Create a new catalog from an uploaded ZIP/JSON content file, auto-translating \
        every card into target_lang. USES ENGRAMO'S PAID AI — this consumes the account's \
        translation quota. Intended for bulk imports of pre-recorded audio + source text; most \
        chat-driven deck creation should use generate_catalog_with_cards instead."
    )]
    pub async fn translate_batch_import(
        &self,
        Parameters(p): Parameters<TranslateBatchImportParams>,
    ) -> Result<CallToolResult, ErrorData> {
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
                Err(e) => err_result(e),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::EngramClient;
    use rmcp::handler::server::wrapper::Parameters;
    use serde_json::json;
    use uuid::Uuid;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mock_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn make_server(base_url: &str) -> EngramMcpServer {
        EngramMcpServer::new(EngramClient::new(base_url, "engram_test"), true)
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
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
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
}
