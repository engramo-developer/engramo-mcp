Run a review→fix loop on $ARGUMENTS (or, if no target is given, the Rust files changed on this branch).

You are the orchestrator running in the main conversation. You are the **only** place that can spawn subagents in
parallel — domain reviewers cannot spawn each other. Follow this loop exactly.

This one command replaces the former `/security`, `/coverage`, and `/test` commands: security and coverage run as
parallel domain reviewers on every pass, and missing tests are written by `@code-implementator` using the
`test-generation` skill.

| Role | Agent | Model | Skill it applies |
|---|---|---|---|
| Rust / MCP conventions reviewer | `@review-rust` | sonnet | `rust-code-review` |
| Security reviewer | `@review-security` | sonnet | `security-audit` |
| Coverage reviewer (skipped on final pass) | `@review-coverage` | sonnet | `coverage-analysis` |
| Merge + dedupe + prioritize | `@synthesis-reviewer` | opus | — |
| Fixer + full verification gate | `@code-implementator` | sonnet | `test-generation` |

## Argument parsing

Parse `$ARGUMENTS` before doing anything else:

- `--iterations N` → `MAX_ITERATIONS = N`. Otherwise `MAX_ITERATIONS = 2`.
- `--continue` → `CONTINUE_MODE = true`. Otherwise `false`.
- `--base <ref>` → `BASE_REF = <ref>`. Otherwise `BASE_REF = main`. Used only to derive the default target.
- The remaining string, after removing all flags, is the **target** (one or more paths, space-separated).
- **Empty target** → the Rust files changed on this branch, committed or not:
  ```bash
  { git diff --name-only "$BASE_REF"...HEAD; git diff --name-only HEAD; git ls-files --others --exclude-standard; } \
    | grep -E '\.rs$' | sort -u | while read -r f; do [ -f "$f" ] && echo "$f"; done
  ```
  If that is empty, stop with: `nothing to review — no Rust changes vs <BASE_REF>; pass a path, e.g. /review src/http_auth.rs`.
  Otherwise say which files were selected before spawning anything.

`MAX_ITERATIONS` bounds how many times `@code-implementator` may run. The reviewer pass always runs once at the start
and once at the end (regression check); neither counts toward `MAX_ITERATIONS`.

Examples:

- `/review` → changed `.rs` files vs `main`; 1 reviewer pass, up to 2 implementator passes, 1 final reviewer pass
- `/review src/http_auth.rs` → that file only, same counting
- `/review src/tools/ --iterations 1` → one module, at most 1 implementator pass
- `/review --continue` → resume from the last cache, same counting

## Agent completion — the ONLY valid signal

Every agent in this loop runs in the background and the harness notifies you when it returns. A step is done **only**
when that agent's **returned result line** (`DOMAIN=…`, `FEEDBACK_FILE=…`) has arrived. Never advance the loop — and
never emit the Final Report — on any other evidence. In particular, these are **not** completion signals:

- **Findings marked `[x]` in the feedback file.** `@code-implementator` ticks checkboxes *while applying fixes*,
  **before** it runs the verification gate. "All `[x]`" means "edits written, gate not yet passed".
- **No `cargo` process running at one instant.** The gate has gaps between commands and between fix attempts.
- **Output files existing on disk.** File creation ≠ agent returned.

Do not poll with `ScheduleWakeup` or `Monitor` — there is no long build to watch here. The full gate is about a
minute on a warm build (see CLAUDE.md), so the completion notification is the signal; just wait for it. If the user
asks for status, say which agent is still running.

**Stuck implementator.** A healthy `@code-implementator` pass is a few minutes at most. If one has been running for
more than ~10 minutes with no result, tell the user — the usual cause is another `cargo` process holding the
`target/` lock — and offer: **(1)** keep waiting, or **(2)** stop it and run the gate yourself (its `Edit`s and `[x]`
marks are already on disk; treat a green gate as `REMAINING=<count of [ ]>` and continue to Step 3).

