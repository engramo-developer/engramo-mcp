# Dependency maintenance — follow-up TODO

Backlog left over after triaging the first Dependabot run (PRs #5–#16, 2026-09-09).
The routine bumps landed on `main`; the items below need real work or a human decision
and were deliberately deferred. Updated 2026-09-09 during PR #19 (Phase A of the
maintenance rollout).

| # | Item | Type | Priority | Status |
|---|---|---|---|---|
| 1 | rmcp 1.8 → 3.x migration | rework (breaking API) | high | open — on a separate branch/PR |
| 2 | Verify `release.yml` after the GitHub Actions major bumps | verification | high (before next release) | static audit DONE; live run still gated on a real tag |
| 3 | Decide on Dependabot auto-merge for patch/minor | decision | low | **closed — no** |
| 4 | Confirm the new Dependabot grouping/ignore config behaves on the next weekly run | verification | low | open — waiting on next weekly run |

Also fixed in this pass, found while surveying (not one of the original four items):
`Cargo.toml` said `version = "1.1.1"` while the latest tagged release is `v1.1.2`
(2026-09-07) — `git show v1.1.2:Cargo.toml` shows it was already stale when tagged.
`release.yml` derives the published version from `GITHUB_REF_NAME`, not from
`Cargo.toml`, so nothing shipped wrong; the manifest just misreported its own
version. Bumped to `1.1.2` to match.

---

## 1. rmcp 1.8 → 3.x migration

**Status:** open, being done on a separate branch with its own PR. This section only
corrects the map of what actually changes — the earlier version of this doc was
transcribed from PR #16's CI failure log and does not match rmcp 3.2.

**Why deferred:** two major versions of breaking API change; the Dependabot PR failed to
compile. This is a focused migration, not a version-bump merge.

### Corrected breaking-change map

