//! Local, bring-your-own-key TTS tools — `stdio` mode only (see `tts::from_env`'s doc
//! comment and `server::build_session_server`, which never wires this router in). This is a
//! **separate** router from `tools/ai.rs`'s `paid_ai_tools_router`: that one spends
//! EngrAmo's own paid quota and is gated by `ENGRAMO_ENABLE_PAID_AI`; this one spends the
//! user's own Gemini quota and is gated purely by whether `EngramoMcpServer::with_tts` was
//! called (i.e. whether `ENGRAMO_TTS_GEMINI_API_KEYS` was set at startup).
//!
//! `list_tts_voices` is a cheap, no-network way for the calling model to see the configured
//! engine/model/voice before a synthesis batch. `generate_card_audio` does the real work:
//! synthesize → encode → upload → attach, per card, with bounded concurrency.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rmcp::{
    ErrorData, handler::server::wrapper::Parameters, model::CallToolResult, schemars, tool,
    tool_router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::client::EngramoClient;
use crate::error::ApiError;
use crate::server::{EngramoMcpServer, sanitize_text};
use crate::tools::catalogs::{err_result, ok_json, parse_uuid};
use crate::tts::mp3::{AudioEncoder, Mp3Encoder};
use crate::tts::{TtsEngine, TtsRequest, check_lang, check_voice};

/// Longest face text `generate_card_audio` will synthesize for a single card — matches
/// engram-api's `PREMIUM_TTS_MAX_CHARS` for its paid TTS.
const MAX_TTS_CHARS: usize = 500;
/// Most cards a single `generate_card_audio` call will process.
const MAX_CARDS_PER_CALL: usize = 20;
/// How many cards `generate_card_audio` synthesizes/uploads concurrently. Bounded so a large
/// batch doesn't open dozens of simultaneous Gemini/EngrAmo requests at once.
const TTS_CONCURRENCY: usize = 4;

#[derive(Debug, Serialize)]
struct VoiceInfo {
    name: &'static str,
    description: &'static str,
}

#[derive(Debug, Serialize)]
struct ListTtsVoicesResult {
    engine: &'static str,
    model: String,
    default_voice: String,
    voices: Vec<VoiceInfo>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GenerateCardAudioParams {
    #[schemars(
        description = "UUIDs of cards to generate FACE-side audio for. 1-20 per call. \
        Duplicates are de-duplicated, keeping the first occurrence's position."
    )]
    pub card_ids: Vec<String>,
    #[schemars(
        description = "Voice name from list_tts_voices. Defaults to the server's configured \
        default voice (see list_tts_voices) when omitted."
    )]
    pub voice: Option<String>,
    #[schemars(
        description = "Optional BCP-47 language code, e.g. 'en', 'uk', 'es'. Omit to let the \
        engine infer it from the text."
    )]
    pub lang: Option<String>,
    #[schemars(
        description = "When true, replace a card's existing face audio instead of skipping it. \
        Default false. The previous audio asset becomes unreferenced and is cleaned up \
        server-side."
    )]
    pub overwrite: Option<bool>,
}

/// Outcome for a single card in a `generate_card_audio` batch.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum CardAudioStatus {
    Generated,
    Skipped,
    Failed,
}

#[derive(Debug, Serialize)]
struct CardAudioResult {
    card_id: Uuid,
    status: CardAudioStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    media_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

impl CardAudioResult {
    fn generated(card_id: Uuid, media_id: Uuid, bytes: usize) -> Self {
        Self {
            card_id,
            status: CardAudioStatus::Generated,
            media_id: Some(media_id),
            bytes: Some(bytes),
            reason: None,
        }
    }

    fn skipped(card_id: Uuid, reason: impl Into<String>) -> Self {
        Self {
            card_id,
            status: CardAudioStatus::Skipped,
            media_id: None,
            bytes: None,
            reason: Some(reason.into()),
        }
    }

    fn skipped_with_media(card_id: Uuid, reason: impl Into<String>, media_id: Uuid) -> Self {
        Self {
            card_id,
            status: CardAudioStatus::Skipped,
            media_id: Some(media_id),
            bytes: None,
            reason: Some(reason.into()),
        }
    }

    fn failed(card_id: Uuid, reason: impl Into<String>) -> Self {
        Self {
            card_id,
            status: CardAudioStatus::Failed,
            media_id: None,
            bytes: None,
            reason: Some(reason.into()),
        }
    }

    fn failed_with_media(card_id: Uuid, reason: impl Into<String>, media_id: Uuid) -> Self {
        Self {
            card_id,
            status: CardAudioStatus::Failed,
            media_id: Some(media_id),
            bytes: None,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Serialize)]
struct GenerateCardAudioResult {
    generated: usize,
    skipped: usize,
    failed: usize,
    results: Vec<CardAudioResult>,
}

/// The subset of a raw `GET /cards/{id}` response `generate_card_audio` needs, plus the fetched
/// `face` object kept intact (including fields this crate doesn't model, e.g. `tts`) so it can
/// be round-tripped back into a `PATCH` body without dropping anything — see `client.rs`'s
/// `get_card_raw`/`patch_card_raw` doc comments.
struct ParsedCard {
    /// Kept as the raw JSON value (not converted to a Rust int) so the PATCH body echoes back
    /// exactly what the server sent, regardless of whether it's encoded as i32 or i64 on the
    /// wire.
    version: serde_json::Value,
    order_number: serde_json::Value,
    catalog_ids: Vec<Uuid>,
    face: serde_json::Map<String, serde_json::Value>,
    has_audio: bool,
    text: String,
}

/// Parses the pieces of a raw card JSON value that `generate_card_audio` needs. Returns
/// `Err(reason)` — always prefixed `"unexpected card shape: "` — for anything missing or the
/// wrong type, so a backend response shape change fails loudly per card instead of panicking.
fn parse_card_shape(raw: &serde_json::Value) -> Result<ParsedCard, String> {
    let obj = raw
        .as_object()
        .ok_or_else(|| "unexpected card shape: response is not a JSON object".to_string())?;

    let version = obj
        .get("version")
        .cloned()
        .ok_or_else(|| "unexpected card shape: missing 'version'".to_string())?;
    let order_number = obj
        .get("orderNumber")
        .cloned()
        .ok_or_else(|| "unexpected card shape: missing 'orderNumber'".to_string())?;

    let catalogs = obj
        .get("catalogs")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "unexpected card shape: missing or malformed 'catalogs'".to_string())?;
    let mut catalog_ids = Vec::with_capacity(catalogs.len());
    for entry in catalogs {
        let id_str = entry.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
            "unexpected card shape: a 'catalogs' entry is missing 'id'".to_string()
        })?;
        let id = Uuid::parse_str(id_str).map_err(|_| {
            format!("unexpected card shape: catalog id '{id_str}' is not a valid UUID")
        })?;
        catalog_ids.push(id);
    }

    let face = obj
        .get("face")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "unexpected card shape: missing or malformed 'face'".to_string())?
        .clone();
    let text = face
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let has_audio = face.get("audioId").is_some_and(|v| !v.is_null());

    Ok(ParsedCard {
        version,
        order_number,
        catalog_ids,
        face,
        has_audio,
        text,
    })
}

