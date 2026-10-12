# TDD: List engramo-mcp in public MCP directories (Claude, ChatGPT, MCP Registry, Antigravity)

> **Status:** DRAFT for review, 2026-10-13. Nothing in it has been implemented yet.
> **Goal:** EngrAmo installs in one click with OAuth from Claude (web, desktop, mobile, Code), ChatGPT, and
> MCP-aware dev tools (VS Code/Copilot, Cursor, Antigravity), and shows up in their official directories.
> **Origin:** a first-pass plan drafted outside this repo ("Let's prepare comprehensive plan to do this step b….md").
> We checked it against this codebase, engram-api, the live prod endpoints, and the directories' current
> documentation. Section 2 lists what changed.

---

## 1. Decisions already made

| Question | Decision |
|---|---|
| Has engram-api's OAuth authorization server (Track 3 Phase 3) had its security review? | **Yes, it's done.** We treat OAuth as production-ready, and this TDD only closes the remaining gaps. |
| Scope of the first execution wave | **All platforms, in order:** code gaps → MCP Registry → Claude directory → ChatGPT directory → Antigravity. |
| ChatGPT integration style | **Native MCP app** (Apps SDK / app directory). We are **not** building an OpenAPI gateway or a Custom GPT. |
| Branching | Feature branches and PRs only, never `main`. Releases stay user-driven (`/release` runs separately). |

---

## 2. Current state vs. the original draft plan

The original draft assumed everything still had to be built. Most of the infrastructure already exists.

| Original draft said | Verified reality |
|---|---|
| **Phase 1.1:** implement dynamic per-request auth instead of a startup env token | **Done** (Track 3 Phase 1). `engramo-mcp http` is multi-tenant. `bearer_auth_middleware` (`src/http_auth.rs`) scopes each session's bearer into `CURRENT_BEARER_TOKEN`; the session factory in `build_app` (`src/main.rs`) builds one `EngramoClient` per MCP session; `SessionTokens` binds each session id to the token that opened it. |
| The endpoint is `/mcp` | It is served at **root** `/` (rmcp `StreamableHttpService` via `fallback_service`). Prod is `https://mcp.engramo.app/` and dev is `https://mcp-engramo.volmyr.com/` (`docs/gcp_deployment_guide.md`). |
| **Phase 1.2:** provide OAuth 2.0 endpoints / DCR | **Done and reviewed** (Track 3 Phase 3, `engram-ws/tdds/track3-mcp-chatgpt-app.md`). engram-api is the authorization server: `/.well-known/oauth-authorization-server`, `/oauth/register` (DCR), `/oauth/authorize` (Supabase login bridge page), `/oauth/token`, PKCE S256 only, public clients (`token_endpoint_auth_methods_supported: ["none"]`). engramo-mcp is the resource server and serves `/.well-known/oauth-protected-resource` (`src/well_known.rs`). |
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

### 2.2 Other relevant facts found during research

- **Redirect URIs:** `engram-api/service/src/oauth_service.rs::validate_redirect_uri_format` accepts any
  `https://` URI and loopback `http://` URIs at DCR time. Claude's callback (`https://claude.ai/api/mcp/auth_callback`) and
  ChatGPT's (a `chatgpt.com` https URL) therefore both register without an allowlist change.
- **Token model:** each OAuth access token is a minted 90-day `engram_` PAT, and there is **no refresh token**. When it expires the
  user must redo OAuth. Clients only do that when the server answers with HTTP 401. See 4.3.
- **Tool annotations:** `grep` finds **no** `annotations(...)` on any `#[tool(...)]` in the crate. Counts per file:
  `server.rs` 29, `ai.rs` 5, `cards.rs` 4, `learning.rs` 4, `learning_paths.rs` 5, `media.rs` 2, `tts.rs` 2. Both
  Claude's and OpenAI's reviews require annotations.
- **MCPB:** the Claude directory **no longer accepts local servers packaged as MCPB** (desktop-extension listings
  are deprecated, and local servers now go into the directory only inside a *plugin*). The `.mcpb` from the `feat/mcpb-bundle`
  branch is still worth shipping. It's a GitHub-release download, and the MCP Registry accepts it as a package type.
- **Known gap carried over from Track 3:** OAuth-minted PATs are not visible or revocable in the web-app dashboard. Reviewers
  may ask how users revoke access. That work belongs to engram-api and engram-web-app and is out of scope here, but it should be flagged.

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
- Automated tool scan. Tool annotations need a justification, and tool names should be verb-style and unique.
- Assets: a square logo (48×48 minimum), a dark variant (recommended), brand colors, display name (30 chars max), subtitle (30 max), long
  description (4000 max), category, and up to 20 capabilities.
