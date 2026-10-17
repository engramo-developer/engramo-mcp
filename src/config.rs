use std::env;

/// Wraps a secret value so it is redacted from `{:?}` by construction, rather
/// than relying on a hand-maintained `Debug` impl on whatever struct holds it —
/// a field added to that struct later is covered automatically instead of
/// silently reaching a log line through a derived `Debug`.
#[derive(Clone)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    pub fn new(inner: T) -> Self {
        Self(inner)
    }
}

impl<T> std::fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// `Target = str` (not `String`) so `Option<Redacted<String>>::as_deref()` —
/// used by `require_token` and by tests — yields `Option<&str>` directly.
impl std::ops::Deref for Redacted<String> {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct McpConfig {
    /// Base URL of the Engramo REST API (e.g. "https://api.engram.dev")
    pub api_url: String,
    /// User's API token (starts with "engramo_"). Required for `stdio` mode
    /// (one server process = one user). `None` in `http` mode, where each
    /// session supplies its own bearer token — see `require_token`. Wrapped in
    /// [`Redacted`] so it can never reach a log line through a `?`-sigil
    /// tracing field or a `{:?}` format — see `error_reporting::ErrorReportingLayer`.
    pub api_token: Option<Redacted<String>>,
    /// Whether the paid-AI tool router (TTS, translation, dictionary, AI-agent
    /// chat) is registered. Loaded from `ENGRAMO_ENABLE_PAID_AI` (default `false`).
    pub paid_ai_enabled: bool,
    /// This server's own canonical public URL (e.g. `https://mcp.engramo.app`),
    /// used only by `http` mode's `.well-known/oauth-protected-resource` route
    /// (Track 3 Phase 3 — see `engram-ws/tdds/track3-mcp-chatgpt-app.md`) to
    /// advertise the `resource` value a ChatGPT-side OAuth client must request a
    /// token for. `None` in `stdio` mode (unused there) and, in `http` mode, when
    /// `MCP_PUBLIC_URL` hasn't been set — a real deployment must set it.
    pub public_url: Option<String>,
    /// Extra hostnames/`host:port` values permitted on the inbound HTTP `Host`
    /// header, on top of the one auto-derived from `public_url` — see
    /// [`Self::allowed_hosts`]. Loaded from `MCP_ALLOWED_HOSTS` (comma-separated),
    /// e.g. to also allow a Cloud Run service's own `*.run.app` fallback URL
    /// alongside its custom domain mapping.
    pub extra_allowed_hosts: Vec<String>,
    /// Token served verbatim at `GET /.well-known/openai-apps-challenge` for
    /// OpenAI domain verification. Loaded from `OPENAI_APPS_CHALLENGE` (trimmed;
    /// empty/unset → `None`, and the route 404s). `http` mode only.
    pub openai_apps_challenge: Option<String>,
}

impl McpConfig {
    /// Construct directly from values (used in tests and programmatic setup).
    pub fn new(
        api_url: impl Into<String>,
        api_token: Option<String>,
        paid_ai_enabled: bool,
    ) -> Result<Self, ConfigError> {
        let api_url = api_url.into();

        if api_url.is_empty() {
            return Err(ConfigError::EmptyVar("ENGRAMO_API_URL"));
        }
        if let Some(ref token) = api_token
            && token.is_empty()
        {
            return Err(ConfigError::EmptyVar("ENGRAMO_API_TOKEN"));
        }

        let api_url = api_url.trim_end_matches('/').to_string();
        Ok(Self {
            api_url,
            api_token: api_token.map(Redacted::new),
            paid_ai_enabled,
            public_url: None,
            extra_allowed_hosts: Vec::new(),
            openai_apps_challenge: None,
        })
    }

    /// Sets [`Self::public_url`]. Builder-style so `from_env` and tests can
    /// attach it without changing `new`'s signature (which many call sites
    /// already depend on).
    pub fn with_public_url(mut self, public_url: impl Into<String>) -> Self {
        self.public_url = Some(public_url.into());
        self
    }

    /// Sets [`Self::openai_apps_challenge`]; blank values become `None`.
    pub fn with_openai_apps_challenge(mut self, token: impl AsRef<str>) -> Self {
        let t = token.as_ref().trim();
        self.openai_apps_challenge = (!t.is_empty()).then(|| t.to_string());
        self
    }

