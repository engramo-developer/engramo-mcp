[![npm version](https://img.shields.io/npm/v/@engramo/mcp)](https://www.npmjs.com/package/@engramo/mcp)
[![CI](https://github.com/engramo-developer/engramo-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/engramo-developer/engramo-mcp/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

# engramo-mcp

MCP server for [EngrAmo](https://engramo.app), the spaced-repetition flashcard platform.

## Prerequisites

- An EngrAmo account
- An EngrAmo API token — generate one from your account's Settings → API Tokens page

## Environments

`engramo-mcp` is environment-agnostic — the binary itself has no baked-in "dev" or "prod." Which server it
talks to is controlled entirely by the `ENGRAMO_API_URL` you set in your client config, paired with a token
minted from that **same** environment:

| Environment | `ENGRAMO_API_URL` | MCP server URL (`http` transport only) | Who it's for |
|---|---|---|---|
| **Production** (default in every example below) | `https://api.engramo.app` | `https://mcp.engramo.app` | Real accounts — use this unless you have a specific reason not to |
| **Dev** | `https://api-engram.volmyr.com` | `https://mcp-engramo.volmyr.com` | EngrAmo team / internal testing only |

**Dev and prod are separate backends with separate accounts.** A token minted from one will not authenticate
against the other — if you switch `ENGRAMO_API_URL`, you must also swap in a token generated from that same
environment's Settings → API Tokens page.

**`ENGRAMO_API_URL` vs the MCP server URL — these are not interchangeable.** `ENGRAMO_API_URL` is the plain
EngrAmo backend; it's never something a client connects to directly. In `stdio` config (Claude Desktop,
Claude Code, Codex, VS Code, Cursor, Windsurf, Gemini CLI, Antigravity stdio below), the client sets
`ENGRAMO_API_URL` **together with** `ENGRAMO_API_TOKEN`, since the locally-launched binary calls the backend
itself. In `http`/remote config (Antigravity remote, ChatGPT), the
client instead points `serverUrl` at the MCP server's own URL (`mcp.engramo.app` / `mcp-engramo.volmyr.com`
above) and authenticates separately per session — via OAuth by default, or a static `Authorization: Bearer`
header if you'd rather skip the login prompt. `ENGRAMO_API_URL` still matters in `http` mode, but only as an
env var set by whoever deploys `engramo-mcp http` — a client never sets it.

## Installation

Install the MCP server globally so it's available as a system command:

```bash
npm install -g @engramo/mcp
```

Verify the installation:

```bash
engramo-mcp --version
```

> Alternatively, skip installation entirely and use `npx -y @engramo/mcp` in your client config — npm will download the binary automatically on first use.

## Choosing a client

Any MCP client works. Local [text-to-speech](#text-to-speech-bring-your-own-key) needs a `stdio`
(locally launched) connection, so remote-only clients never get it.

| Client | Models | Local TTS | Setup |
|---|---|---|---|
| Claude Desktop | Claude | ✅ | [Claude Desktop](#claude-desktop) — recommended for non-technical users: the free plan supports local MCP servers, and setup is one JSON file (Settings → Developer → Edit Config) |
| Claude Code | Claude | ✅ | [Claude Code](#claude-code) |
| Codex (CLI / IDE extension) | OpenAI models — sign in with your ChatGPT account | ✅ | [Codex](#codex) |
| VS Code (GitHub Copilot) | GPT / Claude / Gemini | ✅ | [VS Code](#vs-code-github-copilot-agent-mode) |
| Cursor | GPT / Claude / Gemini | ✅ | [Cursor](#cursor) |
| Windsurf | GPT / Claude / Gemini | ✅ | [Windsurf](#windsurf) |
| Gemini CLI | Gemini | ✅ | [Gemini CLI](#gemini-cli) |
| Antigravity | Gemini / Claude / GPT | ✅ stdio / ❌ remote | [Antigravity](#antigravity) |
| ChatGPT | OpenAI models | ❌ — remote connector only; local TTS intentionally unavailable | [ChatGPT](#use-in-chatgpt-remote-mcp-over-streamable-http) |

## Claude Desktop

Open **Settings → Developer → Edit Config**, or edit the file directly:
`~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) or
`%APPDATA%\Claude\claude_desktop_config.json` (Windows):

**Using npx** (no prior installation needed):
```json
{
  "mcpServers": {
    "engramo": {
      "command": "npx",
      "args": ["-y", "@engramo/mcp"],
      "env": {
        "ENGRAMO_API_URL": "https://api.engramo.app",
        "ENGRAMO_API_TOKEN": "your-token",
        "ENGRAMO_TTS_GEMINI_API_KEYS": "your-gemini-key"
      }
    }
  }
}
```

`ENGRAMO_TTS_GEMINI_API_KEYS` is optional — it enables `list_tts_voices`/`generate_card_audio` using your own
Gemini quota; omit the line to disable. See [Text-to-speech](#text-to-speech-bring-your-own-key).

**Using a global install** (`npm install -g @engramo/mcp`):
```json
{
  "mcpServers": {
    "engramo": {
      "command": "engramo-mcp",
      "env": {
        "ENGRAMO_API_URL": "https://api.engramo.app",
        "ENGRAMO_API_TOKEN": "your-token"
      }
    }
  }
}
```

After editing, fully quit Claude Desktop (⌘Q on macOS — closing the window isn't enough) and reopen it.
If the server doesn't show up, check the logs at `~/Library/Logs/Claude/mcp-server-engramo.log` (macOS) or
`%APPDATA%\Claude\logs\` (Windows). The Gemini key is never logged.

## Claude Code

Register the server once for all your projects (`--scope user`):

```bash
claude mcp add engramo --scope user \
  -e ENGRAMO_API_URL=https://api.engramo.app \
  -e ENGRAMO_API_TOKEN=your-token \
  -e ENGRAMO_TTS_GEMINI_API_KEYS=your-gemini-key \
  -- npx -y @engramo/mcp
```

`ENGRAMO_TTS_GEMINI_API_KEYS` is optional — it enables `list_tts_voices`/`generate_card_audio` using your own
Gemini quota; omit the `-e ENGRAMO_TTS_GEMINI_API_KEYS=…` line to disable. See
[Text-to-speech](#text-to-speech-bring-your-own-key).

With a global install, replace `npx -y @engramo/mcp` with `engramo-mcp`.

## Codex

The OpenAI Codex CLI and the Codex IDE extension share one config file, `~/.codex/config.toml`. This is the
way to use your ChatGPT account's models **with** local TTS (the ChatGPT app's remote connector can't have it):

**Using npx** (no prior installation needed):
```toml
[mcp_servers.engramo]
command = "npx"
args = ["-y", "@engramo/mcp"]
tool_timeout_sec = 300  # default 60s; a 20-card generate_card_audio batch can take longer
env = { ENGRAMO_API_URL = "https://api.engramo.app", ENGRAMO_API_TOKEN = "your-token", ENGRAMO_TTS_GEMINI_API_KEYS = "your-gemini-key" }
```

`ENGRAMO_TTS_GEMINI_API_KEYS` is optional — it enables `list_tts_voices`/`generate_card_audio` using your own
Gemini quota; omit it from `env` to disable. See [Text-to-speech](#text-to-speech-bring-your-own-key).

**Using a global install** (`npm install -g @engramo/mcp`):
```toml
[mcp_servers.engramo]
command = "engramo-mcp"
env = { ENGRAMO_API_URL = "https://api.engramo.app", ENGRAMO_API_TOKEN = "your-token" }
```

## VS Code (GitHub Copilot agent mode)

Add to `.vscode/mcp.json` (workspace), or run **MCP: Open User Configuration** from the Command Palette for a
user-wide config. Use `inputs` with `"password": true` so VS Code prompts for the secrets and stores them
itself instead of you writing them into the file. Note the top-level key is `servers` (not `mcpServers`):

**Using npx** (no prior installation needed):
```json
{
  "inputs": [
    { "type": "promptString", "id": "engramo-token", "description": "EngrAmo API token", "password": true },
    { "type": "promptString", "id": "gemini-key", "description": "Gemini API key (for TTS)", "password": true }
  ],
  "servers": {
    "engramo": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "@engramo/mcp"],
      "env": {
        "ENGRAMO_API_URL": "https://api.engramo.app",
        "ENGRAMO_API_TOKEN": "${input:engramo-token}",
        "ENGRAMO_TTS_GEMINI_API_KEYS": "${input:gemini-key}"
      }
    }
  }
}
```

`ENGRAMO_TTS_GEMINI_API_KEYS` is optional — it enables `list_tts_voices`/`generate_card_audio` using your own
Gemini quota; omit the line (and the `gemini-key` input) to disable. See
[Text-to-speech](#text-to-speech-bring-your-own-key).

With a global install, use `"command": "engramo-mcp"` and drop `args`.

## Cursor

Add to `~/.cursor/mcp.json`:

**Using npx** (no prior installation needed):
```json
{
  "mcpServers": {
    "engramo": {
      "command": "npx",
      "args": ["-y", "@engramo/mcp"],
      "env": {
        "ENGRAMO_API_URL": "https://api.engramo.app",
        "ENGRAMO_API_TOKEN": "your-token",
        "ENGRAMO_TTS_GEMINI_API_KEYS": "your-gemini-key"
      }
    }
  }
}
```

`ENGRAMO_TTS_GEMINI_API_KEYS` is optional — it enables `list_tts_voices`/`generate_card_audio` using your own
Gemini quota; omit the line to disable. See [Text-to-speech](#text-to-speech-bring-your-own-key).

**Using a global install** (`npm install -g @engramo/mcp`):
```json
{
  "mcpServers": {
    "engramo": {
      "command": "engramo-mcp",
      "env": {
        "ENGRAMO_API_URL": "https://api.engramo.app",
        "ENGRAMO_API_TOKEN": "your-token"
      }
    }
  }
}
```

## Windsurf

Add to `~/.codeium/windsurf/mcp_config.json`:

**Using npx** (no prior installation needed):
```json
{
  "mcpServers": {
    "engramo": {
      "command": "npx",
      "args": ["-y", "@engramo/mcp"],
      "env": {
        "ENGRAMO_API_URL": "https://api.engramo.app",
        "ENGRAMO_API_TOKEN": "your-token",
        "ENGRAMO_TTS_GEMINI_API_KEYS": "your-gemini-key"
      }
    }
  }
}
```

`ENGRAMO_TTS_GEMINI_API_KEYS` is optional — it enables `list_tts_voices`/`generate_card_audio` using your own
Gemini quota; omit the line to disable. See [Text-to-speech](#text-to-speech-bring-your-own-key).

**Using a global install** (`npm install -g @engramo/mcp`):
```json
{
  "mcpServers": {
    "engramo": {
      "command": "engramo-mcp",
      "env": {
        "ENGRAMO_API_URL": "https://api.engramo.app",
        "ENGRAMO_API_TOKEN": "your-token"
      }
    }
  }
}
```

## Gemini CLI

Add to `~/.gemini/settings.json`:

**Using npx** (no prior installation needed):
```json
{
  "mcpServers": {
    "engramo": {
      "command": "npx",
      "args": ["-y", "@engramo/mcp"],
      "env": {
        "ENGRAMO_API_URL": "https://api.engramo.app",
        "ENGRAMO_API_TOKEN": "your-token",
        "ENGRAMO_TTS_GEMINI_API_KEYS": "your-gemini-key"
      }
    }
  }
}
```

`ENGRAMO_TTS_GEMINI_API_KEYS` is optional — it enables `list_tts_voices`/`generate_card_audio` using your own
Gemini quota; omit the line to disable. See [Text-to-speech](#text-to-speech-bring-your-own-key).

**Using a global install** (`npm install -g @engramo/mcp`):
```json
{
  "mcpServers": {
    "engramo": {
      "command": "engramo-mcp",
      "env": {
        "ENGRAMO_API_URL": "https://api.engramo.app",
        "ENGRAMO_API_TOKEN": "your-token"
      }
    }
  }
}
```

## Antigravity

Antigravity supports both transports directly via its own config file — no separate remote-connector UI
needed like ChatGPT's. Add to `~/.gemini/config/mcp_config.json` (global) or `.agents/mcp_config.json`
(workspace-local):

**stdio, via npx** (no prior installation needed):
```json
{
  "mcpServers": {
    "engramo": {
      "command": "npx",
      "args": ["-y", "@engramo/mcp"],
      "env": {
        "ENGRAMO_API_URL": "https://api.engramo.app",
        "ENGRAMO_API_TOKEN": "your-token"
      }
    }
  }
}
```

**Remote, over Streamable HTTP** (point at a running `engramo-mcp http` deployment — see below).
`engramo-mcp` supports OAuth 2.1 dynamic client registration, so Antigravity can handle
authentication automatically — no token to paste or manage:
```json
{
  "mcpServers": {
    "engramo": {
      "serverUrl": "https://mcp.engramo.app/"
    }
  }
}
```

If you'd rather use a static token instead (no OAuth login prompt), add `headers` manually:
```json
{
  "mcpServers": {
    "engramo": {
      "serverUrl": "https://mcp.engramo.app/",
      "headers": {
        "Authorization": "Bearer your-token"
      }
    }
  }
}
```

Text-to-speech is unavailable over the remote transport — use the stdio config above if you want
`generate_card_audio`.

## Use in ChatGPT (remote MCP over Streamable HTTP)

`engramo-mcp` also runs as a **remote** server, so it can be added to ChatGPT as a custom connector (developer
mode) without installing anything locally. This is the same binary — the `http` subcommand instead of the
default `stdio` — so a self-hosted deployment serves both Claude Desktop users (stdio) and ChatGPT users
(HTTP) from one codebase.

```bash
ENGRAMO_API_URL=https://api.engramo.app MCP_BIND_ADDR=0.0.0.0:8080 engramo-mcp http
```

This serves MCP over Streamable HTTP at `POST /` (root). Unlike `stdio` mode, there is **no global
`ENGRAMO_API_TOKEN`** — every session authenticates with its own `Authorization: Bearer <token>` header, so one
deployment safely serves many users at once (each session's calls to the EngrAmo API use only that session's
token). Requests without a well-formed, non-empty bearer token are rejected with `401` before a session is
created, and once a session exists every later request on it must carry the same token that opened it.

`http` mode also serves an unauthenticated `GET /version` that returns `{"name","version"}` (this
binary's build version, read from `Cargo.toml` at compile time) — handy for deployment health and
version probes without an MCP handshake.

In ChatGPT: **Settings → Connectors → Advanced → Developer mode**, then add a custom connector pointing at
your deployment's `https://<host>/`, pasting an EngrAmo API token (from Settings → API Tokens) as the
bearer token. Once connected, prompts like *"Make me a 10-card Spanish restaurant deck"* or *"Turn this
conversation into flashcards"* call `generate_catalog_with_cards` directly; open the result at
`https://study.engramo.app/catalog/<shortId>` to study it.

> `generate_card_audio` is never available through a ChatGPT connector — it's a remote connection, and local
> [text-to-speech](#text-to-speech-bring-your-own-key) is `stdio`-only by design. To use your ChatGPT
> account's models with local TTS, use [Codex](#codex) instead.

> A published, one-click ChatGPT App (OAuth login instead of a pasted token) is planned but not yet available
> — see the project tracker for status.

## Bring-your-own-AI

Every deployment is **bring-your-own-AI**: the calling model (Claude, ChatGPT, …) does all generation —
translation, dictionaries, phrasing — and `engramo-mcp` only persists the result via `generate_card` /
`generate_catalog_with_cards` / `generate_cards`. This costs the EngrAmo account nothing beyond normal
storage quotas, and needs no paid EngrAmo plan.

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `ENGRAMO_API_URL` | `https://api.engramo.app` | Base URL of the EngrAmo API |
| `ENGRAMO_API_TOKEN` | *(required for `stdio`)* | Your EngrAmo API token. Unused in `http` mode — each session supplies its own via the `Authorization: Bearer` header. |
| `MCP_BIND_ADDR` | `0.0.0.0:8080` | Bind address for `http` mode |
| `MCP_PUBLIC_URL` | *(unset)* | `http` mode only — this deployment's own public URL (bare origin, no path — the endpoint is served at `/`), e.g. `https://mcp.engramo.app`. Also allowlists that host for inbound requests; without it, every request is rejected (see `MCP_ALLOWED_HOSTS`). |
| `MCP_ALLOWED_HOSTS` | *(unset)* | `http` mode only — extra comma-separated hostnames/`host:port` values to permit, on top of the one derived from `MCP_PUBLIC_URL`. Only needed for extra entry points (e.g. a Cloud Run service's own `*.run.app` fallback URL alongside its custom domain). |
| `ENGRAMO_TTS_GEMINI_API_KEYS` | *(unset → TTS tools absent)* | **`stdio` only.** Your own Gemini API key, or a comma-separated list. Enables `list_tts_voices`/`generate_card_audio`. Ignored (with a startup warning) in `http` mode — see [Text-to-speech](#text-to-speech-bring-your-own-key) below. |
| `ENGRAMO_TTS_PROVIDER` | `gemini` | **`stdio` only.** TTS engine selector. Only `gemini` is supported today; an unrecognized value fails startup. |
| `ENGRAMO_TTS_MODEL` | `gemini-3.8-flash-tts` | **`stdio` only.** Gemini TTS model id. |
| `ENGRAMO_TTS_VOICE` | `Puck` | **`stdio` only.** Default voice used when a `generate_card_audio` call doesn't specify one. |

## Available tools

### Server info
| Tool | Description |
|---|---|
| `get_server_version` | Get this MCP server's own build version (not a catalog/card's `version` field) |

### Catalogs
| Tool | Description |
|---|---|
| `list_catalogs` | List all flashcard catalogs |
| `get_catalog` | Get a single catalog by ID |
| `update_catalog` | Update a catalog's name or description |
| `delete_catalog` | Delete a catalog |

### Cards
| Tool | Description |
|---|---|
| `list_cards` | List cards in a catalog |
| `get_card` | Get a single card by ID |
| `update_card` | Update a card's content |
| `delete_card` | Delete a card |

### Learning
| Tool | Description |
|---|---|
| `get_due_cards` | Get cards due for review today |
| `get_all_learning_cards` | Get all cards in the learning queue |
| `add_card_to_learning` | Add a card to the learning queue |
| `add_catalog_to_learning` | Add all cards from a catalog to the learning queue |

### Learning paths
| Tool | Description |
|---|---|
| `list_learning_paths` | List all learning paths |
| `get_learning_path` | Get a single learning path by ID |
| `create_learning_path` | Create a new learning path |
| `activate_learning_path` | Activate a learning path |
| `deactivate_learning_path` | Deactivate a learning path |

### Search
| Tool | Description |
|---|---|
| `search_global` | Full-text search across all content |
| `search_catalogs` | Search within a specific catalog |

### Media
| Tool | Description |
|---|---|
| `list_media` | List uploaded media assets |
| `upload_media` | Upload a file you already have (a voice recording, an image) and get back a `media_id` to attach to a card or catalog |

### AI generation (bring-your-own-AI)
| Tool | Description |
|---|---|
| `generate_card` | Create a flashcard — the calling model does any translation/wording itself |
| `generate_catalog_with_cards` | Create a catalog with cards in one call — same bring-your-own-AI model |
| `generate_cards` | Add multiple cards to an existing catalog — same bring-your-own-AI model |

### Text-to-speech (stdio, opt-in)
Only present when `ENGRAMO_TTS_GEMINI_API_KEYS` is set (`stdio` mode only) — see
[Text-to-speech](#text-to-speech-bring-your-own-key) below.

| Tool | Description |
|---|---|
| `list_tts_voices` | List the configured TTS engine, model, default voice, and available voices. No network call. |
| `generate_card_audio` | Synthesize speech for the FACE side of up to 20 cards, upload it to your own media, and attach it — using your own Gemini key/quota |

## Rich cards: dictionary, styling, images, audio

Beyond plain text, a card's `face`/`back` can carry:

| Field | What it does |
|---|---|
| `dictionary` | Word → translation map, rendered as clickable highlights in the app |
| `rich_text` | Styled spans (bold, color, monospace) within the text |
| `style` | Font/color/background/alignment for the whole face or back |
| `audio_id` | Your own audio (a recording, a clip from your own TTS) — upload it with `upload_media` first, or set it automatically by calling `generate_card_audio` (`stdio` only, see below) |
| `visual_id` + `visual_type` | Your own image/video, same pattern as `audio_id` |

A catalog can also have `image_id` (cover image) set the same way. None of this needs a paid EngrAmo
plan — dictionary/translation are done by the calling model, images are entirely bring-your-own via
`upload_media` (max ~10MB per file), and face audio is either bring-your-own the same way or
generated locally with your own TTS key via `generate_card_audio`. See `engramo://card-schema` for
the full schema and four worked examples, and [`docs/prompt-examples.md`](docs/prompt-examples.md)
for ready-to-paste prompts covering all of this end to end.

**Adding a lot of cards/media at once?** `upload_media` is one file per call, fine for a handful of
files through a chat client. For bulk imports (many cards, many audio files, in one request) see
[`docs/bulk-import.md`](docs/bulk-import.md) — a direct API recipe for CLI/scripted use (Claude
Code, a terminal, a script), not a chat-driven MCP tool.

## Text-to-speech (bring your own key)

Set `ENGRAMO_TTS_GEMINI_API_KEYS` and two extra tools appear: `list_tts_voices` and
`generate_card_audio`. Given a list of card ids, `generate_card_audio` synthesizes speech for each
card's **face** text with Gemini TTS, encodes it to MP3, uploads it to your own EngrAmo media, and
attaches it as that card's `audio_id` — the same place a manually recorded `upload_media` file would
go, just done for you.

**`stdio` only, and on purpose.** The key is read once, locally, from your own machine's
environment and is never sent to EngrAmo or logged. In `http` mode the server is a shared,
multi-tenant deployment — there is no per-session place to put a secret key that stays on the
caller's machine — so `ENGRAMO_TTS_GEMINI_API_KEYS` is read only by `stdio`; in `http` mode it is
ignored entirely (with a startup warning naming the variable, never its value) and neither tool is
registered.

**Why a namespaced variable instead of the standard `GEMINI_API_KEY`?** `GEMINI_API_KEY` is the
de-facto standard name set by the Gemini CLI, Google's own SDKs, and many developers' shell
profiles. If `engramo-mcp` read it too, a key already sitting in your environment for an unrelated
tool could silently start spending itself the moment you ran `engramo-mcp stdio`. `engramo-mcp`
**never reads `GEMINI_API_KEY`** — only the explicit, namespaced `ENGRAMO_TTS_GEMINI_API_KEYS` opts
you in.

**Whose quota does this spend?** Yours, not EngrAmo's. This is a different mechanism from the paid
`generate_tts_for_cards` tool (`ENGRAMO_ENABLE_PAID_AI`), which spends EngrAmo's own metered TTS
quota — `generate_card_audio` spends only your own Gemini API quota/billing, same as if you'd called
Gemini directly.

**Multiple keys / rotation.** `ENGRAMO_TTS_GEMINI_API_KEYS` accepts a comma-separated list. On each
synthesis call the keys are tried in a random order; a key that's rate-limited, rejected (invalid or
revoked), or hits a transient error is skipped in favor of the next one, so one bad key in the list
doesn't fail the whole call.

**Limits.** Up to 20 cards per `generate_card_audio` call, up to 500 characters of face text per
card (longer cards are skipped, not truncated). Cards that already have face audio are skipped
unless you pass `overwrite: true`, in which case the previous audio becomes unreferenced and is
cleaned up server-side. Call `list_tts_voices` first to see the configured model and voice catalog.

**Getting a key.** Create one for free at [Google AI Studio](https://aistudio.google.com/apikey).

**Where the key goes.** Every `stdio` client section above shows the exact place for
`ENGRAMO_TTS_GEMINI_API_KEYS` — see [Choosing a client](#choosing-a-client). It's always one more entry
next to `ENGRAMO_API_TOKEN` in the server's `env`:
```json
"env": {
  "ENGRAMO_API_URL": "https://api.engramo.app",
  "ENGRAMO_API_TOKEN": "your-token",
  "ENGRAMO_TTS_GEMINI_API_KEYS": "your-gemini-key"
}
```

**Quick check.** Ask the assistant to call `list_tts_voices`. If the tool is missing, the key isn't set
(or the client wasn't restarted) or the client is using a remote connection.

## Third-party licenses

The prebuilt binaries distributed via npm statically link [LAME](https://lame.sourceforge.io/)
(LGPL-2.0) for the MP3 encoding `generate_card_audio` uses. See
[`THIRD_PARTY_LICENSES`](THIRD_PARTY_LICENSES) for the full notice and source-availability terms.

## Resources and Prompts

The server also exposes **MCP Resources** (live data readable as context):

| URI | Description |
|---|---|
| `engramo://card-schema` | CardContent JSON schema with validation rules and examples |
| `engramo://catalogs` | All catalogs (id, name, card_count) |
| `engramo://learning/due` | Cards due for review today |
| `engramo://learning/stats` | Learning stats (due_count, total_count) |
| `engramo://learning-paths` | All learning paths (id, name) |
| `engramo://subscription` | User subscription/plan information |

And **MCP Prompts** (guided workflows):

| Prompt | Description |
|---|---|
| `review_session` | Start a guided spaced-repetition review session |
| `create_flashcard` | Create a high-quality flashcard for a topic |
| `create_language_deck` | Create a styled, translated, dictionary-annotated language-learning deck — see [`docs/prompt-examples.md`](docs/prompt-examples.md) |
| `explain_card` | Explain a flashcard in depth with examples |
| `study_plan` | Build a structured study plan from your catalogs |

## Publishing to npm
```bash
git tag vx.y.z                                                                                                                                                                                                      
git push origin vx.y.z
```

## Contributing

Bug reports and pull requests are welcome at
[github.com/engramo-developer/engramo-mcp/issues](https://github.com/engramo-developer/engramo-mcp/issues).
