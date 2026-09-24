//! Gemini TTS engine — ported from `engram-api/premium-tts/src/gemini.rs`, with three
//! deliberate deviations required by this crate's threat model (bring-your-own, user-supplied
//! keys, over stdio only — see the local-TTS rollout plan's §0 R3/R9/R10 and §2):
//!
//! 1. **Single key pool with per-key rejection handling.** The reference has a free/paid tier
//!    split that doesn't apply here; instead, 401/403 and a 400 whose body reports
//!    `API_KEY_INVALID` are treated as *per-key* failures that rotate to the next key, so one
//!    bad key in a user-typed list can't take down the whole call (R3).
//! 2. **Scrubbing.** Every string built from an upstream response (error bodies, transport
//!    error text) passes through [`scrub`] — replace configured keys with `[REDACTED]`, then
//!    truncate — before it can land in a [`TtsError`] or a `tracing` call (R9).
//! 3. **PCM out, not encoded audio.** `synthesize` returns raw PCM; MP3 encoding is the
//!    caller's job via `tts::mp3`, so this module has no audio-codec dependency.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use base64::Engine;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::config::Redacted;
use crate::tts::mp3::GEMINI_PCM_SAMPLE_RATE;
use crate::tts::{TtsEngine, TtsError, TtsPcm, TtsRequest, Voice};

/// Longest error string (bodies, transport error text) allowed to reach a [`TtsError`] or a
/// log line, matching `ApiError::from_response`'s convention (`src/error.rs`).
const MAX_SCRUBBED_LEN: usize = 512;

/// Cap on an error-path response body read into memory before scrubbing/truncating — error
/// bodies only ever need a few hundred bytes (see [`MAX_SCRUBBED_LEN`]); this just bounds how
/// much of a misbehaving/oversized upstream response gets buffered first.
const MAX_ERROR_BODY_BYTES: usize = 16 * 1024;
/// Cap on the success-path (`generateContent`) response body read into memory — sized with
/// generous headroom over a base64-encoded few-hundred-KB PCM clip.
const MAX_AUDIO_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// Total attempts (per key) for a [`CallError::NoAudio`] outcome. Gemini TTS models sometimes
/// read a very short transcript (e.g. `你好`) as a chat turn and try to *answer* it — a 400
/// "Model tried to generate text" or an empty candidate (`finishReason: OTHER`). It's
/// nondeterministic, so one identical retry often succeeds; each attempt spends the user's
/// quota, so keep this small. (The `gemini-2.5-*-preview-tts` models do this far more often
/// than the 3.x ones — see issue #32. Prefixing the transcript with a read-aloud instruction
/// doesn't help reliably and gets *spoken* by the 3.x models, so the text is sent bare.)
const NO_AUDIO_ATTEMPTS: u32 = 2;

/// Reads at most `cap` bytes of `resp`'s body. Used on every path that reads a Gemini response
/// into memory (§R9/size-limit note in the rollout plan) so an oversized or slow-drip upstream
/// response can't grow unbounded in this process before it's scrubbed/parsed.
async fn read_capped(mut resp: reqwest::Response, cap: usize) -> Result<Vec<u8>, reqwest::Error> {
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        let room = cap.saturating_sub(buf.len());
        buf.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if buf.len() >= cap {
            break;
        }
    }
    Ok(buf)
}

/// Curated Gemini prebuilt voice catalog. The Gemini API has no runtime "list voices"
/// endpoint, so this is a fixed, documented list — kept in sync with `engram-api`'s so both
/// products describe the same voices.
pub const VOICES: &[Voice] = &[
    Voice {
        name: "Puck",
        description: "Upbeat",
    },
    Voice {
        name: "Charon",
        description: "Informative",
    },
    Voice {
        name: "Kore",
        description: "Firm",
    },
    Voice {
        name: "Aoede",
        description: "Breezy",
    },
    Voice {
        name: "Fenrir",
        description: "Excitable",
    },
    Voice {
        name: "Orus",
        description: "Firm",
    },
    Voice {
        name: "Leda",
        description: "Youthful",
    },
    Voice {
        name: "Zephyr",
        description: "Bright",
    },
];

/// Outcome classification for a single Gemini API call against one key (§2 of the rollout
/// plan). `Fatal` and `NoAudio` short-circuit rotation (neither is a per-key problem, so
/// another key would fail the same way); `RateLimited`/`Transient`/`KeyRejected` advance to
/// the next key. Message text carried here has already been through [`scrub`].
#[derive(Debug)]
enum CallError {
    RateLimited {
        retry_after: Option<Duration>,
    },
    Transient(String),
    KeyRejected(String),
    /// `retryable` is `false` for a deterministic content block (a `finishReason` other than
    /// absent/`"OTHER"`/`"STOP"`, e.g. `"SAFETY"`) — [`GeminiTts::call_with_no_audio_retry`]
    /// must not spend a same-key retry on an outcome guaranteed to repeat.
    NoAudio {
        retryable: bool,
    },
    Fatal(String),
}

impl CallError {
    /// Short, key-value-free label used to build the per-position summary in
    /// [`TtsError::AllKeysExhausted`] (e.g. "key #1: rate limited; key #2: rejected"). Never
    /// includes `detail`/`retry_after` — those are for tracing only, so a future refactor of
    /// the public summary text can't accidentally widen it with upstream detail.
    fn label(&self) -> &'static str {
        match self {
            CallError::RateLimited { .. } => "rate limited",
            CallError::Transient(_) => "transient error",
            CallError::KeyRejected(_) => "rejected",
            CallError::NoAudio { .. } => "no audio",
            CallError::Fatal(_) => "fatal error",
        }
    }

    /// Already-scrubbed detail message, for tracing only.
    fn detail(&self) -> Option<&str> {
        match self {
            CallError::Transient(msg) | CallError::KeyRejected(msg) => Some(msg),
            CallError::RateLimited { .. } | CallError::NoAudio { .. } | CallError::Fatal(_) => None,
        }
    }

    /// Parsed `Retry-After`/`retryDelay`, for tracing only — rotation never sleeps on it.
    fn retry_after(&self) -> Option<Duration> {
        match self {
            CallError::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }
}

#[derive(Serialize)]
struct GenerateContentRequest<'a> {
    contents: Vec<Content<'a>>,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig<'a>,
}

#[derive(Serialize)]
struct Content<'a> {
    parts: Vec<Part<'a>>,
}

#[derive(Serialize)]
struct Part<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct GenerationConfig<'a> {
    #[serde(rename = "responseModalities")]
    response_modalities: [&'static str; 1],
    #[serde(rename = "speechConfig")]
    speech_config: SpeechConfig<'a>,
}

#[derive(Serialize)]
struct SpeechConfig<'a> {
    #[serde(rename = "voiceConfig")]
    voice_config: VoiceConfig<'a>,
    /// Omitted entirely (not sent as `null`) when the caller doesn't specify a language —
    /// Gemini treats an absent field differently from some callers' expectation of a default.
    #[serde(rename = "languageCode", skip_serializing_if = "Option::is_none")]
    language_code: Option<&'a str>,
}

#[derive(Serialize)]
struct VoiceConfig<'a> {
    #[serde(rename = "prebuiltVoiceConfig")]
    prebuilt_voice_config: PrebuiltVoiceConfig<'a>,
}

#[derive(Serialize)]
struct PrebuiltVoiceConfig<'a> {
    #[serde(rename = "voiceName")]
    voice_name: &'a str,
}

#[derive(Debug, Deserialize, Default)]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    content: Option<ResponseContent>,
    /// Why generation stopped, e.g. `"STOP"`, `"OTHER"`, `"SAFETY"`, `"PROHIBITED_CONTENT"`,
    /// `"BLOCKLIST"`. Absent or `"OTHER"`/`"STOP"` alongside no `inlineData` is the
    /// nondeterministic "model answered as chat" case a same-key retry can fix (see
    /// [`NO_AUDIO_ATTEMPTS`]); anything else names a deterministic content block that an
    /// identical retry is guaranteed to repeat, so it must not be retried.
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponseContent {
    parts: Option<Vec<ResponsePart>>,
}

#[derive(Debug, Deserialize)]
struct ResponsePart {
    #[serde(rename = "inlineData")]
    inline_data: Option<InlineData>,
}

