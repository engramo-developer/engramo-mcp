# TDD — Subscription, limits and usage via MCP

Status: **implemented** (2026-10-26). Part A: engram-api #26. Part B: this PR. All open questions
in §5 were decided before implementation.

## 1. Goal

A user talking to EngrAmo through an AI client should be able to:

1. Ask "what plan am I on, what are my limits, how much have I used?" and get a clear, complete
   answer that includes their **userId**, their plan, every metered resource, the limit, the usage,
   the remaining amount, and when each limit resets.
2. Understand exactly what happened when a call fails on a limit: which limit was hit, on which
   window (lifetime, monthly, daily, hourly), how much was requested, when it resets, and what to do.
   The same goes for calls that fail because the plan doesn't include a feature.

## 2. Verified current state

### engramo-mcp (main @ 30100e0)

| Piece | State |
|---|---|
| `client.get_me()` → `GET /me` | Works. Used only by http-mode session-open validation. `MeDto { user_id }`. |
| `client.get_subscription()` → `GET /me/subscription` | **Broken shape.** `UserSubscriptionDto { plan_id: Option<String> }`, but the API serializes this DTO `camelCase` (`planId`, `planName`, …). `plan_id` is therefore always `None`, and the `engramo://subscription` resource returns `{"plan_id":null}`. |
| `client.get_usage()` → `GET /me/usage` | **Broken shape and unused.** MCP expects `{ resources: [...] }`; the API sends `{ plan_id, usage: [...] }`. Nothing outside tests calls it. |
| Tests for both | Mock the *wrong* shapes (`{"plan_id":"pro"}`, `{"resources":[...]}`), so they pass while hiding the bug. |
| `ApiError::QuotaExceeded` (402/429) | Parses only `resource_type`, `used`, `limit`. Ignores `requested`, `plan_id`, `window`, `code`. Message: `Quota exceeded: cards_total limit reached (500/500). Delete unused resources, or manage your plan in the EngrAmo app.` It doesn't name the window or the reset time, `limit: 0` reads as "0/0", and a too-large batch (`used < limit`) reads as a contradiction. |
| Plan-feature denial (403) | Arrives as a bare `"Forbidden"`, which `forbidden_hint` expands to the generic "may not exist / belong to another user / not allowed on your plan" text. The user can't tell it's a plan gate. |
| Tools | No subscription or usage tool exists. The data is only reachable through the (broken) resource, and many clients never read resources. |

### engram-api (origin/main @ 92e6ffb)

- `GET /me` → `{ user_id, canManageSubjects }`, with no PII by design (`app/src/routes/me.rs`).
- `GET /me/subscription` → `UserSubscriptionDto` (camelCase): `planId`, `planName`, `limits` (the raw
  merged plan+override JSON), `startsAt`, `expiresAt`, `pendingPlanId?`, `lapsedPlanId?`,
  `deactivatedPlanId?`, `features[]` (`db/src/models/quota.rs:223`).
- `GET /me/usage` → `UsageSummaryDto` (snake_case): `plan_id`, and `usage[]` of `UsageEntryDto
  { resource_type, used, limit, used_daily?, limit_daily?, used_hourly?, limit_hourly? }`
  (`db/src/models/quota.rs:253`; built by `quota_service.rs:1074 get_usage_summary`).
  - Resource types: `ai_agent_messages`, `tts_generations`, `translation_chunks`, `catalogs_total`,
    `cards_total`, `cards_per_catalog`, `premium_tts_generations`.
  - `limit: -1` means unlimited. A missing key reads as `0` (`unwrap_or(0)`).
  - Windows (`ResourceType::is_absolute_for_plan`): `catalogs_total` and `cards_total` are lifetime
    on Free/Payg and **monthly** on Basic/Professional. `cards_per_catalog` is always lifetime. The
    rest are monthly. Daily and hourly sub-limits exist for some types. All periods are UTC calendar
    boundaries (`monthly_period`/`daily_period`/`hourly_period`).
  - **None of this window or reset information is in the response**, so a client can't say
    "resets on 1 Nov" or even whether a number is lifetime or monthly.
  - **Known bug** (memory 2026-10-14): `cards_per_catalog.used` is the *sum over all catalogs*,
    because it sums every scope_id row. Enforcement is per catalog and correct; only the report is
    wrong.
- 429 quota body (`app/src/routes/mod.rs:654`): `{ error:"quota_exceeded", code:"QUOTA_EXCEEDED",
  resource_type, used, limit, requested, plan_id, window }`, where window is one of
  `monthly|absolute|daily|hourly`.
- 403 for `FeatureForbidden` (plan does not include a feature): the body is stripped to
  `{ "error": "Forbidden" }` (`mod.rs:711`). The comment says this is to avoid leaking resource and
  user ids, but `feature` and the caller's own `plan_id` are not sensitive. Features that MCP tools hit:
  `catalog_share`, `tts_generate`, `dictionary_generate`, `ai_chat`, `translate_cards`,
  `premium_tts`.
- No web or mobile client in the sibling repos consumes `/me/usage` (grep for `me/usage`,
  `cards_per_catalog` and `used_daily` found nothing), so additive changes there are safe.

