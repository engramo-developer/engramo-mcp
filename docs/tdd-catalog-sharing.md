# TDD: Share a catalog with other users via MCP

**Status:** approved 2026-10-26 (§10). Phase 1a implemented; see the progress table in §8.
**Branch (planned):** `feat/catalog-sharing-tools`, cut from `main` at `14c1da2`.
**Release:** not part of this work. The user cuts the release with `/release` later.

## 1. Goal

Let an AI client share one of the user's catalogs with other EngrAmo users by their user ids, change or revoke that
access, and see who a catalog is shared with. The web app can do all of this today (share dialog,
`engram-web-app/src/app/features/dashboard/ui/right-panel/components/share-dialog/`). The MCP server can't do any of it.

## 2. Decisions already made

| # | Decision | Source |
|---|---|---|
| D1 | The tool freeze recorded on 2026-10-18 was lifted on 2026-10-26, so new tools may be added. The listing has to be re-scanned after release. | Memory, 2026-10-26 |
| D2 | MCP-only change. engram-api `main` already has everything needed (§3). | §3 |
| D3 | Tools go in the always-on catalog router. Sharing costs no paid AI, and it works in both stdio and http. | Task brief |
| D4 | Orchestrated rollout: one fresh subagent per phase, and the main session reviews diffs. | `docs/tdd-catalog-shadowing.md` §8 precedent |

## 3. Backend facts (verified 2026-10-26)

I read engram-api's working tree, which is on `feat/extended-catalog-filters`. The sharing code there is identical to
`main`: `diff` against `git show main:…` matched for both `repo::catalogs::share` and the `share` route handler, and
`main` carries `ea4d1f6 feat(sharing): allow catalog sharing on every plan, metered by quota (#27)`. Line numbers below
are from the working tree.

### 3.1 Endpoints

There are only two, both on one path (`app/src/routes/mod.rs:196-199`):

| Method + path | Handler | Who may call |
|---|---|---|
| `GET /catalogs/{catalog_id}/collaborators` | `catalogs::list_collaborators` (`app/src/routes/catalogs.rs:1084-1112`) | The author **or an editor** (`allow_edit`, `db/src/repo/catalogs.rs:1333-1342`) |
| `POST /catalogs/{catalog_id}/collaborators` | `catalogs::share` (`app/src/routes/catalogs.rs:1114-1196`) | The **author only** (`allow_delete`, `db/src/repo/catalogs.rs:1365-1374`; the comment says this prevents an editor from adding editors) |

There is **no separate revoke endpoint**. Revoking is the same POST with `accessLevel: null`. There is also no DELETE
route for a collaborator.

Auth works like every other protected route (`AuthUser`). The MCP sends `X-Api-Key`.

### 3.2 Wire shapes

`Collaborator` (`db/src/models/dtos.rs:702-709`) and `ShareRequest` (`:711-714`):

```json
// POST body
{ "changes": [ { "userId": "<uuid>", "accessLevel": "viewer" | "editor" | null } ] }
// GET 200 body
[ { "userId": "<uuid>", "accessLevel": "viewer" | "editor" } ]
// POST 200 body
{ "id": "<catalog uuid>", "shared": "successfully" }
```

- `AccessLevel` is lowercase `viewer` | `editor` (`db/src/models/shared.rs:66-71`).
- `accessLevel` set means upsert (`INSERT … ON CONFLICT DO UPDATE`), so the same call also **changes** an existing
  collaborator's level. `null` means remove, and removing someone who isn't a collaborator does nothing
  (`db/src/repo/catalogs.rs:1477-1529`).
- `changes` is a `HashSet<Collaborator>` that hashes on `(userId, accessLevel)`. The repo then rejects two changes for
  the same user with **400** `multiple changes for user <id> in one request` (`:1383-1397`).
- An empty `changes` returns 200 and does nothing (route test `test_share_entitled_user_reaches_repo_and_returns_200`).
- The whole request runs in **one transaction** (`:1400`). One bad id rolls back every change.
- The GET returns ids and levels only: no emails or names. The author isn't in the list.

### 3.3 User ids

- They're UUIDs. The FK `catalog_access_user_id_fkey` turns an unknown id into **404** with body
  `{"error":"Not found: user <uuid>"}` (`:1497-1501`). Since everything is rolled back, nothing is shared.