#[derive(Debug, Deserialize)]
struct InlineData {
    #[serde(rename = "mimeType", default)]
    mime_type: Option<String>,
    data: String,
}

/// [`TtsEngine`] backed by the Gemini `generateContent` API.
///
/// `Debug`-derived: the only field that could carry secrets is `keys`, and
/// `Vec<Redacted<String>>` always formats as `[[REDACTED], ...]` regardless of field name, so
/// no manual `Debug` impl is needed to keep a key out of a log line.
#[derive(Debug, Clone)]
pub struct GeminiTts {
    http: reqwest::Client,
    api_base: String,
    model: String,
    default_voice: String,
    keys: Vec<Redacted<String>>,
}

/// Builds the `reqwest::Client` used for real Gemini calls. Its only production caller is
/// [`GeminiTts::new`], which is reached from `tts::build_engine`. Hardcodes
/// `redirect::Policy::none()`: reqwest's default policy
/// follows up to 10 redirects and only strips `Authorization`/`Cookie`/`Proxy-Authorization`/
/// `WWW-Authenticate` on a cross-host redirect — a custom header like `x-goog-api-key` is
/// **not** stripped and would be forwarded verbatim to whatever host a 3xx response names.
/// The Gemini API key must never leave the user's machine for anywhere other than
/// `generativelanguage.googleapis.com`, so this is the one place that client gets built.
pub fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client with static, well-formed config must build")
}

impl GeminiTts {
    /// Builds a `GeminiTts` with its own hardened client (see [`build_http_client`]) — the
    /// only way to construct one outside `#[cfg(test)]`, so production code can never supply
    /// a client that hasn't had `redirect::Policy::none()` applied and quietly let the Gemini
    /// key follow a 3xx to another host.
    pub fn new(model: String, default_voice: String, keys: Vec<Redacted<String>>) -> Self {
        Self::from_parts(build_http_client(), model, default_voice, keys)
    }

    /// Shared field-assembly helper. Private (not `pub(crate)`) so [`Self::new`] and the
    /// `#[cfg(test)]`-only `Self::with_http_client` (test-only) are the only ways to reach it —
    /// no non-test production path can hand in a client that skips [`build_http_client`]'s
    /// no-redirect policy.
    fn from_parts(
        http: reqwest::Client,
        model: String,
        default_voice: String,
        keys: Vec<Redacted<String>>,
    ) -> Self {
        Self {
            http,
            api_base: "https://generativelanguage.googleapis.com/v1beta/models".to_string(),
            model,
            default_voice,
            keys,
        }
    }

    /// Client-injecting constructor, for tests only — lets tests point at a `wiremock` server
    /// via a plain `reqwest::Client` without going through [`build_http_client`]. Gated behind
    /// `#[cfg(test)]` (not merely `pub(crate)`) so no production code path in this crate can
    /// construct a `GeminiTts` with a client that hasn't had `redirect::Policy::none()`
    /// applied — see [`Self::new`].
    #[cfg(test)]
    pub(crate) fn with_http_client(
        http: reqwest::Client,
        model: String,
        default_voice: String,
        keys: Vec<Redacted<String>>,
    ) -> Self {
        Self::from_parts(http, model, default_voice, keys)
    }

    /// Overrides the Gemini API base URL — used by tests to route calls at a `wiremock`
    /// server instead of the real API. Not reachable outside `#[cfg(test)]`: the key must
    /// never be sendable to an attacker-chosen host.
    #[cfg(test)]
    pub(crate) fn with_api_base_for_tests(mut self, api_base: String) -> Self {
        self.api_base = api_base;
        self
    }

    /// Validates a request before any network call. Returns `Err(InvalidInput)` naming what's
    /// wrong with `req`.
    fn validate(&self, req: &TtsRequest<'_>) -> Result<(), TtsError> {
        if req.text.trim().is_empty() {
            return Err(TtsError::InvalidInput("text must not be empty".to_string()));
        }
        crate::tts::check_voice(VOICES, req.voice).map_err(TtsError::InvalidInput)?;
        if let Some(lang) = req.lang {
            crate::tts::check_lang(lang).map_err(TtsError::InvalidInput)?;
        }
        Ok(())
    }

    /// Single-call primitive: calls Gemini with one key and classifies the outcome into a
    /// [`CallError`]. Every upstream-derived string is scrubbed before being stored.
    async fn call_gemini(
        &self,
        key: &str,
        text: &str,
        voice: &str,
        lang: Option<&str>,
    ) -> Result<(Vec<u8>, Option<String>), CallError> {
        let url = format!("{}/{}:generateContent", self.api_base, self.model);
        let body = GenerateContentRequest {
            contents: vec![Content {
                parts: vec![Part { text }],
            }],
            generation_config: GenerationConfig {
                response_modalities: ["AUDIO"],
                speech_config: SpeechConfig {
                    voice_config: VoiceConfig {
                        prebuilt_voice_config: PrebuiltVoiceConfig { voice_name: voice },
                    },
                    language_code: lang,
                },
            },
        };

        let response = self
            .http
            .post(&url)
            .header("x-goog-api-key", key)
            .json(&body)
            .send()
            .await
            .map_err(|e| CallError::Transient(self.scrub(&e.to_string())))?;

        let status = response.status();

        if status.as_u16() == 429 {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs);
            let body_bytes = read_capped(response, MAX_ERROR_BODY_BYTES)
                .await
                .unwrap_or_default();
            let body_text = String::from_utf8_lossy(&body_bytes);
            let retry_after = retry_after.or_else(|| parse_retry_delay(&body_text));
            return Err(CallError::RateLimited { retry_after });
        }
        if matches!(status.as_u16(), 500 | 502 | 503 | 504) {
            return Err(CallError::Transient(format!("HTTP {status}")));
        }
        if status.as_u16() == 401 || status.as_u16() == 403 {
            let body_bytes = read_capped(response, MAX_ERROR_BODY_BYTES)
                .await
                .unwrap_or_default();
            let body_text = String::from_utf8_lossy(&body_bytes);
            return Err(CallError::KeyRejected(self.scrub(&body_text)));
        }
        if status.as_u16() == 400 {
            let body_bytes = read_capped(response, MAX_ERROR_BODY_BYTES)
                .await
                .unwrap_or_default();
            let body_text = String::from_utf8_lossy(&body_bytes);
            if is_api_key_invalid(&body_text) {
                return Err(CallError::KeyRejected(self.scrub(&body_text)));
            }
            if is_model_generated_text(&body_text) {
                // Nondeterministic "answered as chat" case — a same-key retry can fix it.
                return Err(CallError::NoAudio { retryable: true });
            }
            return Err(CallError::Fatal(format!(
                "Gemini rejected the request (HTTP 400) — check ENGRAMO_TTS_MODEL and the \
                voice name: {}",
                self.scrub(&body_text)
            )));
        }
        if !status.is_success() {
            let body_bytes = read_capped(response, MAX_ERROR_BODY_BYTES)
                .await
                .unwrap_or_default();
            let body_text = String::from_utf8_lossy(&body_bytes);
            return Err(CallError::Fatal(format!(
                "Gemini returned HTTP {status}: {}",
                self.scrub(&body_text)
            )));
        }

        let body_bytes = read_capped(response, MAX_AUDIO_RESPONSE_BYTES)
            .await
            .map_err(|e| CallError::Transient(self.scrub(&e.to_string())))?;
        if body_bytes.len() >= MAX_AUDIO_RESPONSE_BYTES {
            return Err(CallError::Fatal(
                "Gemini response exceeded size limit".to_string(),
            ));
        }
        let parsed: GenerateContentResponse = serde_json::from_slice(&body_bytes)
            .map_err(|e| CallError::Fatal(self.scrub(&format!("malformed response: {e}"))))?;

        let inline_data = parsed
            .candidates
            .first()
            .and_then(|c| c.content.as_ref())
            .and_then(|c| c.parts.as_ref())
            .and_then(|parts| parts.iter().find_map(|p| p.inline_data.as_ref()));

        let Some(inline_data) = inline_data else {
            // No audio part at all: retry only when the candidate's `finishReason` doesn't
            // name a deterministic content block — see `CallError::NoAudio`'s doc comment.
            let finish_reason = parsed
                .candidates
                .first()
                .and_then(|c| c.finish_reason.as_deref());
            let retryable = matches!(finish_reason, None | Some("OTHER") | Some("STOP"));
            return Err(CallError::NoAudio { retryable });
        };

