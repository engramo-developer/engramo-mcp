# TDD: Expose catalog shadowing setup as an MCP tool (plus two directory-scan fixes)

**Status:** draft for review (2026-10-26). Nothing implemented yet.
**Branch (planned):** `feat/catalog-shadowing-tool`, cut from `main` at `6748a63`.
**Release:** not part of this work. The user cuts the release with `/release` later.

## 1. Goal

This branch also carries the two directory-scan fixes queued on 2026-10-18 (§5.4).


Let an AI client turn shadowing on or off for a catalog, and see which catalogs already have it. Today the
EngrAmo app and web client can do this; the MCP server can't.

Shadowing is the app's speaking practice. The learner hears a card's **face**, repeats it aloud, and
on-device or server speech-to-text scores the attempt against `face.text`. A catalog only shows up in the
app's Shadowing tab when it has a speaking language.

## 2. Decisions already made

| # | Decision | Source |
|---|---|---|
| D1 | Don't wait for the ChatGPT app review. It could take months, so the tool-definition freeze recorded on 2026-10-18 is lifted for this work. | User, 2026-10-26 |
| D2 | Ship a dedicated tool, not a new parameter on `update_catalog`. | Proposed 2026-10-26, user said go ahead |
| D3 | MCP-only change. engram-api already supports everything needed on `main`. | §3 |
| D4 | Orchestrated rollout: one fresh subagent per phase; the main session reviews diffs. | User, 2026-10-26 |
| D5 | Params are `enabled` + `language`, and turning shadowing on **requires** a language: without one the app can't pick a speech-recognition locale, so shadowing wouldn't work. Missing or blank `language` with `enabled: true` is a local error. | User, 2026-10-26 (§10 Q1) |
| D6 | `destructive_hint = true`, matching how `docs/directory-submission.md` classifies every overwrite (`update_catalog`, `update_card`, `update_learning_path`). Clients may ask for confirmation per call; accepted for a rarely used call. | User, 2026-10-26 (§10 Q2) |
| D7 | One branch: the shadowing tool plus the two directory-scan fixes queued on 2026-10-18 (§5.4). | User, 2026-10-26 (§10 Q3) |
| D8 | Open GitHub issues #74, #85 and #86 are unrelated to shadowing and stay out of this branch. | User, 2026-10-26 |

## 3. Backend facts (verified 2026-10-26 against engram-api `origin/main`)

engram-api's working tree is on `feat/extended-catalog-filters`, but every item below is also on `origin/main`
(checked with `git show origin/main:…`). Line numbers are from the working tree.

### 3.1 One column is the whole feature

- `catalogs.speaking_language text NULL` (migration `20260911090742_shadowing_practice_schema.up.sql`).
  NULL means "not a shadowing catalog". A non-null BCP-47 tag both enables shadowing and supplies the
  speech-recognizer locale. There is no separate `enabled` flag.
- No per-card setting. A card's `speakingEnabled` in learning responses is derived from its catalog
  (`db/src/repo/learning.rs:666`: `cat.archived = false AND cat.speaking_language IS NOT NULL`).
- Per the app's `engram_app/SHADOWING_TDD.md` §0.16 and §2, the locale comes from the catalog only, never
  from `card.face.tts.language`. So `language` must be the language of the **card faces**.

### 3.2 Write path: `PATCH /catalogs/{id}`

`speakingSettings` is three-state (`SpeakingSettingsUpdate`, `db/src/models/dtos.rs:101-125`):

| Body | Effect |
|---|---|
| `speakingSettings` key absent, or `{}` | no change |
| `{"speakingSettings": {"language": null}}` | turn off (column set to NULL) |
| `{"speakingSettings": {"language": "es-ES"}}` | turn on, or change the language |

- `version` is required (optimistic locking). Every other field is `COALESCE`d, so a PATCH carrying only
  `version` + `speakingSettings` touches nothing else (`db/src/repo/catalogs.rs:1046-1060`).
- Validation (`is_valid_bcp47_subset`, `db/src/repo/catalogs.rs:852`) is syntactic only:
  `^[a-z]{2,3}(-[A-Z]{2}|-[A-Z][a-z]{3})?$`. So `es`, `es-ES`, `pt-BR` and `sr-Latn` pass; `spanish`,
  `es_ES`, `ES-es` and `zh-Hans-CN` fail. A bad tag gets a 400 whose body names the expected shape:
  `invalid speakingSettings.language '<x>'; expected a BCP-47-like tag such as 'es', 'es-ES', or 'sr-Latn'`.
