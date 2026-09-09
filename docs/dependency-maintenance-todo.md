# Dependency maintenance — follow-up TODO

Backlog left over after triaging the first Dependabot run (PRs #5–#16, 2026-09-09).
The routine bumps landed on `main`; the items below need real work or a human decision
and were deliberately deferred. Updated 2026-09-09 during PR #19 (Phase A of the
maintenance rollout).

| # | Item | Type | Priority | Status |
|---|---|---|---|---|
| 1 | rmcp 1.8 → 3.x migration | rework (breaking API) | high | **closed — done** |
| 2 | Verify `release.yml` after the GitHub Actions major bumps | verification | high (before next release) | static audit DONE; dry-run path added — open until exercised |
| 3 | Decide on Dependabot auto-merge for patch/minor | decision | low | **closed — no** |
| 4 | Confirm the new Dependabot grouping/ignore config behaves on the next weekly run | verification | low | open — waiting on next weekly run |

Also fixed in this pass, found while surveying (not one of the original four items):
`Cargo.toml` said `version = "1.1.1"` while the latest tagged release is `v1.1.2`
(2026-09-07) — `git show v1.1.2:Cargo.toml` shows it was already stale when tagged.
`release.yml` derives the published version from `GITHUB_REF_NAME`, not from
`Cargo.toml`, so nothing shipped wrong; the manifest just misreported its own
version. Bumped to `1.1.2` to match.

---

## 1. rmcp 1.8 → 3.x migration — CLOSED

Done: `rmcp` is on **3.2**. The migration came to six mechanical changes, not the
sweeping rewrite the original entry here implied — that list had been transcribed from
PR #16's CI failure and was wrong for 3.2 (`CallToolResult` / `ReadResourceResult` are
not renamed; new `*Response` enums wrap them to carry an MRTR `InputRequired` variant
this server never produces, and macro-routed `#[tool]` methods were unaffected
throughout). See the migrating PR for the full account.

Two consequences worth carrying forward:

- **The `rmcp` `version-update:semver-major` ignore has been removed from
  `.github/dependabot.yml`**, so rmcp majors will start arriving as PRs again. That is
  intended — it existed only because majors were broken. It also means item 4's "no
  stray rmcp-major PRs" check no longer applies.
- The server's advertised identity was wrong and is now fixed. `ServerInfo::new`
  defaults `server_info` to `Implementation::from_build_env()`, whose `env!` macros
  expand when *rmcp* is compiled, so clients displayed this server as `rmcp` rather
  than `engramo-mcp`. Long-standing, unrelated to the bump; the changed version string
  is just what surfaced it. A regression test now guards it.

---

## 2. Verify `release.yml` after the GitHub Actions major bumps

**Status:** static audit **DONE** (PR #19). `release.yml` now also has a `workflow_dispatch`
dry-run path (this pass): a `resolve` job derives `version`/`dry_run` instead of every job
reading `GITHUB_REF_NAME` directly, and on a dry run the `github-release` job creates a
deletable draft release (tagged `dry-run-<version>-<run id>`, never `v<version>` —
`action-gh-release` upserts by tag, so a real version tag would edit the existing
published release instead of creating a draft) and
`npm-publish` runs `npm publish --dry-run` (no `--provenance`) instead of publishing for
real — so `build` → `github-release` / `npm-publish` can now be exercised end-to-end,
including `download-artifact@v8` + `merge-multiple: true` (checked by a new verification
step) and `action-gh-release@v3`'s asset upload, without shipping anything. A real tag push
is unaffected: `dry_run` resolves to `false` there and every step keeps its exact prior
inputs. What's still open is actually *running* the dry-run dispatch once and, separately,
watching a real tag release end-to-end — see work items below.

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

- [ ] Run `workflow_dispatch` with `dry_run: true` once and confirm: the verification step
      after `download-artifact@v8` reports all four binaries present and non-empty, a draft
      GitHub release is created with the four assets attached, and `npm-publish` logs a
      `--dry-run` publish (no `--provenance`) for all five packages. Delete the draft
      release afterward — dry runs don't clean up after themselves; it's the one
      titled "DRY RUN ... delete me".
- [ ] On the next real release tag, watch `build` → `github-release` / `npm-publish`
      end-to-end too — the dry run proves the mechanics, but a real tag is still the only
      way to confirm the actual publish (npm registry, non-draft GitHub release) succeeds.

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
- `rmcp` `version-update:semver-major` was ignored. **No longer** — the ignore was
  removed once the 3.x migration landed (item 1), so rmcp majors arrive as PRs again.
- The `npm` ecosystem entry was removed entirely — `npm/engramo-mcp` has no third-party
  deps; its `@engramo/mcp-*` `optionalDependencies` are `0.0.0` placeholders that the
  `npm-publish` job in `release.yml` rewrites to the release tag at publish time.

What's left to verify is genuinely time-gated on the scheduler — it can only be checked
once the next weekly Dependabot run actually happens, not by static inspection:

### Work items

- [ ] After the next weekly Dependabot run, confirm action updates arrive as a single
      grouped PR and that no `npm`-ecosystem PRs appear. An rmcp-major PR is now
      expected rather than stray, since item 1 removed that ignore.

The other half of this item — confirming a workflow/docs/config-only Dependabot PR shows
the `Test` / `Audit` checks and is mergeable without admin — is no longer open: PR #17
already settled it for workflow-only PRs (widened `ci.yml` paths to
`.github/workflows/**` and `.github/dependabot.yml`), and this pass (PR #19) settled it
for docs/markdown-only PRs by adding `docs/**` and `**.md` to the same `paths:` filters
in `.github/workflows/ci.yml` — which is exactly what unblocked PR #19 itself.
