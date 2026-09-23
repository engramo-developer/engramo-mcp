//! Engine-agnostic local text-to-speech layer.
//!
//! This module holds the [`TtsEngine`] trait (§1 of the local-TTS rollout plan) plus the
//! request/response/error types every engine speaks, the Gemini engine implementation
//! (`gemini`), the PCM -> MP3 audio-encoding seam (`mp3`), kept independent of any TTS
//! engine so it can be unit-tested (and swapped for a pure-Rust fallback) without a network
//! call, and TTS *config* (env vars, key parsing — Phase 3 of the rollout plan, §3).
//!
//! **Stdio-only, structurally.** [`from_env`] must be called from exactly one place:
//! `run_stdio` in `main.rs`. `http` mode never calls it and never holds a
//! [`TtsConfig`]/engine — see `main.rs::run_http` and `server::build_session_server`, which
//! has no TTS parameter at all. `McpConfig::from_env` (`config.rs`) never reads a TTS env
//! var either, so there is no path from a bearer token or the shared `http` process
//! environment to a Gemini key.

pub mod gemini;
pub mod mp3;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use thiserror::Error;

use crate::config::Redacted;

/// The only env var carrying key material. Namespaced deliberately (see the rollout plan's
/// R2): the generic `GEMINI_API_KEY` — the de-facto standard name set by the Gemini CLI,
/// Google SDKs, and many users' shell profiles — is **never read** by this module, so a key
/// already sitting in a user's environment can't silently opt them into spending it.
pub const ENV_KEYS: &str = "ENGRAM_TTS_GEMINI_API_KEYS";
/// Engine selector. Only `"gemini"` (case-insensitive) is accepted today.
pub const ENV_PROVIDER: &str = "ENGRAM_TTS_PROVIDER";
/// Gemini TTS model id override.
pub const ENV_MODEL: &str = "ENGRAM_TTS_MODEL";
/// Default voice override.
pub const ENV_VOICE: &str = "ENGRAM_TTS_VOICE";

const DEFAULT_PROVIDER: TtsProvider = TtsProvider::Gemini;
const DEFAULT_MODEL: &str = "gemini-2.5-flash-preview-tts";
const DEFAULT_VOICE: &str = "Puck";

/// Longest accepted `ENGRAM_TTS_MODEL` value — it is interpolated directly into a Gemini URL
/// path (`gemini.rs::call_gemini`), so it is restricted to a conservative, URL-path-safe
/// character set rather than trusted verbatim.
const MAX_MODEL_LEN: usize = 64;

/// Longest accepted `lang` value for a synthesis request (e.g. `generate_card_audio`'s `lang`
/// param, or a Gemini `languageCode`). Shared by [`check_lang`] so the handler-side check
/// (`tools/tts.rs`) and the engine-side check (`gemini::GeminiTts::validate`) can never
/// disagree on the limit.
pub(crate) const MAX_LANG_LEN: usize = 16;

/// Which local TTS engine to use. Only one variant today; a second engine is a new match arm
/// in [`build_engine`] plus a new variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsProvider {
    Gemini,
}

impl TtsProvider {
    pub fn name(self) -> &'static str {
        match self {
            TtsProvider::Gemini => "gemini",
        }
    }
}

/// Parsed, validated local-TTS configuration. Never logged or `Debug`-printed with its keys
/// visible — `keys` is `Vec<Redacted<String>>`, whose `Debug` always renders `[REDACTED]`
/// regardless of field name (see `config::Redacted`).
#[derive(Clone)]
pub struct TtsConfig {
    pub provider: TtsProvider,
    pub keys: Vec<Redacted<String>>,
    pub model: String,
    pub default_voice: String,
}

impl std::fmt::Debug for TtsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TtsConfig")
            .field("provider", &self.provider)
            .field("keys", &self.keys)
            .field("key_count", &self.keys.len())
            .field("model", &self.model)
            .field("default_voice", &self.default_voice)
            .finish()
    }
}

/// Errors building a [`TtsConfig`]. Every `Display` string here is safe to log or surface to
/// a user directly — never built from key material.
#[derive(Debug, Error)]
pub enum TtsConfigError {
    #[error("Unknown {ENV_PROVIDER} value '{0}'; accepted values: gemini (case-insensitive)")]
    UnknownProvider(String),

    #[error("Invalid {ENV_VOICE} value '{value}'; valid voices: {valid}")]
    InvalidVoice { value: String, valid: String },