- Sharing with yourself (the author) returns **400** `cannot share a catalog with its own author` (`:1384-1389`).
- **There's no way to look up a user id from an email.** `GET /public-api/users/{email}` (`app/src/routes/public_api.rs`)
  is unauthenticated and returns only `true`/`false`, and the web app uses it for sign-up. The web share dialog asks
  for a raw id (label "Add User ID (UUID)", `share-dialog.component.html:13`). I found no place in the web or Flutter
  app that shows users their own id, and `engram_app` has no sharing UI at all.
- The one way the MCP can reveal an id is `get_subscription_usage`, which returns the caller's **own** `userId`
  (`src/subscription.rs:74`, from `GET /me`). So the recipient asks their own AI client "what's my EngrAmo user id?"
  and sends it to the owner. The tool descriptions must say this (§5.2).

### 3.4 Errors

| Status | When | Body | MCP mapping today (`src/error.rs:257-330`) |
|---|---|---|---|
| 400 | Sharing with the author; duplicate user in one request | `{"error":"<msg>"}` | `BadRequest`, message passed through |
| 403 | A non-author POSTs, or a viewer or stranger GETs | `{"error":"Forbidden"}` (ids stripped, `app/src/routes/mod.rs:754-763`) | `PermissionDenied` with the generic `forbidden_hint` text, which doesn't help here |
| 404 | The catalog is missing or archived (`get_catalog_permissions`, `db/src/repo/catalogs.rs:556-563`), or a user id is unknown | `{"error":"Not found: Catalog with id … not found"}` / `"Not found: user <uuid>"` | `NotFound`, message passed through after removing the duplicate prefix |
| 409 | Listed in the OpenAPI annotations, but `share` takes no version and the repo never raises it | — | `Conflict` |
| 429 | Over a share limit (§3.5) | `{"error":"quota_exceeded","code":"QUOTA_EXCEEDED","resource_type":"shared_catalogs"\|"collaborators_per_catalog","used","limit","requested","plan_id","window":"absolute"}` (`app/src/routes/catalogs.rs:1161-1186`) | `QuotaExceeded`. The message works, but neither resource has a label or hint (§5.4) |

### 3.5 Plan limits and entitlement

- `PlanFeature::CatalogShare` is entitled on **every** plan (`db/src/models/quota.rs:73`, `:528-552`). The share handler
  doesn't call `require_entitlement`, so `FEATURE_NOT_ENTITLED` can't come back from these routes today.
  `feature_label("catalog_share")` already exists (`src/subscription.rs:31`) in case it ever does.
- There are two absolute (lifetime, non-resetting) quotas, set by migration `20261026120000_add_catalog_share_quotas.up.sql`:

  | Plan | `shared_catalogs` | `collaborators_per_catalog` |
  |---|---|---|
  | free | 1 | 5 |
  | basic | unlimited (-1) | 20 |
  | professional | unlimited | 100 |
  | payg | unlimited | unlimited |

  - **`collaborators_per_catalog`** is checked only when the request adds someone and the resulting set is over the limit.
    The 429 has `used` = current count and `requested` = number of newly added users (`db/src/repo/catalogs.rs:1429-1442`).
  - **`shared_catalogs`** is checked only when a catalog goes from 0 to more than 0 collaborators. `used` = how many of the
    author's other non-archived catalogs are shared, and `requested` = 1 (`:1445-1466`).
  - A missing or malformed limit key means **deny** (`ShareLimits::from_plan_limits`, `db/src/models/quota.rs:203-218`).
- `GET /me/usage` already reports both quotas (`service/src/quota_service.rs:1197-1227`; also on `main`).
  `collaborators_per_catalog` carries `scope_id` = the catalog with the most collaborators. So `get_subscription_usage`
  already returns them, but without labels.
- The request size isn't capped beyond those limits: payg is unlimited.

### 3.6 How shared catalogs appear to recipients

- `GET /catalogs` (MCP `list_catalogs`) defaults to `filter=all`, which covers own, shared and public catalogs. Its
  `filter` param accepts `all|public|shared|own` (`app/src/routes/catalogs.rs:83-84`), but the **MCP doesn't expose it**
  (`ListCatalogsParams`, `src/tools/catalogs.rs:8-19`).
