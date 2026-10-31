use rmcp::schemars;
use serde::Deserialize;

/// Matches the `/media` upload endpoint's own limit (`engram-api/app/src/routes/media.rs`) —
/// checked here too so an oversized upload fails fast with a clear message instead of a
/// confusing server error after the (wasted) network round trip. `/catalogs/translate-batch-import`
/// carries the same whole-request limit (see `docs/bulk-import.md`).
pub(crate) const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

/// The same limit expressed on the *encoded* string, so an oversized payload is rejected
/// before `base64::decode` allocates a buffer for it (base64 expands data by 4/3).
pub(crate) const MAX_UPLOAD_BASE64_LEN: usize = MAX_UPLOAD_BYTES.div_ceil(3) * 4;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListMediaParams {
    #[schemars(description = "Filter by media type (e.g., 'image', 'audio')")]
    pub media_type: Option<String>,
    #[schemars(
        description = "Maximum number of items to return (default 20; values are clamped to 1..=50)"
    )]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination cursor from a previous response")]
    pub cursor: Option<String>,
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