- Permissions (`db/src/repo/catalogs.rs:885-968`):
  - Only the catalog's **author**, or a caller holding `Action::CatalogShadowingPublish` (moderator or
    admin), may change `speakingSettings`. Editor-collaborators (`catalog_access.access_level = 'editor'`)
    can edit the rest of the catalog but get a 403 here.
  - A moderator gets full edit rights on any catalog. That's backend policy; the MCP doesn't change it.
- Response: the updated `CatalogDto`, which serializes `speakingSettings` as `{"language": "es-ES"}` or `{}`.

### 3.3 Read path

- `GET /catalogs/{id}` and `GET /catalogs` both return `speakingSettings` on each catalog (route tests
  `test_get_catalog_route_serializes_speaking_settings_with_language` and
  `test_get_catalog_route_serializes_empty_speaking_settings_when_null`).
- `GET /catalogs?speakingOnly=true` restricts the list to catalogs with a speaking language. A cursor from
  one `speakingOnly` value replayed with the other is a 400
  (`test_list_catalogs_rejects_cursor_from_a_different_speaking_only_with_400`).
- `GET /catalogs/{id}/permissions` reports `canSetShadowing`. This plan doesn't use it (§5.3, step 4).

### 3.4 Out of scope: practice history

`/shadowing/sessions` (POST) and `/shadowing/catalogs` (GET, DELETE) store which catalogs the learner
practiced on their device. An AI client has no good reason to record sessions, and clearing history is a
device-side concern. Not exposed.

## 4. Current MCP gaps

| Gap | Where |
|---|---|
| No way to send `speakingSettings` | `UpdateCatalogRequest`, `src/dto.rs:92` |
| `speakingSettings` dropped on read, so the AI can't tell which catalogs are set up | `CatalogDto`, `src/dto.rs:50` |
| `list_catalogs` can't pass `speakingOnly` | `ListCatalogsParams`, `src/tools/catalogs.rs:8`; `EngramoClient::list_catalogs`, `src/client.rs:303` |

Baseline before any change: **755 tests passing** (`cargo test`, 2026-10-26).

## 5. Design

### 5.1 Wire DTOs (`src/dto.rs`)

- New `SpeakingSettings { language: Option<String> }` (Serialize + Deserialize, `#[serde(default)]`).
  - Output: `language` skipped when `None`.
  - Helper `is_empty()` for skipping the whole object.
- `CatalogDto` gains
  `#[serde(rename = "speakingSettings", default, skip_serializing_if = "…is_empty")] speaking_settings: Option<SpeakingSettings>`
  (or a non-`Option` field with `Default`; implementer's choice, as long as the output rules hold):
  - Absent and `{}` deserialize fine.
  - A non-shadowing catalog's output has **no** `speakingSettings` key, so `list_catalogs` output doesn't grow
    for the common case.
  - A shadowing catalog's output has `"speakingSettings": {"language": "es-ES"}`. This matches the wire shape,
    the same way `cardCount` already keeps its wire name.
- `CatalogSummary` (resources) is unchanged.
- New `SpeakingSettingsUpdate { language: Option<String> }` for requests. `language` is **not**
  `skip_serializing_if`, so `None` serializes as `"language": null` (= turn off).
- `UpdateCatalogRequest` gains
  `#[serde(rename = "speakingSettings", skip_serializing_if = "Option::is_none")] speaking_settings: Option<SpeakingSettingsUpdate>`.
  Every existing constructor passes `None`, so `update_catalog`'s request body doesn't change.

### 5.2 Read side

- `ListCatalogsParams` gains `speaking_only: Option<bool>`. Schema description: "If true, return only catalogs
  with shadowing turned on. Keep the same value for every page: a cursor from one value is rejected with the
  other."
- `EngramoClient::list_catalogs(limit, cursor, speaking_only: bool)`: when `true`, push `("speakingOnly", "true")`.
  When `false`, send nothing, so the request is byte-identical to today's. Update the one other caller
  (`src/resources/mod.rs:298`, the `engramo://catalogs` resource) to pass `false`.
- Tool descriptions:
  - `list_catalogs`: today it claims it returns only "id, name, and card_count", which isn't true (it returns
    the full `CatalogDto`). Fix that sentence, and mention that shadowing catalogs carry `speakingSettings.language`.
  - `get_catalog`: add "and shadowing settings" to the list of details.

