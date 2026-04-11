Identify uncovered code paths in $ARGUMENTS (or the entire codebase if none specified) and suggest missing tests for this Rust MCP server.

## Coverage Focus Areas

### 1. Tool Handler Error Branches
For each tool in `server.rs` and `tools/*.rs`, verify tests exist for:
- Happy path (200/201 response)
- 401 Unauthorized
- 403 Forbidden / Permission Denied
- 404 Not Found
- 409 Conflict (for mutation tools)
- 402/429 Quota Exceeded (for creation tools)
- 500 Internal Server Error
- Network error (connection refused)
- Invalid UUID parameter

### 2. All `ApiError` Variants
In `error.rs`, verify `ApiError::from_response` is tested for:
- `Unauthorized` (401)
- `PermissionDenied` (403)
- `NotFound` (404)
- `Conflict` (409)
- `QuotaExceeded` with JSON body (402/429)
- `QuotaExceeded` with malformed body
- `BadRequest` (400)
- `Internal` (500+)
- `Network` (reqwest error)

### 3. Rich-Text Normalization Rules (R1–R5)
In `server.rs` / `normalize_card_content`, verify:
- **R1**: `text` empty → derived from spans (backward compat)
- **R2**: All spans are verbatim substrings (validated implicitly)
- **R3/R4**: Span concatenation mismatch → rich_text discarded
- **R5**: No spans → plain text returned as-is
- Control chars stripped (tab, carriage return, null byte)
- Emoji stripped from plain text
- CJK boundary marker stripped when isolated
- Symbol boundary marker (✈) stripped when only at boundaries
- CJK word (consecutive ideographs) preserved
- Single-char CJK span preserved

### 4. Config Validation
In `config.rs`:
- Empty `ENGRAM_API_URL` → `ConfigError::EmptyVar`
- Empty `ENGRAM_API_TOKEN` → `ConfigError::EmptyVar`
- Missing `ENGRAM_API_URL` → `ConfigError::MissingVar`
- Missing `ENGRAM_API_TOKEN` → `ConfigError::MissingVar`
- Trailing slash stripped from `api_url`
- Multiple trailing slashes stripped

### 5. Prompts (`prompts/mod.rs`)
- All four prompts returned by `list_all`
- Each prompt has a description
- `review_session` with custom limit
- `create_flashcard` with and without `catalog_id`
- `explain_card` includes face and back text
- `study_plan` with and without goal
- Unknown prompt returns `ErrorCode::INVALID_PARAMS`

### 6. Resources (`resources/mod.rs`)
- `list_all` returns six resources
- Each non-schema resource has `application/json` MIME type
- `engram://card-schema` returns text containing R4, dictionary, #27AE60
- `engram://catalogs` happy path
- `engram://learning/stats` (parallel fetch of count + total)
- `engram://learning/due` API error → `INTERNAL_ERROR`
- Unknown URI → `INVALID_PARAMS`

### 7. Update Card: Merge Logic (`server.rs`)
- `audio_id` preserved from existing card when LLM omits it
- `dictionary` preserved from existing card when LLM omits it
- Explicitly set `audio_id` not overwritten
- GET skipped when neither face nor back updated
- GET fails → error returned immediately (no PATCH attempted)
- Rich-text normalized before merge

## Output Format

For each gap found:
1. **Module** — file and function/section
2. **Missing scenario** — what is not tested
3. **Suggested test name** — `test_<function>_<scenario>`
4. **Test sketch** — short pseudocode or actual test code

Prioritise `Critical` gaps (error branches that could silently swallow bugs) over `Low` (cosmetic display).
