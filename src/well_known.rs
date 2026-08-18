//! `http` mode's OAuth 2.0 Protected Resource Metadata route (RFC 9728) — the
//! resource-server half of Track 3 Phase 3 (see
//! `engram-ws/tdds/track3-mcp-chatgpt-app.md`, "Phase 3 design"). engram-api
//! is the authorization server (DCR, `/oauth/authorize`, `/oauth/token`, and
//! `.well-known/oauth-authorization-server`); this route only tells a client
//! where to find it.
//!
//! Deliberately outside `bearer_auth_middleware` — per the MCP auth spec, a
//! client must be able to fetch this metadata *before* it has a token.

use axum::{Json, extract::State, response::IntoResponse};
use serde::Serialize;

/// The values needed to answer `.well-known/oauth-protected-resource`. Built
/// once at `http`-mode startup from `McpConfig` (`public_url` + `api_url`) and
/// held in axum `State` for the route.
#[derive(Debug, Clone)]
pub struct ProtectedResourceState {
    /// This MCP server's own canonical public URL, e.g. `https://mcp.engramo.app`.
    pub resource: String,
    /// The authorization server's issuer URL (engram-api's base URL).
    pub authorization_server: String,
}

#[derive(Serialize)]
struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
}

/// GET /.well-known/oauth-protected-resource
pub async fn protected_resource_metadata(
    State(state): State<ProtectedResourceState>,
) -> impl IntoResponse {
    Json(ProtectedResourceMetadata {
        resource: state.resource,
        authorization_servers: vec![state.authorization_server],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;

    #[tokio::test]
    async fn returns_resource_and_authorization_servers() {
        let state = ProtectedResourceState {
            resource: "https://mcp.engramo.app".to_string(),
            authorization_server: "https://api.engramo.app".to_string(),
        };

        let response = protected_resource_metadata(State(state))
            .await
            .into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["resource"], "https://mcp.engramo.app");
        assert_eq!(body["authorization_servers"][0], "https://api.engramo.app");
        assert_eq!(body["authorization_servers"].as_array().unwrap().len(), 1);
    }
}