- **Website, support, privacy policy, and Terms of Service URLs are all required.**
- **5 positive and 3 negative test cases**, a demo video, and a test account with sample data. The account must not need MFA, email codes, or a private network.
- Policy points: the app must be first-party, not a pass-through relay, and it **must not sell subscriptions inside the app**.

### 3.3 Official MCP Registry
Sources: <https://modelcontextprotocol.info/tools/registry/publishing/>,
<https://www.stackone.com/blog/mcp-registry-how-to-get-listed/>

- Publish with the `mcp-publisher` CLI (`init`, `login`, `validate`, `publish`) and a `server.json`.
- Namespace auth: `io.github.<owner>/…` through GitHub login or **GitHub OIDC in Actions** (no secret needed), or a
  reverse-DNS namespace through DNS or HTTP verification.
- `remotes[]` lists remote endpoints, and the URL itself is the proof of ownership.
- `packages[]` covers npm and others. An npm package needs `"mcpName": "<server name>"` in its `package.json`.
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

Everything below goes on a single branch, for example `feat/directory-readiness`. The `/review` loop runs before the PR is opened.

### 4.0 Prerequisite: land `feat/mcpb-bundle`
That branch has uncommitted work:
- `release.yml` hardening: semver validation, a locked `mcpb` CLI, `npm audit`, a SHA-256 file, and a separate
  `mcpb-attest` provenance job.
- The README verification note.
- The manifest `default: ""`.
- The `mcpb_manifest_matches_env_contract` test in `src/config.rs`.

Finish it, run the gate, and open its PR first. Phase 2's registry entry depends on the `.mcpb` and `.sha256` release assets.

### 4.1 Tool annotations (hard requirement for both directories)
- Add `annotations(...)` to every `#[tool(...)]` in `src/server.rs` and `src/tools/{cards,learning,learning_paths,media,ai,tts}.rs`:
  - `title`: a human-readable name, for example "List catalogs".
  - `read_only_hint`: `true` for tools that only read.
  - `destructive_hint`: `true` for delete and archive tools. Set it to `false` explicitly for non-destructive writes.
  - `idempotent_hint`: where it is meaningful, for example `update_*`.
  - `open_world_hint: false`. The tools only touch the user's own EngrAmo account. `ai.rs` and local TTS call external
    AI providers, so they are a per-tool judgment call.
- Starting classification, to confirm per tool during implementation:

| Class | Tools (pattern) | Hints |
|---|---|---|
| Read | `list_*`, `get_*`, `search*`, due-cards, `list_tts_voices`, `get_server_version` | `read_only_hint = true` |
| Destructive write | `delete_*`, including `delete_catalog`'s archive path (see #81) | `destructive_hint = true` |
| Additive/updating write | `create_*`, `update_*`, `add_*_to_learning`, `generate_*` (whatever it persists), `generate_card_audio` | `read_only_hint = false, destructive_hint = false` |
| Paid AI (`ai.rs`) | TTS, translate, dictionary, agent chat, `translate_batch_import` | Classify each tool by whether it persists anything |

- **Regression test** in `src/server.rs` tests: build the server with paid AI and local TTS enabled, so all routers
  are summed. For each tool in `list_all()`, assert that `annotations.title` is set and that at least one of
  `read_only_hint`/`destructive_hint` is `Some`. This way a future tool can't ship without annotations.
- While editing, check tool names and descriptions against review criteria: verb-style, unique, accurate, and with no
  instructions aimed at the model in descriptions (the prompt-injection policy).

### 4.2 `WWW-Authenticate` on 401
MCP's authorization spec expects a 401 from the resource server to carry
`WWW-Authenticate: Bearer resource_metadata="<url of .well-known/oauth-protected-resource>"`. Clients use it to start discovery.
Some clients fall back to probing `.well-known` themselves, but relying on that is fragile.

- `bearer_auth_middleware` (`src/http_auth.rs`, around line 186) has two 401 paths: a missing or malformed bearer, and a
  session/token mismatch. Both should send the header.
- The middleware state is currently `SessionTokens`. Replace it with a small `AuthState { sessions, resource_metadata_url }`, or
  something similar. Build the URL in `build_app` (`src/main.rs`) from the same value used for
  `ProtectedResourceState.resource` (`MCP_PUBLIC_URL`, falling back to the localhost placeholder).
