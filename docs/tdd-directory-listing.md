# TDD: List engramo-mcp in public MCP directories (Claude, ChatGPT, MCP Registry, Antigravity)

> **Status:** IN PROGRESS, 2026-10-13. Open questions resolved (section 1). Implementation progress is tracked in section 10.
> **Goal:** EngrAmo installs in one click with OAuth from Claude (web, desktop, mobile, Code), ChatGPT, and
> MCP-aware dev tools (VS Code/Copilot, Cursor, Antigravity), and shows up in their official directories.
> **Origin:** a first-pass plan drafted outside this repo ("Let's prepare comprehensive plan to do this step b….md").
> We checked it against this codebase, engram-api, engram_app, engramo-landing, the live prod endpoints, and the
> directories' current documentation. Section 2 lists what changed.

---

## 1. Decisions already made

| Question | Decision |
|---|---|
| Has engram-api's OAuth authorization server (Track 3 Phase 3) had its security review? | **Yes, it's done.** We treat OAuth as production-ready, and this TDD only closes the remaining gaps. |
| Scope of the first execution wave | **All platforms, in order:** code gaps → MCP Registry → Claude directory → ChatGPT directory → Antigravity. |
| ChatGPT integration style | **Native MCP app** (Apps SDK / app directory). We are **not** building an OpenAPI gateway or a Custom GPT. |
| Branching | engramo-mcp: feature branches and PRs only, never `main`. engram-api: changes go **uncommitted on the branch the user names**; no branches, commits, or PRs unless asked. Releases stay user-driven (`/release` runs separately). |
| 4.3: cache token validation? | **No cache.** Every session open costs one engram-api call, behind a free token-prefix pre-filter (`engramo_` current, `engram_` legacy). |
| 4.3: which endpoint validates a token? | **A new `GET /me` in engram-api** (Phase A), built before 4.3. 4.3 waits for it; there is no interim probe. |
| Terms of Service / privacy policy | ToS exists (`engramo-landing/src/pages/policy/terms.astro`). The privacy policy needs fixes before submitting; see 6.1. |
| Paid AI on the public prod endpoint | **Stays off** while listed. The quota error's "Upgrade your plan" wording is reworded anyway (4.5). |
| Revoking OAuth tokens | **engram-api endpoints plus a "Connected apps" section in engram_app** (Phases A and B), done **before** the Claude and ChatGPT submissions. |
| MCP Registry namespace | **Branded `app.engramo/mcp`**, with DNS verification on `engramo.app` and an Ed25519 key held as a CI secret (section 5). |

---

## 2. Current state vs. the original draft plan

The original draft assumed everything still had to be built. Most of the infrastructure already exists.

| Original draft said | Verified reality |
|---|---|
| **Phase 1.1:** implement dynamic per-request auth instead of a startup env token | **Done** (Track 3 Phase 1). `engramo-mcp http` is multi-tenant. `bearer_auth_middleware` (`src/http_auth.rs`) scopes each session's bearer into `CURRENT_BEARER_TOKEN`; the session factory in `build_app` (`src/main.rs`) builds one `EngramoClient` per MCP session; `SessionTokens` binds each session id to the token that opened it. |
| The endpoint is `/mcp` | It is served at **root** `/` (rmcp `StreamableHttpService` via `fallback_service`). Prod is `https://mcp.engramo.app/` and dev is `https://mcp-engramo.volmyr.com/` (`docs/gcp_deployment_guide.md`). |
| **Phase 1.2:** provide OAuth 2.0 endpoints / DCR | **Done and reviewed** (Track 3 Phase 3, `engram-ws/tdds/track3-mcp-chatgpt-app.md`). engram-api is the authorization server: `/.well-known/oauth-authorization-server`, `/oauth/register` (DCR), `/oauth/authorize` (Supabase login bridge page), `/oauth/token`, PKCE S256 only, public clients (`token_endpoint_auth_methods_supported: ["none"]`). engramo-mcp is the resource server and serves `/.well-known/oauth-protected-resource` (`src/well_known.rs`). **But OAuth has never been run end to end by a real client**: Track 3 validated ChatGPT only with a pasted token. Phase 0 closes that. |
| **Claude:** fork `modelcontextprotocol/servers` and PR a listing | That repo stopped accepting third-party server listings. The two real channels are the **official MCP Registry** (`registry.modelcontextprotocol.io`, published with the `mcp-publisher` CLI and a `server.json`) and Anthropic's **Connectors Directory** (developer portal at `claude.ai/directory/manage`). |
| **Smithery:** add `smithery.yaml` | Smithery only needs that file for servers it hosts or builds. For an already hosted remote server, you register the URL in the Smithery UI. Glama auto-indexes public GitHub MCP repos, and you claim the listing. |
| **Antigravity:** a "standard Antigravity schema" with `transport`/`auth.oauth2` blocks, plus a Google partner intake | The schema is invented. Antigravity reads `mcp_config.json`, using `serverUrl` for remote servers and `command`/`args` for local ones. Its MCP Store is curated, and we found **no public submission process**. |
| **ChatGPT** doesn't speak MCP, so build an OpenAPI gateway (`/openapi.json`, `POST /tools/*`) and a Custom GPT | **Outdated.** ChatGPT consumes MCP natively: developer-mode connectors, plus the Apps SDK and app directory. Our existing remote server *is* the ChatGPT app. Track 3 already validated a ChatGPT developer-mode connector against dev with a pasted `engram_` token. |