    /// Sets [`Self::extra_allowed_hosts`]. Builder-style, same reasoning as
    /// [`Self::with_public_url`].
    pub fn with_extra_allowed_hosts(
        mut self,
        hosts: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.extra_allowed_hosts = hosts.into_iter().map(Into::into).collect();
        self
    }

    /// Hosts permitted on the inbound HTTP `Host` header in `http` mode (rmcp's
    /// `StreamableHttpServerConfig::allowed_hosts` DNS-rebinding guard — see
    /// `main.rs::run_http`): the loopback defaults (for local testing without
    /// `MCP_PUBLIC_URL` set), the host:port parsed out of `public_url` (a real
    /// deployment's own domain), and `extra_allowed_hosts`.
    pub fn allowed_hosts(&self) -> Vec<String> {
        let mut hosts = vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
        ];
        if let Some(url) = &self.public_url
            && let Some(host) = authority_from_url(url)
        {
            hosts.push(host);
        }
        hosts.extend(self.extra_allowed_hosts.iter().cloned());
        hosts
    }

    /// Load configuration from environment variables.
    ///
    /// Required:
    /// - `ENGRAMO_API_URL`
    ///
    /// Optional:
    /// - `ENGRAMO_API_TOKEN` — required by `stdio` mode only (see `require_token`);
    ///   ignored by `http` mode, which derives a client token per session.
    /// - `ENGRAMO_ENABLE_PAID_AI` — `true`/`1`/`yes`/`on` to enable the paid-AI
    ///   tools; defaults to `false`.
    /// - `MCP_PUBLIC_URL` — this server's own canonical public URL; required by
    ///   `http` mode (see `require_public_url`), unused by `stdio`.
    /// - `OPENAI_APPS_CHALLENGE` — OpenAI domain-verification token served at
    ///   `/.well-known/openai-apps-challenge` in `http` mode; unset/blank → 404.
    /// - `MCP_ALLOWED_HOSTS` — comma-separated extra hostnames/`host:port` values
    ///   for the `http` mode inbound `Host`-header allowlist (see `allowed_hosts`).
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_url =
            env_or_legacy("ENGRAMO_API_URL").ok_or(ConfigError::MissingVar("ENGRAMO_API_URL"))?;
        let api_token = env_or_legacy("ENGRAMO_API_TOKEN");
        let paid_ai_enabled = env_or_legacy("ENGRAMO_ENABLE_PAID_AI")
            .map(|v| parse_bool_flag(&v))
            .unwrap_or(false);
        let cfg = Self::new(api_url, api_token, paid_ai_enabled)?;
        let cfg = match env::var("MCP_PUBLIC_URL").ok() {
            Some(url) if !url.is_empty() => cfg.with_public_url(url),
            _ => cfg,
        };
        let extra_hosts: Vec<String> = env::var("MCP_ALLOWED_HOSTS")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let cfg = match env::var("OPENAI_APPS_CHALLENGE").ok() {
            Some(t) => cfg.with_openai_apps_challenge(t),
            None => cfg,
        };
        Ok(cfg.with_extra_allowed_hosts(extra_hosts))
    }

    /// Returns the public URL, or an error if none was set. Call this in `http`
    /// mode, which cannot advertise correct OAuth metadata without it.
    ///
    /// The value is advertised in the `WWW-Authenticate` challenge and the RFC 9728
    /// `resource` field, so it must be an absolute `https://` URL (`http://` only for a
    /// loopback host) free of whitespace, control characters, `"` and `\`.
    pub fn require_public_url(&self) -> Result<&str, ConfigError> {
        let url = self
            .public_url
            .as_deref()
            .ok_or(ConfigError::MissingVar("MCP_PUBLIC_URL"))?;
        validate_public_url(url)?;
        Ok(url)
    }

    /// Returns the configured API token, or an error if none was set.
    /// Call this in `stdio` mode, where a single global token is required.
    pub fn require_token(&self) -> Result<&str, ConfigError> {
        self.api_token
            .as_deref()
            .ok_or(ConfigError::MissingVar("ENGRAMO_API_TOKEN"))
    }
}