- `CatalogDto` / `CatalogWithPinnedDto` have **no owner, author or access-level field** (`db/src/models/dtos.rs:163-199`,
  `:212-`). `GET /catalogs/{id}/permissions` returns `{allowRead, allowEdit, allowDelete, authorId, canPublish,
  canSetShadowing}` (`db/src/models/dtos.rs:13-`). The MCP has no client method for it.
- Editors can edit cards and catalog fields but can't share (§3.1) or change shadowing
  (`docs/tdd-catalog-shadowing.md` §3.2).

## 4. Current MCP gaps

| Gap | Where |
|---|---|
| No collaborator DTOs | `src/dto.rs` |
| No client methods for `GET`/`POST /catalogs/{id}/collaborators` | `src/client.rs` (catalog section, `:301-335`) |
| No sharing tools | `src/server.rs` `catalog_tools_router`; params in `src/tools/catalogs.rs` |
| `shared_catalogs` / `collaborators_per_catalog` have no label or quota hint | `resource_label`, `src/subscription.rs:15-26`; `quota_message` hints, `src/error.rs:403-411` |
| A 403 on share reads as the generic "may not exist / belong to another user / higher role" | `forbidden_hint`, `src/error.rs:516-525` |

Baseline before any change: **829 tests passing, 0 failed** (`cargo test`, `main` @ `14c1da2`, 2026-10-26).
`test_all_live_tools_have_title_and_annotations` asserts 38 tools (`src/server.rs:6316`: 31 core + 5 paid AI + 2 TTS).

## 5. Design

### 5.1 DTOs and client (`src/dto.rs`, `src/client.rs`)

- `AccessLevel` enum, `#[serde(rename_all = "lowercase")]`, with `Viewer` and `Editor`. It derives `Serialize`,
  `Deserialize` and `schemars::JsonSchema`, so it can be both the tool param type and the wire type.
- `CollaboratorDto { #[serde(rename = "userId")] user_id: Uuid, #[serde(rename = "accessLevel")] access_level: AccessLevel }`
  is used for the GET response and for tool output.
- `CollaboratorChange { userId: Uuid, accessLevel: Option<AccessLevel> }` for requests. `access_level` is **not**
  `skip_serializing_if`, so `None` serializes as `"accessLevel": null` (= remove). Same pattern as
  `SpeakingSettingsUpdate`.
- `ShareCatalogRequest { changes: Vec<CollaboratorChange> }`. The MCP dedupes before building it, so a `Vec` is fine.
- `ShareCatalogResponse { id: Uuid }`, with `#[serde(default)]` on everything else, so a body that later drops or
  renames `shared` still decodes. Or use `post::<_, serde_json::Value>` and ignore the body; the implementer chooses.
- `EngramoClient::list_collaborators(catalog_id) -> Result<Vec<CollaboratorDto>, ApiError>` → `GET /catalogs/{id}/collaborators`.
- `EngramoClient::update_collaborators(catalog_id, &ShareCatalogRequest) -> Result<(), ApiError>` →
  `POST /catalogs/{id}/collaborators` through the existing `post` helper. That way responses keep the size cap and
  decode errors stay `Decode`.

### 5.2 Tools (always-on `catalog_tools_router`, `src/server.rs`)

Three tools mirror the backend's three operations. The backend has one POST, but splitting grant and revoke gives each
tool honest annotations and leaves no `null` level for a model to send by mistake (Q1).

**Shared param validation** (pure helper `parse_user_ids(&[String]) -> Result<Vec<Uuid>, String>` in `src/tools/catalogs.rs`, unit-tested):

1. An empty list is a local error, with no HTTP call. A backend no-op would otherwise look like success.
2. Each entry is trimmed and passed to `parse_uuid`. On the first bad entry the error names its index and value, e.g.
   `user_ids[2]: Invalid UUID: 'bob@example.com' — user ids are UUIDs, not emails…`, and no HTTP call is made.