        let pcm = base64::engine::general_purpose::STANDARD
            .decode(&inline_data.data)
            .map_err(|e| {
                CallError::Fatal(self.scrub(&format!("invalid base64 audio data: {e}")))
            })?;
        Ok((pcm, inline_data.mime_type.clone()))
    }

    /// [`Self::call_gemini`], retried on the same key up to [`NO_AUDIO_ATTEMPTS`] times while
    /// the outcome is [`CallError::NoAudio`] — the model's text-vs-audio choice is
    /// nondeterministic, so an identical retry can succeed. Every other outcome returns as-is.
    async fn call_with_no_audio_retry(
        &self,
        key: &str,
        req: &TtsRequest<'_>,
    ) -> Result<(Vec<u8>, Option<String>), CallError> {
        let mut attempt = 1;
        loop {
            match self.call_gemini(key, req.text, req.voice, req.lang).await {
                Err(CallError::NoAudio { retryable: true }) if attempt < NO_AUDIO_ATTEMPTS => {
                    debug!(attempt, "Gemini returned no audio, retrying");
                    attempt += 1;
                }
                outcome => return outcome,
            }
        }
    }

    /// Key rotation (§2 of the rollout plan): shuffles a clone of the key indices, tries each
    /// once, never sleeps. `Fatal` and `NoAudio` abort rotation immediately — retrying with
    /// another key would fail identically (`NoAudio` gets a bounded same-key retry first, see
    /// [`Self::call_with_no_audio_retry`]). On exhaustion, outcomes are re-sorted back into
    /// configured order so the summary always reads "key #1: ...; key #2: ..." regardless of
    /// the random try order.
    async fn rotate(&self, req: &TtsRequest<'_>) -> Result<(Vec<u8>, Option<String>), TtsError> {
        if self.keys.is_empty() {
            return Err(TtsError::AllKeysExhausted {
                summary: "no Gemini API keys configured — set ENGRAMO_TTS_GEMINI_API_KEYS"
                    .to_string(),
                all_rejected: false,
            });
        }

        let mut order: Vec<usize> = (0..self.keys.len()).collect();
        order.shuffle(&mut rand::rng());

        let mut outcomes: Vec<(usize, CallError)> = Vec::new();
        for idx in order {
            let key: &str = &self.keys[idx];
            match self.call_with_no_audio_retry(key, req).await {
                Ok((pcm, mime_type)) => {
                    debug!(
                        voice = req.voice,
                        chars = req.text.chars().count(),
                        "Gemini synthesis succeeded"
                    );
                    return Ok((pcm, mime_type));
                }
                Err(CallError::Fatal(msg)) => {
                    warn!(
                        key_position = idx + 1,
                        "Gemini rejected the request, aborting rotation"
                    );
                    return Err(TtsError::Fatal(msg));
                }
                Err(CallError::NoAudio { .. }) => {
                    // A content/model issue, not a key problem (see `TtsError::NoAudio`'s
                    // doc comment) — rotating to another key would just spend more of the
                    // user's Gemini quota on the same text for the same result.
                    warn!(
                        key_position = idx + 1,
                        "Gemini returned no audio, aborting rotation"
                    );
                    return Err(TtsError::NoAudio);
                }
                Err(other) => {
                    warn!(
                        key_position = idx + 1,
                        reason = other.label(),
                        detail = ?other.detail(),
                        retry_after = ?other.retry_after(),
                        "Gemini key failed, rotating to next key"
                    );
                    outcomes.push((idx, other));
                }
            }
        }

        outcomes.sort_by_key(|(idx, _)| *idx);

        let all_rejected = outcomes
            .iter()
            .all(|(_, e)| matches!(e, CallError::KeyRejected(_)));

        info!(
            keys_tried = outcomes.len(),
            all_rejected, "Gemini key rotation exhausted"
        );

        let per_key = outcomes
            .iter()
            .map(|(idx, e)| format!("key #{}: {}", idx + 1, e.label()))
            .collect::<Vec<_>>()
            .join("; ");

        let summary = if all_rejected {
            format!(
                "all configured Gemini API keys were rejected — check \
                ENGRAMO_TTS_GEMINI_API_KEYS: {per_key}"
            )
        } else {
            format!("all Gemini API keys were exhausted: {per_key}")
        };

        Err(TtsError::AllKeysExhausted {
            summary,
            all_rejected,
        })
    }

    async fn do_synthesize(&self, req: TtsRequest<'_>) -> Result<TtsPcm, TtsError> {
        self.validate(&req)?;
        let (audio, mime_type) = self.rotate(&req).await?;
        pcm_from_payload(audio, mime_type.as_deref()).map_err(|e| TtsError::Fatal(self.scrub(&e)))
    }

    /// Replaces every configured key with `[REDACTED]`, then truncates to
    /// [`MAX_SCRUBBED_LEN`] bytes on a char boundary — matching
    /// `ApiError::from_response`'s truncation convention (`src/error.rs`).
    fn scrub(&self, text: &str) -> String {
        scrub(text, &self.keys)
    }
}

impl TtsEngine for GeminiTts {
    fn synthesize<'a>(
        &'a self,
        req: TtsRequest<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<TtsPcm, TtsError>> + Send + 'a>> {
        Box::pin(self.do_synthesize(req))
    }

    fn voices(&self) -> &'static [Voice] {
        VOICES
    }

    fn default_voice(&self) -> &str {
        &self.default_voice
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn name(&self) -> &'static str {
        "gemini"
    }
}

/// Replaces every configured, non-empty key in `text` with `[REDACTED]`, then truncates to
/// [`MAX_SCRUBBED_LEN`] bytes on a char boundary. Standalone (not a method) so it can be
/// unit-tested directly and reused wherever upstream text needs scrubbing before it reaches a
/// [`TtsError`] or a `tracing` call.
fn scrub(text: &str, keys: &[Redacted<String>]) -> String {
    // Longest key first: if one configured key is a substring/prefix of another (e.g. a user
    // accidentally duplicates or overlaps entries in ENGRAMO_TTS_GEMINI_API_KEYS), replacing
    // the shorter one first would leave a fragment of the longer key un-redacted.
    let mut sorted: Vec<&str> = keys
        .iter()
        .map(|k| -> &str { k })
        .filter(|k| !k.is_empty())
        .collect();
    sorted.sort_by_key(|k| std::cmp::Reverse(k.len()));
    sorted.dedup();

    let mut scrubbed = text.to_string();
    for key in sorted {
        scrubbed = scrubbed.replace(key, "[REDACTED]");
    }
    truncate(&scrubbed)
}

fn truncate(s: &str) -> String {
    if s.len() <= MAX_SCRUBBED_LEN {
        s.to_string()
    } else {
        let cut = (0..=MAX_SCRUBBED_LEN)
            .rev()
            .find(|&i| s.is_char_boundary(i))
            .unwrap_or(0);
        format!("{}...[truncated]", &s[..cut])
    }
}

/// True when a Gemini 400 body reports an invalid/revoked API key rather than a malformed
/// request — the primary signal is `error.details[].reason == "API_KEY_INVALID"`; the
/// fallback (`error.status == "INVALID_ARGUMENT"` plus a message mentioning "API key") covers
/// Gemini responses that omit the structured `details` array.
fn is_api_key_invalid(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    let Some(error) = value.get("error") else {
        return false;
    };

    let reason_matches = error
        .get("details")
        .and_then(|d| d.as_array())
        .is_some_and(|details| {
            details
                .iter()
                .any(|d| d.get("reason").and_then(|r| r.as_str()) == Some("API_KEY_INVALID"))
        });
    if reason_matches {
        return true;
    }

    let status_is_invalid_argument =
        error.get("status").and_then(|s| s.as_str()) == Some("INVALID_ARGUMENT");
    let message_mentions_api_key = error
        .get("message")
        .and_then(|m| m.as_str())
        .is_some_and(|m| m.contains("API key"));
    status_is_invalid_argument && message_mentions_api_key
}

/// Sample rates the MP3 encoder config (`tts::mp3::pcm16_mono_to_mp3` — CBR 128 kbps) is known
/// to accept. Validated here, against the WAV header, so a mismatched rate fails with a message
/// that points at the Gemini payload rather than surfacing later as an opaque LAME
/// `set_sample_rate`/`build` error.
const SUPPORTED_RATES: &[u32] = &[16_000, 22_050, 24_000, 32_000, 44_100, 48_000];

