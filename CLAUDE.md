# engram-mcp — Claude Code Guide

## Project Overview

Standalone Rust 2024 binary that exposes the Engram flashcard API to AI clients (Claude Desktop, Cursor) via the **Model Context Protocol (MCP)** over `stdio`.

**Stack:** Rust 2024 · rmcp 1.3 · reqwest 0.13 · Tokio · Tracing

---

## Architecture

```
src/
├── main.rs          — entry point: parse CLI, init tracing, wire server → stdio
├── lib.rs           — re-exports public modules
├── config.rs        — McpConfig: loads ENGRAM_API_URL + ENGRAM_API_TOKEN from env
├── client.rs        — EngramClient: typed HTTP client, attaches X-Api-Key header
├── error.rs         — ApiError enum: maps HTTP status → typed error
├── dto.rs           — lightweight DTOs mirroring API JSON shapes
├── server.rs        — EngramMcpServer: ServerHandler impl + all tool handlers
├── tools/
│   ├── mod.rs
│   ├── catalogs.rs  — ListCatalogsParams, GetCatalogParams, … + ok_json/err_result helpers
│   ├── cards.rs     — ListCardsParams, GetCardParams, UpdateCardParams, DeleteCardParams
│   ├── learning.rs  — DueCardsParams, AddCardToLearningParams, AddCatalogToLearningParams
│   ├── learning_paths.rs — ListLearningPathsParams, GetLearningPathParams, …
│   ├── search.rs    — SearchParams
│   ├── media.rs     — ListMediaParams
│   └── generate.rs  — GenerateCardParams, GenerateCatalogWithCardsParams, GenerateCardsParams
├── resources/
│   └── mod.rs       — MCP Resources: engram://catalogs, due, stats, learning-paths, subscription, card-schema
└── prompts/
    └── mod.rs       — MCP Prompts: review_session, create_flashcard, explain_card, study_plan
```

---

## MCP-Specific Conventions

### Tool Error Handling
- **Never** propagate `Err()` from a tool handler — this crashes the MCP client's tool-calling loop.
- All errors must be returned as `Ok(CallToolResult { is_error: true, content: [error_text] })`.
- Use `err_result(e)` (from `tools::catalogs`) to build the error result.
- Use `parse_uuid(s)` to parse UUID strings — returns `Result<Uuid, String>` for use with `err_result`.

### Rich-Text Span Validation (R1–R5)
Spans must satisfy all five rules or `rich_text` is discarded:
- **R1**: `text` is set to the full plain sentence (validation anchor)
- **R2**: Every `span.text` is a verbatim substring of `text`
- **R3**: Spans cover ALL of `text` — no gaps, no extra characters
- **R4**: Concatenation of all `span.text` values equals `text` exactly
- **R5**: If no styling is needed, omit `rich_text` entirely

`normalize_card_content` in `server.rs` enforces these rules and strips LLM-injected marker characters.

### HTTP Client
- All requests include `X-Api-Key: <token>` header.
- `EngramClient` methods return `Result<T, ApiError>` — callers convert errors to `err_result`.
- Config loaded from env vars:
  - `ENGRAM_API_URL` — base URL, e.g. `http://localhost:8080`
  - `ENGRAM_API_TOKEN` — user's API token

### Transport
- **stdio** (default and only mode): compatible with Claude Desktop and Cursor.
- Start with: `ENGRAM_API_URL=... ENGRAM_API_TOKEN=... cargo run`

---

## Mandatory After Every Code Change

Run in order — fix all issues before moving on:

```bash
cargo fmt --all
cargo check
cargo clippy
cargo test
```

If a `Cargo.toml` dependency was added or removed:
```bash
cargo sort
```

---

## Skills

| Command | Purpose |
|---|---|
| `/review` | Rust code review: idioms, performance, antipatterns, security |
| `/test` | Generate comprehensive tests targeting >90% coverage |
| `/security` | Security audit: token handling, input validation, panic safety |
| `/coverage` | Identify uncovered code paths and suggest missing tests |