3. Duplicates are removed **silently**, keeping first-seen order. Otherwise the backend returns 400 "multiple changes".
4. At most `MAX_SHARE_USER_IDS = 100` after dedupe (Professional's per-catalog cap). Over the cap is a local error.
   This bounds the request, since the backend has no cap on payg (Q3).

#### `share_catalog`

| Field | Type | Meaning |
|---|---|---|
| `catalog_id` | `String` (UUID) | A catalog you own |
| `user_ids` | `Vec<String>` (UUIDs), 1–100 | EngrAmo user ids to grant access |
| `access_level` | `Option<AccessLevel>`, default `viewer` | `viewer` can read and study it; `editor` can also edit its cards and details (not share it, delete it, or change shadowing) |

Flow:
1. Validate `catalog_id` and `user_ids`.
2. POST `{changes: [{userId, accessLevel}]}`, with the same level for every id.
3. Re-read with `list_collaborators` and return
   `{"catalogId", "granted": [ids], "accessLevel", "collaborators": [...]}`. If that re-read fails, still return success
   with the granted ids and a note that the list couldn't be fetched. The write already committed, so reporting an
   error would invite a pointless retry (Q4).

Error rewrites, done in a small private mapper shared with `unshare_catalog`:
- `PermissionDenied` → "Permission denied: only the catalog's owner can share it or change who has access (editors and
  viewers can't)."
- `NotFound` whose message contains `user ` → "Not found: no EngrAmo user has id <uuid>; nothing was shared (the
  request is all-or-nothing). Check the id with its owner." Keep the backend message's uuid.
- A catalog `NotFound`, `BadRequest` (self-share, for example) and `QuotaExceeded` pass through `err_result` unchanged.

Annotations: `read_only_hint = false`, `destructive_hint = true`, `idempotent_hint = true`, `open_world_hint = true`.
- **Destructive:** it overwrites an existing collaborator's level, so an editor can be downgraded to viewer. That fits
  `docs/directory-submission.md`'s "overwrite = destructive" rule.
- **Open world:** it makes private content visible to other EngrAmo users. That's exactly why `update_catalog` /
  `generate_catalog_with_cards` / `update_learning_path` carry `openWorldHint: true` (directory-submission §3).

Description (draft):
> Share a catalog you own with other EngrAmo users by their user ids, or change their access level. `viewer` (default)
> lets them see and study the catalog; `editor` also lets them edit its cards and details, but only you can share,
> delete or change shadowing. User ids are UUIDs, not emails, and there's no lookup by email: ask each person for their
> EngrAmo user id (they can get it by asking their own AI assistant to call get_subscription_usage). Sharing with a user
> who already has access updates their level. The request is all-or-nothing: an unknown id shares with nobody. Free
> plans can share 1 catalog with up to 5 people; call get_subscription_usage for your limits.

Quoting plan numbers in the description goes stale. The alternative is to say only "plan limits apply" (Q5).

#### `unshare_catalog`

`catalog_id` + `user_ids` (same validation). POST with `accessLevel: null` for each id. Returns
`{"catalogId", "removed": [ids], "collaborators": [...]}`, re-reading the list the same way as `share_catalog`. Ids that
weren't collaborators are a backend no-op, and the description says so. The same 403 rewrite applies. A user `NotFound`
can't happen here, because a DELETE has no FK check.

Annotations: `read_only_hint = false`, `destructive_hint = true`, `idempotent_hint = true`, `open_world_hint = false`.

Description (draft): "Stop sharing a catalog you own with the given user ids: they lose access right away, along with
any edits they could make. Ids that don't currently have access are ignored. Only the owner can do this."

#### `list_catalog_shares`

`catalog_id` only. Calls `GET` and returns `{"catalogId", "collaborators": [{userId, accessLevel}]}`. An empty list
means the catalog isn't shared. `PermissionDenied` → "Permission denied: only the catalog's owner or its editors can see
who it is shared with." The description says it returns ids and levels only, not names or emails, and that the owner
isn't listed.

Annotations: `read_only_hint = true`, `destructive_hint = false`, `idempotent_hint = true`, `open_world_hint = false`.

Every handler returns `Ok(CallToolResult)` and never `Err()` (CLAUDE.md tool-error contract). Every top-level schema
property needs a `type` or `anyOf`, which the all-tools schema test from the shadowing branch enforces. Check that
`Option<AccessLevel>` doesn't come out as a bare `$ref`. If it does, add `#[schemars(extend("type" = "string"))]` as
`generate_card` did.

