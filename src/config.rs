use std::env;

#[derive(Debug, Clone)]
pub struct McpConfig {
    /// Base URL of the Engram REST API (e.g. "https://api.engram.dev")
    pub api_url: String,
    /// User's API token (starts with "engram_"). Required for `stdio` mode
    /// (one server process = one user). `None` in `http` mode, where each
    /// session supplies its own bearer token — see `require_token`.
    pub api_token: Option<String>,
    /// Whether the paid-AI tool router (TTS, translation, dictionary, AI-agent
    /// chat) is registered. Loaded from `ENGRAM_ENABLE_PAID_AI` (default `false`).
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
            return Err(ConfigError::EmptyVar("ENGRAM_API_URL"));
        }
        if let Some(ref token) = api_token
            && token.is_empty()
        {
            return Err(ConfigError::EmptyVar("ENGRAM_API_TOKEN"));
        }

        let api_url = api_url.trim_end_matches('/').to_string();
        Ok(Self {
            api_url,
            api_token,
            paid_ai_enabled,
            public_url: None,
            extra_allowed_hosts: Vec::new(),
        })
    }

    /// Sets [`Self::public_url`]. Builder-style so `from_env` and tests can
    /// attach it without changing `new`'s signature (which many call sites
    /// already depend on).
    pub fn with_public_url(mut self, public_url: impl Into<String>) -> Self {
        self.public_url = Some(public_url.into());
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
    /// - `ENGRAM_API_URL`
    ///
    /// Optional:
    /// - `ENGRAM_API_TOKEN` — required by `stdio` mode only (see `require_token`);
    ///   ignored by `http` mode, which derives a client token per session.
    /// - `ENGRAM_ENABLE_PAID_AI` — `true`/`1`/`yes`/`on` to enable the paid-AI
    ///   tools; defaults to `false`.
    /// - `MCP_PUBLIC_URL` — this server's own canonical public URL; only read by
    ///   `http` mode's OAuth protected-resource metadata route.
    /// - `MCP_ALLOWED_HOSTS` — comma-separated extra hostnames/`host:port` values
    ///   for the `http` mode inbound `Host`-header allowlist (see `allowed_hosts`).
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_url =
            env::var("ENGRAM_API_URL").map_err(|_| ConfigError::MissingVar("ENGRAM_API_URL"))?;
        let api_token = env::var("ENGRAM_API_TOKEN").ok();
        let paid_ai_enabled = env::var("ENGRAM_ENABLE_PAID_AI")
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
        Ok(cfg.with_extra_allowed_hosts(extra_hosts))
    }

    /// Returns the configured API token, or an error if none was set.
    /// Call this in `stdio` mode, where a single global token is required.
    pub fn require_token(&self) -> Result<&str, ConfigError> {
        self.api_token
            .as_deref()
            .ok_or(ConfigError::MissingVar("ENGRAM_API_TOKEN"))
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_success() {
        let cfg = McpConfig::new(
            "https://api.engram.dev",
            Some("engram_abc123".to_string()),
            false,
        )
        .unwrap();
        assert_eq!(cfg.api_url, "https://api.engram.dev");
        assert_eq!(cfg.api_token.as_deref(), Some("engram_abc123"));
        assert!(!cfg.paid_ai_enabled);
    }

    #[test]
    fn test_new_trailing_slash_stripped() {
        let cfg = McpConfig::new(
            "https://api.engram.dev/",
            Some("engram_abc123".to_string()),
            false,
        )
        .unwrap();
        assert_eq!(cfg.api_url, "https://api.engram.dev");
    }

    #[test]
    fn test_new_multiple_trailing_slashes_stripped() {
        let cfg = McpConfig::new(
            "https://api.engram.dev///",
            Some("engram_abc123".to_string()),
            false,
        )
        .unwrap();
        assert_eq!(cfg.api_url, "https://api.engram.dev");
    }

    #[test]
    fn test_new_empty_url() {
        let err = McpConfig::new("", Some("engram_abc123".to_string()), false).unwrap_err();
        assert!(matches!(err, ConfigError::EmptyVar("ENGRAM_API_URL")));
        assert!(err.to_string().contains("ENGRAM_API_URL"));
    }

    #[test]
    fn test_new_empty_token() {
        let err = McpConfig::new("https://api.engram.dev", Some(String::new()), false).unwrap_err();
        assert!(matches!(err, ConfigError::EmptyVar("ENGRAM_API_TOKEN")));
        assert!(err.to_string().contains("ENGRAM_API_TOKEN"));
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
            Some("engram_abc123".to_string()),
            false,
        )
        .unwrap();
        assert_eq!(cfg.require_token().unwrap(), "engram_abc123");
    }

    #[test]
    fn test_require_token_missing() {
        let cfg = McpConfig::new("https://api.engram.dev", None, false).unwrap();
        let err = cfg.require_token().unwrap_err();
        assert!(matches!(err, ConfigError::MissingVar("ENGRAM_API_TOKEN")));
    }

    #[test]
    fn test_missing_var_display() {
        let err = ConfigError::MissingVar("ENGRAM_API_URL");
        assert!(err.to_string().contains("ENGRAM_API_URL"));
        assert!(err.to_string().contains("not set"));
    }

    #[test]
    fn test_empty_var_display() {
        let err = ConfigError::EmptyVar("ENGRAM_API_TOKEN");
        assert!(err.to_string().contains("ENGRAM_API_TOKEN"));
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
    fn test_from_env_parses_mcp_allowed_hosts() {
        // SAFETY: test-only env mutation, serialized by the process-wide env lock
        // pattern already used by other from_env tests in this module... this is
        // the first one that needs MCP_ALLOWED_HOSTS specifically, set/cleared
        // within the same test to avoid leaking into others.
        unsafe {
            std::env::set_var("ENGRAM_API_URL", "https://api.engram.dev");
            std::env::set_var("MCP_ALLOWED_HOSTS", "example.com, foo.example.com:9000 ,,");
        }
        let cfg = McpConfig::from_env().unwrap();
        unsafe {
            std::env::remove_var("ENGRAM_API_URL");
            std::env::remove_var("MCP_ALLOWED_HOSTS");
        }
        assert_eq!(
            cfg.extra_allowed_hosts,
            vec!["example.com", "foo.example.com:9000"]
        );
    }
}