## 3. Design

### Part A: engram-api (separate PR, branched from `origin/main`)

All changes are **additive**. Existing fields keep their meaning except the `cards_per_catalog.used`
bug fix.

**A1. Window and reset info on `GET /me/usage`.** Add to `UsageEntryDto`:

```rust
/// "absolute" (lifetime, never resets) or "monthly" — for this user's plan.
pub window: &'static str,
/// Start of the next window (UTC). None for "absolute".
#[serde(skip_serializing_if = "Option::is_none")]
pub resets_at: Option<DateTime<Utc>>,
#[serde(skip_serializing_if = "Option::is_none")]
pub resets_at_daily: Option<DateTime<Utc>>,   // set iff limit_daily is
#[serde(skip_serializing_if = "Option::is_none")]
pub resets_at_hourly: Option<DateTime<Utc>>,  // set iff limit_hourly is
```

`window` comes from `rt.is_absolute_for_plan(plan_id)`, which is the same function enforcement uses,
so the report can't drift from enforcement. Reset times are the next month, day and hour boundary,
computed from one `now`. The OpenAPI schema (utoipa) is updated with them.

**A2. Fix `cards_per_catalog.used`.** Report the **max** count over scopes (the fullest catalog)
instead of the sum, so `used/limit` reads as "your fullest catalog has N of M cards". Add
`#[serde(skip_serializing_if)] scope_id: Option<Uuid>` naming that fullest catalog, which is the
caller's own catalog id and so not a leak. This lets the AI say which catalog is full.

**A3. Explain plan-feature 403s.** For `ApiError::FeatureForbidden(_, feature)` only, the body becomes:

```json
{ "error": "Forbidden", "code": "FEATURE_NOT_ENTITLED", "feature": "ai_chat", "plan_id": "free" }
```

`error` stays `"Forbidden"`, so nothing that string-matches it breaks. The user id is still stripped
and only the caller's own plan and the feature name are added. This needs `FeatureForbidden` to carry
`plan_id`; today it carries `(Uuid, PlanFeature)`, and `QuotaError::FeatureNotEntitled` already has
`plan_id`. `Forbidden`/`ActionForbidden` are unchanged.

**Tests (API):** unit tests in `quota_service.rs` covering:
- window per plan (Free vs Basic for `catalogs_total`)
- `resets_at` at month, day and hour boundaries, plus `None` for absolute
- the `cards_per_catalog` max and `scope_id` with 2+ catalogs
- an empty usage case

Route tests in `mod.rs` and `catalogs.rs` assert the new 403 body: the code, feature and plan are
present, and no user id appears. Gate: the API's own fmt/clippy/test flow (scoped `-p` crates, then
the workspace).

### Part B: engramo-mcp (this repo, branch `feat/subscription-usage-tool`)

**B1. Fix DTOs** (`dto.rs`) to the real shapes:
- `UserSubscriptionDto` becomes camelCase with all of its fields.
- `UsageSummaryDto { plan_id, usage: Vec<UsageEntryDto> }`, where `UsageEntryDto` has every field
  above, and the A1/A2 fields are `Option`/`#[serde(default)]`. That way the MCP still works against
  a backend deployed before Part A, and just shows less.

The tests are rewritten against the real API JSON, copied from the API DTOs, so a shape drift fails
loudly.

**B2. New always-on tool `get_subscription_usage`** (read-only, idempotent, no cost). It calls
`/me`, `/me/subscription` and `/me/usage` concurrently (`tokio::try_join!`) and returns one
normalized view:

```json
{
  "userId": "f4c1…",
  "plan": { "id": "free", "name": "Free", "startsAt": "…", "expiresAt": null,
            "pendingPlanId": null, "lapsedPlanId": null, "deactivatedPlanId": null,
            "features": ["api_token_create"] },
  "usage": [
    { "resource": "catalogs_total", "label": "Catalogs", "window": "absolute",
      "used": 3, "limit": 5, "remaining": 2, "unlimited": false, "included": true,
      "resetsAt": null },
    { "resource": "ai_agent_messages", "label": "AI chat messages", "window": "monthly",
      "used": 0, "limit": 0, "remaining": 0, "unlimited": false, "included": false,
      "resetsAt": "2026-11-01T00:00:00Z",
      "daily":  { "used": 0, "limit": 0, "remaining": 0, "resetsAt": "…" },
      "hourly": { … } },
    { "resource": "cards_per_catalog", "label": "Cards in your fullest catalog",
      "catalogId": "…", … }
  ],
  "howToChange": "Plan changes aren't self-service yet — contact the EngrAmo team to change your plan."
}
```

- The plan block includes `planId`/`planName`/`features`/dates and the pending, lapsed and deactivated
  markers. The raw `limits` JSON is left out because it duplicates `usage` with internal key names.
- Normalization rules:
  - `-1` → `unlimited: true` with `limit`/`remaining` null
  - `0` → `included: false` ("not part of your plan")
  - `remaining = max(0, limit - used)`
- `label` is a fixed human name per resource type. An unknown future type passes through with its
  raw name instead of being dropped.
