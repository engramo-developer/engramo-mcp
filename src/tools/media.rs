use base64::Engine;
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::CallToolResult,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

use crate::client::EngramClient;
use crate::dto::UploadMediaResult;
use crate::tools::catalogs::{err_result, ok_json};

/// Matches the `/media` upload endpoint's own limit (`engram-api/app/src/routes/media.rs`) —
/// checked here too so an oversized upload fails fast with a clear message instead of a
/// confusing server error after the (wasted) network round trip.
const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListMediaParams {
    #[schemars(description = "Filter by media type (e.g., 'image', 'audio')")]
    pub media_type: Option<String>,
    #[schemars(description = "Maximum number of items to return (default: 20)")]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UploadMediaParams {
    #[schemars(description = "Base64-encoded file bytes. Max ~10MB after decoding.")]
    pub content_base64: String,
    #[schemars(
        description = "MIME type of the file, e.g. 'audio/mpeg', 'audio/wav', 'image/png', 'image/jpeg'."
    )]
    pub content_type: String,
    #[schemars(
        description = "Original filename, e.g. 'card1_face.mp3'. Optional — only used for your own \
        bookkeeping when matching files to cards, not required by the server."
    )]
    pub filename: Option<String>,
}

#[derive(Clone)]
pub struct MediaTools {
    pub client: EngramClient,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl MediaTools {
    pub fn new(client: EngramClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List uploaded media files. Optionally filter by media type ('image', 'audio', etc.)."
    )]
    async fn list_media(
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

    #[tool(
        description = "Upload a media file you already have — a user's own voice recording, a \
        self-generated audio clip, or an image — and get back a media_id. Use that id as \
        `audio_id`/`visual_id` on a card's face/back, or as `image_id` for a catalog cover, via \
        generate_card/generate_catalog_with_cards/generate_cards/update_card. This does NOT \
        generate audio or images itself — EngrAmo has no server-side TTS/image generation in this \
        flow; the caller must already have the file. Max ~10MB after decoding. \
        If you use a shell/code tool to prepare content_base64: prefer standard line-wrapped \
        `base64` output over `-w 0`/`--wrap=0` (a single unbroken multi-KB line can break some \
        tool-output pipelines), and encode directly to stdout in one step rather than writing to \
        a file and reading it back in a second command."
    )]
    async fn upload_media(
        &self,
        Parameters(p): Parameters<UploadMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
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

#[tool_handler]
impl ServerHandler for MediaTools {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_tools(base_url: &str) -> MediaTools {
        MediaTools::new(EngramClient::new(base_url, "engram_test"))
    }

    #[tokio::test]
    async fn test_list_media_ok() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "cursor": null
            })))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .list_media(Parameters(ListMediaParams {
                media_type: None,
                limit: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_list_media_unauthorized_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .list_media(Parameters(ListMediaParams {
                media_type: None,
                limit: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[tokio::test]
    async fn test_upload_media_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": { "ids": ["00000000-0000-0000-0000-000000000001"] }
            })))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .upload_media(Parameters(UploadMediaParams {
                content_base64: b64(b"fake audio"),
                content_type: "audio/mpeg".to_string(),
                filename: Some("card1_face.mp3".to_string()),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_upload_media_invalid_base64_returns_error_without_network_call() {
        let server = MockServer::start().await;
        // No mock mounted — a network call here would fail the test with a connection error,
        // proving the base64 validation short-circuits before any request is sent.
        let result = make_tools(&server.uri())
            .upload_media(Parameters(UploadMediaParams {
                content_base64: "not-valid-base64!!!".to_string(),
                content_type: "audio/mpeg".to_string(),
                filename: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_upload_media_oversized_rejected_without_network_call() {
        let server = MockServer::start().await;
        let oversized = vec![0u8; MAX_UPLOAD_BYTES + 1];
        let result = make_tools(&server.uri())
            .upload_media(Parameters(UploadMediaParams {
                content_base64: b64(&oversized),
                content_type: "audio/mpeg".to_string(),
                filename: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_upload_media_server_error_returns_tool_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&server)
            .await;

        let result = make_tools(&server.uri())
            .upload_media(Parameters(UploadMediaParams {
                content_base64: b64(b"x"),
                content_type: "image/png".to_string(),
                filename: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }
}