    #[error(
        "Invalid {ENV_MODEL} value '{0}': must be 1-{MAX_MODEL_LEN} ASCII alphanumeric \
        characters, '-', '.', or '_' (it is used directly in a Gemini API URL path)"
    )]
    InvalidModel(String),
}

/// Validates a voice name against `voices`. Shared by `tools/tts.rs::generate_card_audio`'s
/// handler-side check and `gemini::GeminiTts::validate`'s engine-side check, so the two paths
/// can never disagree about which voices are valid or the wording of the error.
pub(crate) fn check_voice(voices: &[Voice], voice: &str) -> Result<(), String> {
    if voices.iter().any(|v| v.name == voice) {
        Ok(())
    } else {
        let valid: Vec<&str> = voices.iter().map(|v| v.name).collect();
        Err(format!(
            "unknown voice '{voice}'; valid voices: {}",
            valid.join(", ")
        ))
    }
}

/// Validates a BCP-47-ish language code: 1-[`MAX_LANG_LEN`] ASCII alphanumeric/`-` characters.
/// Shared by `tools/tts.rs::generate_card_audio`'s handler-side check and
/// `gemini::GeminiTts::validate`'s engine-side check.
pub(crate) fn check_lang(lang: &str) -> Result<(), String> {
    let valid_lang = !lang.is_empty()
        && lang.len() <= MAX_LANG_LEN
        && lang.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if valid_lang {
        Ok(())
    } else {
        Err(format!(
            "invalid lang '{lang}': expected 1-{MAX_LANG_LEN} ASCII alphanumeric/'-' characters"
        ))
    }
}

/// Builds a [`TtsConfig`] from an arbitrary key/value lookup — never touches process env
/// directly, so it is fully unit-testable without mutating `std::env` (rollout plan R8;
/// process-env mutation across parallel tests is fragile — see `config.rs`'s own tests).
///
/// Returns `Ok(None)` — the feature is off — when [`ENV_KEYS`] is unset, empty, all
/// whitespace, or contains only commas/blank entries. Never reads `GEMINI_API_KEY`.
pub fn from_lookup(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Option<TtsConfig>, TtsConfigError> {
    let keys: Vec<Redacted<String>> = match get(ENV_KEYS) {
        Some(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| Redacted::new(s.to_string()))
            .collect(),
        None => Vec::new(),
    };
    if keys.is_empty() {
        return Ok(None);
    }

    let provider = match get(ENV_PROVIDER) {
        Some(raw) if !raw.trim().is_empty() => {
            let trimmed = raw.trim();
            if trimmed.eq_ignore_ascii_case("gemini") {
                TtsProvider::Gemini
            } else {
                return Err(TtsConfigError::UnknownProvider(trimmed.to_string()));
            }
        }
        _ => DEFAULT_PROVIDER,
    };

    let model = match get(ENV_MODEL) {
        Some(raw) if !raw.trim().is_empty() => {
            let trimmed = raw.trim().to_string();
            validate_model(&trimmed)?;
            trimmed
        }
        _ => DEFAULT_MODEL.to_string(),
    };

    let default_voice = match get(ENV_VOICE) {
        Some(raw) if !raw.trim().is_empty() => {
            let trimmed = raw.trim().to_string();
            validate_voice(provider, &trimmed)?;
            trimmed
        }
        _ => DEFAULT_VOICE.to_string(),
    };

    Ok(Some(TtsConfig {
        provider,
        keys,
        model,
        default_voice,
    }))
}

fn validate_model(model: &str) -> Result<(), TtsConfigError> {
    let valid = !model.is_empty()
        && model.len() <= MAX_MODEL_LEN
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
    if valid {
        Ok(())
    } else {
        Err(TtsConfigError::InvalidModel(model.to_string()))
    }
}

fn validate_voice(provider: TtsProvider, voice: &str) -> Result<(), TtsConfigError> {
    let catalog = match provider {
        TtsProvider::Gemini => gemini::VOICES,
    };
    // Shares `check_voice`'s catalog lookup with the two tool-call-time checks (handler and
    // engine) so all three can never disagree on which voices are valid — this call site keeps
    // its own `TtsConfigError::InvalidVoice { value, valid }` shape (structured, not a
    // formatted string) since it reports a startup config error, not a tool result.
    check_voice(catalog, voice).map_err(|_| {
        let valid = catalog
            .iter()
            .map(|v| v.name)
            .collect::<Vec<_>>()
            .join(", ");
        TtsConfigError::InvalidVoice {
            value: voice.to_string(),
            valid,
        }
    })
}

/// One-line wrapper over [`from_lookup`] reading real process env vars. Call this from
/// exactly one place: `run_stdio` in `main.rs` — see the module-level doc comment.
pub fn from_env() -> Result<Option<TtsConfig>, TtsConfigError> {
    from_lookup(|k| std::env::var(k).ok())
}

/// Builds the engine matching `cfg.provider`. Always builds its own hardened client (see
/// `gemini::build_http_client`) — callers can no longer supply their own, so the no-redirect
/// policy that keeps the Gemini key from following a 3xx to another host can't be dropped by
/// a future edit to a call site.
pub fn build_engine(cfg: TtsConfig) -> Arc<dyn TtsEngine> {
    match cfg.provider {
        TtsProvider::Gemini => Arc::new(gemini::GeminiTts::new(
            cfg.model,
            cfg.default_voice,
            cfg.keys,
        )),
    }
}

/// One entry in a TTS engine's voice catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Voice {
    pub name: &'static str,
    pub description: &'static str,
}