/// Sets `face.audioId` and drops the response-only `audioUrl`/`visualUrl` fields a server
/// response carries but a `PATCH` body must not echo back.
fn set_audio_id(face: &mut serde_json::Map<String, serde_json::Value>, media_id: Uuid) {
    face.insert(
        "audioId".to_string(),
        serde_json::Value::String(media_id.to_string()),
    );
    face.remove("audioUrl");
    face.remove("visualUrl");
}

/// Builds a `PATCH /cards/{id}` body carrying only `catalogIds`, `orderNumber`, `version`, and
/// `face` — `back` is omitted entirely so the server keeps it (`CLAUDE.md`: "omitted `back` is
/// kept").
fn build_patch_body(
    catalog_ids: &[Uuid],
    order_number: &serde_json::Value,
    version: &serde_json::Value,
    face: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "catalogIds": catalog_ids,
        "orderNumber": order_number,
        "version": version,
        "face": face,
    })
}

/// First 8 characters of a card's UUID (hyphenated form — the hyphen falls at index 8, so this
/// is always the UUID's first hex group), used to build a human-readable media filename.
fn short_id(id: Uuid) -> String {
    id.to_string()[..8].to_string()
}

/// Attempts the `PATCH` attaching `media_id` to `parsed`'s face, re-fetching and retrying once
/// on a 409 conflict (rollout plan §0 R4) before giving up. Never performs a second synthesis
/// or upload — `media_id`/`bytes` are already in hand by the time this runs. `synthesized_text`
/// is the sanitized text the audio was actually generated from — on a conflict retry, if the
/// re-fetched face text no longer matches it, a concurrent edit changed the sentence out from
/// under the synthesized audio, so the stale audio is refused rather than attached.
async fn attach_media(
    client: &EngramoClient,
    card_id: Uuid,
    parsed: ParsedCard,
    synthesized_text: &str,
    media_id: Uuid,
    bytes: usize,
    overwrite: bool,
) -> CardAudioResult {
    let mut face = parsed.face;
    set_audio_id(&mut face, media_id);
    let body = build_patch_body(
        &parsed.catalog_ids,
        &parsed.order_number,
        &parsed.version,
        &face,
    );

    match client.patch_card_raw(card_id, &body).await {
        Ok(_) => CardAudioResult::generated(card_id, media_id, bytes),
        Err(ApiError::Conflict(_)) => {
            let raw = match client.get_card_raw(card_id).await {
                Ok(v) => v,
                Err(e) => {
                    return CardAudioResult::failed_with_media(
                        card_id,
                        format!(
                            "audio was uploaded but the card could not be re-fetched after a \
                            conflict ({e}); attach media_id manually via update_card"
                        ),
                        media_id,
                    );
                }
            };
            let refreshed = match parse_card_shape(&raw) {
                Ok(p) => p,
                Err(reason) => {
                    return CardAudioResult::failed_with_media(
                        card_id,
                        format!(
                            "audio was uploaded but the re-fetched card shape was unexpected \
                            ({reason}); attach media_id manually via update_card"
                        ),
                        media_id,
                    );
                }
            };
            let refreshed_text = sanitize_text(refreshed.text.trim());
            if refreshed_text != synthesized_text {
                return CardAudioResult::failed_with_media(
                    card_id,
                    "face text changed concurrently after audio was generated; not attaching \
                    stale audio — re-run generate_card_audio for this card",
                    media_id,
                );
            }
            if refreshed.catalog_ids.is_empty() {
                return CardAudioResult::failed_with_media(
                    card_id,
                    "audio was uploaded, but after a conflict the card now has no catalog \
                    membership; refusing to update. Attach media_id manually via update_card"
                        .to_string(),
                    media_id,
                );
            }
            if refreshed.has_audio && !overwrite {
                return CardAudioResult::skipped_with_media(
                    card_id,
                    "already has face audio; pass overwrite: true to replace",
                    media_id,
                );
            }
            let mut refreshed_face = refreshed.face;
            set_audio_id(&mut refreshed_face, media_id);
            let retry_body = build_patch_body(
                &refreshed.catalog_ids,
                &refreshed.order_number,
                &refreshed.version,
                &refreshed_face,
            );
            match client.patch_card_raw(card_id, &retry_body).await {
                Ok(_) => CardAudioResult::generated(card_id, media_id, bytes),
                Err(e) => CardAudioResult::failed_with_media(
                    card_id,
                    format!(
                        "audio was uploaded but attaching it failed again after a conflict \
                        retry ({e}); attach media_id manually via update_card"
                    ),
                    media_id,
                ),
            }
        }
        Err(e) => CardAudioResult::failed_with_media(
            card_id,
            format!(
                "audio was uploaded but attaching it failed ({e}); attach media_id manually \
                via update_card"
            ),
            media_id,
        ),
    }
}