### 5.3 New tool: `set_catalog_shadowing`

Always on (main router in `src/server.rs`, next to `update_catalog`). Works in stdio and http. No paid-AI cost.

**Params** (`SetCatalogShadowingParams`, `src/tools/catalogs.rs`):

| Field | Type | Meaning |
|---|---|---|
| `catalog_id` | `String` (UUID) | The catalog |
| `enabled` | `bool`, required | `true` turns shadowing on (or changes the language); `false` turns it off |
| `language` | `Option<String>` | Required when `enabled` is true. Ignored when false. |

Why `enabled` plus `language` instead of a single nullable `language`: with one nullable field, a model that
leaves `language` out (or sends an empty string) would silently turn shadowing **off**. With an explicit
`enabled`, "on without a language" is a clear local error and "off" is always deliberate.

**Language normalization** (a pure helper, unit-tested). It only fixes case and separator; validation stays on
the backend so the rule can't drift.

1. Trim whitespace. Empty after trimming with `enabled: true` → local error, no HTTP call.
2. Split at the first `-` or `_` into a language part and an optional rest.
3. Lowercase the language part.
4. If the rest is 2 characters, uppercase it (`es_es` → `es-ES`). If it is 4, title-case it (`sr-latn` →
   `sr-Latn`). Otherwise leave it as-is and let the backend reject it.
5. Rejoin with `-`. ASCII only; anything non-ASCII passes through unchanged for the backend to reject.

**Handler flow:**

1. `parse_uuid(catalog_id)`. Invalid → `err_result`, no HTTP call.
2. Compute the target `Option<String>`: `None` when `enabled` is false, otherwise the normalized language.
   Return the local error from step 1 of normalization here.
3. `GET /catalogs/{id}` (`client.get_catalog`) to read `version` and the current `speakingSettings`.
   Errors (404 and so on) → `err_result`.
4. **No-op:** if the current language already equals the target, return the fetched catalog without a PATCH.
   This saves a write and a `version` bump. A permission problem won't surface on this path, which is fine:
   nothing changed.
5. `PATCH /catalogs/{id}` with exactly `{"version": v, "speakingSettings": {"language": <target or null>}}`.
6. On `ApiError::Conflict`, go back to step 3 **once**, then give up with the Conflict error. A retry is safe:
   the write touches one column and is idempotent, so it can't overwrite someone else's change to another
   field.