/// A single synthesis request, engine-agnostic.
#[derive(Debug, Clone, Copy)]
pub struct TtsRequest<'a> {
    /// Plain text to speak.
    pub text: &'a str,
    /// Voice name — must be one of the engine's [`TtsEngine::voices`].
    pub voice: &'a str,
    /// Optional BCP-47 language code, e.g. `"en"`, `"uk"`. `None` lets the engine pick.
    pub lang: Option<&'a str>,
}

/// Raw engine output, before encoding. `tools/tts.rs::process_card` encodes it through the
/// `tts::mp3::AudioEncoder` seam (`Mp3Encoder`); the engine itself never touches an audio codec.
#[derive(Debug, Clone)]
pub struct TtsPcm {
    /// Little-endian 16-bit mono PCM samples.
    pub pcm: Vec<u8>,
    pub sample_rate: u32,
}

/// Errors a [`TtsEngine`] can return. Every `Display` string here may reach the calling LLM
/// (surfaced through a tool's `err_result`/result envelope) and may be logged — engines must
/// scrub any key material out of these before constructing them; see `gemini::scrub`.
#[derive(Debug, Error)]
pub enum TtsError {
    /// A non-recoverable, non-key-specific failure (malformed request, malformed response,
    /// an HTTP status that isn't a per-key rotation signal). Rotation across keys stops
    /// immediately — retrying with another key would fail the same way.
    #[error("{0}")]
    Fatal(String),

    /// The first key tried returned a successful response with no audio payload — a
    /// content/model issue, not a key problem, so rotation stops and it is reported directly
    /// rather than wrapped in [`Self::AllKeysExhausted`].
    #[error(
        "The TTS engine returned no audio for this text. Try shortening it, rephrasing it, \
        or using a different voice."
    )]
    NoAudio,

    /// Every configured key was tried (in a random order) and none produced audio. `summary`
    /// lists each key's outcome by its configured position (e.g. "key #1: rate limited; key
    /// #2: rejected"), never by the key value itself. `all_rejected` is true when every key
    /// failed with a key-rejected outcome (invalid/revoked key) — a configuration problem,
    /// not a quota one, and `summary` is worded accordingly.
    #[error("{summary}")]
    AllKeysExhausted { summary: String, all_rejected: bool },

    /// The request itself is invalid before any network call was made (unknown voice, bad
    /// language code, empty text, ...).
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

/// A local text-to-speech engine. Implementations own their own key rotation, request
/// building, and error classification — the trait only fixes the shape callers
/// (`tools/tts.rs`'s `generate_card_audio`) see.
///
/// The `synthesize` method returns a boxed future rather than being declared `async fn` so
/// this trait stays dyn-compatible (`Arc<dyn TtsEngine>`) without an `async-trait` dependency.
pub trait TtsEngine: Send + Sync {
    fn synthesize<'a>(
        &'a self,
        req: TtsRequest<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<TtsPcm, TtsError>> + Send + 'a>>;

    /// The engine's static voice catalog (name + short description).
    fn voices(&self) -> &'static [Voice];

    /// The voice used when a caller's request doesn't specify one.
    fn default_voice(&self) -> &str;

    /// The underlying model id (e.g. `"gemini-2.5-flash-preview-tts"`).
    fn model(&self) -> &str;

    /// Short engine identifier, e.g. `"gemini"`.
    fn name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Builds a `from_lookup`-compatible closure over a fixed map — no process env
    /// mutation anywhere in this module's tests (rollout plan R8).
    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    // --- keys unset/blank => feature off ---

    #[test]
    fn test_from_lookup_unset_returns_none() {
        let get = lookup(&[]);
        assert!(from_lookup(get).unwrap().is_none());
    }

