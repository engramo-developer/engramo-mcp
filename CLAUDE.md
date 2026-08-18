# engramo-mcp — Claude Code Guide

## Project Overview

Standalone Rust 2024 binary that exposes the EngrAmo flashcard API to AI clients. Two transports, one binary:
- `engramo-mcp stdio` (default) — Claude Desktop, Cursor. One process = one user (`ENGRAM_API_TOKEN` env var).
- `engramo-mcp http` — Streamable HTTP at `/` (root) for remote clients (e.g. ChatGPT). Multi-user: each session
  authenticates with its own `Authorization: Bearer <token>`; there is no global token in this mode.

**Stack:** Rust 2024 · rmcp 1.3 · reqwest 0.13 · Tokio · Tracing

---

## Architecture

```
src/
├── main.rs          — CLI (Stdio/Http subcommands); http mode wires an axum Router (auth
│                       middleware + task-local bearer token) around rmcp's StreamableHttpService
├── lib.rs           — re-exports public modules
├── config.rs        — McpConfig: ENGRAM_API_URL (required), ENGRAM_API_TOKEN (optional — required
│                       only for stdio, see require_token()), ENGRAM_ENABLE_PAID_AI (default off)
├── client.rs        — EngramClient: typed HTTP client, attaches X-Api-Key header
├── error.rs         — ApiError enum: maps HTTP status → typed error
├── dto.rs           — lightweight DTOs mirroring API JSON shapes
├── server.rs        — EngramMcpServer: ServerHandler impl + always-on tool handlers; `new()`
│                       conditionally sums in `Self::paid_ai_tools_router()` when the flag is on
├── tools/
│   ├── mod.rs
│   ├── catalogs.rs  — ListCatalogsParams, GetCatalogParams, … + ok_json/err_result helpers
│   ├── cards.rs     — ListCardsParams, GetCardParams, UpdateCardParams, DeleteCardParams
│   ├── learning.rs  — DueCardsParams, AddCardToLearningParams, AddCatalogToLearningParams
│   ├── learning_paths.rs — ListLearningPathsParams, GetLearningPathParams, …
│   ├── search.rs    — SearchParams
│   ├── media.rs     — ListMediaParams
│   ├── generate.rs  — GenerateCardParams, GenerateCatalogWithCardsParams, GenerateCardsParams
│   │                   (bring-your-own-AI — always on, no server-side generation cost)
│   └── ai.rs        — feature-flagged paid-AI tools (TTS, translate, dictionary, AI-agent chat,
│                       translate_batch_import) — own `#[tool_router(router = paid_ai_tools_router)]`
│                       impl block on `EngramMcpServer`, only registered when ENGRAM_ENABLE_PAID_AI is on
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
  - `ENGRAM_API_TOKEN` — user's API token (required for `stdio`; unused in `http` mode)
  - `ENGRAM_ENABLE_PAID_AI` — `true`/`1`/`yes`/`on` to register the paid-AI tools (default off)
  - `MCP_BIND_ADDR` — bind address for `http` mode (default `0.0.0.0:8080`)

### Transport
- **stdio** (default): `cargo run -- stdio` (or no subcommand). One process = one user, `EngramClient` built
  once from `ENGRAM_API_TOKEN`. Compatible with Claude Desktop and Cursor.
  Start with: `ENGRAM_API_URL=... ENGRAM_API_TOKEN=... cargo run`
- **http**: `cargo run -- http`. Serves rmcp's `StreamableHttpService` at `/` (root, via `fallback_service` —
  axum no longer allows `nest_service` at root) behind an axum `Router`.
  A `bearer_auth_middleware` extracts `Authorization: Bearer <token>` and scopes it into a
  `tokio::task_local!` (`CURRENT_BEARER_TOKEN`) — the only way to reach the rmcp session factory, since
  `StreamableHttpService::new` takes a plain `Fn() -> Result<S, io::Error>` with no access to request
  headers. The factory runs synchronously inside `handle_post` while establishing a new session
  (`initialize` request), which is still within the task the middleware scoped, so `try_with` sees the
  value. Missing/empty bearer → `401` at the middleware, before a session is ever created. One
  `EngramClient` (and thus one EngrAmo account) per MCP session, for the session's lifetime.

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