/// Turns Gemini's decoded `inlineData` payload into raw PCM, dispatching on the `mimeType` that
/// accompanied it (see [`InlineData`]). The `gemini-2.5-*-preview-tts` models return bare
/// 16-bit mono PCM at [`GEMINI_PCM_SAMPLE_RATE`] (`audio/L16`); the 3.x models wrap the same
/// samples in a WAV container (`audio/wav`), whose header would otherwise be encoded as an
/// audible click. Only 16-bit mono PCM WAV, or raw L16/PCM at a rate the encoder supports, is
/// accepted — anything else is an error rather than garbage or mispitched audio.
fn pcm_from_payload(audio: Vec<u8>, mime_type: Option<&str>) -> Result<TtsPcm, String> {
    let Some(mime_type) = mime_type else {
        // No MIME type reported: fall back to sniffing, the only signal available.
        return if audio.starts_with(b"RIFF") {
            parse_wav(&audio)
        } else {
            Ok(TtsPcm {
                pcm: audio,
                sample_rate: GEMINI_PCM_SAMPLE_RATE,
            })
        };
    };

    let base_type = mime_type.split(';').next().unwrap_or("").trim();
    if base_type.eq_ignore_ascii_case("audio/wav") || base_type.eq_ignore_ascii_case("audio/x-wav")
    {
        return parse_wav(&audio);
    }
    if base_type.eq_ignore_ascii_case("audio/l16") || base_type.eq_ignore_ascii_case("audio/pcm") {
        let rate = match mime_param(mime_type, "rate") {
            None => GEMINI_PCM_SAMPLE_RATE,
            Some(v) => v
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|r| SUPPORTED_RATES.contains(r))
                .ok_or_else(|| format!("unsupported PCM sample rate from Gemini: {v}"))?,
        };
        return Ok(TtsPcm {
            pcm: audio,
            sample_rate: rate,
        });
    }
    Err(format!(
        "unsupported audio MIME type from Gemini: {mime_type}"
    ))
}

/// Extracts a `key=value` parameter from a `;`-separated MIME type string, e.g.
/// `mime_param("audio/L16;codec=pcm;rate=24000", "rate") == Some("24000")`.
fn mime_param<'a>(mime_type: &'a str, key: &str) -> Option<&'a str> {
    mime_type.split(';').skip(1).find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        k.eq_ignore_ascii_case(key).then_some(v)
    })
}

/// Parses a WAV container and returns its `data` chunk as [`TtsPcm`]. Only 16-bit mono PCM at
/// one of [`SUPPORTED_RATES`] is accepted.
fn parse_wav(audio: &[u8]) -> Result<TtsPcm, String> {
    let bad = |why: &str| format!("unsupported WAV audio from Gemini: {why}");
    if audio.get(8..12) != Some(b"WAVE".as_slice()) {
        return Err(bad("missing WAVE header"));
    }

    let mut sample_rate = None;
    let mut pos = 12;
    while let Some(header) = audio.get(pos..pos + 8) {
        let id = &header[..4];
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let body_start = pos + 8;
        // Streamed WAVs may carry a placeholder size; clamp to what's actually there.
        let body_end = body_start.saturating_add(size).min(audio.len());
        let body = &audio[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(bad("truncated fmt chunk"));
                }
                let format = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                let bits = u16::from_le_bytes([body[14], body[15]]);
                if format != 1 || channels != 1 || bits != 16 || !SUPPORTED_RATES.contains(&rate) {
                    return Err(bad(&format!(
                        "format {format}, {channels} channel(s), {bits}-bit, {rate} Hz \
                        (need 16-bit mono PCM at one of {SUPPORTED_RATES:?} Hz)"
                    )));
                }
                sample_rate = Some(rate);
            }
            b"data" => {
                let sample_rate = sample_rate.ok_or_else(|| bad("data chunk before fmt chunk"))?;
                return Ok(TtsPcm {
                    pcm: body.to_vec(),
                    sample_rate,
                });
            }
            _ => {}
        }
        // Chunks are word-aligned: an odd-sized body is followed by one pad byte.
        pos = body_end + (size & 1);
    }
    Err(bad("no data chunk"))
}

/// True when a Gemini 400 body says the model produced text instead of audio ("Model tried to
/// generate text, but it should only be used for TTS…") — the model read the transcript as a
/// prompt to answer. That's the same content/model problem as an empty-audio 200, not a
/// request-shape one, so it maps to [`CallError::NoAudio`] rather than `Fatal`.
fn is_model_generated_text(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")?
                .get("message")?
                .as_str()
                .map(|m| m.contains("Model tried to generate text"))
        })
        .unwrap_or(false)
}