    #[test]
    fn test_from_lookup_empty_string_returns_none() {
        let get = lookup(&[(ENV_KEYS, "")]);
        assert!(from_lookup(get).unwrap().is_none());
    }

    #[test]
    fn test_from_lookup_whitespace_only_returns_none() {
        let get = lookup(&[(ENV_KEYS, "   ")]);
        assert!(from_lookup(get).unwrap().is_none());
    }

    #[test]
    fn test_from_lookup_only_commas_and_blanks_returns_none() {
        let get = lookup(&[(ENV_KEYS, ",, ,")]);
        assert!(from_lookup(get).unwrap().is_none());
    }

    #[test]
    fn test_from_lookup_never_reads_generic_gemini_api_key() {
        // The de-facto standard var name (rollout plan R2) must never enable the feature.
        let get = lookup(&[("GEMINI_API_KEY", "some-key-value")]);
        assert!(from_lookup(get).unwrap().is_none());
    }

    // --- key parsing: split, trim, drop blanks ---

    #[test]
    fn test_from_lookup_splits_trims_and_drops_blank_keys() {
        let get = lookup(&[(ENV_KEYS, "a, b ,,c")]);
        let cfg = from_lookup(get).unwrap().unwrap();
        let keys: Vec<&str> = cfg.keys.iter().map(|k| k.as_ref()).collect();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    // --- provider ---

    #[test]
    fn test_from_lookup_provider_case_insensitive_gemini_ok() {
        let get = lookup(&[(ENV_KEYS, "key1"), (ENV_PROVIDER, "GEMINI")]);
        let cfg = from_lookup(get).unwrap().unwrap();
        assert_eq!(cfg.provider, TtsProvider::Gemini);
    }

    #[test]
    fn test_from_lookup_unknown_provider_is_err() {
        let get = lookup(&[(ENV_KEYS, "key1"), (ENV_PROVIDER, "openai")]);
        let err = from_lookup(get).unwrap_err();
        assert!(matches!(err, TtsConfigError::UnknownProvider(_)));
        assert!(err.to_string().contains(ENV_PROVIDER));
    }

    #[test]
    fn test_from_lookup_default_provider_is_gemini() {
        let get = lookup(&[(ENV_KEYS, "key1")]);
        let cfg = from_lookup(get).unwrap().unwrap();
        assert_eq!(cfg.provider, TtsProvider::Gemini);
    }

    // --- model/voice defaults ---

    #[test]
    fn test_from_lookup_blank_model_and_voice_use_defaults() {
        let get = lookup(&[(ENV_KEYS, "key1"), (ENV_MODEL, "  "), (ENV_VOICE, "")]);
        let cfg = from_lookup(get).unwrap().unwrap();
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert_eq!(cfg.default_voice, DEFAULT_VOICE);
    }

    #[test]
    fn test_from_lookup_unset_model_and_voice_use_defaults() {
        let get = lookup(&[(ENV_KEYS, "key1")]);
        let cfg = from_lookup(get).unwrap().unwrap();
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert_eq!(cfg.default_voice, DEFAULT_VOICE);
    }

    // --- voice validation ---

    #[test]
    fn test_from_lookup_bad_voice_is_err_listing_voices() {
        let get = lookup(&[(ENV_KEYS, "key1"), (ENV_VOICE, "NotAVoice")]);
        let err = from_lookup(get).unwrap_err();
        match err {
            TtsConfigError::InvalidVoice { value, valid } => {
                assert_eq!(value, "NotAVoice");
                assert!(valid.contains("Puck"), "{valid}");
                assert!(valid.contains("Charon"), "{valid}");
            }
            other => panic!("expected InvalidVoice, got {other:?}"),
        }
    }

    #[test]
    fn test_from_lookup_valid_voice_is_trimmed_and_accepted() {
        let get = lookup(&[(ENV_KEYS, "key1"), (ENV_VOICE, " Charon ")]);
        let cfg = from_lookup(get).unwrap().unwrap();
        assert_eq!(cfg.default_voice, "Charon");
    }

    // --- model validation ---

    #[test]
    fn test_from_lookup_bad_model_path_traversal_is_err() {
        let get = lookup(&[(ENV_KEYS, "key1"), (ENV_MODEL, "../x")]);
        let err = from_lookup(get).unwrap_err();
        assert!(matches!(err, TtsConfigError::InvalidModel(_)));
    }

    #[test]
    fn test_from_lookup_bad_model_slash_is_err() {
        let get = lookup(&[(ENV_KEYS, "key1"), (ENV_MODEL, "a/b")]);
        let err = from_lookup(get).unwrap_err();
        assert!(matches!(err, TtsConfigError::InvalidModel(_)));
    }

    #[test]
    fn test_from_lookup_bad_model_too_long_is_err() {
        let long_model = "a".repeat(65);
        let vars = [(ENV_KEYS, "key1"), (ENV_MODEL, long_model.as_str())];
        let get = lookup(&vars);
        let err = from_lookup(get).unwrap_err();
        assert!(matches!(err, TtsConfigError::InvalidModel(_)));
    }

    #[test]
    fn test_from_lookup_model_at_max_len_ok() {
        let model = "a".repeat(MAX_MODEL_LEN);
        let vars = [(ENV_KEYS, "key1"), (ENV_MODEL, model.as_str())];
        let get = lookup(&vars);
        let cfg = from_lookup(get).unwrap().unwrap();
        assert_eq!(cfg.model, model);
    }

    #[test]
    fn test_from_lookup_model_with_dot_underscore_dash_ok() {
        let get = lookup(&[(ENV_KEYS, "key1"), (ENV_MODEL, "gemini-2.5_flash.tts")]);
        let cfg = from_lookup(get).unwrap().unwrap();
        assert_eq!(cfg.model, "gemini-2.5_flash.tts");
    }

    // --- error messages never contain key material ---

    #[test]
    fn test_error_display_never_contains_a_key() {
        const DISTINCTIVE_KEY: &str = "super-distinctive-test-key-zzz999";
        let get = lookup(&[
            (ENV_KEYS, DISTINCTIVE_KEY),
            (ENV_PROVIDER, "not-a-provider"),
        ]);
        let err = from_lookup(get).unwrap_err();
        assert!(!err.to_string().contains(DISTINCTIVE_KEY), "{err}");

        let get = lookup(&[(ENV_KEYS, DISTINCTIVE_KEY), (ENV_VOICE, "NotAVoice")]);
        let err = from_lookup(get).unwrap_err();
        assert!(!err.to_string().contains(DISTINCTIVE_KEY), "{err}");

        let get = lookup(&[(ENV_KEYS, DISTINCTIVE_KEY), (ENV_MODEL, "a/b")]);
        let err = from_lookup(get).unwrap_err();
        assert!(!err.to_string().contains(DISTINCTIVE_KEY), "{err}");
    }

    // --- Debug never leaks a key ---

    #[test]
    fn test_tts_config_debug_never_prints_key() {
        const DISTINCTIVE_KEY: &str = "another-distinctive-test-key-999";
        let get = lookup(&[(ENV_KEYS, DISTINCTIVE_KEY)]);
        let cfg = from_lookup(get).unwrap().unwrap();
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains(DISTINCTIVE_KEY), "{rendered}");
        assert!(rendered.contains("REDACTED"), "{rendered}");
        assert!(rendered.contains("key_count: 1"), "{rendered}");
    }