/// Processes one card end to end: fetch, validate, synthesize, encode, upload, attach. Never
/// panics on a malformed response or a downstream error — every path returns a `CardAudioResult`
/// (CLAUDE.md's tool-handler error contract applies to the whole batch this feeds into, not just
/// this function, but the same discipline is kept here so a panic never has to cross a
/// `spawn`ed task boundary).
async fn process_card(
    client: EngramoClient,
    engine: Arc<dyn TtsEngine>,
    card_id: Uuid,
    voice: String,
    lang: Option<String>,
    overwrite: bool,
) -> CardAudioResult {
    let raw = match client.get_card_raw(card_id).await {
        Ok(v) => v,
        Err(e) => return CardAudioResult::failed(card_id, e.to_string()),
    };
    let parsed = match parse_card_shape(&raw) {
        Ok(p) => p,
        Err(reason) => return CardAudioResult::failed(card_id, reason),
    };

    if parsed.catalog_ids.is_empty() {
        return CardAudioResult::failed(
            card_id,
            "card has no catalog membership; refusing to update (an empty catalog list would \
            move it to your default catalog)",
        );
    }
    if parsed.has_audio && !overwrite {
        return CardAudioResult::skipped(
            card_id,
            "already has face audio; pass overwrite: true to replace",
        );
    }

    let text = sanitize_text(parsed.text.trim());
    if text.trim().is_empty() {
        return CardAudioResult::skipped(card_id, "face text is empty");
    }
    let char_count = text.chars().count();
    if char_count > MAX_TTS_CHARS {
        return CardAudioResult::skipped(
            card_id,
            format!("face text is {char_count} chars; limit is {MAX_TTS_CHARS}"),
        );
    }

    let pcm = match engine
        .synthesize(TtsRequest {
            text: &text,
            voice: &voice,
            lang: lang.as_deref(),
        })
        .await
    {
        Ok(pcm) => pcm,
        Err(e) => return CardAudioResult::failed(card_id, e.to_string()),
    };

    // CPU-bound FFI (LAME) — run off the async worker threads. Goes through the
    // `AudioEncoder` seam (not `pcm16_mono_to_mp3` directly) so the encoder is the single
    // source of truth for the MIME type/extension used below too.
    let mp3 = {
        let pcm_bytes = pcm.pcm;
        let sample_rate = pcm.sample_rate;
        match tokio::task::spawn_blocking(move || {
            Mp3Encoder.encode_pcm16_mono(&pcm_bytes, sample_rate)
        })
        .await
        {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(e)) => return CardAudioResult::failed(card_id, e.to_string()),
            Err(join_err) => {
                return CardAudioResult::failed(
                    card_id,
                    format!("audio encoding task failed: {join_err}"),
                );
            }
        }
    };
    let bytes_len = mp3.len();

    let filename = format!(
        "card-{}-face.{}",
        short_id(card_id),
        Mp3Encoder.file_extension()
    );
    let media_id = match client
        .upload_media(mp3, &filename, Mp3Encoder.content_type())
        .await
    {
        Ok(id) => id,
        Err(e) => return CardAudioResult::failed(card_id, e.to_string()),
    };

    attach_media(
        &client, card_id, parsed, &text, media_id, bytes_len, overwrite,
    )
    .await
}

