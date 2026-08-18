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
talks to is controlled entirely by the `ENGRAM_API_URL` you set in your client config, paired with a token
minted from that **same** environment:

| Environment | `ENGRAM_API_URL` | Who it's for |
|---|---|---|
| **Production** (default in every example below) | `https://api.engramo.app` | Real accounts — use this unless you have a specific reason not to |
| **Dev** | `https://api-engram.volmyr.com` | EngrAmo team / internal testing only |

**Dev and prod are separate backends with separate accounts.** A token minted from one will not authenticate
against the other — if you switch `ENGRAM_API_URL`, you must also swap in a token generated from that same
environment's Settings → API Tokens page.

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

## Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) or
`%APPDATA%\Claude\claude_desktop_config.json` (Windows):

**Using npx** (no prior installation needed):
```json
{
  "mcpServers": {
    "engram": {
      "command": "npx",
      "args": ["-y", "@engramo/mcp"],
      "env": {
        "ENGRAM_API_URL": "https://api.engramo.app",
        "ENGRAM_API_TOKEN": "your-token"
      }
    }
  }
}
```

**Using a global install** (`npm install -g @engramo/mcp`):
```json
{
  "mcpServers": {
    "engram": {
      "command": "engramo-mcp",
      "env": {
        "ENGRAM_API_URL": "https://api.engramo.app",
        "ENGRAM_API_TOKEN": "your-token"
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
    "engram": {
      "command": "npx",
      "args": ["-y", "@engramo/mcp"],
      "env": {
        "ENGRAM_API_URL": "https://api.engramo.app",
        "ENGRAM_API_TOKEN": "your-token"
      }
    }
  }
}
```

**Using a global install** (`npm install -g @engramo/mcp`):
```json
{
  "mcpServers": {
    "engram": {
      "command": "engramo-mcp",
      "env": {
        "ENGRAM_API_URL": "https://api.engramo.app",
        "ENGRAM_API_TOKEN": "your-token"
      }
    }
  }
}
```

## Cursor

Add to `~/.cursor/mcp.json`:

**Using npx** (no prior installation needed):
```json
{
  "mcpServers": {
    "engram": {
      "command": "npx",
      "args": ["-y", "@engramo/mcp"],
      "env": {
        "ENGRAM_API_URL": "https://api.engramo.app",
        "ENGRAM_API_TOKEN": "your-token"
      }
    }
  }
}
```

**Using a global install** (`npm install -g @engramo/mcp`):
```json
{
  "mcpServers": {
    "engram": {
      "command": "engramo-mcp",
      "env": {
        "ENGRAM_API_URL": "https://api.engramo.app",
        "ENGRAM_API_TOKEN": "your-token"
      }
    }
  }
}
```

## Use in ChatGPT (remote MCP over Streamable HTTP)

`engramo-mcp` also runs as a **remote** server, so it can be added to ChatGPT as a custom connector (developer
mode) without installing anything locally. This is the same binary — the `http` subcommand instead of the
default `stdio` — so a self-hosted deployment serves both Claude Desktop users (stdio) and ChatGPT users
(HTTP) from one codebase.

```bash
ENGRAM_API_URL=https://api.engramo.app MCP_BIND_ADDR=0.0.0.0:8080 engramo-mcp http
```

This serves MCP over Streamable HTTP at `POST /mcp`. Unlike `stdio` mode, there is **no global
`ENGRAM_API_TOKEN`** — every session authenticates with its own `Authorization: Bearer <token>` header, so one
deployment safely serves many users at once (each session's calls to the EngrAmo API use only that session's
token). Requests without a valid, non-empty bearer token are rejected with `401` before a session is created.

In ChatGPT: **Settings → Connectors → Advanced → Developer mode**, then add a custom connector pointing at
your deployment's `https://<host>/mcp`, pasting an EngrAmo API token (from Settings → API Tokens) as the
bearer token. Once connected, prompts like *"Make me a 10-card Spanish restaurant deck"* or *"Turn this
conversation into flashcards"* call `generate_catalog_with_cards` directly; open the result at
`https://study.engramo.app/catalog/<shortId>` to study it.

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
| `ENGRAM_API_URL` | `https://api.engramo.app` | Base URL of the EngrAmo API |
| `ENGRAM_API_TOKEN` | *(required for `stdio`)* | Your EngrAmo API token. Unused in `http` mode — each session supplies its own via the `Authorization: Bearer` header. |
| `MCP_BIND_ADDR` | `0.0.0.0:8080` | Bind address for `http` mode |
| `MCP_PUBLIC_URL` | *(unset)* | `http` mode only — this deployment's own public URL, e.g. `https://mcp.engramo.app/mcp`. Also allowlists that host for inbound requests; without it, every request is rejected (see `MCP_ALLOWED_HOSTS`). |
| `MCP_ALLOWED_HOSTS` | *(unset)* | `http` mode only — extra comma-separated hostnames/`host:port` values to permit, on top of the one derived from `MCP_PUBLIC_URL`. Only needed for extra entry points (e.g. a Cloud Run service's own `*.run.app` fallback URL alongside its custom domain). |

## Available tools

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

### AI generation (bring-your-own-AI)
| Tool | Description |
|---|---|
| `generate_card` | Create a flashcard — the calling model does any translation/wording itself |
| `generate_catalog_with_cards` | Create a catalog with cards in one call — same bring-your-own-AI model |
| `generate_cards` | Add multiple cards to an existing catalog — same bring-your-own-AI model |

## Resources and Prompts

The server also exposes **MCP Resources** (live data readable as context):

| URI | Description |
|---|---|
| `engram://card-schema` | CardContent JSON schema with validation rules and examples |
| `engram://catalogs` | All catalogs (id, name, card_count) |
| `engram://learning/due` | Cards due for review today |
| `engram://learning/stats` | Learning stats (due_count, total_count) |
| `engram://learning-paths` | All learning paths (id, name) |
| `engram://subscription` | User subscription/plan information |

And **MCP Prompts** (guided workflows):

| Prompt | Description |
|---|---|
| `review_session` | Start a guided spaced-repetition review session |
| `create_flashcard` | Create a high-quality flashcard for a topic |
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