- Tests in `http_auth.rs`: both 401 paths send the header with the exact URL. The existing 401 tests keep passing.

### 4.3 Expired or revoked token → HTTP 401 at session open
- **Problem:** OAuth tokens are 90-day PATs with no refresh. Today any bearer that isn't empty opens a session, and a dead
  token only fails later inside a tool call as `is_error: true`. Claude and ChatGPT only re-run OAuth on an HTTP 401, so
  after 90 days a user silently gets errors on every call.
- **Design:** when a request has **no `Mcp-Session-Id`** (it is opening a session), validate the bearer with one cheap
  engram-api call before passing the request to rmcp:
  - Reuse an existing lightweight authenticated endpoint already wrapped in `client.rs`, such as the subscription
    resource. Use the shared `HardenedClient`.
  - On `ApiError` unauthorized or forbidden → 401 with the 4.2 header.
  - On network or 5xx errors → 502 or 503, **not** 401, so a transient API outage doesn't force a re-login.
- Cost: one extra API call per session, not per request. Requests on an existing session are unchanged.
- Security: the token is never logged. The rule against logging the session id is unchanged.
- Tests: wiremock covering valid → session opens, 401 from the API → 401 with the header, 500 from the API → 503, and a request with a
  session id → no validation call.
- Open question for review: is "every session open costs an API call" acceptable, or should we cache positive results
  (token hash → expiry, for example 5 min)? The proposal is no cache at first, to keep it simple.

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
- Confirm that prod Cloud Run has `ENGRAMO_ENABLE_PAID_AI` **off**. If it's on, make sure no tool sells or upsells a subscription
  in-app, per OpenAI policy. The `engramo://subscription` resource is read-only info, which is fine.
- Local TTS is already stdio-only by design (see CLAUDE.md), so it is never exposed on the public endpoint.

### 4.6 Not changing
- OAuth server logic in engram-api: reviewed, and DCR plus https redirects already cover Claude and ChatGPT.
- Endpoint path: stays at `/`. Directories accept any https URL. *Check during the ChatGPT submission:* some guides say the URL "ends
  with /mcp". If OpenAI insists, add `/mcp` as an alias route instead of moving root.

---

## 5. Design: MCP Registry publication

- **Name:** `io.github.engramo-developer/engramo-mcp`. GitHub OIDC from Actions means no new secret. A reverse-DNS
  name (`app.engramo/…`) would need DNS verification and a signing key held as a secret, which we're avoiding.
- **New `server.json`** (repo root), with schema-pinned `version` placeholder `0.0.0` like the other manifests:
  - `remotes`: `[{ "type": "streamable-http", "url": "https://mcp.engramo.app/" }]`.
  - `packages`:
    - npm `@engramo/mcp`, stdio transport. Env vars: `ENGRAMO_API_TOKEN` (required, secret),
      `ENGRAMO_API_URL` (default `https://api.engramo.app`), `ENGRAMO_TTS_GEMINI_API_KEYS` (optional, secret).
    - mcpb: `https://github.com/engramo-developer/engramo-mcp/releases/download/v<ver>/engramo.mcpb` with
      `fileSha256`.
- `npm/engramo-mcp/package.json`: add `"mcpName": "io.github.engramo-developer/engramo-mcp"`.
- **`release.yml`:** add a new job `mcp-registry`:
  - `needs: [resolve, github-release, npm-publish]`, skipped when `dry_run == 'true'`.
  - `permissions: { contents: read, id-token: write }`.
  - Download the release's `engramo.mcpb.sha256`, then rewrite `version` and `fileSha256` into a staged `server.json` with
    `process.env` (same pattern as the hardened manifest rewrite).
  - Install `mcp-publisher` pinned to a version **and checksum**, matching the repo's pinning discipline.
  - Run `mcp-publisher validate`, `mcp-publisher login github-oidc`, and `mcp-publisher publish`.
  - It keeps no other secrets and never shares a job with `NPM_TOKEN`.
- **Test** in the same style as `mcpb_manifest_matches_env_contract`: the env var names in `server.json` match `McpConfig` and
  `tts::ENV_KEYS`, and secret fields are marked secret. The http-only and paid-AI vars must be absent.
- **Aggregators**, after the first publish:
  - Register `https://mcp.engramo.app/` on Smithery (UI).
  - Claim the Glama listing.
  - Check that PulseMCP picks it up from the registry.

---

## 6. Rollout: submissions (manual and human-driven)

### 6.1 Shared prerequisites
- A **prod reviewer test account** with email and password login through the Supabase bridge and no MFA. Populate it with several
  catalogs, cards, due cards, and a learning path.