#[tool_router(router = local_tts_tools_router, vis = "pub(crate)")]
impl EngramoMcpServer {
    #[tool(
        description = "List the voices available for generate_card_audio, along with the \
        currently configured TTS engine, model, and default voice. No network call. Only \
        available when the user has configured their own TTS key (ENGRAMO_TTS_GEMINI_API_KEYS) \
        — this uses the user's own Gemini quota, never EngrAmo's."
    )]
    pub async fn list_tts_voices(&self) -> Result<CallToolResult, ErrorData> {
        let Some(engine) = self.tts.as_ref() else {
            return Ok(err_result(
                "TTS is not configured — set ENGRAMO_TTS_GEMINI_API_KEYS (stdio mode only) to \
                enable generate_card_audio and list_tts_voices.",
            ));
        };
        let result = ListTtsVoicesResult {
            engine: engine.name(),
            model: engine.model().to_string(),
            default_voice: engine.default_voice().to_string(),
            voices: engine
                .voices()
                .iter()
                .map(|v| VoiceInfo {
                    name: v.name,
                    description: v.description,
                })
                .collect(),
        };
        Ok(ok_json(&result))
    }

    #[tool(
        description = "Generate spoken audio for the FACE side of up to 20 flashcards, upload \
        it to your EngrAmo media, and attach it to each card. Uses YOUR OWN locally-configured \
        TTS key (ENGRAMO_TTS_GEMINI_API_KEYS) — this spends your own Gemini quota, never \
        EngrAmo's paid AI. Cards that already have face audio are skipped unless overwrite is \
        true, in which case the previous audio asset becomes unused and is cleaned up \
        server-side. Cards with empty face text, face text over 500 characters, or no catalog \
        membership are skipped/failed without spending any quota. Partial failure across a \
        batch is normal — check each entry in `results` rather than assuming the whole call \
        succeeded or failed together. Use list_tts_voices first to see available voices and \
        the configured default."
    )]
    pub async fn generate_card_audio(
        &self,
        Parameters(p): Parameters<GenerateCardAudioParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(engine) = self.tts.clone() else {
            return Ok(err_result(
                "TTS is not configured — set ENGRAMO_TTS_GEMINI_API_KEYS (stdio mode only) to \
                enable generate_card_audio and list_tts_voices.",
            ));
        };

        if p.card_ids.is_empty() {
            return Ok(err_result("card_ids must not be empty"));
        }
        if p.card_ids.len() > MAX_CARDS_PER_CALL {
            return Ok(err_result(format!(
                "card_ids has {} entries; the limit is {MAX_CARDS_PER_CALL} per call",
                p.card_ids.len()
            )));
        }

        let mut seen: HashSet<Uuid> = HashSet::new();
        let mut ids: Vec<Uuid> = Vec::with_capacity(p.card_ids.len());
        for raw in &p.card_ids {
            let id = match parse_uuid(raw) {
                Ok(id) => id,
                Err(e) => return Ok(err_result(format!("invalid card_id '{raw}': {e}"))),
            };
            if seen.insert(id) {
                ids.push(id);
            }
        }

        let voice = p
            .voice
            .unwrap_or_else(|| engine.default_voice().to_string());
        if let Err(e) = check_voice(engine.voices(), &voice) {
            return Ok(err_result(e));
        }
        if let Some(lang) = &p.lang
            && let Err(e) = check_lang(lang)
        {
            return Ok(err_result(e));
        }

        let overwrite = p.overwrite.unwrap_or(false);
        let lang = p.lang;

        let semaphore = Arc::new(tokio::sync::Semaphore::new(TTS_CONCURRENCY));
        let mut join_set: tokio::task::JoinSet<CardAudioResult> = tokio::task::JoinSet::new();
        let mut idx_by_task: HashMap<tokio::task::Id, usize> = HashMap::with_capacity(ids.len());
        for (idx, card_id) in ids.iter().copied().enumerate() {
            let client = self.client.clone();
            let engine = Arc::clone(&engine);
            let voice = voice.clone();
            let lang = lang.clone();
            let sem = Arc::clone(&semaphore);
            let handle = join_set.spawn(async move {
                let permit = match sem.acquire_owned().await {
                    Ok(permit) => permit,
                    Err(_) => {
                        return CardAudioResult::failed(
                            card_id,
                            "internal error: concurrency semaphore closed unexpectedly",
                        );
                    }
                };
                let result = process_card(client, engine, card_id, voice, lang, overwrite).await;
                drop(permit);
                result
            });
            idx_by_task.insert(handle.id(), idx);
        }

        let mut ordered: Vec<Option<CardAudioResult>> = (0..ids.len()).map(|_| None).collect();
        while let Some(joined) = join_set.join_next_with_id().await {
            match joined {
                Ok((task_id, result)) => {
                    if let Some(&idx) = idx_by_task.get(&task_id) {
                        ordered[idx] = Some(result);
                    }
                }
                Err(join_err) => {
                    if let Some(&idx) = idx_by_task.get(&join_err.id()) {
                        let card_id = ids[idx];
                        ordered[idx] = Some(CardAudioResult::failed(
                            card_id,
                            format!("internal error while processing this card: {join_err}"),
                        ));
                    }
                }
            }
        }

        let results: Vec<CardAudioResult> = ordered.into_iter().flatten().collect();
        let generated = results
            .iter()
            .filter(|r| matches!(r.status, CardAudioStatus::Generated))
            .count();
        let skipped = results
            .iter()
            .filter(|r| matches!(r.status, CardAudioStatus::Skipped))
            .count();
        let failed = results
            .iter()
            .filter(|r| matches!(r.status, CardAudioStatus::Failed))
            .count();

        Ok(ok_json(&GenerateCardAudioResult {
            generated,
            skipped,
            failed,
            results,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Redacted;
    use crate::tts::gemini::GeminiTts;
    use crate::tts::{TtsError, TtsPcm, Voice};
    use base64::Engine;
    use serde_json::json;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fake_engine() -> Arc<GeminiTts> {
        Arc::new(GeminiTts::new(
            "gemini-2.5-flash-preview-tts".to_string(),
            "Puck".to_string(),
            vec![Redacted::new("super-secret-test-key".to_string())],
        ))
    }

    /// Extracts the text of a tool result's first content block. `expect`s rather than
    /// falling back to `""` so a result with no text content fails the test with a clear
    /// message instead of a vague `contains` mismatch.
    fn result_text(result: &CallToolResult) -> &str {
        result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .expect("tool result must carry a text content block")
    }

    // ── list_tts_voices (existing) ───────────────────────────────────────────

    #[tokio::test]
    async fn test_list_tts_voices_without_engine_returns_error() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let result = server.list_tts_voices().await.unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("TTS is not configured"), "{text}");
        assert!(text.contains("ENGRAMO_TTS_GEMINI_API_KEYS"), "{text}");
    }

    #[tokio::test]
    async fn test_list_tts_voices_returns_catalog() {
        let client = EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false).with_tts(fake_engine());
        let result = server.list_tts_voices().await.unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("\"engine\": \"gemini\""), "{text}");
        assert!(text.contains("\"default_voice\": \"Puck\""), "{text}");
        assert!(text.contains("Puck"), "{text}");
        assert!(text.contains("Charon"), "{text}");
        assert_eq!(text.matches("\"name\"").count(), 8, "{text}");
        assert!(
            !text.contains("super-secret-test-key"),
            "tool result must never contain a configured key: {text}"
        );
    }

    // ── generate_card_audio: fake engine ─────────────────────────────────────

    /// In-memory `TtsEngine` so most `generate_card_audio` tests don't need to mock Gemini's
    /// `generateContent` endpoint — only the engram-api endpoints (`GET`/`PATCH /cards`,
    /// `POST /media`) are wiremocked. Fails synthesis for any request whose `text` contains
    /// `fail_marker`, and sleeps `delay` first when `text` contains `delay_marker` — used by
    /// the output-ordering test.
    struct FakeTtsEngine {
        calls: AtomicUsize,
        fail_marker: Option<&'static str>,
        delay: Option<(&'static str, Duration)>,
        /// Records the `(text, voice, lang)` of the most recent `synthesize` call, so tests
        /// can assert what actually reached the engine (sanitization, explicit voice/lang
        /// forwarding) without mocking Gemini itself.
        last_call: Mutex<Option<(String, String, Option<String>)>>,
    }

    impl FakeTtsEngine {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                fail_marker: None,
                delay: None,
                last_call: Mutex::new(None),
            }
        }

        fn failing_on(marker: &'static str) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                fail_marker: Some(marker),
                delay: None,
                last_call: Mutex::new(None),
            }
        }

        fn delaying_on(marker: &'static str, delay: Duration) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                fail_marker: None,
                delay: Some((marker, delay)),
                last_call: Mutex::new(None),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn last_call(&self) -> Option<(String, String, Option<String>)> {
            self.last_call.lock().unwrap().clone()
        }
    }

    const FAKE_VOICES: &[Voice] = &[
        Voice {
            name: "Puck",
            description: "Upbeat",
        },
        Voice {
            name: "Charon",
            description: "Informative",
        },
    ];

    impl TtsEngine for FakeTtsEngine {
        fn synthesize<'a>(
            &'a self,
            req: TtsRequest<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<TtsPcm, TtsError>> + Send + 'a>> {
            let text = req.text.to_string();
            let voice = req.voice.to_string();
            let lang = req.lang.map(|l| l.to_string());
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                *self.last_call.lock().unwrap() = Some((text.clone(), voice, lang));
                if let Some((marker, delay)) = &self.delay
                    && text.contains(marker)
                {
                    tokio::time::sleep(*delay).await;
                }
                if let Some(marker) = self.fail_marker
                    && text.contains(marker)
                {
                    return Err(TtsError::Fatal("synth failed for test".to_string()));
                }
                // Small silent PCM buffer — enough for `pcm16_mono_to_mp3` to produce real
                // output without paying for a full second of sine generation per test.
                Ok(TtsPcm {
                    pcm: vec![0u8; 400],
                    sample_rate: 24_000,
                })
            })
        }

        fn voices(&self) -> &'static [Voice] {
            FAKE_VOICES
        }

        fn default_voice(&self) -> &str {
            "Puck"
        }

        fn model(&self) -> &str {
            "fake-model"
        }

        fn name(&self) -> &'static str {
            "fake"
        }
    }

    fn server_with_engine(base_url: &str, engine: Arc<dyn TtsEngine>) -> EngramoMcpServer {
        EngramoMcpServer::new(EngramoClient::new(base_url, "engramo_test"), false).with_tts(engine)
    }

    fn card_json(
        id: Uuid,
        version: i64,
        order_number: i64,
        catalog_ids: &[Uuid],
        face: serde_json::Value,
    ) -> serde_json::Value {
        json!({
            "id": id,
            "version": version,
            "orderNumber": order_number,
            "catalogs": catalog_ids.iter().map(|c| json!({"id": c})).collect::<Vec<_>>(),
            "face": face,
            "back": {"text": "back text"}
        })
    }

    #[tokio::test]
    async fn test_generate_card_audio_happy_path_preserves_unmodelled_fields() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat1 = Uuid::new_v4();
        let cat2 = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        let face = json!({
            "text": "Hello world",
            "richText": [{"text": "Hello world"}],
            "tts": {"voice": "Puck", "custom": true},
            "audioId": null,
            "audioUrl": "https://signed.example/old.mp3",
            "visualUrl": "https://signed.example/img.png"
        });
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                3,
                7,
                &[cat1, cat2],
                face,
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .and(wiremock::matchers::body_json(json!({
                "catalogIds": [cat1, cat2],
                "orderNumber": 7,
                "version": 3,
                "face": {
                    "text": "Hello world",
                    "richText": [{"text": "Hello world"}],
                    "tts": {"voice": "Puck", "custom": true},
                    "audioId": media_id
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                4,
                7,
                &[cat1, cat2],
                json!({"text": "Hello world", "audioId": media_id}),
            )))
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("\"generated\": 1"), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_audio_empty_catalogs_fails_without_patch() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[],
                json!({"text": "hi"}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("\"failed\": 1"), "{text}");
        assert!(text.contains("catalog membership"), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_audio_existing_audio_is_skipped_without_synth_or_upload() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let existing_media = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hi", "audioId": existing_media}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&server)
            .await;

        let engine = Arc::new(FakeTtsEngine::new());
        let srv = server_with_engine(&server.uri(), engine.clone());
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"skipped\": 1"), "{text}");
        assert_eq!(engine.call_count(), 0);
    }

    #[tokio::test]
    async fn test_generate_card_audio_overwrite_true_regenerates_existing_audio() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let existing_media = Uuid::new_v4();
        let new_media = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hi", "audioId": existing_media}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [new_media]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                2,
                1,
                &[cat],
                json!({"text": "hi", "audioId": new_media}),
            )))
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: Some(true),
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"generated\": 1"), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_audio_empty_and_overlong_text_are_skipped_without_synth() {
        let server = MockServer::start().await;
        let empty_card = Uuid::new_v4();
        let long_card = Uuid::new_v4();
        let cat = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{empty_card}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                empty_card,
                1,
                1,
                &[cat],
                json!({"text": "   "}),
            )))
            .mount(&server)
            .await;
        let long_text = "x".repeat(501);
        Mock::given(method("GET"))
            .and(path(format!("/cards/{long_card}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                long_card,
                1,
                1,
                &[cat],
                json!({"text": long_text}),
            )))
            .mount(&server)
            .await;

        let engine = Arc::new(FakeTtsEngine::new());
        let srv = server_with_engine(&server.uri(), engine.clone());
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![empty_card.to_string(), long_card.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"skipped\": 2"), "{text}");
        assert_eq!(engine.call_count(), 0);
    }

    #[tokio::test]
    async fn test_generate_card_audio_sanitizes_face_text_before_synthesis() {
        let server = MockServer::start().await;
        let card_a = Uuid::new_v4();
        let card_b = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_a}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_a,
                1,
                1,
                &[cat],
                json!({"text": "hi\tthere \u{1F600}"}),
            )))
            .mount(&server)
            .await;
        // Face text made only of emoji — sanitizes down to empty, so it's skipped exactly
        // like whitespace-only text, before spending any quota.
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_b}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_b,
                1,
                1,
                &[cat],
                json!({"text": "\u{1F600}\u{1F600}"}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_a,
                2,
                1,
                &[cat],
                json!({"text": "hithere", "audioId": media_id}),
            )))
            .mount(&server)
            .await;

        let engine = Arc::new(FakeTtsEngine::new());
        let srv = server_with_engine(&server.uri(), engine.clone());
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_a.to_string(), card_b.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"generated\": 1"), "{text}");
        assert!(text.contains("\"skipped\": 1"), "{text}");
        assert!(text.contains("face text is empty"), "{text}");

        let (last_text, _voice, _lang) = engine.last_call().expect("engine must have been called");
        assert!(!last_text.contains('\t'), "{last_text}");
        assert!(!last_text.contains('\u{1F600}'), "{last_text}");
        assert_eq!(
            engine.call_count(),
            1,
            "the emoji-only card must not reach synthesis"
        );
    }

    #[tokio::test]
    async fn test_generate_card_audio_500_multibyte_chars_is_accepted() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();
        let face_text = "字".repeat(500);

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": face_text}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                2,
                1,
                &[cat],
                json!({"text": "text", "audioId": media_id}),
            )))
            .mount(&server)
            .await;

        let engine = Arc::new(FakeTtsEngine::new());
        let srv = server_with_engine(&server.uri(), engine.clone());
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"generated\": 1"), "{text}");
        assert_eq!(engine.call_count(), 1);
    }

    #[tokio::test]
    async fn test_generate_card_audio_one_synth_failure_is_isolated() {
        let server = MockServer::start().await;
        let ok_card = Uuid::new_v4();
        let bad_card = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{ok_card}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                ok_card,
                1,
                1,
                &[cat],
                json!({"text": "fine text"}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{bad_card}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                bad_card,
                1,
                1,
                &[cat],
                json!({"text": "BOOM text"}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                ok_card,
                2,
                1,
                &[cat],
                json!({"text": "fine text", "audioId": media_id}),
            )))
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::failing_on("BOOM")));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![ok_card.to_string(), bad_card.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("\"generated\": 1"), "{text}");
        assert!(text.contains("\"failed\": 1"), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_audio_upload_failure_isolated_to_one_card() {
        let server = MockServer::start().await;
        let ok_card = Uuid::new_v4();
        let fail_card = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        for id in [ok_card, fail_card] {
            Mock::given(method("GET"))
                .and(path(format!("/cards/{id}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                    id,
                    1,
                    1,
                    &[cat],
                    json!({"text": format!("text for {id}")}),
                )))
                .mount(&server)
                .await;
        }
        // First upload succeeds, second fails — wiremock matches mounted mocks in
        // registration order when priorities tie, so this simulates one card's upload
        // failing without depending on request ordering by using distinct filenames is not
        // possible with a generic path matcher; instead use up_to(1) plus a fallback.
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(
                ResponseTemplate::new(400).set_body_json(json!({"error": "upload rejected"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                ok_card,
                2,
                1,
                &[cat],
                json!({"text": "ok", "audioId": media_id}),
            )))
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![ok_card.to_string(), fail_card.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"generated\": 1"), "{text}");
        assert!(text.contains("\"failed\": 1"), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_audio_initial_fetch_failure_isolated_to_one_card() {
        let server = MockServer::start().await;
        let ok_card = Uuid::new_v4();
        let missing_card = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{ok_card}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                ok_card,
                1,
                1,
                &[cat],
                json!({"text": "fine text"}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{missing_card}")))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "not found"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                ok_card,
                2,
                1,
                &[cat],
                json!({"text": "fine text", "audioId": media_id}),
            )))
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![ok_card.to_string(), missing_card.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"generated\": 1"), "{text}");
        assert!(text.contains("\"failed\": 1"), "{text}");
        assert!(text.contains(&missing_card.to_string()), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_audio_conflict_then_success_retries_once_no_second_synth() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hi"}),
            )))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Second GET (after the 409) returns a fresh version.
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                2,
                1,
                &[cat],
                json!({"text": "hi"}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(wiremock::matchers::body_partial_json(json!({"version": 1})))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(wiremock::matchers::body_partial_json(json!({"version": 2})))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                3,
                1,
                &[cat],
                json!({"text": "hi", "audioId": media_id}),
            )))
            .mount(&server)
            .await;

        let engine = Arc::new(FakeTtsEngine::new());
        let srv = server_with_engine(&server.uri(), engine.clone());
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"generated\": 1"), "{text}");
        assert_eq!(engine.call_count(), 1, "synthesis must happen exactly once");

        let requests = server.received_requests().await.unwrap();
        let media_posts = requests.iter().filter(|r| r.url.path() == "/media").count();
        assert_eq!(media_posts, 1, "upload must happen exactly once");
        let patches = requests
            .iter()
            .filter(|r| r.method.as_str() == "PATCH")
            .count();
        assert_eq!(patches, 2, "PATCH must be attempted twice (409 then retry)");
    }

    #[tokio::test]
    async fn test_generate_card_audio_conflict_with_overwrite_true_replaces_refetched_audio() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();
        let old_media = Uuid::new_v4();
        let other_old_media = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hi", "audioId": old_media}),
            )))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Second GET (after the 409) returns a fresh version, still with pre-existing audio.
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                2,
                1,
                &[cat],
                json!({"text": "hi", "audioId": other_old_media}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(wiremock::matchers::body_partial_json(json!({"version": 1})))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(wiremock::matchers::body_partial_json(json!({"version": 2})))
            .and(wiremock::matchers::body_partial_json(
                json!({"face": {"audioId": media_id}}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                3,
                1,
                &[cat],
                json!({"text": "hi", "audioId": media_id}),
            )))
            .mount(&server)
            .await;

        let engine = Arc::new(FakeTtsEngine::new());
        let srv = server_with_engine(&server.uri(), engine.clone());
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: Some(true),
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"generated\": 1"), "{text}");
        assert_eq!(engine.call_count(), 1, "synthesis must happen exactly once");

        let requests = server.received_requests().await.unwrap();
        let patches = requests
            .iter()
            .filter(|r| r.method.as_str() == "PATCH")
            .count();
        assert_eq!(
            patches, 2,
            "conflict retry with overwrite=true must still PATCH with the refreshed version"
        );
    }

    #[tokio::test]
    async fn test_generate_card_audio_conflict_twice_fails_with_media_id() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hi"}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"failed\": 1"), "{text}");
        assert!(text.contains(&media_id.to_string()), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_audio_conflict_then_audio_already_present_skips_with_media_id() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();
        let other_media = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hi"}),
            )))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                2,
                1,
                &[cat],
                json!({"text": "hi", "audioId": other_media}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"skipped\": 1"), "{text}");
        assert!(text.contains(&media_id.to_string()), "{text}");

        let requests = server.received_requests().await.unwrap();
        let patches = requests
            .iter()
            .filter(|r| r.method.as_str() == "PATCH")
            .count();
        assert_eq!(
            patches, 1,
            "only the first PATCH attempt should have been sent"
        );
    }

    #[tokio::test]
    async fn test_generate_card_audio_conflict_then_refetch_failure_reports_media_id() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        // First GET (pre-synthesis) succeeds; the conflict-retry GET fails.
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hi"}),
            )))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"failed\": 1"), "{text}");
        assert!(
            text.contains("could not be re-fetched after a conflict"),
            "{text}"
        );
        assert!(text.contains(&media_id.to_string()), "{text}");
        assert!(text.contains("attach media_id manually"), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_audio_conflict_then_malformed_refetch_reports_media_id() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hi"}),
            )))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Re-fetch after the 409 returns a shape parse_card_shape rejects (missing "face").
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": card_id, "version": 2, "orderNumber": 1,
                "catalogs": [{"id": cat}],
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"failed\": 1"), "{text}");
        assert!(
            text.contains("re-fetched card shape was unexpected"),
            "{text}"
        );
        assert!(text.contains(&media_id.to_string()), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_audio_conflict_then_text_changed_refuses_stale_audio() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        // First GET (pre-synthesis) has "hi"; the conflict-retry GET returns different text —
        // a concurrent edit changed the sentence after audio was already synthesized for "hi".
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hi"}),
            )))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                2,
                1,
                &[cat],
                json!({"text": "a completely different sentence"}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(409))
            .expect(1)
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"failed\": 1"), "{text}");
        assert!(text.contains("changed concurrently"), "{text}");
        assert!(text.contains(&media_id.to_string()), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_audio_patch_500_fails_with_media_id_no_retry() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hi"}),
            )))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("\"failed\": 1"), "{text}");
        assert!(text.contains(&media_id.to_string()), "{text}");
        assert!(text.contains("attach media_id manually"), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_audio_conflict_then_empty_catalogs_refuses_second_patch() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hi"}),
            )))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Conflict-retry GET: the card lost all catalog membership between the first fetch
        // and the 409.
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                2,
                1,
                &[],
                json!({"text": "hi"}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(409))
            .expect(1)
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"failed\": 1"), "{text}");
        assert!(text.contains("no catalog membership"), "{text}");
        assert!(text.contains(&media_id.to_string()), "{text}");
    }

    // ── parse_card_shape: malformed GET /cards/{id} responses ────────────────

    #[tokio::test]
    async fn test_generate_card_audio_malformed_card_shapes_fail_with_reason() {
        let cases: Vec<(serde_json::Value, &str)> = vec![
            (json!([1, 2, 3]), "response is not a JSON object"),
            (
                json!({"orderNumber": 1, "catalogs": [], "face": {"text": "hi"}}),
                "missing 'version'",
            ),
            (
                json!({"version": 1, "catalogs": [], "face": {"text": "hi"}}),
                "missing 'orderNumber'",
            ),
            (
                json!({"version": 1, "orderNumber": 1, "face": {"text": "hi"}}),
                "missing or malformed 'catalogs'",
            ),
            (
                json!({"version": 1, "orderNumber": 1, "catalogs": "not-array", "face": {"text": "hi"}}),
                "missing or malformed 'catalogs'",
            ),
            (
                json!({"version": 1, "orderNumber": 1, "catalogs": [{}], "face": {"text": "hi"}}),
                "a 'catalogs' entry is missing 'id'",
            ),
            (
                json!({"version": 1, "orderNumber": 1, "catalogs": [{"id": "not-a-uuid"}], "face": {"text": "hi"}}),
                "is not a valid UUID",
            ),
            (
                json!({"version": 1, "orderNumber": 1, "catalogs": []}),
                "missing or malformed 'face'",
            ),
            (
                json!({"version": 1, "orderNumber": 1, "catalogs": [], "face": "not-an-object"}),
                "missing or malformed 'face'",
            ),
        ];

        for (body, expected_substr) in cases {
            let server = MockServer::start().await;
            let card_id = Uuid::new_v4();
            Mock::given(method("GET"))
                .and(path(format!("/cards/{card_id}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
                .mount(&server)
                .await;

            let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
            let result = srv
                .generate_card_audio(Parameters(GenerateCardAudioParams {
                    card_ids: vec![card_id.to_string()],
                    voice: None,
                    lang: None,
                    overwrite: None,
                }))
                .await
                .unwrap();
            let text = result_text(&result);
            assert!(text.contains("\"failed\": 1"), "body={body:?} text={text}");
            assert!(
                text.contains("unexpected card shape"),
                "body={body:?} text={text}"
            );
            assert!(
                text.contains(expected_substr),
                "body={body:?} expected={expected_substr} text={text}"
            );
        }
    }

    // ── whole-call validation (no mocks mounted) ─────────────────────────────

    #[tokio::test]
    async fn test_generate_card_audio_no_engine_is_error_no_requests() {
        let server = MockServer::start().await;
        let srv = EngramoMcpServer::new(EngramoClient::new(server.uri(), "t"), false);
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![Uuid::new_v4().to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("TTS is not configured"), "{text}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_generate_card_audio_empty_card_ids_is_error_no_requests() {
        let server = MockServer::start().await;
        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("must not be empty"), "{text}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_generate_card_audio_over_cap_is_error_no_requests() {
        let server = MockServer::start().await;
        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let ids: Vec<String> = (0..21).map(|_| Uuid::new_v4().to_string()).collect();
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: ids,
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("limit is 20"), "{text}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_generate_card_audio_exactly_20_ids_is_accepted() {
        // Proves the cap is `>`, not `>=` — exactly `MAX_CARDS_PER_CALL` must be accepted.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wiremock::matchers::path_regex("^/cards/"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "not found"})))
            .mount(&server)
            .await;

        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let ids: Vec<String> = (0..20).map(|_| Uuid::new_v4().to_string()).collect();
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: ids,
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("\"failed\": 20"), "{text}");
    }

    #[tokio::test]
    async fn test_generate_card_audio_face_without_text_key_is_skipped() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"audioId": null}),
            )))
            .mount(&server)
            .await;

        let engine = Arc::new(FakeTtsEngine::new());
        let srv = server_with_engine(&server.uri(), engine.clone());
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("\"skipped\": 1"), "{text}");
        assert!(text.contains("face text is empty"), "{text}");
        assert_eq!(engine.call_count(), 0);
    }

    #[tokio::test]
    async fn test_generate_card_audio_bad_uuid_is_error_no_requests() {
        let server = MockServer::start().await;
        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec!["not-a-uuid".to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("invalid card_id 'not-a-uuid'"), "{text}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_generate_card_audio_unknown_voice_is_error_no_requests() {
        let server = MockServer::start().await;
        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![Uuid::new_v4().to_string()],
                voice: Some("NotAVoice".to_string()),
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("NotAVoice"), "{text}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_generate_card_audio_bad_lang_is_error_no_requests() {
        let server = MockServer::start().await;
        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![Uuid::new_v4().to_string()],
                voice: None,
                lang: Some("this-lang-code-is-way-too-long".to_string()),
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("invalid lang"), "{text}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_generate_card_audio_empty_and_bad_char_lang_is_error_no_requests() {
        let server = MockServer::start().await;
        let srv = server_with_engine(&server.uri(), Arc::new(FakeTtsEngine::new()));
        for lang in ["", "en_US"] {
            let result = srv
                .generate_card_audio(Parameters(GenerateCardAudioParams {
                    card_ids: vec![Uuid::new_v4().to_string()],
                    voice: None,
                    lang: Some(lang.to_string()),
                    overwrite: None,
                }))
                .await
                .unwrap();
            assert!(result.is_error.unwrap_or(false), "lang={lang:?}");
            let text = result_text(&result);
            assert!(text.contains("invalid lang"), "lang={lang:?} text={text}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_generate_card_audio_forwards_explicit_voice_and_lang() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hola"}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                2,
                1,
                &[cat],
                json!({"text": "hola", "audioId": media_id}),
            )))
            .mount(&server)
            .await;

        let engine = Arc::new(FakeTtsEngine::new());
        let srv = server_with_engine(&server.uri(), engine.clone());
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: Some("Charon".to_string()),
                lang: Some("es".to_string()),
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("\"generated\": 1"), "{text}");

        let (_last_text, last_voice, last_lang) =
            engine.last_call().expect("engine must have been called");
        assert_eq!(last_voice, "Charon");
        assert_eq!(last_lang.as_deref(), Some("es"));
    }

    // ── duplicates and ordering ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_generate_card_audio_duplicates_are_deduplicated() {
        let server = MockServer::start().await;
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        for id in [a, b] {
            Mock::given(method("GET"))
                .and(path(format!("/cards/{id}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                    id,
                    1,
                    1,
                    &[cat],
                    json!({"text": format!("text {id}")}),
                )))
                .mount(&server)
                .await;
        }
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                a,
                2,
                1,
                &[cat],
                json!({"text": "text", "audioId": media_id}),
            )))
            .mount(&server)
            .await;

        let engine = Arc::new(FakeTtsEngine::new());
        let srv = server_with_engine(&server.uri(), engine.clone());
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![a.to_string(), b.to_string(), a.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        assert_eq!(text.matches("\"card_id\"").count(), 2, "{text}");
        assert_eq!(engine.call_count(), 2);
    }

    #[tokio::test]
    async fn test_generate_card_audio_output_order_matches_input_order_when_first_is_slow() {
        let server = MockServer::start().await;
        let slow_card = Uuid::new_v4();
        let fast_card = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{slow_card}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                slow_card,
                1,
                1,
                &[cat],
                json!({"text": "SLOW text here"}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/cards/{fast_card}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                fast_card,
                1,
                1,
                &[cat],
                json!({"text": "fast text here"}),
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                slow_card,
                2,
                1,
                &[cat],
                json!({"text": "text", "audioId": media_id}),
            )))
            .mount(&server)
            .await;

        let engine = Arc::new(FakeTtsEngine::delaying_on(
            "SLOW",
            Duration::from_millis(200),
        ));
        let srv = server_with_engine(&server.uri(), engine);
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![slow_card.to_string(), fast_card.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        let slow_pos = text.find(&slow_card.to_string()).unwrap();
        let fast_pos = text.find(&fast_card.to_string()).unwrap();
        assert!(
            slow_pos < fast_pos,
            "expected the slow card first in results despite finishing last: {text}"
        );
    }

    // ── end-to-end with a real GeminiTts against wiremock ────────────────────

    #[tokio::test]
    async fn test_generate_card_audio_end_to_end_with_real_gemini_engine() {
        let server = MockServer::start().await;
        let card_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        let media_id = Uuid::new_v4();

        Mock::given(method("GET"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                1,
                1,
                &[cat],
                json!({"text": "hola"}),
            )))
            .mount(&server)
            .await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 400]);
        Mock::given(method("POST"))
            .and(path("/gemini-2.5-flash-preview-tts:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {"parts": [{"inlineData": {"data": pcm_b64}}]}
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/media"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "media_ids": {"ids": [media_id]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/cards/{card_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(card_json(
                card_id,
                2,
                1,
                &[cat],
                json!({"text": "hola", "audioId": media_id}),
            )))
            .mount(&server)
            .await;

        let engine = GeminiTts::with_http_client(
            reqwest::Client::new(),
            "gemini-2.5-flash-preview-tts".to_string(),
            "Puck".to_string(),
            vec![Redacted::new("test-key".to_string())],
        )
        .with_api_base_for_tests(server.uri());
        let srv = server_with_engine(&server.uri(), Arc::new(engine));
        let result = srv
            .generate_card_audio(Parameters(GenerateCardAudioParams {
                card_ids: vec![card_id.to_string()],
                voice: None,
                lang: None,
                overwrite: None,
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = result_text(&result);
        assert!(text.contains("\"generated\": 1"), "{text}");
    }
}
