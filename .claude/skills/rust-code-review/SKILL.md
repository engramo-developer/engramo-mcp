---
name: rust-code-review
description: Rust + MCP code review checklist for engramo-mcp (rmcp 3, reqwest, axum, Tokio). Covers Rust idioms, MCP tool-handler error contract, tool router registration, rich-text R1–R5 rules, HTTP client and ApiError mapping, antipatterns, and test quality. Used by the review-rust agent inside the /review loop.
user-invocable: false
---

# Rust Code Review Skill — engramo-mcp

Apply this checklist to the target. Surface real issues, not stylistic noise. Security issues belong to
`security-audit` and missing tests to `coverage-analysis` — do not duplicate them here.

## Checklist

### MCP tool handlers (CRITICAL)
- [ ] No `?` or `return Err(...)` out of a tool handler — every error becomes `Ok(CallToolResult { is_error: true })`
      via `err_result(e)` (`src/tools/catalogs.rs`). A propagated `Err` crashes the client's tool-calling loop.
- [ ] No `panic!` / `unwrap()` / `expect()` / `unreachable!` reachable from a tool handler
- [ ] UUID arguments parsed with `parse_uuid(s)` before any HTTP call
- [ ] Success payloads built with `ok_json(&value)`, not hand-assembled JSON strings
- [ ] Every `#[tool_router]` method has `#[tool(description = "...")]`; descriptions tell the model when to use it
- [ ] Every params struct derives `schemars::JsonSchema` + `Deserialize`, with doc comments on fields
- [ ] Paid-AI tools live only in the `#[tool_router(router = paid_ai_tools_router)]` block in `src/tools/ai.rs` and are
      summed in by `EngramMcpServer::new()` only when `ENGRAM_ENABLE_PAID_AI` is on — never registered unconditionally
- [ ] Bring-your-own-AI tools (`src/tools/generate.rs`) never call a paid server-side AI endpoint

### Rich text (R1–R5)
- [ ] `normalize_card_content` (`src/server.rs`) runs before every card create **and** update
- [ ] Changes to normalization keep R1–R5: `text` is the anchor; spans are verbatim substrings; spans cover all of
      `text`; concatenation equals `text`; no `rich_text` when no styling is needed
- [ ] `strip_span_boundary_markers` still strips isolated LLM-injected CJK/symbol markers without eating real CJK words
- [ ] Update-card merge keeps server-managed fields (`audio_id`, `dictionary`) when the model omits them

### HTTP client and errors
- [ ] All API calls go through `EngramClient` and carry the `X-Api-Key` header
- [ ] Non-2xx responses mapped with `ApiError::from_response`, not ad-hoc status checks
- [ ] Transport failures wrapped as `ApiError::Network`
- [ ] Error text shown to the model is actionable (e.g. 409 tells it to fetch the latest version)

### Rust idioms
- [ ] `?` instead of manual `match` on `Result`/`Option` (outside tool handlers)
- [ ] `if let` / `let … else` over single-arm `match`
- [ ] Iterators over manual `for` + `push`; no `collect()` immediately re-iterated
- [ ] No avoidable `clone()`; shared state behind `Arc`
- [ ] Error types use `thiserror`; `From` impls over ad-hoc conversion functions
- [ ] Derives are semantically correct — no `Debug` on a type holding a raw secret unless it uses `Redacted`

### Antipatterns
- [ ] Blocking calls in `async fn` (`std::fs`, `std::thread::sleep`)
- [ ] Locks held across `.await`; `std::sync::Mutex` where contention crosses awaits
- [ ] Sequential awaits on independent API calls that could be joined
- [ ] Magic numbers / URLs that belong in `McpConfig` or a named `const`
- [ ] `println!` / `eprintln!` instead of `tracing` — stdout is the stdio transport; stray output corrupts the protocol

### Test quality (existing tests only)
- [ ] HTTP mocked with `wiremock`; no live network calls
- [ ] `#[tokio::test]` on async tests
- [ ] Names follow `test_<function>_<scenario>`
- [ ] Error-path tests assert `is_error == Some(true)` **and** the message, not just "didn't panic"

## Output contract

With a `feedback_path`, write findings in exactly this format. The first line of every finding must be
`### [ ] F<N> · <Severity> · <Category>` — the checkbox is parsed downstream.

````markdown
# Code Review — Iteration <N>

**Target:** <path or scope>
**Date:** <YYYY-MM-DD>

## Summary

- Total findings: <N>
- Critical: <X> | High: <Y> | Medium: <Z> | Low: <W>

## Findings

### [ ] F1 · Critical · MCP
**Location:** `src/server.rs:115`
**Issue:** `parse_uuid(&p.card_id).map_err(|e| ErrorData::invalid_params(e, None))?` propagates `Err` out of the tool handler — the client's tool loop aborts instead of showing the model an error it can recover from.
**Fix:**
```rust
let id = match parse_uuid(&p.card_id) {
    Ok(id) => id,
    Err(e) => return Ok(err_result(e)),
};
```
````

Categories: `MCP` · `RichText` · `HttpClient` · `Idiom` · `Antipattern` · `Performance` · `Testing`.
Severity: `Critical` (protocol crash, data loss) · `High` (correctness bug) · `Medium` (maintainability) · `Low` (polish).

Without a `feedback_path`, produce the same content as plain output and end with a summary table by severity.

## Scope discipline

- Read target files fully before flagging — partial reads produce false positives.
- Do not flag style that matches this codebase's established conventions (see `CLAUDE.md`).
- Do not propose refactors beyond what each finding requires.