### 2.1 Live prod probes (2026-10-12)

```
GET https://mcp.engramo.app/.well-known/oauth-protected-resource
→ {"resource":"https://mcp.engramo.app","authorization_servers":["https://api.engramo.app"]}

GET https://api.engramo.app/.well-known/oauth-authorization-server
→ {"issuer":"https://api.engramo.app",
   "authorization_endpoint":"https://api.engramo.app/oauth/authorize",
   "token_endpoint":"https://api.engramo.app/oauth/token",
   "registration_endpoint":"https://api.engramo.app/oauth/register",
   "response_types_supported":["code"],"grant_types_supported":["authorization_code"],
   "code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"]}

POST https://mcp.engramo.app/   (no Authorization header)
→ HTTP/2 401, content-type: text/plain — and **no `WWW-Authenticate` header**  ← gap, see 4.2
```

Note: `resource` is advertised **without** a trailing slash, while connectors are configured with
`https://mcp.engramo.app/`. Lenient clients normalize the two to the same URL; strict RFC 9728 clients may not. Phase 0 checks this.

### 2.2 Other relevant facts found during research

- **Redirect URIs:** `engram-api/service/src/oauth_service.rs::validate_redirect_uri_format` accepts any
  `https://` URI and loopback `http://` URIs at DCR time. Claude's callback (`https://claude.ai/api/mcp/auth_callback`) and
  ChatGPT's (a `chatgpt.com` https URL) therefore both register without an allowlist change.
- **Token model:** each OAuth access token is a minted 90-day PAT (prefix `engramo_`; pre-rename tokens use the legacy `engram_`, which still validates), stored with `source = 'oauth'`
  (`api_token_service.rs`, `TokenSource::Oauth`), and there is **no refresh token**. When it expires the user must redo
  OAuth. Clients only do that when the server answers with HTTP 401. See 4.3.
