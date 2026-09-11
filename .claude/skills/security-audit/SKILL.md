---
name: security-audit
description: Security audit checklist for engramo-mcp — API token and bearer handling, http-mode auth middleware and session binding, session-id secrecy, panic safety, input validation, body limits, TLS, unauthenticated routes, error leakage, dependencies. Used by the review-security agent inside the /review loop.
user-invocable: false
---

# Security Audit Skill — engramo-mcp

Apply this audit to the target. Be aggressive on Critical/High.

Threat model in one paragraph: in **stdio** mode one process holds one user's `ENGRAM_API_TOKEN`. In **http** mode
one process serves many users; each MCP session is bound to the bearer token that opened it, and a session id is
credential-equivalent — anyone holding it could act as that user if the binding check were missing. Tool arguments
come from an LLM and are untrusted input.

## Audit scope

### 1. Token handling (CRITICAL)
- [ ] `ENGRAM_API_TOKEN` and per-session bearer tokens never logged, including via `{:?}` — `McpConfig.api_token` stays
      `Option<Redacted<String>>`
- [ ] Tokens never echoed in `CallToolResult` content, resource/prompt output, or error messages
- [ ] stdio requires the token via `McpConfig::require_token()`; http mode never falls back to a global token
- [ ] Token forwarded upstream only as `X-Api-Key` to `ENGRAM_API_URL`, never to any other host

### 2. http-mode auth and sessions (CRITICAL)
- [ ] `bearer_auth_middleware` rejects missing, empty, and non-`Bearer` authorization with `401` **before** a session is
      created
- [ ] `CURRENT_BEARER_TOKEN` read with `try_with` inside the scoped task only; no path builds a client with no token
- [ ] `SessionTokens` checked on **every** post-`initialize` request; token comparison stays constant-time
      (`constant_time_eq`), with no early-return on length that leaks timing beyond what's documented
- [ ] Session → token entries removed when a session ends (no unbounded growth, no reuse of a dead session id)
- [ ] Session ids never logged — the `RUST_LOG` default silences rmcp's session manager, and
      `error_reporting::SENSITIVE_TERMS` keeps `session_id` (plus `token`, `secret`, `password`, `_key`)
- [ ] `RequestBodyLimitLayer` (16 MiB, `MAX_BODY_BYTES`) still wraps the rmcp `fallback_service`; axum's
      `DefaultBodyLimit` does not reach it

### 3. Unauthenticated surface
- [ ] `GET /version` returns only crate name + version
- [ ] `/.well-known/oauth-protected-resource` metadata (`src/well_known.rs`) exposes only public URLs, no secrets or
      internal hosts
- [ ] No other route is reachable without the bearer middleware

### 4. Panic safety
- [ ] No `panic!` / `unwrap()` / `expect()` / `unreachable!` / unchecked indexing or slicing in tool handlers, resources,
      prompts, or middleware — a panic kills the stdio server and aborts the http request task
- [ ] String slicing on user/LLM text uses char boundaries (rich-text and CJK handling are high-risk)

### 5. Input validation
- [ ] UUIDs via `parse_uuid()` — never interpolated raw into URL paths
- [ ] Untrusted strings placed into URLs are path-segment/query encoded, not `format!`ed into the path
- [ ] `upload_media` rejects invalid base64 and oversized payloads **before** any network call
- [ ] Pagination/limit params typed and bounded

### 6. Network and TLS
- [ ] No `danger_accept_invalid_certs(true)` on `reqwest::Client`
- [ ] Requests have timeouts; redirects cannot forward `X-Api-Key` to a different host

### 7. Error handling and leakage
- [ ] 401/402/403/429 upstream errors become `is_error: true` results, never panics or `Err`
- [ ] Error text does not leak upstream response bodies containing internals, stack traces, or tokens

### 8. Feature gating
- [ ] Paid-AI tools unreachable when `ENGRAM_ENABLE_PAID_AI` is off — not just hidden from `list_tools` but not callable

### 9. Dependencies (only when `Cargo.toml` / `Cargo.lock` is in scope)
- [ ] `cargo audit` clean (CI enforces `--deny warnings`)
- [ ] No new dependency that duplicates an existing one or widens the attack surface without need

## Severity

| Severity | Examples |
|---|---|
| **Critical** | Token or session id logged/echoed, session binding bypass, auth bypass, panic reachable from tool input |
| **High** | Missing body limit, paid-AI tool callable with flag off, token forwarded to another host |
| **Medium** | Over-broad error text, missing timeout, unbounded session map |
| **Low** | Hardening, defense-in-depth |

## Output contract

With a `feedback_path`, write findings in the shared format:

````markdown
### [ ] F<N> · <Severity> · Security
**Location:** `src/http_auth.rs:142`
**Issue:** <vulnerability + concrete risk>
**Fix:**
```rust
// concrete remediation
```
````

`<Category>` is always `Security`. Continue numbering from the highest `F<N>` in the file.

Without a `feedback_path`, produce findings as plain output and end with a severity table and the top 3 fixes.

## Scope discipline

- This skill owns tokens, auth, sessions, panics-as-DoS, input validation, leakage, TLS, and dependencies. Rust idioms
  go to `rust-code-review`; missing tests go to `coverage-analysis`.
- Never propose disabling a security check as a fix.