/// Checks `MCP_PUBLIC_URL` is safe to embed in a quoted `WWW-Authenticate` parameter and
/// is an absolute URL clients can trust as a resource identifier.
fn validate_public_url(url: &str) -> Result<(), ConfigError> {
    let invalid = |reason| ConfigError::InvalidVar {
        var: "MCP_PUBLIC_URL",
        reason,
    };
    if url
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\')
    {
        return Err(invalid(
            "must not contain whitespace, control characters, '\"' or '\\'",
        ));
    }
    let host_of = |rest: &str| -> Result<String, ConfigError> {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.contains('@') {
            return Err(invalid("must have a plain host[:port] authority"));
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => {
                let (h, rest) = v6
                    .split_once(']')
                    .ok_or_else(|| invalid("must have a plain host[:port] authority"))?;
                (h, rest.strip_prefix(':'))
            }
            None => match authority.split_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (authority, None),
            },
        };
        if host.is_empty() || port.is_some_and(|p| p.parse::<u16>().is_err()) {
            return Err(invalid("must have a plain host[:port] authority"));
        }
        Ok(host.to_string())
    };
    if let Some(rest) = url.strip_prefix("https://") {
        host_of(rest)?;
        return Ok(());
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let host = host_of(rest)?;
        if matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1") {
            return Ok(());
        }
        return Err(invalid("must use https:// unless the host is loopback"));
    }
    Err(invalid("must be an absolute http(s):// URL"))
}

/// Extracts the `host[:port]` authority out of a URL, e.g.
/// `"https://mcp.engramo.app/status"` -> `Some("mcp.engramo.app")`. Used to derive
/// an `allowed_hosts` entry from `public_url` without pulling in a full URL
/// parsing crate for this one call site.
fn authority_from_url(url: &str) -> Option<String> {
    let without_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = without_scheme.split('/').next()?;
    if authority.is_empty() {
        None
    } else {
        Some(authority.to_string())
    }
}

/// Reads `new` (an `ENGRAMO_*` var), falling back to its pre-rebrand `ENGRAM_*` name when `new`
/// is unset — so an existing deployment's config doesn't silently break on upgrade. Emits a
/// `warn!` naming only the variable, never its value, when the legacy fallback is used.
///
/// `pub(crate)` and taking `&str` (not `&'static str`) so `tts::from_env` (`tts/mod.rs`) can
/// reuse it as the `from_lookup` callback instead of duplicating this fallback logic — the two
/// callers must never derive the legacy name or word the `warn!` differently.
pub(crate) fn env_or_legacy(new: &str) -> Option<String> {
    env::var(new).ok().or_else(|| {
        let legacy = new.replacen("ENGRAMO_", "ENGRAM_", 1);
        let v = env::var(&legacy).ok()?;
        tracing::warn!(legacy, new, "deprecated env var in use; rename it");
        Some(v)
    })
}

