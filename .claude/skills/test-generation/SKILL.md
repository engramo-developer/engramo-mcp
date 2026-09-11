---
name: test-generation
description: Writes Rust tests for engramo-mcp using wiremock, tokio::test, and tower::ServiceExt, following the crate's in-file test module conventions. Used by code-implementator to close Coverage/Testing findings from the /review loop.
user-invocable: false
---

# Test Generation Skill — engramo-mcp

Write exactly the tests a finding requires, in the target file's existing `#[cfg(test)] mod tests`.

## Conventions

- **HTTP mocking:** `wiremock` (`MockServer`, `Mock::given`, `ResponseTemplate`). No live network.
- **Async:** `#[tokio::test]`.
- **Bodies:** `serde_json::json!`.
- **Server under test:** reuse the module's existing `make_server(base_url)` helper (`src/tools/catalogs.rs`,
  `src/tools/generate.rs`, `src/tools/ai.rs`) — do not write a new one.
- **Tool calls:** `server.<tool>(Parameters(<Params> { … })).await`.
- **axum / middleware:** build the router and drive it with `tower::ServiceExt::oneshot` — follow `src/http_auth.rs`
  tests.
- **Names:** `test_<function>_<scenario>`.

## The MCP contract every error test asserts

A tool handler must return `Ok` even on failure. Every error-path test therefore:

1. `.await.unwrap()`s the call — an `Err` here is itself the bug
2. asserts `result.is_error.unwrap_or(false)`
3. asserts on the message text, not just the flag

For invalid-input tests, also prove **no request was sent** (`.expect(0)` on the mock).

## Template

```rust
#[tokio::test]
async fn test_get_catalog_not_found_returns_error() {
    use rmcp::handler::server::wrapper::Parameters;
    const ID: &str = "00000000-0000-0000-0000-000000000001";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/catalogs/{ID}")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let result = make_server(&server.uri())
        .get_catalog(Parameters(GetCatalogParams {
            catalog_id: ID.into(),
        }))
        .await
        .unwrap(); // must NOT be Err — MCP contract

    assert!(result.is_error.unwrap_or(false), "expected is_error=true");
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.as_str())
        .unwrap_or("");
    assert!(text.contains("Not found"), "{text}");
}
```

Check the real params struct and tool method name before writing — the template's field names are illustrative.

## Required cases, when the finding asks for them

- **Status mapping:** 401 → mentions token/unauthorized · 403 → "Permission denied" · 404 → "Not found" ·
  409 → "Fetch the latest version" · 402/429 → "Quota exceeded" · 500 → `is_error`
- **Network:** drop the `MockServer` before the call
- **Rich text:** call `normalize_card_content` / `strip_span_boundary_markers` directly with the exact input from the
  finding, and assert the whole resulting struct
- **http auth:** a real `oneshot` request per header variant; session binding needs an `initialize` with token A and
  then a follow-up with token B

## After writing tests

The caller (`code-implementator`) runs the full gate. Do not report a test as written until it has passed there.
Unit tests using `unwrap()` inside `#[cfg(test)]` are fine — the no-`unwrap` rule applies to non-test code.

## Scope discipline

- One finding → the tests it names. No unrelated tests to inflate coverage.
- Reuse existing helpers and mock setup rather than duplicating them.
- Prefer `assert!(matches!(…))` for enum variants.