Verified against the official
[Migrating to 3.0.0 guide](https://github.com/modelcontextprotocol/rust-sdk/discussions/969)
and docs.rs for rmcp 3.2.0.

The old list here claimed `CallToolResult` → `CallToolResponse`, `ReadResourceResult` →
`ReadResourceResponse`, and that `PaginatedRequestParams` had moved. That's wrong:

- `CallToolResult`, `ReadResourceResult` and `PaginatedRequestParams` **all still exist**.
  The crate already uses the plural `*RequestParams` spellings, so nothing to do there.
- `CallToolResponse` is not a rename — it's a **new** enum (`Complete` | `InputRequired`)
  that **wraps** the still-existing `CallToolResult`. Same shape of relationship for
  `ReadResourceResponse` wrapping `ReadResourceResult`, and `GetPromptResponse` wrapping
  `GetPromptResult`.
- `#[tool_router]` / `#[tool]` macro users are **unaffected** — all 43 `#[tool]` methods
  and their `CallToolResult` returns are unchanged by the 3.x bump.

The real surface to migrate:

1. `rmcp::model::Content` → `ContentBlock`. Only 4 sites, all in `src/tools/catalogs.rs`
   (`ok_json` / `ok_text` / `err_result`, lines 45/46/53/65).
2. Three hand-written `ServerHandler` methods in `src/server.rs` need widened return types
   plus `.into()` at the return site: `call_tool` → `CallToolResponse`, `read_resource` →
   `ReadResourceResponse`, `get_prompt` → `GetPromptResponse`. Note `EngramMcpServer` has
   **no** `#[tool_handler]` — its `call_tool` is hand-rolled — so it does not get the
   macro's free ride the way `#[tool]`-routed methods do.
3. `AnnotateAble` / `RawResource` are removed → `Annotations` + direct `Resource`
   construction in `src/resources/mod.rs`.
4. `PromptMessageRole` / `PromptMessageContent` / `GetPromptResult` relocated, affecting
   the hand-rolled `src/prompts/mod.rs`.
5. `StreamableHttpService<S, M>`'s bound moved from `S: Service<RoleServer>` to
   `S: ServerHandler`, and `StreamableHttpServerConfig::stateful_mode` was renamed to
   `legacy_session_mode`.
6. MSRV is now Rust 1.88.

### Risk to watch

`main.rs`'s `DEFAULT_LOG_FILTER` hardcodes the module path
`rmcp::transport::streamable_http_server::session` to silence session-id logging. If that
module path moved in 3.x, session ids start reaching the logs unfiltered — CLAUDE.md
treats a session id as credential-equivalent, so this needs an explicit check during the
migration, not just "does it compile."

### Work items

- [ ] Bump `Cargo.toml`: `rmcp = { version = "3", features = [...] }` (keep the current feature list:
      `server`, `macros`, `schemars`, `transport-io`, `transport-streamable-http-server`, `reqwest`).
- [ ] Update the shared result helpers first — `ok_json` / `err_result` in `src/tools/catalogs.rs`
      (~lines 45–65) — since every tool routes through them. `Content` → `ContentBlock`.
- [ ] Fix the three hand-rolled `ServerHandler` methods in `src/server.rs`
      (`call_tool`/`read_resource`/`get_prompt` → the `*Response` wrapper types, `.into()`
      at each return).
- [ ] Fix `src/resources/mod.rs` (`AnnotateAble`/`RawResource` → `Annotations` + `Resource`).
- [ ] Fix prompt code in `src/prompts/mod.rs` (`PromptMessageRole`, `PromptMessageContent`,
      `GetPromptResult` relocations).
- [ ] Sweep the remaining `rmcp`-touching files:
      `src/main.rs`, `src/http_auth.rs`, `src/error_reporting.rs`, `src/config.rs`,
      `src/tools/{cards,catalogs,search,media,generate,learning,learning_paths,ai}.rs`.
- [ ] Check `StreamableHttpService` wiring in `src/main.rs` — `S: ServerHandler` bound,
      `legacy_session_mode` rename, session factory signature, `CURRENT_BEARER_TOKEN`
      task-local.
- [ ] Confirm `DEFAULT_LOG_FILTER`'s `rmcp::transport::streamable_http_server::session`
      path still matches in 3.x (see "Risk to watch" above); update it if the module moved.
- [ ] Re-check the rich-text / `normalize_card_content` path in `src/server.rs` for the
      `ContentBlock` rename.
- [ ] Run the full gate: `cargo fmt --all` · `cargo check` · `cargo clippy` · `cargo test`.
- [ ] Manual smoke test both transports (`stdio` against a real token; `http` + a `Bearer` request).
- [ ] Update `CLAUDE.md` — it says "rmcp 1.3" in the stack line and architecture notes.
- [ ] Once merged, remove the `rmcp` `ignore` block from `.github/dependabot.yml`.

Consider going straight to the latest 3.x rather than stepping through 2.x — there is no
partial value in landing on an intermediate major.

---

## 2. Verify `release.yml` after the GitHub Actions major bumps

**Status:** static audit **DONE** (PR #19). A live end-to-end run is still out of scope —
it would publish `@engramo/mcp` to npm irreversibly, `release.yml` has no
`workflow_dispatch`, and it triggers only on `push: tags: ['v*']`, so there is no way to
dry-run the `build` → `github-release` / `npm-publish` chain short of pushing a real tag.
That is the one line still open below.

PR #18 bumped these four release-only actions (SHA-pinned):

| Action | Pin claims | Pin honest? | Inputs still valid? | Evidence |
|---|---|---|---|---|
| `actions/upload-artifact` | v7.0.1 | PASS | PASS (`name`, `path` both present) | `git/ref/tags/v7.0.1` → commit `043fb46d…`, matches the pin directly (lightweight tag) |
| `actions/download-artifact` | v8.0.1 | PASS | PASS (`merge-multiple` present) | `git/ref/tags/v8.0.1` → commit `3e5f45b2…`, matches the pin directly |
| `actions/setup-node` | v7.0.0 | PASS | PASS (`node-version`, `registry-url` both present) | `git/ref/tags/v7.0.0` → commit `82076278…`, matches the pin directly |
| `softprops/action-gh-release` | v3.0.3 | PASS | PASS (`generate_release_notes`, `files` both present) | `v3.0.3` is an **annotated** tag → `git/ref` returns tag object `e598afbe…`, dereferenced via `git/tags/e598afbe…` → commit `efb35369…`, matches the pin |

`ci.yml` exercises `checkout` on every run, so that one was already proven; this audit
covers the four that only appear in `release.yml`.

### The two documented behavior changes, re-checked against the actual `action.yml` at each pinned SHA

- **`download-artifact` digest handling.** Confirmed: `digest-mismatch` defaults to
  `'error'` in v8 (checked directly in the action's `inputs:` block at the pinned SHA) —
  i.e. the stricter behavior (fail instead of warn) is what you get with **no**
  configuration, and *loosening* it back to v4's warn-only behavior is what would require
  an explicit opt-in (`digest-mismatch: warn`), not the other way around. Both
  `github-release` and `npm-publish` pass only `merge-multiple: true` — no `path`,
  `pattern`, or `name` — which per the schema means "download every artifact for the run,
  flattened into one directory" (`path` defaults to `$GITHUB_WORKSPACE`), matching the
  original intent. Since these are same-run artifacts downloaded once, immediately after
  upload, a digest mismatch is not a realistic risk here — PASS, no config change needed.
- **`upload-artifact` v7 `archive: false`.** Confirmed present in the schema, default
  `'true'` (i.e. still archives/zips by default — unchanged behavior unless you opt in).
  Not used in `release.yml`. Each matrix leg passes a distinct `name:`
  (`engramo-mcp-{linux,darwin}-{x64,arm64}`), so there is no artifact-name collision
  regardless. PASS.

No incompatibility found — `release.yml` required **no code fix** as a result of this
audit.

`actionlint` is not installed in this environment (`which actionlint` → not found) and
was not installed for this pass, per instructions.

### Work items

- [ ] On the next release tag, watch the `build` → `github-release` / `npm-publish` jobs
      end-to-end — this is the only way to verify these four actions live, since there's
      no `workflow_dispatch` to dry-run them.
- [ ] Confirm `download-artifact@v8` with `merge-multiple: true` still assembles all
      platform binaries (`engramo-mcp-{linux,darwin}-{arm64,x64}`) into one directory.
- [ ] Confirm `action-gh-release@v3` still attaches the renamed binaries and generates notes.
- [ ] If a release is not due soon, consider a throwaway pre-release tag to smoke-test first.

---

## 3. Decide on Dependabot auto-merge for patch/minor

**Status: closed — decided no.** No auto-merge workflow will be added.

Reasons (as originally triaged, plus the blocking fact confirmed in this pass):
- This repo has a deliberate-review posture (SHA-pinned actions, `cargo-audit` from source,
  minimal workflow tokens). Auto-merging deps cuts against that.
- The auto-merge workflow would need `contents: write` in a job that would sit alongside
  (or need to be trusted the same as) the one holding `NPM_TOKEN` — more privilege in more
  automated paths than this repo wants.
- Grouping (`cargo-minor-and-patch`, `github-actions`) already shrinks the queue to ~1–2
  PRs per week, so the backlog problem auto-merge would solve barely exists here.
- **Blocking, confirmed in this pass:** the repo has `allow_auto_merge: false` and the
  maintainer token has `admin: false`. Even if the decision above were reversed, auto-merge
  could not be enabled from the CLI as things stand — enabling it requires repo-admin
  access this token doesn't have.

### Appendix — implementation recipe, kept for a future reversal

If this is ever revisited, the idiomatic approach is a `dependabot/fetch-metadata` +
`gh pr merge --auto` workflow that auto-merges `semver-patch` / `semver-minor` once the
required checks pass, leaving majors for a human:

- [ ] Enable "Allow auto-merge" on the repo (needs admin — not available to the current
      maintainer token).
- [ ] Add `.github/workflows/dependabot-automerge.yml`: `if: github.actor == 'dependabot[bot]'`,
      `dependabot/fetch-metadata` (SHA-pinned), `gh pr merge --auto --squash` gated on
      `update-type` being patch/minor.
- [ ] Keep majors manual.

---

## 4. Confirm the new Dependabot config on the next weekly run

`.github/dependabot.yml` was changed in PR #17:

- `github-actions` updates are now grouped into one PR (`groups: github-actions: patterns: ["*"]`).
- `rmcp` `version-update:semver-major` is ignored.
- The `npm` ecosystem entry was removed entirely — `npm/engramo-mcp` has no third-party
  deps; its `@engramo/mcp-*` `optionalDependencies` are `0.0.0` placeholders that the
  `npm-publish` job in `release.yml` rewrites to the release tag at publish time.

What's left to verify is genuinely time-gated on the scheduler — it can only be checked
once the next weekly Dependabot run actually happens, not by static inspection:

### Work items

- [ ] After the next weekly Dependabot run, confirm action updates arrive as a single
      grouped PR and no stray `npm` or `rmcp`-major PRs appear.

The other half of this item — confirming a workflow/docs/config-only Dependabot PR shows
the `Test` / `Audit` checks and is mergeable without admin — is no longer open: PR #17
already settled it for workflow-only PRs (widened `ci.yml` paths to
`.github/workflows/**` and `.github/dependabot.yml`), and this pass (PR #19) settled it
for docs/markdown-only PRs by adding `docs/**` and `**.md` to the same `paths:` filters
in `.github/workflows/ci.yml` — which is exactly what unblocked PR #19 itself.