/// Parses the server `retryDelay` field out of a Gemini 429 error body. Expects a Google API
/// `RetryInfo` detail with a duration string like `"12s"`.
fn parse_retry_delay(body: &str) -> Option<Duration> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let details = value.get("error")?.get("details")?.as_array()?;
    for detail in details {
        if let Some(delay) = detail.get("retryDelay").and_then(|v| v.as_str()) {
            let digits: String = delay.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(secs) = digits.parse::<u64>() {
                return Some(Duration::from_secs(secs));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_client(server: &MockServer, keys: Vec<&str>) -> GeminiTts {
        GeminiTts::with_http_client(
            reqwest::Client::new(),
            "gemini-2.5-flash-preview-tts".to_string(),
            "Puck".to_string(),
            keys.into_iter()
                .map(|k| Redacted::new(k.to_string()))
                .collect(),
        )
        .with_api_base_for_tests(server.uri())
    }

    fn req<'a>(text: &'a str, voice: &'a str, lang: Option<&'a str>) -> TtsRequest<'a> {
        TtsRequest { text, voice, lang }
    }

    fn audio_response_body_with_mime(pcm_b64: &str, mime_type: &str) -> serde_json::Value {
        json!({
            "candidates": [{
                "content": {
                    "parts": [{ "inlineData": { "mimeType": mime_type, "data": pcm_b64 } }]
                }
            }]
        })
    }

    fn audio_response_body(pcm_b64: &str) -> serde_json::Value {
        json!({
            "candidates": [{
                "content": { "parts": [{ "inlineData": { "data": pcm_b64 } }] }
            }]
        })
    }

    fn api_key_invalid_body() -> serde_json::Value {
        json!({
            "error": {
                "code": 400,
                "message": "API key not valid. Please pass a valid API key.",
                "status": "INVALID_ARGUMENT",
                "details": [{"reason": "API_KEY_INVALID"}]
            }
        })
    }

    // --- read_capped ---

    #[tokio::test]
    async fn test_read_capped_truncates_to_cap_and_returns_full_small_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 1000]))
            .mount(&server)
            .await;

        let resp = reqwest::get(server.uri()).await.unwrap();
        assert_eq!(read_capped(resp, 10).await.unwrap(), vec![7u8; 10]);

        let resp = reqwest::get(server.uri()).await.unwrap();
        assert_eq!(read_capped(resp, 4096).await.unwrap().len(), 1000);

        let resp = reqwest::get(server.uri()).await.unwrap();
        assert_eq!(read_capped(resp, 1000).await.unwrap().len(), 1000); // exact-cap boundary
    }

    // --- request shape ---

    #[tokio::test]
    async fn test_request_body_and_header_and_no_key_query_param() {
        let server = MockServer::start().await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"pcmdata");
        Mock::given(method("POST"))
            .and(path("/gemini-2.5-flash-preview-tts:generateContent"))
            .and(header("x-goog-api-key", "key1"))
            .and(wiremock::matchers::body_json(json!({
                "contents": [{"parts": [{"text": "hola"}]}],
                "generationConfig": {
                    "responseModalities": ["AUDIO"],
                    "speechConfig": {
                        "voiceConfig": {"prebuiltVoiceConfig": {"voiceName": "Charon"}},
                        "languageCode": "es"
                    }
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&pcm_b64)))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["key1"]);
        let pcm = client
            .do_synthesize(req("hola", "Charon", Some("es")))
            .await
            .unwrap();
        assert_eq!(pcm.pcm, b"pcmdata");
        assert_eq!(pcm.sample_rate, GEMINI_PCM_SAMPLE_RATE);

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].url.query().is_none() || !requests[0].url.query().unwrap().contains("key"),
            "the API key must never be sent as a query param: {:?}",
            requests[0].url
        );
    }

    #[tokio::test]
    async fn test_language_code_omitted_when_lang_none() {
        let server = MockServer::start().await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"ok");
        Mock::given(method("POST"))
            .and(wiremock::matchers::body_json(json!({
                "contents": [{"parts": [{"text": "hi"}]}],
                "generationConfig": {
                    "responseModalities": ["AUDIO"],
                    "speechConfig": {
                        "voiceConfig": {"prebuiltVoiceConfig": {"voiceName": "Puck"}}
                    }
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&pcm_b64)))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["key1"]);
        let pcm = client.do_synthesize(req("hi", "Puck", None)).await.unwrap();
        assert_eq!(pcm.pcm, b"ok");
    }

    #[tokio::test]
    async fn test_redirect_response_is_not_followed_and_key_never_reaches_redirect_target() {
        let primary = MockServer::start().await;
        let redirect_target = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(302).insert_header(
                "Location",
                format!(
                    "{}/gemini-2.5-flash-preview-tts:generateContent",
                    redirect_target.uri()
                ),
            ))
            .mount(&primary)
            .await;
        // The redirect target must never be contacted — proves the key doesn't follow the 3xx.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&redirect_target)
            .await;

        let client = GeminiTts::with_http_client(
            build_http_client(),
            "gemini-2.5-flash-preview-tts".to_string(),
            "Puck".to_string(),
            vec![Redacted::new("keyA".to_string())],
        )
        .with_api_base_for_tests(primary.uri());

        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        assert!(matches!(err, TtsError::Fatal(_)), "{err:?}");

        let redirect_target_requests = redirect_target.received_requests().await.unwrap();
        assert_eq!(
            redirect_target_requests.len(),
            0,
            "the redirect target must never receive a request — the key must not follow a 3xx \
            to another host"
        );
    }

    #[tokio::test]
    async fn test_synthesize_via_trait_object() {
        let server = MockServer::start().await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"trait-ok");
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&pcm_b64)))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["key1"]);
        let engine: &dyn TtsEngine = &client;
        assert_eq!(engine.name(), "gemini");
        assert_eq!(engine.default_voice(), "Puck");
        assert_eq!(engine.model(), "gemini-2.5-flash-preview-tts");
        assert_eq!(engine.voices().len(), VOICES.len());

        let pcm = engine.synthesize(req("hi", "Puck", None)).await.unwrap();
        assert_eq!(pcm.pcm, b"trait-ok");
    }

    // --- rotation ---

    #[tokio::test]
    async fn test_rate_limited_key_rotates_to_next_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyA"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({"error": {}})))
            .mount(&server)
            .await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"from-b");
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyB"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&pcm_b64)))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let pcm = client.do_synthesize(req("hi", "Puck", None)).await.unwrap();
        assert_eq!(pcm.pcm, b"from-b");
    }

    #[tokio::test]
    async fn test_api_key_invalid_400_rotates_to_next_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyA"))
            .respond_with(ResponseTemplate::new(400).set_body_json(api_key_invalid_body()))
            .mount(&server)
            .await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"from-b");
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyB"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&pcm_b64)))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let pcm = client.do_synthesize(req("hi", "Puck", None)).await.unwrap();
        assert_eq!(pcm.pcm, b"from-b");
    }

    #[tokio::test]
    async fn test_403_rotates_to_next_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyA"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&server)
            .await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"from-b");
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyB"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&pcm_b64)))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let pcm = client.do_synthesize(req("hi", "Puck", None)).await.unwrap();
        assert_eq!(pcm.pcm, b"from-b");
    }

    #[tokio::test]
    async fn test_503_rotates_to_next_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyA"))
            .respond_with(ResponseTemplate::new(503).set_body_string("service unavailable"))
            .mount(&server)
            .await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"from-b");
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyB"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&pcm_b64)))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let pcm = client.do_synthesize(req("hi", "Puck", None)).await.unwrap();
        assert_eq!(pcm.pcm, b"from-b");
    }

    #[tokio::test]
    async fn test_502_rotates_to_next_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyA"))
            .respond_with(ResponseTemplate::new(502).set_body_string("bad gateway"))
            .mount(&server)
            .await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"from-b");
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyB"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&pcm_b64)))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let pcm = client.do_synthesize(req("hi", "Puck", None)).await.unwrap();
        assert_eq!(pcm.pcm, b"from-b");
    }

    #[tokio::test]
    async fn test_504_rotates_to_next_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyA"))
            .respond_with(ResponseTemplate::new(504).set_body_string("gateway timeout"))
            .mount(&server)
            .await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"from-b");
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyB"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&pcm_b64)))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let pcm = client.do_synthesize(req("hi", "Puck", None)).await.unwrap();
        assert_eq!(pcm.pcm, b"from-b");
    }

    #[tokio::test]
    async fn test_all_keys_500_is_not_all_rejected() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        match err {
            TtsError::AllKeysExhausted {
                summary,
                all_rejected,
            } => {
                assert!(
                    !all_rejected,
                    "5xx is a transient failure, not a key rejection"
                );
                assert!(summary.contains("transient error"), "{summary}");
            }
            other => panic!("expected AllKeysExhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_all_keys_rejected_returns_config_error_wording() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        match err {
            TtsError::AllKeysExhausted {
                summary,
                all_rejected,
            } => {
                assert!(all_rejected);
                assert!(summary.contains("ENGRAMO_TTS_GEMINI_API_KEYS"), "{summary}");
                assert!(!summary.to_lowercase().contains("quota"), "{summary}");
            }
            other => panic!("expected AllKeysExhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_mixed_exhaustion_rate_limited_and_rejected_is_not_all_rejected() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyA"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({"error": {}})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyB"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        match err {
            TtsError::AllKeysExhausted {
                summary,
                all_rejected,
            } => {
                assert!(!all_rejected);
                assert!(summary.contains("key #1: rate limited"), "{summary}");
                assert!(summary.contains("key #2: rejected"), "{summary}");
            }
            other => panic!("expected AllKeysExhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_request_shaped_400_is_fatal_and_second_key_never_called() {
        let server = MockServer::start().await;
        // Both keys are mocked identically (request-shaped 400) so the assertion doesn't
        // depend on which key the random shuffle tries first — only that rotation stops
        // after exactly one call, whichever key that was.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": {
                    "code": 400,
                    "message": "Invalid voice name",
                    "status": "INVALID_ARGUMENT"
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        assert!(matches!(err, TtsError::Fatal(_)));

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            1,
            "a Fatal outcome must short-circuit rotation after exactly one key, regardless \
            of which key the shuffle tried first"
        );
    }

    #[tokio::test]
    async fn test_no_audio_short_circuits_rotation_after_first_key() {
        // `NoAudio` is a content/model issue, not a key problem (see `TtsError::NoAudio`'s
        // doc comment), so it must short-circuit rotation exactly like `Fatal` does — the
        // second key is mocked identically but must never be called, regardless of which key
        // the random shuffle tries first. The first key does get its bounded same-key retry.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"candidates": []})))
            .expect(u64::from(NO_AUDIO_ATTEMPTS))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        assert!(matches!(err, TtsError::NoAudio));

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            NO_AUDIO_ATTEMPTS as usize,
            "a NoAudio outcome must short-circuit rotation after exactly one key"
        );
        let keys: std::collections::HashSet<_> = requests
            .iter()
            .map(|r| r.headers.get("x-goog-api-key").unwrap().clone())
            .collect();
        assert_eq!(keys.len(), 1, "the NoAudio retry must reuse the same key");
    }

    #[tokio::test]
    async fn test_no_audio_then_audio_on_retry_succeeds() {
        let server = MockServer::start().await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"pcm");
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{"finishReason": "OTHER"}]
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&pcm_b64)))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["key1"]);
        let pcm = client
            .do_synthesize(req("再见", "Puck", Some("zh")))
            .await
            .unwrap();
        assert_eq!(pcm.pcm, b"pcm");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_model_generated_text_400_is_no_audio_not_fatal() {
        // Issue #32: Gemini 400s when the model answers the transcript instead of reading it.
        // That's a content outcome (retry, then NoAudio), not a model/voice config error.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": {
                    "code": 400,
                    "message": "Model tried to generate text, but it should only be used for \
                        TTS. Make sure your instructions are clear to only generate audio from \
                        a given text transcript.",
                    "status": "INVALID_ARGUMENT"
                }
            })))
            .expect(u64::from(NO_AUDIO_ATTEMPTS))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let err = client
            .do_synthesize(req("你好", "Puck", Some("zh")))
            .await
            .unwrap_err();
        assert!(matches!(err, TtsError::NoAudio), "got {err:?}");
    }

    #[tokio::test]
    async fn test_safety_finish_reason_is_no_audio_without_a_retry() {
        // A deterministic content block (`finishReason: "SAFETY"`) must not spend a same-key
        // retry — an identical retry is guaranteed to fail again.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{"finishReason": "SAFETY"}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        assert!(matches!(err, TtsError::NoAudio), "got {err:?}");

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            1,
            "a SAFETY finishReason must short-circuit both the same-key retry and rotation"
        );
    }

    #[tokio::test]
    async fn test_no_audio_then_rate_limited_on_retry_rotates_to_next_key() {
        let server = MockServer::start().await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"pcm");
        // 1st request: empty candidate -> NoAudio (triggers same-key retry).
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"candidates": []})))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // 2nd request (same key's retry): 429 -> RateLimited -> rotate.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({"error": {}})))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // 3rd request (other key): audio.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&pcm_b64)))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let pcm = client
            .do_synthesize(req("你好", "Puck", Some("zh")))
            .await
            .unwrap();
        assert_eq!(pcm.pcm, b"pcm");

        let reqs = server.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 3);
        let k = |i: usize| reqs[i].headers.get("x-goog-api-key").unwrap().clone();
        assert_eq!(k(0), k(1), "retry must reuse the same key");
        assert_ne!(
            k(1),
            k(2),
            "after a non-NoAudio retry outcome, rotation moves to the other key"
        );
    }

    fn wav(fmt: &[u8], extra_chunk: bool, data: &[u8]) -> Vec<u8> {
        let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
        if extra_chunk {
            // Odd-sized chunk → exercises the pad byte.
            out.extend_from_slice(b"LIST\x03\0\0\0abc\0");
        }
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    fn fmt_chunk(format: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&format.to_le_bytes());
        f.extend_from_slice(&channels.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits) / 8).to_le_bytes());
        f.extend_from_slice(&(channels * bits / 8).to_le_bytes());
        f.extend_from_slice(&bits.to_le_bytes());
        f
    }

    #[test]
    fn test_pcm_from_payload_raw_pcm_passes_through() {
        let pcm = pcm_from_payload(b"\x01\x02\x03\x04".to_vec(), None).unwrap();
        assert_eq!(pcm.pcm, b"\x01\x02\x03\x04");
        assert_eq!(pcm.sample_rate, GEMINI_PCM_SAMPLE_RATE);
    }

    #[test]
    fn test_pcm_from_payload_strips_wav_header() {
        // Shape returned by gemini-3.x TTS models (`audio/wav`).
        for extra in [false, true] {
            let payload = wav(&fmt_chunk(1, 1, 24_000, 16), extra, b"\x10\x20\x30\x40");
            let pcm = pcm_from_payload(payload, None).unwrap();
            assert_eq!(pcm.pcm, b"\x10\x20\x30\x40", "extra_chunk={extra}");
            assert_eq!(pcm.sample_rate, 24_000);
        }
        let pcm = pcm_from_payload(wav(&fmt_chunk(1, 1, 16_000, 16), false, b"ab"), None).unwrap();
        assert_eq!(pcm.sample_rate, 16_000);
    }

    #[test]
    fn test_pcm_from_payload_clamps_oversized_data_chunk() {
        let mut payload = wav(&fmt_chunk(1, 1, 24_000, 16), false, b"abcd");
        let len = payload.len();
        payload[len - 8..len - 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(pcm_from_payload(payload, None).unwrap().pcm, b"abcd");
    }

    #[test]
    fn test_pcm_from_payload_rejects_unsupported_wav() {
        for (fmt, why) in [
            (fmt_chunk(1, 2, 24_000, 16), "stereo"),
            (fmt_chunk(1, 1, 24_000, 8), "8-bit"),
            (fmt_chunk(3, 1, 24_000, 16), "float"),
            (fmt_chunk(1, 1, 8_000, 16), "8 kHz"),
            (fmt_chunk(1, 1, 24_000, 16)[..10].to_vec(), "truncated fmt"),
        ] {
            let err = pcm_from_payload(wav(&fmt, false, b"ab"), None).unwrap_err();
            assert!(err.contains("unsupported WAV"), "{why}: {err}");
        }
        assert!(pcm_from_payload(b"RIFF\0\0\0\0AVI ".to_vec(), None).is_err());
        assert!(pcm_from_payload(b"RIFF\0\0\0\0WAVE".to_vec(), None).is_err()); // no data
        let mut data_first = b"RIFF\0\0\0\0WAVEdata\x02\0\0\0ab".to_vec();
        data_first.extend_from_slice(b"fmt ");
        assert!(pcm_from_payload(data_first, None).is_err());
    }

    #[test]
    fn test_pcm_from_payload_unsupported_mime_type_is_err() {
        let err = pcm_from_payload(b"whatever".to_vec(), Some("audio/mpeg")).unwrap_err();
        assert!(err.contains("unsupported audio MIME type"), "{err}");
        assert!(err.contains("audio/mpeg"), "{err}");
    }

    #[test]
    fn test_pcm_from_payload_l16_mime_with_explicit_rate() {
        let pcm = pcm_from_payload(
            b"\x01\x02\x03\x04".to_vec(),
            Some("audio/L16;codec=pcm;rate=16000"),
        )
        .unwrap();
        assert_eq!(pcm.pcm, b"\x01\x02\x03\x04");
        assert_eq!(pcm.sample_rate, 16_000);
    }

    #[test]
    fn test_pcm_from_payload_l16_mime_rejects_unsupported_or_unparseable_rate() {
        for mime in [
            "audio/L16;rate=0",
            "audio/L16;rate=8000",
            "audio/L16;rate=96000",
            "audio/L16;rate=abc",
        ] {
            let err = pcm_from_payload(b"\x01\x02".to_vec(), Some(mime)).unwrap_err();
            assert!(err.contains("unsupported PCM sample rate"), "{mime}: {err}");
        }
    }

    #[test]
    fn test_pcm_from_payload_l16_mime_without_rate_defaults_to_gemini_rate() {
        let pcm = pcm_from_payload(b"\x01\x02".to_vec(), Some("audio/L16")).unwrap();
        assert_eq!(pcm.sample_rate, GEMINI_PCM_SAMPLE_RATE);
    }

    #[test]
    fn test_pcm_from_payload_wav_mime_type_dispatches_to_wav_parser() {
        let payload = wav(&fmt_chunk(1, 1, 44_100, 16), false, b"wavd");
        let pcm = pcm_from_payload(payload, Some("audio/wav")).unwrap();
        assert_eq!(pcm.pcm, b"wavd");
        assert_eq!(pcm.sample_rate, 44_100);
    }

    #[tokio::test]
    async fn test_wav_payload_is_unwrapped_end_to_end() {
        let server = MockServer::start().await;
        let payload = wav(&fmt_chunk(1, 1, 24_000, 16), false, b"pcmdata!");
        let b64 = base64::engine::general_purpose::STANDARD.encode(payload);
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&b64)))
            .mount(&server)
            .await;
        let client = make_client(&server, vec!["key1"]);
        let pcm = client.do_synthesize(req("hi", "Puck", None)).await.unwrap();
        assert_eq!(pcm.pcm, b"pcmdata!");
    }

    #[tokio::test]
    async fn test_unsupported_wav_payload_is_fatal_end_to_end() {
        let server = MockServer::start().await;
        let payload = wav(&fmt_chunk(1, 2, 24_000, 16), false, b"abcd"); // stereo
        let b64 = base64::engine::general_purpose::STANDARD.encode(payload);
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&b64)))
            .expect(1) // decode failure happens after rotation; no second key / retry
            .mount(&server)
            .await;
        let client = make_client(&server, vec!["keyA", "keyB"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        match err {
            TtsError::Fatal(msg) => assert!(msg.contains("unsupported WAV"), "{msg}"),
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_unsupported_mime_type_error_is_scrubbed_of_configured_key_end_to_end() {
        // F3: `pcm_from_payload`'s error text is upstream-derived (the success-body
        // `mimeType`), so `do_synthesize` must scrub it exactly like every other
        // upstream-derived `Fatal` in this file before it can reach the tool result or a log.
        let server = MockServer::start().await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"whatever");
        let mime_with_key = "audio/mpeg;codec=keyA-should-never-leak";
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(audio_response_body_with_mime(&pcm_b64, mime_with_key)),
            )
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA-should-never-leak"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        match err {
            TtsError::Fatal(msg) => {
                assert!(
                    !msg.contains("keyA-should-never-leak"),
                    "the configured key must never appear verbatim: {msg}"
                );
                assert!(msg.contains("[REDACTED]"), "{msg}");
            }
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_unsupported_mime_type_error_is_truncated_end_to_end() {
        // F3: the `mimeType` field is only capped by `MAX_AUDIO_RESPONSE_BYTES`, so an
        // oversized upstream value must still come back bounded by `MAX_SCRUBBED_LEN` once it
        // reaches a `TtsError::Fatal`.
        let server = MockServer::start().await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"whatever");
        let huge_mime = format!("audio/mpeg;junk={}", "x".repeat(100 * 1024));
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(audio_response_body_with_mime(&pcm_b64, &huge_mime)),
            )
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["key1"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        match err {
            TtsError::Fatal(msg) => {
                assert!(
                    msg.len() < 100 * 1024,
                    "error text must be truncated, got {} bytes",
                    msg.len()
                );
                assert!(msg.contains("...[truncated]"), "{msg}");
            }
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_key_spanning_truncation_boundary_is_still_scrubbed_end_to_end() {
        // F1 (iter-2 regression): `pcm_from_payload` must not truncate the upstream `mimeType`
        // itself before `do_synthesize` scrubs it — otherwise a configured key that straddles
        // the internal cut point survives as an un-redacted fragment. Place a 64-byte key so it
        // spans byte 512 of the raw `mimeType` value (the old internal truncation boundary) and
        // assert the fix (redact the full string, then truncate once at the call site) still
        // removes it completely.
        let server = MockServer::start().await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"whatever");
        // A 64-byte key placed at mime-relative offset 455: it crosses the old internal
        // truncation point (byte 512 of the raw `mimeType`) at offset 519, while its first 16
        // bytes still sit before the outer scrub-truncation point (full-message byte 512, i.e.
        // mime-relative byte 471) — exactly the window the old code leaked a key prefix from.
        let key = "0123456789ABCDEF".repeat(4);
        let mime_with_key = format!("audio/mpeg;x={}{key}", "x".repeat(442));
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(audio_response_body_with_mime(&pcm_b64, &mime_with_key)),
            )
            .mount(&server)
            .await;

        let client = make_client(&server, vec![key.as_str()]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        match err {
            TtsError::Fatal(msg) => {
                assert!(
                    !msg.contains(&key),
                    "the configured key must never appear verbatim: {msg}"
                );
                assert!(
                    !msg.contains(&key[..16]),
                    "no fragment of the configured key may survive: {msg}"
                );
                assert!(msg.contains("[REDACTED]"), "{msg}");
            }
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_wav_payload_sample_rate_propagates_end_to_end() {
        let server = MockServer::start().await;
        let payload = wav(&fmt_chunk(1, 1, 16_000, 16), true, b"pcm!");
        let b64 = base64::engine::general_purpose::STANDARD.encode(payload);
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&b64)))
            .mount(&server)
            .await;
        let client = make_client(&server, vec!["key1"]);
        let pcm = client.do_synthesize(req("hi", "Puck", None)).await.unwrap();
        assert_eq!(pcm.pcm, b"pcm!");
        assert_eq!(pcm.sample_rate, 16_000);
    }

    #[test]
    fn test_is_model_generated_text() {
        assert!(is_model_generated_text(
            r#"{"error":{"message":"Model tried to generate text, but it should only be used for TTS."}}"#
        ));
        assert!(!is_model_generated_text(
            r#"{"error":{"message":"Invalid voice name"}}"#
        ));
        assert!(!is_model_generated_text("Model tried to generate text")); // not JSON
    }

    #[tokio::test]
    async fn test_candidate_without_inline_data_is_no_audio() {
        // A text-only part (no `inlineData`).
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{"content": {"parts": [{"text": "sorry"}]}}]
            })))
            .mount(&server)
            .await;
        let client = make_client(&server, vec!["key1"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        assert!(matches!(err, TtsError::NoAudio));

        // `content: null`.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"candidates": [{"content": null}]})),
            )
            .mount(&server)
            .await;
        let client = make_client(&server, vec!["key1"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        assert!(matches!(err, TtsError::NoAudio));

        // `parts: null`.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{"content": {"parts": null}}]
            })))
            .mount(&server)
            .await;
        let client = make_client(&server, vec!["key1"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        assert!(matches!(err, TtsError::NoAudio));
    }

    #[tokio::test]
    async fn test_inline_data_in_second_part_is_found() {
        // `find_map` must not stop at the first, audio-less part.
        let server = MockServer::start().await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"pcm2");
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {"parts": [
                        {"text": "x"},
                        {"inlineData": {"data": pcm_b64}}
                    ]}
                }]
            })))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["key1"]);
        let pcm = client.do_synthesize(req("hi", "Puck", None)).await.unwrap();
        assert_eq!(pcm.pcm, b"pcm2");
    }

    #[tokio::test]
    async fn test_network_failure_is_transient_and_exhausts_keys() {
        // A transport/network failure (not an HTTP response at all) must classify as
        // `Transient` and keep rotating — same handling as a 5xx — and must never leak the
        // raw reqwest error text (which could echo the target URL/host) unscrubbed. Port 1 on
        // loopback has no listener and isn't bindable by an unprivileged process, so the
        // connection is refused immediately rather than timing out.
        let client = GeminiTts::with_http_client(
            reqwest::Client::new(),
            "gemini-2.5-flash-preview-tts".to_string(),
            "Puck".to_string(),
            vec![
                Redacted::new("keyA".to_string()),
                Redacted::new("keyB".to_string()),
            ],
        )
        .with_api_base_for_tests("http://127.0.0.1:1".to_string());

        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        match err {
            TtsError::AllKeysExhausted {
                summary,
                all_rejected,
            } => {
                assert!(!all_rejected);
                assert!(summary.contains("key #1: transient error"), "{summary}");
                assert!(summary.contains("key #2: transient error"), "{summary}");
                assert!(!summary.contains("keyA"), "{summary}");
                assert!(!summary.contains("keyB"), "{summary}");
            }
            other => panic!("expected AllKeysExhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_malformed_json_200_is_fatal_and_stops_rotation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json {"))
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        match err {
            TtsError::Fatal(m) => assert!(m.contains("malformed response"), "{m}"),
            other => panic!("expected Fatal, got {other:?}"),
        }

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "a malformed 200 body must stop rotation");
    }

    #[tokio::test]
    async fn test_non_success_status_body_is_scrubbed_and_fatal() {
        // The generic `!status.is_success()` branch (not 429/500/502/503/504/401/403/400) —
        // e.g. a 404 — must also scrub an echoed key and must stop rotation like every other
        // `Fatal` outcome.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_string("model not found for key super-secret-key-404"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["super-secret-key-404", "keyB"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        match err {
            TtsError::Fatal(m) => {
                assert!(m.contains("HTTP 404"), "{m}");
                assert!(m.contains("[REDACTED]"), "{m}");
                assert!(!m.contains("super-secret-key-404"), "{m}");
            }
            other => panic!("expected Fatal, got {other:?}"),
        }

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "rotation must stop after one key");
    }

    #[tokio::test]
    async fn test_bad_base64_is_fatal() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": { "parts": [{ "inlineData": { "data": "not-valid-base64!!!" } }] }
                }]
            })))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        assert!(matches!(err, TtsError::Fatal(_)));
    }

    #[tokio::test]
    async fn test_upstream_body_echoing_key_is_redacted() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": {
                    "code": 400,
                    "message": "request failed for key super-secret-key-123",
                    "status": "SOMETHING_ELSE"
                }
            })))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["super-secret-key-123"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(!msg.contains("super-secret-key-123"), "{msg}");
        assert!(msg.contains("[REDACTED]"), "{msg}");
    }

    #[tokio::test]
    async fn test_no_keys_configured_returns_all_keys_exhausted() {
        let server = MockServer::start().await;
        let client = make_client(&server, vec![]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        assert!(matches!(err, TtsError::AllKeysExhausted { .. }));
    }

    // --- validation (no network call) ---

    fn client_without_network() -> GeminiTts {
        GeminiTts::with_http_client(
            reqwest::Client::new(),
            "gemini-2.5-flash-preview-tts".to_string(),
            "Puck".to_string(),
            vec![Redacted::new("key1".to_string())],
        )
    }

    #[tokio::test]
    async fn test_unknown_voice_is_invalid_input() {
        let client = client_without_network();
        let err = client
            .do_synthesize(req("hi", "NotAVoice", None))
            .await
            .unwrap_err();
        match err {
            TtsError::InvalidInput(msg) => assert!(msg.contains("NotAVoice"), "{msg}"),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_bad_lang_is_invalid_input() {
        let client = client_without_network();
        let err = client
            .do_synthesize(req("hi", "Puck", Some("this-lang-code-is-way-too-long")))
            .await
            .unwrap_err();
        assert!(matches!(err, TtsError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn test_empty_text_is_invalid_input() {
        let client = client_without_network();
        let err = client
            .do_synthesize(req("   ", "Puck", None))
            .await
            .unwrap_err();
        assert!(matches!(err, TtsError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn test_empty_and_bad_char_lang_are_invalid_input() {
        let client = client_without_network();
        for lang in ["", "en_US", "en/x"] {
            let err = client
                .do_synthesize(req("hi", "Puck", Some(lang)))
                .await
                .unwrap_err();
            assert!(
                matches!(err, TtsError::InvalidInput(_)),
                "lang={lang:?} err={err:?}"
            );
        }
    }

    // --- Debug never leaks a key ---

    #[test]
    fn test_debug_never_prints_key() {
        let client = GeminiTts::with_http_client(
            reqwest::Client::new(),
            "gemini-2.5-flash-preview-tts".to_string(),
            "Puck".to_string(),
            vec![Redacted::new("super-secret-key-123".to_string())],
        );
        let rendered = format!("{client:?}");
        assert!(!rendered.contains("super-secret-key-123"), "{rendered}");
        assert!(rendered.contains("REDACTED"), "{rendered}");
    }

    // --- scrub ---

    #[test]
    fn test_scrub_replaces_multiple_keys() {
        let keys = vec![
            Redacted::new("keyA".to_string()),
            Redacted::new("keyB".to_string()),
        ];
        let out = scrub("error for keyA and also keyB failed", &keys);
        assert_eq!(out, "error for [REDACTED] and also [REDACTED] failed");
    }

    #[test]
    fn test_scrub_ignores_empty_key() {
        let keys = vec![
            Redacted::new(String::new()),
            Redacted::new("keyA".to_string()),
        ];
        let out = scrub("error for keyA", &keys);
        assert_eq!(out, "error for [REDACTED]");
    }

    #[test]
    fn test_scrub_redacts_full_key_even_when_a_shorter_key_is_a_prefix() {
        // "abc" is a prefix of "abcdef" — replacing the shorter key first would leave
        // "[REDACTED]def" behind, leaking the tail of the longer key.
        let keys = vec![
            Redacted::new("abc".to_string()),
            Redacted::new("abcdef".to_string()),
        ];
        let out = scrub("k=abcdef", &keys);
        assert_eq!(out, "k=[REDACTED]");
        assert!(!out.contains("def"), "{out}");
    }

    #[test]
    fn test_scrub_truncates_long_body_on_char_boundary() {
        // Multi-byte chars near the 512-byte cut point exercise the char-boundary search.
        let long_body: String = "é".repeat(400); // 800 bytes, well past the 512-byte limit
        let out = scrub(&long_body, &[]);
        assert!(out.len() <= MAX_SCRUBBED_LEN + "...[truncated]".len());
        assert!(out.ends_with("...[truncated]"), "{out}");
        // Must not panic on a non-boundary cut and must stay valid UTF-8 (guaranteed by
        // `String`'s invariants — this just documents the intent).
        let _ = out.chars().count();
    }

    #[test]
    fn test_scrub_short_body_is_unchanged_besides_key_replacement() {
        let out = scrub("short and fine", &[]);
        assert_eq!(out, "short and fine");
    }

    // --- is_api_key_invalid fallback (no `details` array) ---

    #[test]
    fn test_is_api_key_invalid_fallback_without_details_array() {
        let body = json!({
            "error": {
                "code": 400,
                "message": "API key not valid. Please pass a valid API key.",
                "status": "INVALID_ARGUMENT"
            }
        })
        .to_string();
        assert!(is_api_key_invalid(&body));
    }

    #[test]
    fn test_is_api_key_invalid_false_for_invalid_argument_without_api_key_message() {
        // Same status, unrelated message — must not be misclassified as a key rejection.
        let body = json!({
            "error": {
                "code": 400,
                "message": "Invalid voice name",
                "status": "INVALID_ARGUMENT"
            }
        })
        .to_string();
        assert!(!is_api_key_invalid(&body));
    }

    #[test]
    fn test_is_api_key_invalid_false_for_non_json_and_missing_error() {
        assert!(!is_api_key_invalid("<html>Bad Request</html>"));
        assert!(!is_api_key_invalid(r#"{"message":"API key not valid"}"#));
    }

    #[tokio::test]
    async fn test_non_json_400_is_fatal_and_stops_rotation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string("Bad Request"))
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let err = client
            .do_synthesize(req("hi", "Puck", None))
            .await
            .unwrap_err();
        match err {
            TtsError::Fatal(msg) => assert!(msg.contains("HTTP 400"), "{msg}"),
            other => panic!("expected Fatal, got {other:?}"),
        }

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            1,
            "a non-JSON 400 must be Fatal, not per-key"
        );
    }

    #[tokio::test]
    async fn test_api_key_invalid_400_without_details_array_rotates_to_next_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyA"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": {
                    "code": 400,
                    "message": "API key not valid. Please pass a valid API key.",
                    "status": "INVALID_ARGUMENT"
                }
            })))
            .mount(&server)
            .await;
        let pcm_b64 = base64::engine::general_purpose::STANDARD.encode(b"from-b");
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", "keyB"))
            .respond_with(ResponseTemplate::new(200).set_body_json(audio_response_body(&pcm_b64)))
            .mount(&server)
            .await;

        let client = make_client(&server, vec!["keyA", "keyB"]);
        let pcm = client.do_synthesize(req("hi", "Puck", None)).await.unwrap();
        assert_eq!(pcm.pcm, b"from-b");
    }

    // --- parse_retry_delay ---

    #[test]
    fn test_parse_retry_delay_extracts_seconds() {
        let body = json!({
            "error": { "details": [{ "retryDelay": "12s" }] }
        })
        .to_string();
        assert_eq!(parse_retry_delay(&body), Some(Duration::from_secs(12)));
    }

    #[test]
    fn test_parse_retry_delay_missing_returns_none() {
        assert_eq!(parse_retry_delay("{}"), None);
    }

    #[test]
    fn test_parse_retry_delay_malformed_json_returns_none() {
        assert_eq!(parse_retry_delay("not json"), None);
    }

    #[test]
    fn test_parse_retry_delay_skips_non_retry_details_and_non_numeric() {
        // Google usually puts an `ErrorInfo` detail (no `retryDelay`) before `RetryInfo` —
        // the loop must skip it rather than stopping at the first detail.
        let body = json!({
            "error": {"details": [{"reason": "RATE_LIMIT_EXCEEDED"}, {"retryDelay": "7s"}]}
        })
        .to_string();
        assert_eq!(parse_retry_delay(&body), Some(Duration::from_secs(7)));

        let body = json!({
            "error": {"details": [{"retryDelay": "soon"}]}
        })
        .to_string();
        assert_eq!(parse_retry_delay(&body), None);

        let body = json!({
            "error": {"details": [{"retryDelay": "1.5s"}]}
        })
        .to_string();
        assert_eq!(parse_retry_delay(&body), Some(Duration::from_secs(1)));
    }

    // --- voice catalog ---

    #[test]
    fn test_voice_catalog_has_eight_voices_default_puck() {
        assert_eq!(VOICES.len(), 8);
        assert_eq!(VOICES[0].name, "Puck");
    }
}
