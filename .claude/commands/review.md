Review the code in $ARGUMENTS (or the most recently edited file if none specified) for quality, correctness, and production-readiness in this Rust MCP server codebase.

## Checklist

### Rust Idioms
- [ ] No `unwrap()` / `expect()` in library code — only in `main.rs` startup
- [ ] No `clone()` calls that could be avoided with references or `Arc`
- [ ] Use `?` operator instead of manual `match` on `Result`/`Option` where appropriate
- [ ] Prefer `if let` / `while let` over explicit `match` for single-arm patterns
- [ ] Iterators preferred over manual `for` loops with `push`
- [ ] No unnecessary `collect()` before immediately iterating
- [ ] Error types use `thiserror` — no manual `Display` impls when `thiserror` suffices
- [ ] `#[derive(Debug, Clone)]` only where semantically correct
- [ ] Prefer `From`/`Into` impls over explicit conversion functions

### MCP Tool Handler Rules (CRITICAL)
- [ ] No `?` that propagates out of a tool handler fn — all errors → `Ok(CallToolResult { is_error: true })`
- [ ] Every error branch calls `err_result(e)` not `return Err(...)`
- [ ] No `panic!` / `unwrap()` inside any tool handler
- [ ] `#[tool_router]` macro applied correctly; `ServerHandler` impl is complete
- [ ] `#[tool(description = "...")]` present on every `#[tool_router]` method
- [ ] `schemars::JsonSchema` derived on every tool params struct

### Rich-Text Validation
- [ ] `normalize_card_content` called before any card creation or update
- [ ] Span concatenation must equal `text` (rules R1–R5 enforced)
- [ ] `strip_span_boundary_markers` handles LLM-injected CJK/symbol markers

### HTTP Client & Error Mapping
- [ ] All HTTP errors mapped to `ApiError` variants before reaching tool handlers
- [ ] `ApiError::from_response` used consistently — not manual status checks
- [ ] Network errors wrapped in `ApiError::Network`

### Security
- [ ] `ENGRAM_API_TOKEN` never logged — only structured `tracing` fields
- [ ] Token not echoed in tool outputs or error messages
- [ ] Input UUIDs parsed with `parse_uuid(s)` — not used raw in HTTP paths
- [ ] `reqwest` TLS not disabled

### Antipatterns to Flag
- [ ] Blocking calls inside `async fn` (file I/O, `std::thread::sleep`)
- [ ] `Arc<Mutex<T>>` where `tokio::sync::RwLock` is better
- [ ] Magic numbers / hardcoded strings that belong in config

### Testing
- [ ] Every tool handler has tests for happy path AND all error branches
- [ ] Error scenarios use `wiremock` for HTTP mocking
- [ ] `#[tokio::test]` on all async tests
- [ ] Test names follow `test_<function>_<scenario>` convention
- [ ] No live HTTP calls in tests — all mocked via `wiremock`

## Output Format

For each issue found:
1. **File:Line** — exact location
2. **Severity** — `Critical` / `High` / `Medium` / `Low`
3. **Category** — (Security | MCP | Idiom | Antipattern | Testing)
4. **Finding** — what the problem is
5. **Fix** — the idiomatic Rust solution with a code snippet

End with a summary table of findings by severity.

## Mandatory Post-Change Steps

After applying **every** fix, run these commands in order and resolve all reported issues before finishing:

```bash
cargo fmt --all
cargo check
cargo clippy -- -D warnings
cargo test
```

If any `Cargo.toml` dependency was added or removed:
```bash
cargo sort
```

Do not report the module as done until all commands exit cleanly and all tests pass.
