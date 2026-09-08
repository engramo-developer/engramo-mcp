# Dependency maintenance — follow-up TODO

Backlog left over after triaging the first Dependabot run (PRs #5–#16, 2026-09-09).
The routine bumps landed on `main`; the items below need real work or a human decision
and were deliberately deferred.

| # | Item | Type | Priority |
|---|---|---|---|
| 1 | rmcp 1.8 → 3.x migration | rework (breaking API) | high |
| 2 | Verify `release.yml` after the GitHub Actions major bumps | verification | high (before next release) |
| 3 | Decide on Dependabot auto-merge for patch/minor | decision | low |
| 4 | Confirm the new Dependabot grouping/ignore config behaves on the next weekly run | verification | low |

---

## 1. rmcp 1.8 → 3.x migration

**Status:** Dependabot PR #16 (`rmcp 1.8.0 → 3.2.0`) was closed. `.github/dependabot.yml`
now ignores `rmcp` `version-update:semver-major`, so it will not re-open. `Cargo.toml`
still pins `rmcp = "1.3"` (resolves to 1.8.x).

**Why deferred:** two major versions of breaking API change; the Dependabot PR failed to
compile. This is a focused migration, not a version-bump merge.

### Known breaking changes (from the #16 CI failure)

- `rmcp::model::Content` — moved/renamed. Used in `CallToolResult::success(vec![rmcp::model::Content::text(...)])`.
- `CallToolResult` → `CallToolResponse` (the `ServerHandler` trait now resolves to `Result<CallToolResponse, ErrorData>`).
- `ReadResourceResult` → `ReadResourceResponse` (same pattern for the resource handler).
- `rmcp::model::PromptMessageRole` — unresolved import (renamed or relocated).
- `rmcp::model::AnnotateAble`, `rmcp::model::RawResource` — unresolved imports (relocated).
- `rmcp-macros` 3.x — check `#[tool_router]` / `#[tool]` / `#[prompt]` macro attribute changes.

### Work items

- [ ] Bump `Cargo.toml`: `rmcp = { version = "3", features = [...] }` (keep the current feature list:
      `server`, `macros`, `schemars`, `transport-io`, `transport-streamable-http-server`, `reqwest`).
- [ ] Update the shared result helpers first — `ok_json` / `err_result` in `src/tools/catalogs.rs`
      (~lines 45–65) — since every tool routes through them.
- [ ] Fix the handler signatures in `src/server.rs` (`CallToolResult`→`CallToolResponse`) and
      `src/resources/mod.rs` (`ReadResourceResult`→`ReadResourceResponse`).
- [ ] Fix prompt code in `src/prompts/mod.rs` (`PromptMessageRole`, `AnnotateAble`, `RawResource`).
- [ ] Sweep the remaining `rmcp`-touching files:
      `src/main.rs`, `src/http_auth.rs`, `src/error_reporting.rs`, `src/config.rs`,
      `src/tools/{cards,catalogs,search,media,generate,learning,learning_paths,ai}.rs`.
- [ ] Check `StreamableHttpService` wiring in `src/main.rs` — the `http` transport setup
      (`fallback_service`, session factory signature, `CURRENT_BEARER_TOKEN` task-local) may
      have shifted between 1.x and 3.x.
- [ ] Re-check the rich-text / `normalize_card_content` path in `src/server.rs` if `Content`
      construction changed.
- [ ] Run the full gate: `cargo fmt --all` · `cargo check` · `cargo clippy` · `cargo test`.
- [ ] Manual smoke test both transports (`stdio` against a real token; `http` + a `Bearer` request).
- [ ] Update `CLAUDE.md` — it says "rmcp 1.3" in the stack line and architecture notes.
- [ ] Once merged, remove the `rmcp` `ignore` block from `.github/dependabot.yml`.

Consider going straight to the latest 3.x rather than stepping through 2.x — there is no
partial value in landing on an intermediate major.

---

## 2. Verify `release.yml` after the GitHub Actions major bumps

**Status:** PR #18 merged, bumping (SHA-pinned):

| Action | From | To |
|---|---|---|
| `actions/checkout` | v4 | v7.0.1 |
| `actions/upload-artifact` | v4 | v7.0.1 |
| `actions/download-artifact` | v4 | v8.0.1 |
| `actions/setup-node` | v4 | v7.0.0 |
| `softprops/action-gh-release` | v2 | v3.0.3 |

`ci.yml` exercises `checkout` on every run, so that one is proven. The other four only
appear in `release.yml`, which **does not run on PRs or pushes** — it fires on `v*` tags.
So the upgraded artifact actions have never actually executed.

### Changelog notes (checked at bump time — nothing config-breaking found)

- `upload-artifact` v5–v7: Node 24 runtime (needs runner ≥ 2.327.1 — GitHub-hosted is fine);
  ESM migration; new opt-in `archive: false` (not used here — matrix uploads use unique `name`s).
- `download-artifact` v5–v8: Node 24 runtime; ESM; **hash mismatch now errors instead of warning**
  (`digest-mismatch` param to override); direct downloads check `Content-Type` before unzip.
  `merge-multiple: true` flow (used in `release.yml`) is unaffected in normal zipped-artifact use.

### Work items

- [ ] On the next release tag, watch the `build` → `npm-publish` jobs end-to-end.
- [ ] Confirm `download-artifact@v8` with `merge-multiple: true` still assembles all platform
      binaries (`engramo-mcp-{linux,darwin}-{arm64,x64}`) into one directory.
- [ ] Confirm `action-gh-release@v3` still attaches the renamed binaries and generates notes.
- [ ] If a release is not due soon, consider a throwaway pre-release tag to smoke-test first.

---

## 3. Decide on Dependabot auto-merge for patch/minor

**Status:** not implemented. Deliberately skipped during triage.

The idiomatic way to stop a backlog rebuilding is a `dependabot/fetch-metadata` +
`gh pr merge --auto` workflow that auto-merges `semver-patch` / `semver-minor` once the
required checks pass, leaving majors for a human.

Reasons it was not added:
- Needs repo `allow_auto_merge` **and** the workflow needs `contents: write` —
  the current maintainer token has `push` but not `admin`, so it can't be enabled from here.
- This repo has a deliberate-review posture (SHA-pinned actions, `cargo-audit` from source,
  minimal workflow tokens). Auto-merging deps cuts against that.
- Grouping (`cargo-minor-and-patch`, `github-actions`) already shrinks the queue to ~1–2
  PRs per week.

### Work items (only if we decide to do it)

- [ ] Enable "Allow auto-merge" on the repo.
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

Also in PR #17: `ci.yml` path filters were widened to `.github/workflows/**` and
`.github/dependabot.yml` so that workflow/config-only PRs trigger the `Test` / `Audit`
checks the `main` ruleset requires (previously such PRs were unmergeable without admin).

### Work items

- [ ] After the next weekly Dependabot run, confirm action updates arrive as a single
      grouped PR and no stray `npm` or `rmcp`-major PRs appear.
- [ ] Confirm a workflow-only Dependabot PR now shows the `Test` / `Audit` checks and is
      mergeable without admin.