## Setup

**If `CONTINUE_MODE = false`:**

```bash
rm -rf .claude/.review-cache && mkdir -p .claude/.review-cache
cargo test 2>&1 | awk '/^test result:/ {sum += $4} END {print sum+0}'
```

Set `IMPL_ITER = 0`. Record the printed number as `TEST_COUNT` — the baseline used to catch a fix that silently
deletes or disables tests. If `cargo test` does not compile, set `TEST_COUNT = unknown`, tell the user the tree
doesn't build before review starts, and continue (the implementator's gate will have to fix it).

**If `CONTINUE_MODE = true`:**

```bash
mkdir -p .claude/.review-cache
ls .claude/.review-cache/iter-*.md 2>/dev/null | sort -V | tail -1
```

- No files → warn "no previous cache found, starting fresh" and proceed as `CONTINUE_MODE = false`.
- Last file is `iter-N.md` → count unchecked findings: `grep -c '^### \[ \]' .claude/.review-cache/iter-N.md || echo 0`.
  - Count > 0: the implementator did not finish that file — resume it (Step 2a on `iter-N.md`), with `IMPL_ITER = N`.
  - Count == 0: set `IMPL_ITER = N`.
- **Carry forward `final.md` residuals.** If `final.md` exists with unchecked findings (the previous run ended
  `residual-blocked`), copy them into the next feedback file so they are not silently dropped, and say so in the
  Final Report.
- If `IMPL_ITER >= MAX_ITERATIONS` → report "nothing left to do" and emit the Final Report from existing files.
- Re-measure `TEST_COUNT` with the `cargo test | awk` line above.

Compute the absolute path of `.claude/.review-cache` as `<CACHE_DIR>` and use it for every path below.

## Reviewer pass sub-procedure

Whenever the loop says **"run a reviewer pass"** with `feedback_path=<CACHE_DIR>/<basename>.md` and an `iteration`:

**RP-1.** Derive temp paths:

- `rust_path = <CACHE_DIR>/rust-<basename>.md`
- `security_path = <CACHE_DIR>/security-<basename>.md`
- `coverage_path = <CACHE_DIR>/coverage-<basename>.md` *(only if `iteration != final`)*

**RP-2.** Spawn the domain reviewers **in parallel** — **one assistant message containing all the Agent tool calls
as separate tool-use blocks.** Sequential calls defeat the design.

- `review-rust` with `target=<target>`, `output_path=<rust_path>`, `iteration=<iteration>`
- `review-security` with `target=<target>`, `output_path=<security_path>`, `iteration=<iteration>`
- `review-coverage` with `target=<target>`, `output_path=<coverage_path>` *(skip on `iteration=final`)*

On `iteration=final`, also pass `previous_feedback=<the last iter-*.md>` to `review-rust` and `review-security`, so
they can tell regressions introduced by the fixes apart from issues already triaged.

Each returns one line: `DOMAIN=<d> FILE=<path> COUNT=<N>`. Source reading stays inside those Sonnet contexts and never
reaches the main thread or synthesis.

**RP-3.** If an agent returned without a parseable line, re-invoke that one agent once. If it fails again, treat its
COUNT as 0, proceed, and name the failed domain in the Final Report — a silently skipped security pass must not read
as "clean".

**RP-4.** Invoke `synthesis-reviewer` with:

```
target=<target>
feedback_path=<CACHE_DIR>/<basename>.md
iteration=<iteration>
rust_path=<rust_path>
security_path=<security_path>
coverage_path=<coverage_path>     # omit on iteration=final
```

It reads only the temp files — never source — dedupes, prioritizes, renumbers, writes the merged file, deletes the
temps, and returns:

```
FEEDBACK_FILE=<feedback_path> TOTAL=<N> CRITICAL=<X> HIGH=<Y> MEDIUM=<Z> LOW=<W>
```

That line is the result of the reviewer pass.

## Loop

### Step 1 — Initial review