    // --- TtsProvider::name ---

    #[test]
    fn test_tts_provider_name() {
        assert_eq!(TtsProvider::Gemini.name(), "gemini");
    }

    // --- build_engine ---

    #[test]
    fn test_build_engine_returns_gemini_engine() {
        let cfg = TtsConfig {
            provider: TtsProvider::Gemini,
            keys: vec![Redacted::new("key1".to_string())],
            model: DEFAULT_MODEL.to_string(),
            default_voice: DEFAULT_VOICE.to_string(),
        };
        let engine = build_engine(cfg);
        assert_eq!(engine.name(), "gemini");
        assert_eq!(engine.model(), DEFAULT_MODEL);
        assert_eq!(engine.default_voice(), DEFAULT_VOICE);
    }

    // --- check_lang / check_voice ---

    #[test]
    fn test_check_lang_boundaries() {
        assert!(check_lang(&"a".repeat(MAX_LANG_LEN)).is_ok());
        assert!(check_lang(&"a".repeat(MAX_LANG_LEN + 1)).is_err());
        assert!(check_lang("en-US").is_ok());
        assert!(check_lang("").is_err());
        assert!(check_lang("en US").is_err());
    }

    #[test]
    fn test_check_voice_error_lists_all_valid_voices() {
        assert!(check_voice(gemini::VOICES, "Puck").is_ok());
        let e = check_voice(gemini::VOICES, "puck").unwrap_err(); // case-sensitive
        for v in gemini::VOICES {
            assert!(e.contains(v.name), "{e}");
        }
    }
}