/// Parses a truthy env-var string (`"1"`, `"true"`, `"yes"`, `"on"`, case-insensitive,
/// surrounding whitespace ignored). Anything else — including unset/empty — is falsy.
fn parse_bool_flag(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Required environment variable {0} is not set")]
    MissingVar(&'static str),

    #[error("Environment variable {0} is empty")]
    EmptyVar(&'static str),

    #[error("Environment variable {var} is invalid: {reason}")]
    InvalidVar {
        var: &'static str,
        reason: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_success() {
        let cfg = McpConfig::new(
            "https://api.engram.dev",
            Some("engramo_abc123".to_string()),
            false,
        )
        .unwrap();
        assert_eq!(cfg.api_url, "https://api.engram.dev");
        assert_eq!(cfg.api_token.as_deref(), Some("engramo_abc123"));
        assert!(!cfg.paid_ai_enabled);
    }

    #[test]
    fn test_new_trailing_slash_stripped() {
        let cfg = McpConfig::new(
            "https://api.engram.dev/",
            Some("engramo_abc123".to_string()),
            false,
        )
        .unwrap();
        assert_eq!(cfg.api_url, "https://api.engram.dev");
    }

    #[test]
    fn test_new_multiple_trailing_slashes_stripped() {
        let cfg = McpConfig::new(
            "https://api.engram.dev///",
            Some("engramo_abc123".to_string()),
            false,
        )
        .unwrap();
        assert_eq!(cfg.api_url, "https://api.engram.dev");
    }

    #[test]
    fn test_new_empty_url() {
        let err = McpConfig::new("", Some("engramo_abc123".to_string()), false).unwrap_err();
        assert!(matches!(err, ConfigError::EmptyVar("ENGRAMO_API_URL")));
        assert!(err.to_string().contains("ENGRAMO_API_URL"));
    }

    #[test]
    fn test_new_empty_token() {
        let err = McpConfig::new("https://api.engram.dev", Some(String::new()), false).unwrap_err();
        assert!(matches!(err, ConfigError::EmptyVar("ENGRAMO_API_TOKEN")));
        assert!(err.to_string().contains("ENGRAMO_API_TOKEN"));
    }

    #[test]
    fn test_new_no_token_ok_for_http_mode() {
        let cfg = McpConfig::new("https://api.engram.dev", None, false).unwrap();
        assert!(cfg.api_token.is_none());
    }

    #[test]
    fn test_require_token_present() {
        let cfg = McpConfig::new(
            "https://api.engram.dev",
            Some("engramo_abc123".to_string()),
            false,
        )
        .unwrap();
        assert_eq!(cfg.require_token().unwrap(), "engramo_abc123");
    }

    #[test]
    fn test_require_token_missing() {
        let cfg = McpConfig::new("https://api.engram.dev", None, false).unwrap();
        let err = cfg.require_token().unwrap_err();
        assert!(matches!(err, ConfigError::MissingVar("ENGRAMO_API_TOKEN")));
    }

    #[test]
    fn test_require_public_url_present_and_missing() {
        let cfg = McpConfig::new("https://api.engram.dev", None, false).unwrap();
        assert!(matches!(
            cfg.require_public_url().unwrap_err(),
            ConfigError::MissingVar("MCP_PUBLIC_URL")
        ));
        let cfg = cfg.with_public_url("https://mcp.engramo.app");
        assert_eq!(cfg.require_public_url().unwrap(), "https://mcp.engramo.app");
    }

    #[test]
    fn test_invalid_var_display_names_var_and_reason() {
        let err = McpConfig::new("https://api.engram.dev", None, false)
            .unwrap()
            .with_public_url("http://mcp.engramo.app")
            .require_public_url()
            .unwrap_err();
        let s = err.to_string();
        assert!(s.contains("MCP_PUBLIC_URL"), "{s}");
        assert!(s.contains("must use https://"), "{s}");
    }

    #[test]
    fn test_require_public_url_validation() {
        let base = McpConfig::new("https://api.engram.dev", None, false).unwrap();
        for ok in [
            "https://mcp.engramo.app",
            "https://mcp.engramo.app/",
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
        ] {
            let cfg = base.clone().with_public_url(ok);
            assert_eq!(cfg.require_public_url().unwrap(), ok, "{ok}");
        }
        for bad in [
            "mcp.engramo.app",
            "ftp://mcp.engramo.app",
            "http://mcp.engramo.app",
            "https://",
            "https://@",
            "https://:443",
            "https://mcp.engramo.app:abc",
            "https://[::1",
            "http://:8080",
            "http://127.0.0.1:x@evil.example",
            "http://localhost:1@evil.example",
            "https://u@mcp.engramo.app",
            "https://mcp.engramo.app/\"x",
            "https://mcp.engramo.app/a\\b",
            "https://mcp.engramo.app/ x",
            "https://mcp.engramo.app/\nx",
        ] {
            let cfg = base.clone().with_public_url(bad);
            assert!(
                matches!(
                    cfg.require_public_url().unwrap_err(),
                    ConfigError::InvalidVar {
                        var: "MCP_PUBLIC_URL",
                        ..
                    }
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn test_openai_apps_challenge_trimmed_and_blank_is_none() {
        let cfg = McpConfig::new("https://api.engram.dev", None, false).unwrap();
        assert!(cfg.openai_apps_challenge.is_none());
        let set = cfg.clone().with_openai_apps_challenge("  tok123\n");
        assert_eq!(set.openai_apps_challenge.as_deref(), Some("tok123"));
        let blank = cfg.with_openai_apps_challenge("   ");
        assert!(blank.openai_apps_challenge.is_none());
    }

    #[test]
    fn test_missing_var_display() {
        let err = ConfigError::MissingVar("ENGRAMO_API_URL");
        assert!(err.to_string().contains("ENGRAMO_API_URL"));
        assert!(err.to_string().contains("not set"));
    }

    #[test]
    fn test_empty_var_display() {
        let err = ConfigError::EmptyVar("ENGRAMO_API_TOKEN");
        assert!(err.to_string().contains("ENGRAMO_API_TOKEN"));
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn test_parse_bool_flag_truthy_values() {
        for v in ["1", "true", "TRUE", "yes", "on", " On "] {
            assert!(parse_bool_flag(v), "expected '{v}' to be truthy");
        }
    }

    #[test]
    fn test_parse_bool_flag_falsy_values() {
        for v in ["0", "false", "no", "off", "", "garbage"] {
            assert!(!parse_bool_flag(v), "expected '{v}' to be falsy");
        }
    }

    #[test]
    fn test_new_paid_ai_enabled_flag_propagates() {
        let cfg = McpConfig::new("https://api.engram.dev", None, true).unwrap();
        assert!(cfg.paid_ai_enabled);
    }

    #[test]
    fn test_new_public_url_defaults_to_none() {
        let cfg = McpConfig::new("https://api.engram.dev", None, false).unwrap();
        assert!(cfg.public_url.is_none());
    }

    #[test]
    fn test_with_public_url_sets_field() {
        let cfg = McpConfig::new("https://api.engram.dev", None, false)
            .unwrap()
            .with_public_url("https://mcp.engramo.app");
        assert_eq!(cfg.public_url.as_deref(), Some("https://mcp.engramo.app"));
    }

    #[test]
    fn test_authority_from_url_strips_scheme_and_path() {
        assert_eq!(
            authority_from_url("https://mcp.engramo.app/status"),
            Some("mcp.engramo.app".to_string())
        );
    }

    #[test]
    fn test_authority_from_url_keeps_port() {
        assert_eq!(
            authority_from_url("http://localhost:8080"),
            Some("localhost:8080".to_string())
        );
    }

    #[test]
    fn test_authority_from_url_no_scheme() {
        assert_eq!(
            authority_from_url("mcp.engramo.app/status"),
            Some("mcp.engramo.app".to_string())
        );
    }

    #[test]
    fn test_authority_from_url_empty() {
        assert_eq!(authority_from_url(""), None);
        assert_eq!(authority_from_url("https:///status"), None);
    }

    #[test]
    fn test_allowed_hosts_defaults_to_loopback_only() {
        let cfg = McpConfig::new("https://api.engram.dev", None, false).unwrap();
        assert_eq!(cfg.allowed_hosts(), vec!["localhost", "127.0.0.1", "::1"]);
    }

    #[test]
    fn test_allowed_hosts_includes_public_url_authority() {
        let cfg = McpConfig::new("https://api.engram.dev", None, false)
            .unwrap()
            .with_public_url("https://mcp.engramo.app");
        assert_eq!(
            cfg.allowed_hosts(),
            vec!["localhost", "127.0.0.1", "::1", "mcp.engramo.app"]
        );
    }

    #[test]
    fn test_allowed_hosts_includes_extra_hosts() {
        let cfg = McpConfig::new("https://api.engram.dev", None, false)
            .unwrap()
            .with_public_url("https://mcp.engramo.app/mcp")
            .with_extra_allowed_hosts(["engramo-mcp-632347647951.europe-west1.run.app"]);
        assert_eq!(
            cfg.allowed_hosts(),
            vec![
                "localhost",
                "127.0.0.1",
                "::1",
                "mcp.engramo.app",
                "engramo-mcp-632347647951.europe-west1.run.app",
            ]
        );
    }

    #[test]
    fn test_redacted_debug_never_prints_the_wrapped_value() {
        let r = Redacted::new("engramo_super_secret".to_string());
        let rendered = format!("{r:?}");
        assert_eq!(rendered, "[REDACTED]");
    }

    #[test]
    fn test_debug_redacts_api_token() {
        let cfg = McpConfig::new(
            "https://api.engram.dev",
            Some("engramo_super_secret".to_string()),
            false,
        )
        .unwrap();
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("engramo_super_secret"), "{rendered}");
        assert!(rendered.contains("[REDACTED]"), "{rendered}");
    }

    #[test]
    fn test_debug_shows_none_token_as_none() {
        let cfg = McpConfig::new("https://api.engram.dev", None, false).unwrap();
        assert!(format!("{cfg:?}").contains("api_token: None"));
    }

    #[test]
    fn test_from_env_reads_openai_apps_challenge() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("ENGRAMO_API_URL", "https://api.engram.dev");
            std::env::set_var("OPENAI_APPS_CHALLENGE", "  abc-token \n");
        }
        let set = McpConfig::from_env().unwrap();
        unsafe { std::env::set_var("OPENAI_APPS_CHALLENGE", "") };
        let empty = McpConfig::from_env().unwrap();
        unsafe {
            std::env::remove_var("OPENAI_APPS_CHALLENGE");
            std::env::remove_var("ENGRAMO_API_URL");
        }
        assert_eq!(set.openai_apps_challenge.as_deref(), Some("abc-token"));
        assert!(empty.openai_apps_challenge.is_none());
    }

    #[test]
    fn test_from_env_parses_mcp_allowed_hosts() {
        // SAFETY: test-only env mutation, serialized by `ENV_LOCK` — this module's other
        // `from_env` test also mutates `ENGRAMO_API_URL`.
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("ENGRAMO_API_URL", "https://api.engram.dev");
            std::env::set_var("MCP_ALLOWED_HOSTS", "example.com, foo.example.com:9000 ,,");
        }
        let cfg = McpConfig::from_env().unwrap();
        unsafe {
            std::env::remove_var("ENGRAMO_API_URL");
            std::env::remove_var("MCP_ALLOWED_HOSTS");
        }
        assert_eq!(
            cfg.extra_allowed_hosts,
            vec!["example.com", "foo.example.com:9000"]
        );
    }

    /// Serializes this module's `from_env` tests, which mutate process-wide env vars
    /// (`std::env::set_var`/`remove_var`) and would otherwise race under `cargo test`'s
    /// default multi-threaded runner.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_from_env_reads_renamed_engramo_vars_and_missing_url() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("ENGRAMO_API_URL");
            std::env::remove_var("ENGRAMO_API_TOKEN");
            std::env::remove_var("ENGRAMO_ENABLE_PAID_AI");
            std::env::remove_var("ENGRAM_API_URL");
            std::env::remove_var("ENGRAM_API_TOKEN");
            std::env::remove_var("ENGRAM_ENABLE_PAID_AI");
        }
        let err = McpConfig::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::MissingVar("ENGRAMO_API_URL")));

        unsafe {
            std::env::set_var("ENGRAMO_API_URL", "https://api.engram.dev/");
            std::env::set_var("ENGRAMO_API_TOKEN", "engramo_tok");
            std::env::set_var("ENGRAMO_ENABLE_PAID_AI", "yes");
        }
        let cfg = McpConfig::from_env();
        unsafe {
            std::env::remove_var("ENGRAMO_API_URL");
            std::env::remove_var("ENGRAMO_API_TOKEN");
            std::env::remove_var("ENGRAMO_ENABLE_PAID_AI");
        }
        let cfg = cfg.unwrap();
        assert_eq!(cfg.api_url, "https://api.engram.dev");
        assert_eq!(cfg.require_token().unwrap(), "engramo_tok");
        assert!(cfg.paid_ai_enabled);
    }

    #[test]
    fn mcpb_manifest_matches_env_contract() {
        let raw = include_str!("../mcpb/manifest.json");
        let m: serde_json::Value = serde_json::from_str(raw).expect("manifest.json is valid JSON");

        // The version is a placeholder that release.yml rewrites.
        assert_eq!(m["version"], "0.0.0");

        let env = m["server"]["mcp_config"]["env"]
            .as_object()
            .expect("env object");
        let mut keys: Vec<&str> = env.keys().map(String::as_str).collect();
        keys.sort();
        let mut expected = vec!["ENGRAMO_API_URL", "ENGRAMO_API_TOKEN", crate::tts::ENV_KEYS];
        expected.sort();
        assert_eq!(keys, expected);

        // stdio mode requires a token: it must come from a required user_config field.
        assert_eq!(env["ENGRAMO_API_TOKEN"], "${user_config.api_token}");
        assert_eq!(m["user_config"]["api_token"]["required"], true);
        assert_eq!(m["user_config"]["api_token"]["sensitive"], true);
        assert_eq!(m["user_config"]["gemini_keys"]["sensitive"], true);
        // An unset optional field must resolve to "", never a leftover placeholder.
        assert_eq!(m["user_config"]["gemini_keys"]["default"], "");

        // Every ${user_config.X} reference names a defined user_config key.
        let uc = m["user_config"].as_object().expect("user_config object");
        for v in env.values() {
            let s = v.as_str().expect("env values are strings");
            if let Some(name) = s
                .strip_prefix("${user_config.")
                .and_then(|r| r.strip_suffix('}'))
            {
                assert!(uc.contains_key(name), "undefined user_config key: {name}");
            }
        }

        // http-only / paid-AI vars must never be wired into the stdio extension.
        for forbidden in ["ENGRAMO_ENABLE_PAID_AI", "MCP_BIND_ADDR", "GEMINI_API_KEY"] {
            assert!(
                !env.contains_key(forbidden),
                "{forbidden} must not be in manifest env"
            );
        }
    }

    #[test]
    fn server_json_matches_env_contract() {
        let sj: serde_json::Value = serde_json::from_str(include_str!("../server.json"))
            .expect("server.json is valid JSON");
        let pkg_json: serde_json::Value =
            serde_json::from_str(include_str!("../npm/engramo-mcp/package.json"))
                .expect("package.json is valid JSON");

        // Placeholders that release.yml rewrites.
        assert_eq!(sj["version"], "0.0.0");
        // The registry verifies this name against the npm package's mcpName.
        assert_eq!(sj["name"], pkg_json["mcpName"]);
        assert_eq!(sj["name"], "app.engramo/mcp");

        let packages = sj["packages"].as_array().expect("packages array");
        assert!(
            packages.iter().any(|p| p["registryType"] == "npm"),
            "npm package present"
        );
        let mut expected = vec!["ENGRAMO_API_URL", "ENGRAMO_API_TOKEN", crate::tts::ENV_KEYS];
        expected.sort();

        for pkg in packages {
            assert_eq!(pkg["transport"]["type"], "stdio");
            assert_eq!(pkg["version"].as_str().unwrap_or("0.0.0"), "0.0.0");
            let vars = pkg["environmentVariables"].as_array().expect("env vars");
            let mut names: Vec<&str> = vars.iter().map(|v| v["name"].as_str().unwrap()).collect();
            names.sort();
            assert_eq!(names, expected);

            let var = |n: &str| vars.iter().find(|v| v["name"] == n).unwrap();
            // stdio requires the token, and the token / TTS keys are secrets.
            assert_eq!(var("ENGRAMO_API_TOKEN")["isRequired"], true);
            assert_eq!(var("ENGRAMO_API_TOKEN")["isSecret"], true);
            assert_eq!(var(crate::tts::ENV_KEYS)["isSecret"], true);
            assert_ne!(var(crate::tts::ENV_KEYS)["isRequired"], true);
            assert_eq!(var("ENGRAMO_API_URL")["default"], "https://api.engramo.app");

            // http-only / paid-AI vars must never be advertised for stdio.
            for forbidden in [
                "MCP_PUBLIC_URL",
                "MCP_BIND_ADDR",
                "OPENAI_APPS_CHALLENGE",
                "ENGRAMO_ENABLE_PAID_AI",
                "MCP_ALLOWED_HOSTS",
                "GEMINI_API_KEY",
            ] {
                assert!(
                    !names.contains(&forbidden),
                    "{forbidden} must not be in server.json env"
                );
            }
        }

        let mcpb = packages
            .iter()
            .find(|p| p["registryType"] == "mcpb")
            .expect("mcpb package");
        assert!(
            mcpb["identifier"]
                .as_str()
                .unwrap()
                .ends_with("/download/v0.0.0/engramo.mcpb")
        );
        assert!(mcpb["fileSha256"].is_string());

        let remote = &sj["remotes"][0];
        assert_eq!(remote["type"], "streamable-http");
        assert_eq!(remote["url"], "https://mcp.engramo.app/");
    }
}
