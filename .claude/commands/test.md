Generate comprehensive tests for $ARGUMENTS (or the most recently edited file if none specified) targeting >90% coverage in this Rust MCP server codebase.

## Testing Stack

- **`wiremock`** for HTTP mocking (no DB mocks needed — this is a pure HTTP client)
- **`#[tokio::test]`** for all async tests
- **`serde_json::json!`** for request/response bodies

## Test Categories

### 1. Happy Path
- Tool returns `Ok(CallToolResult)` with `is_error = false`
- Response JSON is well-formed and contains expected fields
- HTTP request includes correct `X-Api-Key` header
- Pagination params (limit, cursor) forwarded correctly

### 2. API Error Branches
Test each of these status codes for every relevant tool:
- **401 Unauthorized** → `is_error = true`, message contains "token" or "Unauthorized"
- **403 Forbidden** → `is_error = true`, message contains "Permission denied"
- **404 Not Found** → `is_error = true`, message contains "Not found"
- **409 Conflict** → `is_error = true`, message contains "Fetch the latest version"
- **402/429 Quota Exceeded** → `is_error = true`, message contains quota info
- **500 Internal** → `is_error = true`
- **Network error** → wiremock server shut down before request

### 3. Tool Parameter Validation
- Invalid UUID string → `is_error = true`, message contains "Invalid UUID"
- No HTTP request made when UUID parse fails
- Missing required params (handled by rmcp deserialization)

### 4. Rich-Text Normalization Edge Cases
Test `normalize_card_content` and `strip_span_boundary_markers` directly:
- Tab/CR/control chars stripped from plain text
- Emoji stripped from plain text and spans
- CJK boundary marker stripped when isolated (not part of CJK word)
- Symbol boundary marker (✈) stripped when only at boundaries
- Corrupted spans (mismatch with anchor text) → `rich_text` discarded
- Empty anchor → text derived from spans (backward-compatible path)
- Matching anchor → rich_text preserved

### 5. Update Card: Server-Managed Field Preservation
- `audio_id` copied from existing card when not set by LLM
- `dictionary` copied from existing card when not set by LLM
- Explicitly set `audio_id` not overwritten
- GET not called when neither `face` nor `back` is updated

## Test Naming Convention

```
test_<function>_<scenario>
```

Examples:
- `test_list_catalogs_success`
- `test_get_catalog_not_found`
- `test_generate_card_invalid_uuid`
- `test_normalize_corrupted_spans_discards_rich_text`

## Test Structure Template

```rust
#[tokio::test]
async fn test_list_catalogs_unauthorized_returns_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/catalogs"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let result = make_server(&server.uri())
        .list_catalogs(Parameters(ListCatalogsParams { limit: None, cursor: None }))
        .await
        .unwrap(); // must NOT return Err — MCP contract

    assert!(result.is_error.unwrap_or(false), "expected is_error=true");
    let text = result.content.first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.as_str())
        .unwrap_or("");
    assert!(text.contains("nauthorized") || text.contains("token"), "{text}");
}
```

## Mandatory Post-Generation Steps

After writing tests, run:

```bash
cargo fmt --all
cargo check
cargo clippy -- -D warnings
cargo test
```

Do not report as done until all tests pass and clippy is clean.