### 5.3 Server instructions (`src/server.rs:1252`)

Add one sentence: "Sharing tools take EngrAmo user ids (UUIDs), not emails; a user finds their own id with
get_subscription_usage." The `test_get_info_instructions_*` tests get a matching assertion.

### 5.4 Quota labels and hints (`src/subscription.rs`, `src/error.rs`)

- `resource_label`: `shared_catalogs` → "Shared catalogs", `collaborators_per_catalog` → "People per shared catalog".
- `quota_message` hints:
  - `shared_catalogs` → " Stop sharing a catalog you no longer need to share (unshare_catalog removes everyone from it) to free a slot."
  - `collaborators_per_catalog` → " Remove people who no longer need access with unshare_catalog."
- `window: "absolute"` already renders as "lifetime limit … doesn't reset", which is correct for both.

### 5.5 Out of scope

- An owner/shared flag on `CatalogDto` (the backend has none). `list_catalogs` `filter=own|shared` **is** in scope, see §10.6.
- A `get_catalog_permissions` tool.
- Any lookup by email. That needs backend work (Q2).
- New resources or prompts. `engramo://catalogs` is unchanged.

## 6. Tests (acceptance criteria)

All tests use `wiremock`, `tokio::test`, and the in-file `#[cfg(test)] mod tests` of the module under test. No network.

### Phase 1: DTOs, client, labels

- `CollaboratorChange` serialization: `Some(Viewer)` → `{"userId":"…","accessLevel":"viewer"}`; `None` →
  `"accessLevel":null` with the null present.
- `CollaboratorDto` deserializes the real GET body (`[{"userId":"…","accessLevel":"editor"}]`), plus an empty array.
- `list_collaborators` hits `GET /catalogs/{id}/collaborators` with `X-Api-Key`.
- `update_collaborators` POSTs the exact `body_json` and accepts the real 200 body `{"id":…,"shared":"successfully"}`.
- Client error mapping: 403 `{"error":"Forbidden"}` → `PermissionDenied`; 404 user → `NotFound` containing the uuid;
  the 429 share bodies for both resource types → `QuotaExceeded` with `window: absolute`.
- `resource_label` covers both new types. `quota_message` for `collaborators_per_catalog` with `used:4, limit:5,
  requested:3` gives the "smaller batch (up to 1)" text plus the unshare hint, and `shared_catalogs` with
  `used:1, limit:1` gives "lifetime limit reached" plus its hint.

### Phase 2: tools

- `parse_user_ids` unit tests: empty → error; bad entry → error naming the index; an email → error saying ids aren't
  emails; padded with whitespace → ok; duplicates removed in first-seen order; 101 unique → error; 100 → ok.
- `share_catalog`:
  - Default level: the POST body is exactly `{"changes":[{"userId":A,"accessLevel":"viewer"},…]}`, then the GET, and
    the output lists the collaborators.
  - `access_level: editor` sends `"editor"`.
  - Duplicate ids send one change.
  - Invalid `catalog_id` or `user_ids` → `is_error`, zero requests (`.expect(0)`).
  - 403 → `is_error` with the owner-only text.
  - 404 user → `is_error` with the "no EngrAmo user has id" and all-or-nothing text.
  - 404 catalog → passed through.
  - 400 self-share → passed through.
  - 429 → `Quota exceeded: …` text.
  - POST ok but GET 500 → success result that names the granted ids and the note.
- `unshare_catalog`: the body has `"accessLevel":null` for each id, then a GET. 403 → owner-only text. Local validation
  → zero requests.
- `list_catalog_shares`: happy path; empty list; 403 → owner-or-editor text; invalid UUID → zero requests.
- All three are registered without paid AI and in `build_session_server` (http).
- `test_all_live_tools_have_title_and_annotations`: count 38 → **41** ("34 core + 5 paid AI + 2 local TTS"), and it
  asserts the §5.2 annotations for each new tool.
- The all-tools schema `type` test passes with the new params.
- The server instructions mention user ids.

Test count may only grow from the 829 baseline.

## 7. Docs to update (Phase 3)

