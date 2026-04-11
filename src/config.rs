use std::env;

#[derive(Debug, Clone)]
pub struct McpConfig {
    /// Base URL of the Engram REST API (e.g. "https://api.engram.dev")
    pub api_url: String,
    /// User's API token (starts with "engram_")
    pub api_token: String,
}

impl McpConfig {
    /// Construct directly from values (used in tests and programmatic setup).
    pub fn new(
        api_url: impl Into<String>,
        api_token: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        let api_url = api_url.into();
        let api_token = api_token.into();

        if api_url.is_empty() {
            return Err(ConfigError::EmptyVar("ENGRAM_API_URL"));
        }
        if api_token.is_empty() {
            return Err(ConfigError::EmptyVar("ENGRAM_API_TOKEN"));
        }

        let api_url = api_url.trim_end_matches('/').to_string();
        Ok(Self { api_url, api_token })
    }

    /// Load configuration from environment variables.
    ///
    /// Required:
    /// - `ENGRAM_API_URL`
    /// - `ENGRAM_API_TOKEN`
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_url =
            env::var("ENGRAM_API_URL").map_err(|_| ConfigError::MissingVar("ENGRAM_API_URL"))?;
        let api_token = env::var("ENGRAM_API_TOKEN")
            .map_err(|_| ConfigError::MissingVar("ENGRAM_API_TOKEN"))?;
        Self::new(api_url, api_token)
    }
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
        let cfg = McpConfig::new("https://api.engram.dev", "engram_abc123").unwrap();
        assert_eq!(cfg.api_url, "https://api.engram.dev");
        assert_eq!(cfg.api_token, "engram_abc123");
    }

    #[test]
    fn test_new_trailing_slash_stripped() {
        let cfg = McpConfig::new("https://api.engram.dev/", "engram_abc123").unwrap();
        assert_eq!(cfg.api_url, "https://api.engram.dev");
    }

    #[test]
    fn test_new_multiple_trailing_slashes_stripped() {
        let cfg = McpConfig::new("https://api.engram.dev///", "engram_abc123").unwrap();
        assert_eq!(cfg.api_url, "https://api.engram.dev");
    }

    #[test]
    fn test_new_empty_url() {
        let err = McpConfig::new("", "engram_abc123").unwrap_err();
        assert!(matches!(err, ConfigError::EmptyVar("ENGRAM_API_URL")));
        assert!(err.to_string().contains("ENGRAM_API_URL"));
    }

    #[test]
    fn test_new_empty_token() {
        let err = McpConfig::new("https://api.engram.dev", "").unwrap_err();
        assert!(matches!(err, ConfigError::EmptyVar("ENGRAM_API_TOKEN")));
        assert!(err.to_string().contains("ENGRAM_API_TOKEN"));
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
}
