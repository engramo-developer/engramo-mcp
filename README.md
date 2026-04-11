[![npm version](https://img.shields.io/npm/v/@engram-fc/mcp)](https://www.npmjs.com/package/@engram-fc/mcp)
[![CI](https://github.com/volmyrdot/engram-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/volmyrdot/engram-mcp/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

# engram-mcp

MCP server for the [Engram](https://engram.volmyr.com) spaced-repetition flashcard platform.

## Prerequisites

- An Engram account
- An Engram API token — generate one at [https://engram.volmyr.com](https://engram.volmyr.com)

## Installation

Install the MCP server globally so it's available as a system command:

```bash
npm install -g @engram-fc/mcp
```

Verify the installation:

```bash
engram-mcp --version
```

> Alternatively, skip installation entirely and use `npx -y @engram-fc/mcp` in your client config — npm will download the binary automatically on first use.

## Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) or
`%APPDATA%\Claude\claude_desktop_config.json` (Windows):

**Using npx** (no prior installation needed):
```json
{
  "mcpServers": {
    "engram": {
      "command": "npx",
      "args": ["-y", "@engram-fc/mcp"],
      "env": {
        "ENGRAM_API_URL": "https://api-engram.volmyr.com",
        "ENGRAM_API_TOKEN": "your-token"
      }
    }
  }
}
```

**Using a global install** (`npm install -g @engram-fc/mcp`):
```json
{
  "mcpServers": {
    "engram": {
      "command": "engram-mcp",
      "env": {
        "ENGRAM_API_URL": "https://api-engram.volmyr.com",
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
      "args": ["-y", "@engram-fc/mcp"],
      "env": {
        "ENGRAM_API_URL": "https://api-engram.volmyr.com",
        "ENGRAM_API_TOKEN": "your-token"
      }
    }
  }
}
```

**Using a global install** (`npm install -g @engram-fc/mcp`):
```json
{
  "mcpServers": {
    "engram": {
      "command": "engram-mcp",
      "env": {
        "ENGRAM_API_URL": "https://api-engram.volmyr.com",
        "ENGRAM_API_TOKEN": "your-token"
      }
    }
  }
}
```

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `ENGRAM_API_URL` | `https://api-engram.volmyr.com` | Base URL of the Engram API |
| `ENGRAM_API_TOKEN` | *(required)* | Your Engram API token |

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

### AI generation
| Tool | Description |
|---|---|
| `generate_card` | Generate a flashcard with AI |
| `generate_catalog_with_cards` | Generate a catalog with cards using AI |
| `generate_cards` | Generate multiple cards for an existing catalog |

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
[github.com/volmyrdot/engram-mcp/issues](https://github.com/volmyrdot/engram-mcp/issues).