- **Token revocation gap:** `GET`/`DELETE /auth/api-tokens` (used by engram_app's API-token page) are hard-scoped to
  `source = 'user'` (`db/src/repo/api_token/api_token_repo_pg.rs:266, 308, 331`; `mod.rs:174`: "cannot delete
  OAuth-issued rows"). **OAuth tokens can't be listed or revoked anywhere today.** Phases A and B fix this.
- **Live tool surface:** **36 tools**: 29 in `server.rs`, 5 in `tools/ai.rs` (paid AI, off in prod), 2 in `tools/tts.rs`
  (stdio only). None has `annotations(...)` or a `title`. The `#[tool]`s in `tools/{cards,learning,learning_paths,media}.rs`
  belong to `CardTools`, `LearningTools`, `LearningPathTools`, and `MediaTools`, which are **unused duplicates** (see the regression
  guards at `server.rs:1626`, `:1870`, `:1912`). They are out of scope here, so don't annotate them.
- **Session storage:** rmcp sessions live in memory (`LocalSessionManager`). Cloud Run runs up to 5 instances
  (`cloudbuild.yaml`) with no session affinity, and MCP clients don't keep cookies anyway. A request that lands on another
  instance gets 404, and the client re-initializes. That works, but it adds churn during a reviewer's "call every tool" run.
- **MCPB:** the Claude directory **no longer accepts local servers packaged as MCPB** (desktop-extension listings
  are deprecated, and local servers now go into the directory only inside a *plugin*). The `.mcpb` already ships as a
  GitHub-release asset (#88), and the MCP Registry accepts it as a package type.

---

## 3. Directory requirements (as documented, Oct 2026)

### 3.1 Claude Connectors Directory
Source: <https://claude.com/docs/connectors/building/submission>

- A remote server, reachable over HTTPS. Local servers can't be submitted on their own.
- OAuth 2.0 for user-account tools. The supported modes are **DCR**, client ID metadata documents (CIMD), or Anthropic-held client credentials.
- **Every tool has a `title` and a `readOnlyHint` or `destructiveHint` annotation.** The portal flags any tool that lacks them.
- Tested in Claude as a custom connector. You must confirm that every tool was run.
- Listing materials: name (100 chars max), one-liner (200), description (2000), 1–5 categories, docs URL, privacy
  policy URL, support contact, icon, and a permanent URL slug.
- A test account for reviewers: credentials for a fully populated account.
- Seven compliance acknowledgments. The relevant ones here: first-party API usage, financial transactions, AI media generation (TTS),
  prompt injection, and conversation data collection.
- Process: open the portal → Submit new → MCP connector. After an automated scan the server is listed as a **Community** connector,
  and some submissions also get a human review. Escalations go to `mcp-review@anthropic.com`.
- Any paid Claude plan can submit.

### 3.2 ChatGPT app directory
Source: <https://developers.openai.com/apps-sdk/deploy/submission>

- OpenAI **organization or developer identity verification** must be complete first. Only one app can be in review per org at a time.
- **Domain verification:** serve the exact challenge token as plain text at
  `https://<mcp host or eligible parent>/.well-known/openai-apps-challenge`.
- Automated tool scan. Each tool's `readOnlyHint`, `destructiveHint`, and `openWorldHint` need a justification, and tool names should
  be verb-style and unique. *(Confirm the exact required set against the form at submission time.)*
- Assets: a square logo (48×48 minimum), a dark variant (recommended), brand colors, display name (30 chars max), subtitle (30 max), long
  description (4000 max), category, and up to 20 capabilities.
- **Website, support, privacy policy, and Terms of Service URLs are all required.**
- **5 positive and 3 negative test cases**, a demo video, and a test account with sample data. The account must not need MFA, email codes, or a private network.
- Policy points: the app must be first-party, not a pass-through relay, and it **must not sell subscriptions inside the app**.

### 3.3 Official MCP Registry
Sources: <https://modelcontextprotocol.info/tools/registry/publishing/>,
<https://www.stackone.com/blog/mcp-registry-how-to-get-listed/>

- Publish with the `mcp-publisher` CLI (`init`, `login`, `validate`, `publish`) and a `server.json`.
- Namespace auth: `io.github.<owner>/…` through GitHub login or GitHub OIDC in Actions, or a **reverse-DNS namespace
  (`app.engramo/…`) through DNS or HTTP verification** with an Ed25519 key. We use DNS (section 5).
- `remotes[]` lists remote endpoints. Our remote (`mcp.engramo.app`) sits under the verified domain.
- `packages[]` covers npm and others. An npm package needs `"mcpName": "<server name>"` in its `package.json`, and the registry
  checks it against the **published** npm version.
- An **MCPB** package must be hosted on GitHub or GitLab releases with "mcp" in the URL, and it is verified by a `fileSha256` in the record.
- Downstream, the registry feeds VS Code/Copilot's MCP gallery and aggregators such as PulseMCP, Smithery, and Glama.

### 3.4 Google Antigravity
Sources: Antigravity custom MCP guides (Google Developer Experts on Medium, Google Workspace codelab)

- Users add servers under Agent panel → "…" → MCP Servers → Manage MCP Servers → View raw config → `mcp_config.json`,
  then refresh.
- A remote server uses `serverUrl`, and a local one uses `command`/`args`.
- The MCP Store is a curated list, and we found no documented public submission process.

---

## 4. Design: code changes in engramo-mcp

4.1, 4.2, 4.4, and 4.5 go on one branch, `feat/directory-readiness`. 4.3 waits for Phase A (`GET /me` deployed to dev) and lands as
its own small PR (`feat/session-open-token-check`). The `/review` loop runs before each PR is opened.

### 4.0 Prerequisite: the leftover `feat/mcpb-bundle` commit
**Done (no-op).** The bundle landed in #88, and the squash merge already included `73a59d5 fix(ci): unblock mcpb tools npm audit`. A cherry-pick onto `main` came up empty, and `mcpb/tools/audit.mjs` passes on `main`.

### 4.1 Tool annotations (hard requirement for both directories)
- Scope: the **36 live tools**, meaning the `#[tool(...)]`s in `src/server.rs`, `src/tools/ai.rs`, and `src/tools/tts.rs`. Leave the
  unused `*Tools` duplicates alone (2.2).
- For each tool, set:
  - Top-level `title = "..."` (rmcp-macros 3.5 `#[tool(title = ...)]`; clients show it first), and the same string as
    `annotations(title = ...)`.
  - `read_only_hint`, `destructive_hint`, `idempotent_hint`, and `open_world_hint`, **all explicitly**. For read-only tools,
    set `destructive_hint = false` and `idempotent_hint = true`.
- Classification. Per the MCP spec, `destructiveHint` means "may perform destructive updates", so **overwriting content counts**.
  Confirm each row during implementation:

| Class | Tools | Hints |
|---|---|---|
| Read (14) | `get_server_version`, `list_catalogs`, `get_catalog`, `list_cards`, `get_card`, `get_due_cards`, `get_all_learning_cards`, `list_learning_paths`, `get_learning_path`, `search_global`, `search_catalogs`, `search_learning_paths`, `list_media`, `list_tts_voices` | `read_only = true`, `open_world = false` |
| Additive write (9) | `create_learning_path`, `add_card_to_learning`, `add_catalog_to_learning`, `activate_learning_path`, `add_catalog_to_learning_path`, `upload_media`, `generate_card`, `generate_catalog_with_cards`, `generate_cards` | `read_only = false`, `destructive = false`, `open_world = false`; `idempotent = true` for `add_*`/`activate_*` if the API treats repeats as no-ops |
| Destructive / overwriting write (8) | `delete_card`, `delete_catalog` (including the archive path, #81), `update_card`, `update_catalog`, `update_learning_path`, `remove_catalog_from_learning_path`, `deactivate_learning_path`, `generate_card_audio` (overwrite deletes the old audio, #60) | `read_only = false`, `destructive = true`, `idempotent = true` (except `generate_card_audio`); `open_world = true` only for `generate_card_audio` (calls Gemini) |
| Paid AI (5, off in prod) | `generate_tts_for_cards`, `translate_cards`, `generate_dictionary_for_cards`, `ai_agent_chat`, `translate_batch_import` | `open_world = true`. Destructive if the tool overwrites existing card fields or audio, otherwise additive |

- **Regression test** in `src/server.rs` tests: build the server with paid AI and local TTS enabled, so all three routers are
  summed (assert 36 tools). For each tool in `list_all()`, assert that `title` and `annotations.title` are set and that
  `read_only_hint`, `destructive_hint`, and `open_world_hint` are all `Some`. This way a future tool can't ship without them.
- While editing, check tool names and descriptions against review criteria: verb-style, unique, accurate, and with no
  instructions aimed at the model in descriptions (the prompt-injection policy).

### 4.2 `WWW-Authenticate` on 401
MCP's authorization spec expects a 401 from the resource server to carry
`WWW-Authenticate: Bearer resource_metadata="<url of .well-known/oauth-protected-resource>"`. Clients use it to start discovery.
Some clients fall back to probing `.well-known` themselves, but relying on that is fragile.

- `bearer_auth_middleware` (`src/http_auth.rs`) has two 401 paths today:
  - **Missing or malformed bearer** → keep 401 and **add** the header (no `error=` parameter, per RFC 6750 §3.1).
  - **Session/token mismatch** → change it to **404** ("session not found"), **without** the header. Sending the header here
    causes a loop: a client that re-runs OAuth retries with the **same** `Mcp-Session-Id`, hits the mismatch again, and re-auths
    forever. 404 is the spec's "unknown session, start a new one" signal (rmcp already returns 404 for unknown ids), so the
    client re-initializes with its new token. It reveals nothing beyond what the 401 did. Update the existing mismatch tests.
- 4.3's invalid-token 401 carries the header with `error="invalid_token"`.
- The middleware state is currently `SessionTokens`. Replace it with an `AuthState { sessions, resource_metadata_url }`
  (extended in 4.3). Build the URL in `build_app` (`src/main.rs`) from the same value used for `ProtectedResourceState.resource`,
  with any trailing `/` trimmed before appending `/.well-known/oauth-protected-resource`.
- **Fail fast:** in `http` mode, refuse to start when `MCP_PUBLIC_URL` is unset, instead of falling back to the
  `http://localhost:8080` placeholder. Otherwise a misconfigured prod would send every client to localhost. Local runs set it
  explicitly (section 8). Update `config.rs` tests and the startup warning.
- Tests in `http_auth.rs`: the missing-bearer 401 carries the header with the exact URL, the mismatch returns 404 with no header,
  and the trailing-slash case. The remaining existing tests keep passing.

### 4.3 Expired or revoked token → HTTP 401 at session open
**Depends on Phase A** (`GET /me` deployed to dev).

- **Problem:** OAuth tokens are 90-day PATs with no refresh, and with Phase A users can also revoke them. Today any bearer that
  isn't empty opens a session, and a dead token only fails later inside a tool call as `is_error: true`. Claude and ChatGPT only
  re-run OAuth on an HTTP 401.
- **Design:** for a **POST without an `Mcp-Session-Id`** (a session open), validate the bearer before passing the request to rmcp.
  GET or DELETE without a session id is rejected by rmcp anyway, so it isn't checked.
  1. **Pre-filter, at no cost:** if the bearer starts with neither `engramo_` (engram-api `CURRENT_TOKEN_PREFIX`) nor `engram_` (`LEGACY_TOKEN_PREFIX`), return 401 with `error="invalid_token"` and make no
     upstream call. This takes the cost out of junk-bearer floods (otherwise each unauthenticated request causes one engram-api call).
  2. **Probe:** `EngramoClient::get_me()` → `GET /me` through the shared `HardenedClient`.
     - `ApiError::Unauthorized` (upstream 401) → **401** + `WWW-Authenticate: Bearer error="invalid_token", resource_metadata="…"`.
     - **403 counts as valid.** The backend turns every 403 into a bare `"Forbidden"` (`error.rs:346`), so a 403 doesn't prove
       the token is dead, and a 401 would send the user back through OAuth for nothing.
     - Network or 5xx errors → **503**, **not** 401, so a transient API outage doesn't force a re-login.
  3. **No cache** (decided). Sessions are short: rmcp closes idle sessions after 5 minutes, and Cloud Run scales to zero. So a token
     that dies mid-session is caught at the next session open within minutes. Until then, tool calls return the `Unauthorized`
     error below.
- `AuthState` gains `http: HardenedClient` and `api_url`.
- **Error text:** `ApiError::Unauthorized`'s message (`error.rs:52`) says "Check your ENGRAMO_API_TOKEN", which is wrong for OAuth users.
  Make it work for both modes: "EngrAmo authorization is invalid, expired, or revoked. Reconnect EngrAmo in your AI app (local
  installs: update ENGRAMO_API_TOKEN)." This change goes in with 4.1/4.2.
- Security: the token is never logged. The rule against logging the session id is unchanged.
- Tests: wiremock covering valid → session opens; unknown-prefix bearer → 401 with no upstream call; legacy `engram_` bearer → probed; upstream 401 → 401 with the header;
  upstream 403 → session opens; upstream 500 → 503; a request with a session id → no `/me` call; GET without a session id → no `/me` call.

### 4.4 ChatGPT domain verification route
- New unauthenticated route `GET /.well-known/openai-apps-challenge` on the outer router in `build_app`, next to
  `/version`.
  - If `OPENAI_APPS_CHALLENGE` is set and non-empty, return it as `text/plain` with exactly the token.
  - If it isn't set, return 404.
- `src/config.rs`: add an optional `openai_apps_challenge: Option<String>` read from the env var, with tests.
- `cloudbuild.yaml`: add a `_OPENAI_APPS_CHALLENGE` substitution passed through `--set-env-vars`, with an empty default.
- Docs: add the variable to `docs/gcp_deployment_guide.md` and to CLAUDE.md's env var list.
- The token is a public value, not a secret, so a plain env var is fine.

### 4.5 Public endpoint posture
- Prod Cloud Run keeps `ENGRAMO_ENABLE_PAID_AI` **off** while listed (decided). Confirm the prod trigger's substitution, since
  `cloudbuild.yaml`'s default is the dev value `false`.
- **No commerce wording:** the quota error (`error.rs:41`) says "Upgrade your plan or delete unused resources." It reaches
  ChatGPT even with paid AI off. Reword it neutrally, e.g. "…limit reached ({used}/{limit}). Delete unused resources, or manage
  your plan in the EngrAmo app." The `engramo://subscription` resource is read-only info, which is fine.
- Local TTS is already stdio-only by design (see CLAUDE.md), so it is never exposed on the public endpoint.
- During the review window, consider deploying prod with `--max-instances 1` so sessions don't hop between instances (2.2).
  Revert once the reviews are done.

### 4.6 Not changing
- OAuth server logic in engram-api: reviewed, and DCR plus https redirects already cover Claude and ChatGPT (pending
  Phase 0 confirmation that `/oauth/authorize` and `/oauth/token` accept the RFC 8707 `resource` parameter).
- Endpoint path: stays at `/`. Directories accept any https URL. *Check during the ChatGPT submission:* some guides say the URL "ends
  with /mcp". If OpenAI insists, add `/mcp` as an alias route instead of moving root.

---

## 4A. Design: engram-api (Phase A)

Implemented directly in `/Users/vova/work/workspace/volmyr/github/engram-api`, **uncommitted on the branch the user names**.
Unblocks 4.3 and the revocation story.

### 4A.1 `GET /me`
- Route next to the existing `/me/subscription` and `/me/usage` (`app/src/routes/mod.rs:272`). Handler in a new or existing
  route module, using the `AuthUser` extractor, so it accepts both Supabase JWTs and `X-Api-Key` PATs.
- Response `200 {"user_id": "<uuid>"}`. **No email or other PII.** Its job is "is this credential alive, and whose is it".
- It must do no work beyond authentication: no DB query beyond what `AuthUser` already does.
- An expired, revoked, or unknown token → 401, as for any route. Add a `utoipa` path entry and route tests.

### 4A.2 Connected apps: list and revoke OAuth tokens
- `GET /auth/connected-apps` → the caller's `source = 'oauth'`, non-one-time tokens:
  `[{ "id", "client_name", "created_at", "last_used_at", "expires_at" }]`. Never return the token or its hash.
  `client_name` is whatever OAuth issuance stores (the DCR `client_name`); confirm the column during implementation.
- `DELETE /auth/connected-apps/{id}` → deletes that row only when `user_id` matches **and** `source = 'oauth'`. Otherwise 404,
  mirroring how the existing personal-token delete is scoped to `source = 'user'`.
- The existing `/auth/api-tokens` routes stay as they are (personal token only).
- Effect on MCP: a revoked token gets 401 on the next API call, so the next session open gets 401 (4.3) and the client re-runs OAuth.
- Repo, service, and route tests, including "can't delete another user's row" and "can't delete a `source = 'user'` row through this route".

### 4A.3 OAuth token naming (found during implementation)
- `OauthServiceImpl::exchange_code` stored every OAuth token as `"ChatGPT ({client_name})"`, a leftover from when ChatGPT was the only
  client. A Claude connection would have shown as "ChatGPT (Claude)" in Connected apps.
- Fix: store the DCR `client_name` as-is (blank → "AI app"). The connected-apps list strips the exact legacy `"ChatGPT (X)"` form to `X`
  for rows already in the database.

## 4B. Design: engram_app (Phase B)

- A "Connected apps" section in settings next to `api_token_page.dart`. It lists 4A.2's entries (client name, connected date,
  last used) with a revoke action and a confirmation dialog. Uses the existing bloc and data-source pattern (`api_token_bloc.dart`,
  `api_token_data_source.dart`).
- The empty state explains how to connect Claude or ChatGPT.
- The workflow for that repo follows its own conventions. Ask before branching or committing.

---

## 5. Design: MCP Registry publication

- **Name:** `app.engramo/mcp` (decided: branded). Authenticated by **DNS verification** on `engramo.app`:
  - Generate an Ed25519 keypair once, offline (`openssl genpkey -algorithm ed25519`). Publish the public key as a TXT record on
    `engramo.app` in the format `mcp-publisher` documents (`v=MCPv1; k=ed25519; p=<base64 public key>`; verify the exact format
    at implementation time).
  - Store the private key as a GitHub Actions secret, `MCP_REGISTRY_PRIVATE_KEY`, scoped to a protected **environment**
    (`mcp-registry`) that only release tags can use.
  - Rotation: a new keypair, a new TXT record, then the new secret. Remove the old TXT record after the next successful publish.
  - The remote (`mcp.engramo.app`) is a subdomain of the verified domain.
- **New `server.json`** (repo root), with schema-pinned `version` placeholder `0.0.0` like the other manifests:
  - `remotes`: `[{ "type": "streamable-http", "url": "https://mcp.engramo.app/" }]`.
  - `packages`:
    - npm `@engramo/mcp`, stdio transport. Env vars: `ENGRAMO_API_TOKEN` (required, secret),
      `ENGRAMO_API_URL` (default `https://api.engramo.app`), `ENGRAMO_TTS_GEMINI_API_KEYS` (optional, secret).
    - mcpb: `https://github.com/engramo-developer/engramo-mcp/releases/download/v<ver>/engramo.mcpb` with
      `fileSha256`.
- `npm/engramo-mcp/package.json`: add `"mcpName": "app.engramo/mcp"`.
- **`release.yml`:** add a new job `mcp-registry`:
  - `needs: [resolve, github-release, npm-publish]`, skipped when `dry_run == 'true'`.
  - `environment: mcp-registry`, `permissions: { contents: read }`.
  - Download the release's `engramo.mcpb.sha256`, then rewrite `version` and `fileSha256` into a staged `server.json` with
    `process.env` (same pattern as the hardened manifest rewrite).
  - Install `mcp-publisher` pinned to a version **and checksum**, matching the repo's pinning discipline.
  - Run `mcp-publisher validate`, `mcp-publisher login dns --domain engramo.app --private-key "$MCP_REGISTRY_PRIVATE_KEY"`, and
    `mcp-publisher publish`. Retry `publish` a few times with a backoff, because the registry checks `mcpName` against the
    just-published npm version, which can take a moment to appear.
  - The registry key is the only secret this job sees, and it never shares a job with `NPM_TOKEN`.
- **Test** in the same style as `mcpb_manifest_matches_env_contract`: the env var names in `server.json` match `McpConfig` and
  `tts::ENV_KEYS`, secret fields are marked secret, `name` equals `package.json`'s `mcpName`, and the http-only and paid-AI vars are absent.
- **Aggregators**, after the first publish:
  - Register `https://mcp.engramo.app/` on Smithery (UI).
  - Claim the Glama listing.
  - Check that PulseMCP picks it up from the registry.

---

## 6. Rollout: submissions (manual and human-driven)

### 6.1 Shared prerequisites
- A **prod reviewer test account** with email and password login through the Supabase bridge and no MFA. Populate it with several
  catalogs, cards, due cards, and a learning path.
- **Public pages on engramo.app** (`engramo-landing`):
  - A docs or how-to-connect page, including how to disconnect (4B).
  - **Terms of Service:** exists at `/policy/terms` (`src/pages/policy/terms.astro`). Record the URL.
  - **Privacy policy** (`src/pages/policy/privacy.astro`), a submission blocker. It needs:
    - **Anthropic** added as a processor, worded like the OpenAI entry (line 80). It should cover claude.ai connectors and the
      Claude Desktop/Code stdio and `.mcpb` installs.
    - **Google (Gemini)** added. engram-api's prod config sends content to `generativelanguage.googleapis.com` and to the
      Gemini-based premium-TTS service. The lines saying "No other third-party services receive your personal data" (81) and
      "only… our own servers and Supabase" (67) are currently inaccurate.
    - Line 92 updated: access also works with API tokens and OAuth access tokens (`engramo_` PATs), not only Supabase sessions.
    - A sentence on revoking connected apps (4B), and a new "Last updated" date.
  - Support contact, using the GitHub issues URL or an email.
- **Brand assets:** a square icon (reuse `mcpb/icon.png`), a dark variant, and brand hex colors.

### 6.2 Claude Connectors Directory
1. In claude.ai, open Settings → Connectors → Add custom connector and enter `https://mcp.engramo.app/`. Complete OAuth through DCR, then
   call **every** tool from a conversation. Also check Claude Desktop and mobile.
2. Go to `claude.ai/directory/manage` → Submit new → MCP connector:
   - Connection: the URL.
   - Tools: no annotation warnings after 4.1.
   - Listing.
   - Use cases: reads and writes.
   - Auth: OAuth with DCR.
   - Data handling: our own first-party API; revocation via Connected apps (4B).
   - Test & launch: the test account.
   - Compliance: all seven acknowledgments.
3. Monitor the portal. After the automated scan, the connector goes live as **Community**.

### 6.3 ChatGPT app directory
1. Complete OpenAI org or developer verification.
2. Get the domain challenge token, deploy it as `OPENAI_APPS_CHALLENGE` on prod Cloud Run, and confirm that
   `curl https://mcp.engramo.app/.well-known/openai-apps-challenge` returns it.
3. Test in ChatGPT developer mode against prod with OAuth, not a pasted token.
4. Prepare 5 positive and 3 negative test cases.
   - Positive examples: make a deck from a topic, list due cards, add a catalog to learning, search cards, update a card.
   - Negative examples: requests to delete everything without confirmation, actions on another user's data, purchase or upgrade requests.
5. Record the demo video, fill in the metadata (including the per-tool hint justifications from 4.1), and submit. Iterate on the
   email feedback, then publish when approved.

### 6.4 Google Antigravity
1. Add an "Antigravity" section to the README with an `mcp_config.json` snippet:
   ```json
   { "mcpServers": { "engramo": { "serverUrl": "https://mcp.engramo.app/" } } }
   ```
   Check hands-on whether Antigravity runs the OAuth flow from the 401 and `WWW-Authenticate` header. If it doesn't, document the
   header form with a pasted `engram_` token, plus the stdio alternative (`npx -y @engramo/mcp` with `ENGRAMO_API_TOKEN`).
2. To get a store listing, contact Google through the developer or partner channel and point to the MCP Registry entry. This is best-effort,
   with no committed timeline.

---

## 7. Phasing and owners

| # | Deliverable | Repo / owner | Depends on |
|---|---|---|---|
| 0 | **OAuth spike (no code):** connect dev (`https://mcp-engramo.volmyr.com/`) as a custom connector in claude.ai, in ChatGPT developer mode, and in MCP Inspector, **using OAuth**. Record whether each client gets past the missing `WWW-Authenticate`, whether `/oauth/authorize` and `/oauth/token` accept the `resource` parameter, and whether the slash-less `resource` is accepted. Findings update 4.2 and 4.6. | user + agent | — |
| 0b | PR the leftover `73a59d5` (4.0) | engramo-mcp / agent | — |
| A | `GET /me` + connected-apps endpoints (4A), deployed to dev, then prod | engram-api / agent (uncommitted on the user's branch); user commits and deploys | — |
| B | "Connected apps" section (4B) | engram_app / agent; user ships | A |
| 1 | 4.1, 4.2, 4.4, 4.5 + `Unauthorized` text on `feat/directory-readiness` → `/review` → PR | engramo-mcp / agent | 0 |
| 1b | 4.3 on `feat/session-open-token-check` → `/review` → PR | engramo-mcp / agent | 1, A on dev |
| 1r | `/release`, deploy dev → prod (`MCP_PUBLIC_URL` is already set; now required) | user | 1, 1b |
| 2 | DNS TXT record + `mcp-registry` environment secret (user); `server.json`, `mcpName`, registry job, test → PR (agent) → the next release publishes | engramo-mcp / agent; user DNS, secret, release | 1r |
| 3 | Test account, privacy policy fixes (6.1), assets | user / engramo-landing (agent can draft the policy diff) | — (parallel) |
| 4 | Claude directory submission | user | 1r, 3, A + B in prod |
| 5 | ChatGPT verification, challenge env, submission | user (+ env change in 4.4) | 1r, 3, A + B in prod |
| 6 | Antigravity README section + outreach | agent (docs), user (outreach) | 1r, 2 |

---

## 8. Verification

- **Gate after every code change:** `cargo fmt --all && cargo check && cargo clippy && cargo test`, plus `cargo sort` if
  deps change. engram-api and engram_app run their own gates.
- **Local http mode against dev:**
  `ENGRAMO_API_URL=https://api-engram.volmyr.com MCP_PUBLIC_URL=http://localhost:8080 cargo run -- http`, then:
  - `curl -i -X POST localhost:8080/` → `401` with
    `WWW-Authenticate: Bearer resource_metadata="http://localhost:8080/.well-known/oauth-protected-resource"`.
  - Unset `MCP_PUBLIC_URL` → http mode refuses to start.
  - `initialize` with a bearer like `foo` → `401` with `error="invalid_token"`, and no `/me` call in the logs.
  - `initialize` with a revoked `engramo_` token → `401` with `error="invalid_token"`.
  - `initialize` with a valid dev `engramo_` token, then `tools/list` → 36 tools minus paid AI and TTS (29), each with `title`,
    `annotations.title`, and all three hints.
  - Reuse that session id with a different valid token → `404`.
  - `curl localhost:8080/.well-known/openai-apps-challenge` → 404 when unset, and the token when `OPENAI_APPS_CHALLENGE` is set.
- **MCP Inspector against dev** (`https://mcp-engramo.volmyr.com/`): the full OAuth flow starts from the 401, and every tool
  can be called.
- **Registry:** run `mcp-publisher validate server.json` locally, and check the DNS TXT record with `dig TXT engramo.app`. After
  the release, check `curl "https://registry.modelcontextprotocol.io/v0/servers?search=engramo"`.
- **End-to-end on each platform** (claude.ai custom connector, ChatGPT developer mode, Antigravity config): "make me a deck of 5
  Spanish verbs", then confirm the cards land in the test account at `study.engramo.app`.
- **Revocation path:** connect Claude on dev, revoke it in engram_app's Connected apps, wait for the session to idle out (or start a
  new chat). The next session open gets 401, and Claude re-runs OAuth.

---

## 9. Resolved questions (2026-10-13)

1. **4.3 cache:** none. A token-prefix pre-filter (`engramo_`/`engram_`) handles junk tokens (4.3).
2. **4.3 probe:** a new `GET /me` in engram-api (4A.1). 4.3 waits for it.
3. **Legal pages:** ToS exists. The privacy policy needs Anthropic, Google/Gemini, and token-wording fixes (6.1).
4. **Paid AI in prod:** stays off. The quota wording is neutralized regardless (4.5).
5. **OAuth token revocation:** engram-api endpoints (4A.2) plus engram_app "Connected apps" (4B) before the submissions. The
   earlier assumption that the mobile app already covered it was wrong: its token page only manages the `source = 'user'` token.
6. **Registry namespace:** branded `app.engramo/mcp` with DNS verification (section 5).

---

## Sources
- Claude submission guide: <https://claude.com/docs/connectors/building/submission>
- Anthropic directory FAQ: <https://support.claude.com/en/articles/11596036-anthropic-mcp-directory-faq>
- OpenAI Apps SDK submission: <https://developers.openai.com/apps-sdk/deploy/submission>
- MCP Registry publishing: <https://modelcontextprotocol.info/tools/registry/publishing/>
- Registry listing guide: <https://www.stackone.com/blog/mcp-registry-how-to-get-listed/>
- Antigravity custom MCP: <https://medium.com/google-developer-experts/google-antigravity-custom-mcp-server-integration-to-improve-vibe-coding-f92ddbc1c22d>
- Internal: `engram-ws/tdds/track3-mcp-chatgpt-app.md`, `docs/gcp_deployment_guide.md`,
  `engram-api/app/src/routes/oauth.rs`, `engram-api/service/src/oauth_service.rs`,
  `engram-api/db/src/repo/api_token/api_token_repo_pg.rs`, `engram_app/lib/presentation/screens/settings/api_token_page.dart`,
  `engramo-landing/src/pages/policy/privacy.astro`

---

## 10. Implementation progress (2026-10-13)

Nothing is pushed. engramo-mcp branches carry local commits; the other repos' changes are uncommitted on new branches made from `main`.

| Phase | Repo / branch | State | Verification |
|---|---|---|---|
| 0 OAuth spike | — | **Not started** (needs the user's browser logins) | — |
| 0b leftover mcpb commit | — | Done: no-op (already in #88) | `audit.mjs` passes on `main` |
| 1 (4.1, 4.2, 4.4, 4.5, error text) | engramo-mcp `feat/directory-readiness` | Committed locally (`ac6fa6f`, `7b10e89`) | gate green, 706 tests |
| 1b (4.3) + `/review` fixes | engramo-mcp `feat/session-open-token-check` (on top of 1) | 4.3 committed locally (`9514001`); two review passes' fixes uncommitted | gate green |
| 2 MCP Registry | engramo-mcp `feat/mcp-registry` (worktree `../engram-mcp-registry`) | Committed locally (`a56827d`) | gate green, 698 tests; `mcp-publisher validate` passed |
| A `/me` + connected apps + 4A.3 | engram-api `feat/me-and-connected-apps` | Uncommitted (includes 2 new `.sqlx` files) | 1863 passed, incl. DB integration tests |
| B Connected apps UI | engram_app `feat/connected-apps` | Uncommitted (7 goldens regenerated, unreviewed) | analyze clean, 1691 tests |
| 3 Privacy policy | engramo-landing `docs/privacy-ai-processors` | Uncommitted | `npm run build` passes |
| 6 Antigravity README | — | Already on `main` | — |

**Review-loop changes to 4.2/4.3 beyond the original design** (`.claude/.review-cache/`):
- **Unknown sessions:** an unknown or unbound session id now fails closed, with a 404 that matches rmcp's byte for byte.
- **Session limits:** at most 32 sessions per token, evicting that token's own stale bindings first, otherwise 429 + `Retry-After`.
- **Binding lifetime:** bindings last at least twice rmcp's keep-alive, which is now set explicitly.
- **Token check:** at most 32 `/me` probes run at once, with a 2 s wait for a slot. Still no cache.
- **Startup:** `MCP_PUBLIC_URL` is validated (https unless loopback, no userinfo).

**Open items for the user:**
- Phase 0 spike.
- DNS TXT record on `engramo.app` and the `mcp-registry` GitHub environment and secret (`docs/mcp-registry.md` on `feat/mcp-registry`).
- Confirm DPAs with Anthropic and Google before publishing the privacy policy. Identify the provider behind the standard TTS service (`engram-agent-tts`).
- Publish the privacy policy only after Phase B ships, since it refers to Settings → Connected apps.
- Annotation judgment calls: `deactivate_learning_path` is destructive, and the paid-AI tools are additive.