> Skip only when `CONTINUE_MODE = true` and Setup found an unfinished implementator pass.

Run a reviewer pass with `feedback_path=<CACHE_DIR>/iter-0.md` and `iteration=1`.

- TOTAL == 0 → skip to Final Report, outcome = `clean`.

### Step 2 — Fix loop (while `IMPL_ITER < MAX_ITERATIONS`)

**2a.** Invoke `code-implementator` with `feedback_path=<CACHE_DIR>/iter-<IMPL_ITER>.md`.

Wait for its completion notification and parse:

```
FEEDBACK_FILE=<path> FIXED=<N> BLOCKED=<M> REMAINING=<K> TESTS=<count|n/a> VERIFY=<full|n/a>
```

That line is returned only **after** the full gate (`fmt → clippy --all-targets -D warnings → test`) passed, so its
arrival means the whole crate compiles and every test is green. This repo is a single crate — there is no scoped
verification and nothing is owed to CI beyond what already ran.

- `TESTS` is a number lower than `TEST_COUNT` → stop, outcome = `stuck`, and report `test count dropped <TEST_COUNT> → <TESTS>`
  (a fix deleted, renamed away, or `#[ignore]`d tests). Otherwise set `TEST_COUNT = TESTS`.
- REMAINING > 0 and FIXED == 0 → stop, outcome = `stuck`.
- REMAINING == 0 → go to Step 3.

Increment `IMPL_ITER`.

**2b.** If `IMPL_ITER < MAX_ITERATIONS` and REMAINING > 0, run a reviewer pass with
`feedback_path=<CACHE_DIR>/iter-<IMPL_ITER>.md` and `iteration=<IMPL_ITER + 1>`.

- TOTAL == 0 → stop, outcome = `clean`.

If `IMPL_ITER == MAX_ITERATIONS` with REMAINING > 0 → stop, outcome = `residual-blocked`.

### Step 3 — Final regression review (runs unless the loop ended `stuck`)

Run a reviewer pass with `feedback_path=<CACHE_DIR>/final.md` and `iteration=final` (coverage skipped).

- TOTAL == 0 → outcome = `clean`.
- Otherwise → outcome = `residual-blocked` (the fixes introduced new findings).

### Step 4 — Orchestrator gate (only if an implementator pass changed files)

Confirm the end state yourself, mirroring CI's format check:

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | tail -5
```

On failure, report it verbatim in the Final Report; do not start another fix pass on your own.

## Fallback parsing

Only for an agent that **has returned** but whose result line is malformed — never to decide an agent is done.

- TOTAL / REMAINING: `grep -c '^### \[ \]' <feedback_path>` (0 if the file is missing)
- FIXED: previous TOTAL minus current REMAINING

## Final Report

Nothing is committed — all fixes are left in the working tree for the user to review.

```markdown
# Review Loop Result

**Target:** <target>
**Implementator passes:** <IMPL_ITER> / <MAX_ITERATIONS>
**Mode:** <fresh | continued from iter-N | carried N residuals from prior final.md>
**Outcome:** <clean | residual-blocked | stuck>
**Verification:** <full gate green, <TEST_COUNT> tests | n/a — no code changed | FAILED: <error>>
**Skipped domains:** <none | e.g. security (agent failed twice)>

| Pass    | Role          | Total | Critical | High | Medium | Low | Fixed | Blocked |
|:--------|:--------------|------:|---------:|-----:|-------:|----:|------:|--------:|
| Initial | reviewer      | ...   |          |      |        |     | —     | —       |
| 1       | implementator | —     | —        | —    | —      | —   | ...   | ...     |
| 2       | reviewer      | ...   |          |      |        |     | —     | —       |
| Final   | reviewer      | ...   |          |      |        |     | —     | —       |

**Feedback files:**

- .claude/.review-cache/iter-0.md
- .claude/.review-cache/final.md
```

List every still-`[ ]` finding (ID, severity, one line) under the table so the user doesn't have to open the files.
