---
name: orchestration
description: Playbook for running the main session as an ORCHESTRATOR that drives a multi-phase implementation (e.g. a new tool family, a transport change, a TDD rollout) by spawning one fresh subagent per phase. Load this when the user explicitly puts the main session in an orchestrator/coordinator role, asks you to spawn a subagent per task/phase, or says to keep main-session token usage minimal while delegating work. Encodes how to divide labor, size tasks, and keep subagent work verifiable in this single-crate Rust MCP server.
---

# Orchestration Skill — engramo-mcp

You are the **orchestrator**: a thin coordinator. The expensive work (reading files, writing code,
compiling, running tests) happens in subagents; your job is to plan, dispatch, review outcomes, and
keep your own context lean. Optimise for **trustworthy progress per cache window**, not for doing the
work yourself.

## Know your verification budget (it is small — use it)

This repo is a **single crate** with **no database, no Docker, no testcontainers, no sqlx, no
workspace**. Measured on this machine:

| Chain | Wall time |
|---|---|
| `fmt --check` + `clippy --all-targets -D warnings` + `test` — warm target, no source change | ~46 s |
| Same chain after touching one `src/*.rs` | ~7 s |
| The 259 unit tests alone | 0.4 s |

Cold (`cargo clean`, or a fresh `target/`) is a few minutes, once.

**Consequence — the division of labor is the inverse of engram-api's.** There, verification was the
bottleneck and had to stay on the orchestrator. Here it costs seconds, so:

> **Every subagent runs the FULL chain itself, in the foreground, before reporting.**
> There is no "scoped" verification worth designing — the scoped run and the full run are the same run.

Do not import slow-repo habits: no background verification, no `Monitor` polling, no `--offline`
gymnastics, no `caffeinate` wrapper. A chain that finishes in seconds never needs any of it. If a
verify appears to hang for minutes, something is genuinely wrong (network fetch on a dirty
`Cargo.toml`, or a `target/` lock held by another cargo) — check `ps` rather than waiting.

## Division of labor (default)

- **Subagent** implements the phase and self-verifies with the chain below. It reports a **tight
  summary** — files changed, test counts, deviations — no code dumps, no log paste.
- **Orchestrator (you)** plans, dispatches, reviews the **diff**, re-runs the chain once at the end of
  the whole rollout (it is nearly instant and incremental), and updates the spec's progress section.

## The verification chain (subagent and orchestrator, identical)

CLAUDE.md mandates `cargo fmt --all` → `cargo check` → `cargo clippy` → `cargo test`.
`clippy --all-targets -- -D warnings` subsumes `cargo check` and also lints test code, so run:

```bash
cargo fmt --all \
  && cargo clippy --all-targets -- -D warnings \
  && cargo test 2>&1 | tail -30
```

Add `cargo sort` **only** if the phase touched `Cargo.toml` dependencies.

Instruct subagents: **one foreground Bash call, `run_in_background=false`**. Never fire-and-forget and
end the turn — a background subagent does not self-resume on its child command; the harness notifies
*you* instead, and you end up adopting a job that would have taken 10 seconds inline.

To extract just the totals without pulling a log into context:

```bash
# NOTE: skill bodies substitute awk positional fields as skill arguments, so isolate
# the numbers with grep and sum with paste+bc instead of awk.
grep -E 'test result:' "$LOG" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+' | paste -sd+ - | bc
grep -E 'test result:' "$LOG" | grep -oE '[0-9]+ failed' | grep -oE '[0-9]+' | paste -sd+ - | bc
```

## Spawning subagents

- **One fresh subagent per phase** (`subagent_type: general-purpose`, `model: sonnet` unless the user
  says otherwise). A fresh agent starts cold, so the prompt must be **self-contained**: point it at the
  spec section and at `CLAUDE.md`, name the exact files in scope, state the guardrails below, and give
  the report format. Do not assume it shares your context.
- **Do NOT use `fork`** for phase work — a fork inherits your full context (defeating the token goal)
  and runs on your model.
- **Sequence dependent phases**: dispatch → review diff → verify → update the spec's progress table →
  dispatch the next.
- **Parallel phases are risky here for a structural reason:** tests live in `#[cfg(test)] mod tests`
  *inside* the same file as the code (`src/tools/*.rs`, `src/server.rs`). Two subagents touching one
  tool module will collide in the working tree. Only parallelise phases whose file sets are disjoint —
  e.g. `src/tools/cards.rs` vs `src/resources/mod.rs` — and say so explicitly in each prompt.

## Reviewing a subagent's work without bloating context

- Review the **diff**, not the transcript: `git diff <scope files>`, `git status --short`,
  `git diff --stat`. **Never** `Read`/`tail` a subagent's JSONL output — it will overflow your context.
- Sanity-check against the spec's acceptance criteria before accepting.
- After a phase is green, update the spec/progress table (status, files, pass/fail counts) so the
  rollout is resumable. **Do not commit unless the user asks.**

## Repo-specific guardrails (from CLAUDE.md — restate in every phase prompt)

- **Never propagate `Err()` out of a tool handler** — it crashes the client's tool-calling loop. Every
  error path returns `Ok(CallToolResult { is_error: true, .. })` via `err_result(e)`. Parse UUIDs with
  `parse_uuid(s)`, never interpolate a raw string into a request path.
- **Rich text R1–R5**: spans must exactly tile `text`. `normalize_card_content` in `server.rs` enforces
  this and strips LLM-injected marker characters — a phase that adds a card-writing path must route
  through it.
- **Paid-AI tools** (`src/tools/ai.rs`) live in their own `#[tool_router(router = paid_ai_tools_router)]`
  block and are only summed into `EngramMcpServer::new()` when `ENGRAM_ENABLE_PAID_AI` is on. A new tool
  that costs the user money belongs there, not in the always-on router.
- **Never log an MCP session id** — it is credential-equivalent. `error_reporting` redacts `session_id`
  and the default `RUST_LOG` filter silences rmcp's session manager; keep it that way. Same for
  `ENGRAM_API_TOKEN` / bearer tokens.
- **http mode auth invariants**: bearer → `CURRENT_BEARER_TOKEN` task-local → one `EngramClient` per
  session, and `SessionTokens` rejects a request presenting a different token than the one that opened
  the session. A phase touching `http_auth.rs` or `main.rs` must keep both halves.
- **Tests use `wiremock`**, not a live API. A phase must not introduce a test that needs the network.
- New tool params structs need `schemars::JsonSchema`; new tools need `#[tool(description = "...")]`.

## Token discipline (orchestrator)

- Don't re-read a file you just edited — `Edit`/`Write` already confirmed the change.
- Pipe verification output through `tail`/`grep`; never let a full `cargo test` log land in your context.
- Relay only what matters from a subagent's report; don't quote it verbatim.
- Keep your own tool calls few and batched.

## Cleanup after all phases finish

There is nothing to reap in the normal case — no containers, no long-lived build. If a subagent was
killed mid-verify, confirm no orphaned build is still holding the `target/` lock before the final run:

```bash
pgrep -fl 'cargo (test|clippy|build)' || echo "no stray cargo"
```

Kill only a process you can identify as an orphan of this session. Do **not** blanket-kill
`cargo`/`rustc` — that clobbers an unrelated build the user may be running. Never `pkill caffeinate`:
`caffeinate -i -t <N>` is the harness's own keep-awake heartbeat, not a subagent artifact.
