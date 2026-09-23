# engramo-mcp — Claude Code Guide

## Project Overview

Standalone Rust 2024 binary that exposes the EngrAmo flashcard API to AI clients. Two transports, one binary:
- `engramo-mcp stdio` (default) — Claude Desktop, Cursor. One process = one user (`ENGRAM_API_TOKEN` env var).
- `engramo-mcp http` — Streamable HTTP at `/` (root) for remote clients (e.g. ChatGPT). Multi-user: each session
  authenticates with its own `Authorization: Bearer <token>`; there is no global token in this mode.

**Stack:** Rust 2024 · rmcp 3 · reqwest 0.13 · Tokio · Tracing

---

## Architecture

```
src/
├── main.rs          — CLI (Stdio/Http subcommands); http mode wires an axum Router (auth
│                       middleware + task-local bearer token + request body limit) around rmcp's
│                       StreamableHttpService, plus an unauthenticated `GET /version` route
├── version.rs       — server_version() (compile-time Cargo name/version) + the `GET /version`
│                       handler; reused by the always-on `get_server_version` MCP tool
├── lib.rs           — re-exports public modules
├── config.rs        — McpConfig: ENGRAM_API_URL (required), ENGRAM_API_TOKEN (optional — required
│                       only for stdio, see require_token()), ENGRAM_ENABLE_PAID_AI (default off)
├── client.rs        — EngramClient: typed HTTP client, attaches X-Api-Key header
├── error.rs         — ApiError enum: maps HTTP status → typed error
├── http_auth.rs     — http mode's auth edge: bearer extraction/validation, the CURRENT_BEARER_TOKEN
│                       task-local, and SessionTokens (binds each MCP session to the token that
│                       opened it — rmcp authorizes later requests on the session id alone)
├── dto.rs           — lightweight DTOs mirroring API JSON shapes
├── server.rs        — EngramMcpServer: ServerHandler impl + always-on tool handlers; `new()`
│                       conditionally sums in `Self::paid_ai_tools_router()` when the flag is on;
│                       `with_tts()` separately sums in the local-TTS router (see `tts/` below)
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
│   ├── ai.rs        — feature-flagged paid-AI tools (TTS, translate, dictionary, AI-agent chat,
│   │                   translate_batch_import) — own `#[tool_router(router = paid_ai_tools_router)]`
│   │                   impl block on `EngramMcpServer`, only registered when ENGRAM_ENABLE_PAID_AI is on
│   └── tts.rs       — local, bring-your-own-key TTS tools (list_tts_voices, generate_card_audio);
│                       own `#[tool_router(router = local_tts_tools_router)]` impl block, only
│                       registered by `EngramMcpServer::with_tts()` — stdio only, see `tts/` below
├── tts/             — engine-agnostic local TTS layer, **stdio-only** (see the Transport invariant
│                       below); never wired into `http` mode
│   ├── mod.rs       — TtsEngine trait, TtsConfig::from_env()/from_lookup(), env var names/defaults
│   ├── gemini.rs    — GeminiTts: key rotation across ENGRAM_TTS_GEMINI_API_KEYS, voice catalog,
│   │                   per-key error classification, upstream-text scrubbing
│   └── mp3.rs       — AudioEncoder seam + pcm16_mono_to_mp3 (statically linked LAME, LGPL-2.0 —
│                       see THIRD_PARTY_LICENSES)
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
  - `ENGRAM_TTS_GEMINI_API_KEYS` — one key or comma-separated list; **stdio only** (see `tts/mod.rs`,
    read only by `run_stdio`). Unset/blank → both local-TTS tools absent. The generic `GEMINI_API_KEY`
    is deliberately never read.
  - `ENGRAM_TTS_PROVIDER` — engine selector, default `gemini` (only value accepted today)
  - `ENGRAM_TTS_MODEL` — Gemini TTS model id, default `gemini-2.5-flash-preview-tts`
  - `ENGRAM_TTS_VOICE` — default voice for `generate_card_audio`, default `Puck`

### Transport
- **stdio** (default): `cargo run -- stdio` (or no subcommand). One process = one user, `EngramClient` built
  once from `ENGRAM_API_TOKEN`. Compatible with Claude Desktop and Cursor.
  Start with: `ENGRAM_API_URL=... ENGRAM_API_TOKEN=... cargo run`
- **http**: `cargo run -- http`. Serves rmcp's `StreamableHttpService` at `/` (root, via `fallback_service` —
  axum no longer allows `nest_service` at root) behind an axum `Router`.
  A `bearer_auth_middleware` (`http_auth.rs`) extracts `Authorization: Bearer <token>` and scopes it into a
  `tokio::task_local!` (`CURRENT_BEARER_TOKEN`) — the only way to reach the rmcp session factory, since
  `StreamableHttpService::new` takes a plain `Fn() -> Result<S, io::Error>` with no access to request
  headers. The factory runs synchronously inside `handle_post` while establishing a new session
  (`initialize` request), which is still within the task the middleware scoped, so `try_with` sees the
  value. Missing/empty bearer → `401` at the middleware, before a session is ever created. One
  `EngramClient` (and thus one EngrAmo account) per MCP session, for the session's lifetime.
- **Session binding (security):** rmcp authorizes every post-`initialize` request on the `Mcp-Session-Id`
  header alone, so `SessionTokens` records which token opened each session and 401s a request that presents a
  different one — otherwise a leaked session id would let anyone act as that session's owner. Requests are
  capped at 16 MiB (`RequestBodyLimitLayer`): rmcp buffers the whole body before parsing, and axum's
  `DefaultBodyLimit` does not reach a `fallback_service`.
- **Never log a session id.** It is credential-equivalent; the default `RUST_LOG` filter silences rmcp's
  session manager and `error_reporting` redacts `session_id` fields.
- **Local TTS is stdio-only (security):** `tts::from_env()` is called only from `run_stdio`;
  `build_session_server` has no TTS input; keys are `Redacted` and scrubbed from upstream error text.
  Never add a code path from http mode to a TTS engine.

---

## Git Workflow

- **Never commit directly to `main`.** `main` is push-protected; a direct commit/push will be rejected. Always
  create a feature/fix branch, push it, and open a PR — even for small fixes like a dependency bump.
- If a commit is accidentally made on `main`, move it off before doing anything else: `git branch <name>` to
  capture it, `git reset --hard origin/main` to restore `main`, then `git checkout <name>`.

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
| `/review [target] [--iterations N] [--continue]` | Agentic review→fix loop (below). Default target: `.rs` files changed vs `main` |
| `/release [vX.Y.Z]` | Bump the version (patch +1, or an explicit `vX.Y.Z`), verify, open the bump PR, then tag to publish |
| `/orchestration` | Orchestrator playbook: drive a multi-phase rollout by spawning one subagent per phase |

### Review loop

`/review` fans out parallel Sonnet reviewers, merges their findings with an Opus synthesis step, fixes them, and
re-reviews for regressions. Security, coverage, and test writing all run inside this loop — there are no separate
commands for them.

| Agent (`.claude/agents/`) | Applies skill (`.claude/skills/`) |
|---|---|
| `review-rust` | `rust-code-review` — idioms, MCP tool-handler contract, rich text, HTTP client |
| `review-security` | `security-audit` — tokens, http auth + session binding, panics, leakage |
| `review-coverage` | `coverage-analysis` — untested branches (skipped on the final pass) |
| `synthesis-reviewer` | — dedupes and prioritizes into one feedback file |
| `code-implementator` | `test-generation` — applies fixes, writes tests, runs the full gate |

Feedback files land in `.claude/.review-cache/` (gitignored). Nothing is committed by the loop.
