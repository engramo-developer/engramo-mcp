---
name: coverage-analysis
description: Test coverage gap analysis for engramo-mcp. Maps functions and branches in the target to existing tests and flags uncovered paths — tool-handler error branches, ApiError mappings, rich-text normalization, config, http auth/session binding, resources, prompts. Used by the review-coverage agent inside the /review loop; gaps are fixed with the test-generation skill.
user-invocable: false
---

# Coverage Analysis Skill — engramo-mcp

Surface untested code paths as findings the implementer can close with the `test-generation` skill. Analysis is
static: `cargo-llvm-cov` is not installed. Enumerate branches, then look for tests that exercise them. Unit tests
sit in each file's `#[cfg(test)] mod tests`.

## Enumerate testable units in the target

- Every `pub` / `pub(crate)` fn and every `#[tool]` method
- Every `match` arm, `if`/`else` branch, early `return`, and `?` point
- Every `ApiError` / `ConfigError` variant a function can produce
- Every `From` / `TryFrom` impl

## Always-check gaps, by area

### Tool handlers (`src/server.rs`, `src/tools/*.rs`)
For each tool, is there a test for:
- happy path, with `is_error` false and the expected JSON fields
- 401, 403, 404, 409 (mutations), 402/429 quota (creation tools), 500
- network failure (mock server dropped)
- invalid UUID, returning `is_error` **with no HTTP request made**

### `ApiError::from_response` (`src/error.rs`)
- `Unauthorized`, `PermissionDenied`, `NotFound`, `Conflict`, `BadRequest`, `Internal`
- `QuotaExceeded` with a JSON body **and** with a malformed body
- `Network`

### Rich text (`normalize_card_content`, `strip_span_boundary_markers` in `src/server.rs`)
- empty anchor → `text` derived from spans
- span concatenation mismatch → `rich_text` discarded
- no spans → plain text untouched
- control chars (tab, CR, NUL) and emoji stripped
- isolated CJK / symbol boundary markers stripped; real CJK words and single-char CJK spans preserved

### Update-card merge
- `audio_id` / `dictionary` preserved from the existing card when omitted; explicit `audio_id` not overwritten
- GET skipped when neither face nor back changes; GET failure → error, no PATCH

### Config (`src/config.rs`)
- missing / empty `ENGRAM_API_URL` → `MissingVar` / `EmptyVar`
- `ENGRAM_API_TOKEN` is optional at load: empty → `EmptyVar`; absent → `require_token()` returns `MissingVar`
- trailing slash(es) stripped from the URL
- `ENGRAM_ENABLE_PAID_AI` truthy values (`true`/`1`/`yes`/`on`) vs default off

### http mode (`src/http_auth.rs`, `src/main.rs`)
- missing, empty, and non-`Bearer` authorization → 401; oversized bearer rejected
- session opened with token A, then a request with token B → 401
- task-local token visible to the session factory
- body over `MAX_BODY_BYTES` rejected

### Server surface
- paid-AI tools absent from the router when the flag is off, present when on
- resources: every URI in `list_all` readable; API error → `INTERNAL_ERROR`; unknown URI → `INVALID_PARAMS`
- prompts: `list_all` count matches the implementation; each has a description; argument variants; unknown prompt →
  `INVALID_PARAMS`
- `GET /version` and the `get_server_version` tool report the Cargo name/version
- `error_reporting`: sensitive fields redacted

## Prioritize

1. Security-critical (auth, session binding, redaction, paid-AI gating)
2. Error branches that could hide a bug (a tool returning `Err`, a swallowed status)
3. Business logic (rich text, merge)
4. Happy paths still missing

## Output contract

With a `feedback_path`, write one finding per missing test:

````markdown
### [ ] F<N> · <Severity> · Coverage
**Location:** `src/tools/cards.rs:delete_card`
**Issue:** No test exercises the 403 branch — a permission error could regress to a propagated `Err` unnoticed.
**Fix:**
```rust
#[tokio::test]
async fn test_delete_card_forbidden_returns_error() {
    // wiremock DELETE /cards/{id} → 403; assert is_error == Some(true) and text contains "Permission denied"
}
```
**Apply with:** the `test-generation` skill
````

Severity: `High` for an uncovered security-critical branch · `Medium` for an uncovered error or logic branch ·
`Low` for a happy path covered only transitively. Continue numbering from the highest `F<N>` in the file.

## Scope discipline

- Flag only **missing** coverage; test quality belongs to `rust-code-review`.
- Do not invent edge cases to pad the count when a function is already exhaustively covered.
- Each finding must be implementable by `test-generation` without further design.