- If `/me/usage` or `/me/subscription` fails, the whole tool returns `err_result`. A partial answer
  about limits is worse than a clear error.
- The description tells the model to call the tool when the user asks about plan, limits or usage,
  **or after any "Quota exceeded" / "not included in your plan" error.**

**B3. `engramo://subscription` resource** returns the same normalized view, via a shared builder
function, so the resource and the tool can't diverge. Its description is updated.

**B4. Clearer quota errors** (`error.rs`). `QuotaExceededBody` gains `requested`, `plan_id`,
`window` and `code`, and `QuotaExceeded` gains `requested: Option<i64>`, `plan_id: Option<String>` and
`window: Option<String>`. The message is built from a small renderer:

- Normal: `Quota exceeded: Cards (total) — monthly limit reached: 500 of 500 used on your basic
  plan; resets 2026-11-01 00:00 UTC.` The reset time is computed in MCP from `window` with the same UTC
  rules. Daily resets at the next 00:00 UTC and hourly at the next hour.
- Batch larger than headroom (`used + requested > limit`, `used < limit`): `… 480 of 500 used; this
  request needed 30. Try a smaller batch (up to 20) …`
- `limit == 0`: `… isn't included in your free plan.`
- Absolute window: `… lifetime limit; it doesn't reset. Delete unused catalogs/cards to free room.`
- A per-resource hint, e.g. `cards_per_catalog` → "put further cards in another catalog".
- It always ends with: `Call get_subscription_usage for all your limits. Plan changes aren't self-service yet — contact the EngrAmo team to change your plan.`
  The current `error.rs:41` text ("manage your plan in the EngrAmo app") is replaced, because there is
  no in-app plan management; `error.rs:766` is updated accordingly.
- The `Quota exceeded:` prefix and the `QuotaExceeded { .. }` matches in `server.rs:783,1806` stay
  unchanged.
- An unparseable body keeps today's fallback, but with "limit details unavailable" in place of the
  misleading `0/0`.

**B5. Plan-feature 403** (`error.rs`). When a 403 body has `code: "FEATURE_NOT_ENTITLED"`, it maps to a
new `ApiError::FeatureNotEntitled { feature, plan_id }`. The message reads: `Not included in your plan:
AI chat (ai_chat) isn't available on the free plan. Call get_subscription_usage to see what your plan
includes. Plan changes aren't self-service yet — contact the EngrAmo team to change your plan.` Bare `"Forbidden"` keeps the existing `forbidden_hint`
path, so older backends behave the same. Retry loops that skip on `Unauthorized | QuotaExceeded`
(`server.rs:783,1806`) also treat `FeatureNotEntitled` as non-retriable, because the next id won't fix
it.

**B6. Docs:** CLAUDE.md architecture line for the new tool, README tool list, and
`docs/prompt-examples.md` gets one example ("How much of my plan have I used?").

**Tests (MCP), with wiremock:**
- DTO round-trip on real API JSON, with and without the A1/A2 fields.
- Tool happy path: all three calls, the normalized output, `userId` present, `-1`/`0` handling, and
  an unknown resource type passed through.
- Tool error paths: each of the three calls failing (401, 500) → `is_error`, never `Err`.
- Resource equals the tool output.
- Quota message matrix: monthly, daily, hourly and absolute windows; the batch-too-large case;
  `limit 0`; a missing body; the old body without the new fields.
- `FEATURE_NOT_ENTITLED` mapping, plus bare `Forbidden` unchanged.
- Retry loops stop on `FeatureNotEntitled`.
- Tool-list test: the new tool is present in both stdio and http routers.

## 4. Rollout

1. **engram-api PR** (`feat/usage-transparency` from `origin/main`). engram-api is currently on a
   clean `feat/extended-catalog-filters`, so I switch branches and come back afterwards. Commit, push
   and open the PR. You review, merge and deploy.
2. **engramo-mcp PR** (`feat/subscription-usage-tool`, already created from main). Implement B1–B6,
   run the full gate, then commit, push and open the PR. It is backward-compatible with the old
   backend (window and reset time come from the quota body or are omitted), so it can merge before
   or after the API deploy.
3. You release (`/release`) when ready. The new tool changes the tool list, so re-scan the Claude
   directory listing afterwards (the tool freeze was lifted on 2026-10-26).

## 5. Decisions (were open questions)

1. **Tool name:** ✅ decided 2026-10-26 — `get_subscription_usage`.
2. **Upgrade link:** ✅ decided 2026-10-26 — there is none. Plans are changed manually in the DB, so no
   message may say "in the EngrAmo app". All three places (tool `howToChange`, quota error, feature
   error) use one constant `PLAN_CHANGE_HINT` in `error.rs`, so a future URL is a one-line change.
3. **A3 body:** ✅ decided 2026-10-26 — add `code` / `feature` / `plan_id` to the feature-gate 403,
   keeping `error: "Forbidden"`; ownership/permission 403s stay bare.
4. **A2:** ✅ decided 2026-10-26 — report the max (fullest catalog) with `scope_id`.
5. **http mode:** ✅ decided 2026-10-26 — return the caller's own `userId` in both modes.