| File | Change |
|---|---|
| `README.md` "Catalogs" tool table (~line 498) | Add `share_catalog`, `unshare_catalog`, `list_catalog_shares` |
| `docs/directory-submission.md` | §3 intro: 31 → 34 tools, `openWorldHint: false` 28 → 30, open-world list gains `share_catalog`. §3.1 gains `list_catalog_shares`. §3.3 gains `share_catalog` and `unshare_catalog`. **Also fix line 19**, which still says "Reviewers see **30 tools**" while §3 says 31 (stale since #119); it becomes 34. Add a test-case row ("Share my Spanish deck with user <id> as a viewer"). |
| `CLAUDE.md` architecture tree | `tools/catalogs.rs` line: add `ShareCatalogParams`, … |
| `docs/prompt-examples.md` | One example covering share, list and revoke |

`mcpb/manifest.json` has `"tools_generated": true` and lists no tools, so it needs nothing. `chatgpt-plugin/` and
`server.json` don't enumerate tools either.

## 8. Phasing (orchestrated)

The file sets are disjoint where they can be. Phases 1a and 1b may run **in parallel**. Phase 2 needs both, and Phase 3
needs Phase 2's final descriptions.

| Phase | Scope | Files | Agent | Status |
|---|---|---|---|---|
| 0 | Cut `feat/catalog-sharing-tools` from `main` | — | orchestrator | pending |
| 1a | §5.1 DTOs and client (incl. `list_catalogs` filter), plus their Phase 1 tests | `src/dto.rs`, `src/client.rs` | general-purpose, sonnet | done (uncommitted) |
| 1b | §5.4 labels and hints, plus their Phase 1 tests | `src/subscription.rs`, `src/error.rs` | general-purpose, sonnet | pending |
| 2 | §5.2 + §5.3 + `list_catalogs` `filter` tool param, plus Phase 2 tests | `src/tools/catalogs.rs`, `src/server.rs` | general-purpose, sonnet | done (uncommitted) |
| 3 | §7 docs | `README.md`, `docs/directory-submission.md`, `CLAUDE.md`, `docs/prompt-examples.md` | general-purpose, sonnet | done (uncommitted) |
| 4 | `/review` loop on the branch | as found | `/review` skill | pending |
| 5 | Commit, push (engramo SSH identity), open PR | — | orchestrator | pending |

If 1a and 1b run in parallel in separate worktrees, verify them **one after the other** with `cargo clean -p engramo-mcp`
in between, because a shared `CARGO_TARGET_DIR` tests the wrong tree. The simpler option is to run them sequentially in
one tree.

Every phase subagent runs the full gate itself, in one foreground call, before reporting:

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | tail -30
```

It reports the files changed, the test count against the baseline, and any deviation from this TDD. The orchestrator
reviews `git diff` and updates the Status column.

## 9. After merge (user-driven)

- The user runs `/release`.
- Re-scan tools in the Claude Connectors Directory listing (34 tools). The open ChatGPT review sees the new tools on its
  next scan. `share_catalog` is a new open-world, destructive tool, so expect reviewers to look at it.

## 10. Decisions (approved 2026-10-26)

The user approved every recommendation from the review round:

1. **Three tools**: `share_catalog`, `unshare_catalog`, `list_catalog_shares` (not a single mixed-batch tool).
2. **Raw user ids for v1.** Sharing by email is out of scope (needs backend work). Recipients get their id from
   `get_subscription_usage`.
3. **Local cap of 100 ids** per call, after dedupe.
4. **Writes re-fetch** the collaborator list and return it in full.
5. **No plan numbers in tool descriptions** ("plan limits apply; see get_subscription_usage").
6. **`list_catalogs` `filter=own|shared` is exposed in this branch** (client side in Phase 1a as
   `list_catalogs_filtered` + `CatalogFilter`; the tool param lands in Phase 2). `public` stays unexposed.
7. **`share_catalog`** is `destructive_hint = true` and `open_world_hint = true`.

Implementation note (Phase 1a): `list_catalogs(limit, cursor, speaking_only)` keeps its signature (default = no filter)
and delegates to the new `list_catalogs_filtered(limit, cursor, speaking_only, Option<CatalogFilter>)`, so existing
callers in `server.rs`, `resources/` and tests compile unchanged. `update_collaborators` ignores the POST response body.