7. On `ApiError::PermissionDenied`, replace the message with: "Permission denied: only the catalog's owner can
   turn shadowing on or off or change its language (editors with shared access can't)". The GET in step 3
   already succeeded, so the catalog exists and is readable; the 403 means "not the owner".
8. Every other error → `err_result(e)`. The backend's 400 text for a bad tag passes through as-is.
9. Success → `ok_json(&catalog)` with the updated `CatalogDto`, including `speakingSettings`.

No `Err()` leaves the handler (CLAUDE.md tool-error contract).

**Annotations:** `read_only_hint = false`, `destructive_hint = true`, `idempotent_hint = true`,
`open_world_hint = false`.

- `destructive_hint = true`: it overwrites the previous setting, and turning shadowing off throws the language
  away. `docs/directory-submission.md` already counts every overwrite as destructive (`update_catalog`,
  `update_card`, `update_learning_path`), so this stays consistent with what the reviewers were told.
- `open_world_hint = false`: it never changes visibility. A public catalog is already visible to others; this
  only decides whether it appears in their Shadowing tab.

**Description (draft):**

> Turn shadowing on or off for a catalog, or change its language. Shadowing is EngrAmo's speaking practice: the
> learner hears each card's front (face), repeats it aloud, and speech recognition scores the attempt against the
> face text. Turning it on lists the catalog in the app's Shadowing tab. `language` is the language of the card
> fronts as a BCP-47 tag such as 'es', 'es-ES', 'pt-BR' or 'sr-Latn'; the app uses it as the speech-recognition
> locale. Only the catalog's owner can change this. You don't need the catalog version: the tool fetches it.
> Calling it with the current setting changes nothing.

Every top-level schema property must have a `type` (or `anyOf`). That's the Claude directory scan rule noted for
1.2.14; `String`, `bool` and `Option<String>` all produce one, and a test checks it (§6).

### 5.4 Also in this branch (D7)

**5.4a `generate_card` schema `type`.** The Claude directory's tool scan (2026-10-18) flagged `generate_card`'s
`face` and `back` (`src/tools/generate.rs:53`, `:62`; `CardContent`): the property is a bare `$ref` with no
`"type"` ("Add a type to these parameters"). Fix: `#[schemars(extend("type" = "object"))]` on those fields, and on
any other top-level property the new test finds. `update_card` already passes; its `anyOf` form is accepted.

Add **one test over every registered tool** (all routers: always-on, paid-AI, local TTS): each top-level input-schema
property has `type` or `anyOf`. This test also covers `set_catalog_shadowing` once Phase 3 adds it.

**5.4b `upload_media` description.** It ends with "For generated speech, use generate_card_audio instead if it's
available (stdio mode with your own TTS key)." That tool doesn't exist in http mode, so remote clients are pointed
at a tool they don't have. Drop the sentence. The server instructions already mention `generate_card_audio` when a
TTS key is configured (`src/server.rs:1167`). The text appears twice (`src/tools/media.rs:101`, `src/server.rs:1084`);
fix both, keeping them identical.

### 5.5 Unchanged

- `update_catalog` gets **no** shadowing parameter (D2), and its request body is unchanged
  (`speaking_settings: None`).
- No new resource or prompt. `engramo://catalogs` keeps `CatalogSummary`.
- No paid-AI router change, and no http-auth or session change.

## 6. Tests (acceptance criteria)

All tests use `wiremock`, in the in-file `#[cfg(test)] mod tests` of the module they cover. No network.

### Phase 1: directory-scan fixes (§5.4)

- All-tools schema test: every top-level property of every tool, across all three routers, has `type` or `anyOf`.
  It fails on `main` today (`generate_card` `face`/`back`) and passes after the fix.
- `upload_media` description no longer contains `generate_card_audio`, in both places.

### Phase 2: DTOs and read side

- `CatalogDto` deserializes `speakingSettings` as `{"language":"es-ES"}`, as `{}`, and when absent.
- `CatalogDto` serializes no `speakingSettings` key when there's no language, and
  `"speakingSettings":{"language":"es-ES"}` when there is one.
- `UpdateCatalogRequest` serialization:
  - `speaking_settings: None` → no `speakingSettings` key. Existing `update_catalog` body tests still pass
    unmodified.
  - `Some(SpeakingSettingsUpdate { language: None })` → `"speakingSettings":{"language":null}`, with the null
    present.
  - `Some(…Some("es-ES"))` → `"speakingSettings":{"language":"es-ES"}`.
- `list_catalogs`:
  - `speaking_only: Some(true)` → query `speakingOnly=true`.
  - `None` and `Some(false)` → no `speakingOnly` param (wiremock `query_param_is_missing`).
- `get_catalog` and `list_catalogs` tool output include `speakingSettings.language` for a shadowing catalog.

### Phase 3: `set_catalog_shadowing`

- Enable: GET returns `version: 3`, no language → PATCH body is **exactly**
  `{"version":3,"speakingSettings":{"language":"es-ES"}}` (`body_json`) → returns the updated catalog.
- Disable: GET returns `es-ES` → PATCH body is exactly `{"version":3,"speakingSettings":{"language":null}}`.
- Change language: `es-ES` → `pt-BR` sends a PATCH.
- No-op: GET already matches the target (for both on and off) → zero PATCH calls (`.expect(0)`), and the
  fetched catalog is returned.
- Normalization unit tests: `es_es`→`es-ES`, ` PT-br `→`pt-BR`, `sr-latn`→`sr-Latn`, `es`→`es`, and
  `spanish` unchanged (the backend rejects it).
- `enabled: true` with `language` missing, empty or whitespace → `is_error`, zero HTTP calls.
- `enabled: false` with a `language` given → the language is ignored, PATCH sends `null`.
- Invalid UUID → `is_error`, zero HTTP calls.
- First PATCH 409, second GET + PATCH 200 → success, with exactly 2 GETs and 2 PATCHes. Two 409s in a row →
  `is_error` Conflict, and no third attempt.
- PATCH 403 → `is_error` whose text contains the owner-only message from §5.3.
- PATCH 400 with the backend's BCP-47 message → `is_error` whose text contains that message.
- GET 404 → `is_error`, zero PATCH calls.
- The tool is registered in the always-on router, so it's present with `ENGRAMO_ENABLE_PAID_AI` off and in the
  http session server.
- Its annotations match §5.3.
- Schema `type` check: covered by the all-tools test from Phase 1 (§5.4a), no separate test.

Test count must only grow from the 755 baseline.

## 7. Docs to update (Phase 4)

| File | Change |
|---|---|
| `README.md` (tool table near line 488) | Add `set_catalog_shadowing`; mention `speaking_only` on `list_catalogs` |
| `docs/directory-submission.md` | Tool count 29 → 30 (line 114); add the tool to the read/write and destructive tables (lines 121, 143) |
| `CLAUDE.md` architecture tree | `tools/catalogs.rs` line: add `SetCatalogShadowingParams` |
| `docs/prompt-examples.md` | One example ("Turn on shadowing for my Spanish travel catalog") |

## 8. Phasing (orchestrated)

Phases 1–3 all touch `src/server.rs` (and 2–3 share `src/dto.rs` and `src/tools/catalogs.rs`), and the tests live
in those files, so they run **in sequence**. Phase 4 touches only docs but needs Phase 3's final description, so
it runs after.

| Phase | Scope | Files | Agent | Status |
|---|---|---|---|---|
| 0 | Cut `feat/catalog-shadowing-tool` from `main` | — | orchestrator | done 2026-10-26 |
| 1 | §5.4 directory-scan fixes + Phase 1 tests (lands the all-tools schema test first) | `src/tools/generate.rs`, `src/tools/media.rs`, `src/server.rs` | general-purpose, sonnet | done 2026-10-26: 757 passed / 0 failed (+2). Only `generate_card` `face`/`back` lacked `type`. `upload_media` text also lives in the unregistered `MediaTools` in `src/tools/media.rs`; both copies fixed. |
| 2 | §5.1 + §5.2 + Phase 2 tests | `src/dto.rs`, `src/client.rs`, `src/tools/catalogs.rs`, `src/server.rs`, `src/resources/mod.rs` (caller update only) | general-purpose, sonnet | done 2026-10-26: 770 passed / 0 failed (+13). `CatalogDto.speaking_settings: Option<SpeakingSettings>`, omitted unless a language is set. |
| 3 | §5.3 + Phase 3 tests | `src/tools/catalogs.rs`, `src/server.rs` | general-purpose, sonnet | done 2026-10-26: 784 passed / 0 failed (+14). Retry logic in private `apply_shadowing`; tool count 36 → 37 in `test_all_live_tools_have_title_and_annotations`. |
| 4 | §7 docs | `README.md`, `docs/directory-submission.md`, `CLAUDE.md`, `docs/prompt-examples.md` | general-purpose, sonnet | done 2026-10-26: prod tool count 29 → 30, `openWorldHint: false` 26 → 27. `docs/tdd-directory-listing.md` left as historical. |
| 5 | `/review` loop on the branch | as found | `/review` skill | done 2026-10-26: 6 findings (1 medium, 5 low), all fixed (64-byte `language` cap, retry/error-path tests); final pass 1 low fixed inline. 789 passed / 0 failed. |
| 6 | Commit, push (engramo SSH identity), open PR | — | orchestrator | done 2026-10-26 |

Every phase subagent runs the full gate itself, in one foreground call, before reporting:

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | tail -30
```

It reports the files changed, the test count against the baseline, and any deviation from this TDD. The
orchestrator reviews `git diff` (never transcripts), re-runs the gate after the last phase, and updates the
Status column above.

## 9. After merge (user-driven)

- The user runs `/release` (expected 1.2.14).
- After the release, re-scan tools from the Claude Connectors Directory listing (Edit → Tools) so the listing
  shows 30 tools.
- The open ChatGPT app review will see the new tool on its next scan. That was accepted under D1.

## 10. Open questions for review

1. ~~**`enabled` + `language` vs one nullable `language`.**~~ **Resolved (D5):** the pair, with `language`
   required to turn shadowing on.
2. ~~**`destructive_hint = true` or `false`.**~~ **Resolved (D6):** `true`.
3. ~~**Should the queued schema and description fixes join this branch?**~~ **Resolved (D7):** yes, and
   nothing else (§5.4, D8).