- **Public pages on engramo.app:**
  - A docs or how-to-connect page.
  - Privacy policy. It must list Anthropic and OpenAI as sub-processors; Track 3 already added OpenAI.
  - **Terms of Service.** Confirm it exists, since OpenAI requires it.
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
   - Data handling: our own first-party API.
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
5. Record the demo video, fill in the metadata, and submit. Iterate on the email feedback, then publish when approved.

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
| 0 | Land `feat/mcpb-bundle` PR | engramo-mcp / agent + user merge | — |
| 1 | 4.1–4.5 on `feat/directory-readiness` → `/review` → PR | engramo-mcp / agent | 0 |
| 1r | `/release`, deploy dev → prod, set `MCP_PUBLIC_URL` (already set) | user | 1 |
| 2 | `server.json`, `mcpName`, registry job, test → PR → next release publishes | engramo-mcp / agent; user releases | 1r |
| 3 | Test account, legal pages, assets | user / engramo.app | — (parallel) |
| 4 | Claude directory submission | user | 1r, 3 |
| 5 | ChatGPT verification, challenge env, submission | user (+ env change in 4.4) | 1r, 3 |
| 6 | Antigravity README section + outreach | agent (docs), user (outreach) | 1r, 2 |

---

## 8. Verification

- **Gate after every code change:** `cargo fmt --all && cargo check && cargo clippy && cargo test`, plus `cargo sort` if
  deps change.
- **Local http mode against dev:**
  `ENGRAMO_API_URL=https://api-engram.volmyr.com MCP_PUBLIC_URL=http://localhost:8080 cargo run -- http`, then:
  - `curl -i -X POST localhost:8080/` → `401` with
    `WWW-Authenticate: Bearer resource_metadata="http://localhost:8080/.well-known/oauth-protected-resource"`.
  - `initialize` with a bogus bearer → `401` with the header.
  - `initialize` with a valid dev `engram_` token, then `tools/list` → every tool has `annotations.title` and a hint.
  - `curl localhost:8080/.well-known/openai-apps-challenge` → 404 when unset, and the token when `OPENAI_APPS_CHALLENGE` is set.
- **MCP Inspector against dev** (`https://mcp-engramo.volmyr.com/`): the full OAuth flow starts from the 401, and every tool
  can be called.
- **Registry:** run `mcp-publisher validate server.json` locally. After the release, check
  `curl "https://registry.modelcontextprotocol.io/v0/servers?search=engramo"`.
- **End-to-end on each platform** (claude.ai custom connector, ChatGPT developer mode, Antigravity config): "make me a deck of 5
  Spanish verbs", then confirm the cards land in the test account at `study.engramo.app`.
- **Token expiry path:** revoke or expire the PAT in the DB on dev. The next session open should get a 401, and the client should re-run OAuth.

---

## 9. Open questions for review

1. 4.3: validate at session open with no cache (proposed), or cache positive results for a few minutes?
2. 4.3: which existing engram-api endpoint is the cheapest authenticated probe? Is there a `/me`, or is subscription good enough?
3. Does engramo.app have a Terms of Service page, and does the privacy policy name Anthropic as a sub-processor?
4. Should the prod MCP endpoint ever enable paid AI? If so, re-check OpenAI's no-in-app-commerce rule.
5. OAuth PAT revocation in the dashboard (the Track 3 known gap): do we need it before directory review, or is it a follow-up?
6. Registry namespace: is `io.github.engramo-developer/engramo-mcp` acceptable, or do we want the branded `app.engramo/…`, which
   needs DNS verification and a key secret?

---

## Sources
- Claude submission guide: <https://claude.com/docs/connectors/building/submission>
- Anthropic directory FAQ: <https://support.claude.com/en/articles/11596036-anthropic-mcp-directory-faq>
- OpenAI Apps SDK submission: <https://developers.openai.com/apps-sdk/deploy/submission>
- MCP Registry publishing: <https://modelcontextprotocol.info/tools/registry/publishing/>
- Registry listing guide: <https://www.stackone.com/blog/mcp-registry-how-to-get-listed/>
- Antigravity custom MCP: <https://medium.com/google-developer-experts/google-antigravity-custom-mcp-server-integration-to-improve-vibe-coding-f92ddbc1c22d>
- Internal: `engram-ws/tdds/track3-mcp-chatgpt-app.md`, `docs/gcp_deployment_guide.md`,
  `engram-api/app/src/routes/oauth.rs`, `engram-api/service/src/oauth_service.rs`
