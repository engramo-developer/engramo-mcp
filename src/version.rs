//! Build-time identity of this MCP server binary. Shared by the `get_server_version`
//! MCP tool (both transports) and `http` mode's unauthenticated `GET /version` route,
//! so the two can never disagree.

use axum::{Json, response::IntoResponse};
use serde::Serialize;

/// This server's own name and build/release version, from Cargo's compile-time env
/// (`CARGO_PKG_NAME` / `CARGO_PKG_VERSION`). NOT a catalog's or card's optimistic-locking
/// `version` field.
#[derive(Debug, Clone, Serialize)]
pub struct ServerVersionInfo {
    pub name: &'static str,
    pub version: &'static str,
}

/// The one function both the MCP tool and the HTTP route call, so the version they
/// report can never drift apart.
pub fn server_version() -> ServerVersionInfo {
    ServerVersionInfo {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
    }
}

/// GET /version — unauthenticated: it exposes only the public build version, nothing
/// user- or token-scoped (same rationale as the `.well-known` route living outside
/// `bearer_auth_middleware`).
pub async fn version_endpoint() -> impl IntoResponse {
    Json(server_version())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_version_reports_crate_name_and_version() {
        let info = server_version();
        assert_eq!(info.name, env!("CARGO_PKG_NAME"));
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.version.is_empty());
    }

    #[tokio::test]
    async fn version_endpoint_returns_200_with_name_and_version() {
        let response = version_endpoint().await.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["name"], env!("CARGO_PKG_NAME"));
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    }
}
