use rmcp::schemars;
use serde::Deserialize;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Search query string")]
    pub query: String,
}
