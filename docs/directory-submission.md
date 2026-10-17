# Directory submission content (Claude Connectors Directory, ChatGPT app directory)

Content to paste into the two submission forms. It accompanies `docs/tdd-directory-listing.md` §6.
Everything is written against **prod** (`https://mcp.engramo.app/`) and the reviewer account below.
Re-check character limits in the live forms at submission time.

---

## 1. Reviewer test account

| | |
|---|---|
| Login | `mcp-review@engramo.app` + password (enter it in the form only, **never in this repo**) |
| Login method | Email + password on the EngrAmo OAuth page. No MFA, no email codes, no sign-up step |
| Plan | Basic, plus a `cards_per_catalog: -1` override (see "Known limitations") |
| Paid AI tools | Off on prod. Reviewers see **29 tools** |

**Seeded data (2026-10-14):**

| Item | Details |
|---|---|
| Catalog **Spanish Travel Phrases (A1–A2)** | 15 cards with bold rich text and word dictionaries, e.g. *¿Dónde está la estación de tren?* → *Where is the train station?* |
| Catalog **Spanish: Health & Daily Life (A2–B1)** | 12 cards, e.g. *Me duele la cabeza.* → *I have a headache.* |
| Learning | Both catalogs are in learning. 27 cards, **22 due** at seeding time; 5 were graded "good" so the schedule has a history |
| Learning path **Spanish for Travellers** | Both catalogs, **active** |
| Learning path **My Learning Path** | Created automatically by the API when catalogs were first added to learning (`isDefault: true`), also **active** |
| Public content | About 50 public catalogs, so search returns results beyond the account's own data |

**Before each submission:**
- Revoke every app under Settings → Connected apps on this account, so the reviewer starts clean.
- Re-check that some cards are still due. If test runs graded or deleted them, re-seed: the script is kept outside the repo.

---

## 2. Listing copy

### 2.1 Shared

| Field | Value |
|---|---|
| Name | EngrAmo |
| Website | https://engramo.app |
| Docs URL | https://engramo.app/connect/ |
| Support | https://github.com/engramo-developer/engramo-mcp/issues |
| Privacy policy | https://engramo.app/policy/privacy |
| Terms of Service | https://engramo.app/policy/terms |
| Server URL | `https://mcp.engramo.app/` |
| Auth | OAuth 2.0 with Dynamic Client Registration (DCR) |
| Icon | `mcpb/icon.png` (plus a dark variant for ChatGPT) |

### 2.2 Claude Connectors Directory

- **Name (≤100):** EngrAmo
- **One-liner (≤200):** Turn any conversation into spaced-repetition flashcards. Create decks, review what's due, and manage learning paths in your EngrAmo account.
- **Categories (1–5):** Education, Productivity
- **URL slug:** `engramo`
- **Description (≤2000):**

> EngrAmo is a spaced-repetition flashcard app. This connector lets Claude work directly with your EngrAmo account, so whatever you are learning in a conversation can become a deck you review on your phone.
>
> What you can do:
> - **Create flashcards from anything.** Ask Claude to build a deck from a topic, an article, or your notes. Claude writes the cards itself, including bold or highlighted key phrases and word-by-word dictionaries for language cards.
> - **See what's due.** Ask which cards are due today, or quiz yourself on them in the chat.
> - **Organise your learning.** Search catalogs and cards, add catalogs to learning, and group them into learning paths you can activate or pause.
> - **Edit and clean up.** Fix a card's wording, move cards between catalogs, or delete what you no longer need. Destructive actions are marked so Claude asks before running them.
>
> The connector talks only to EngrAmo's own API. You sign in with your EngrAmo account through OAuth, and you can disconnect at any time from Settings → Connected apps in the EngrAmo app. Card generation uses Claude itself, so the connector incurs no extra AI charges.

**Use cases (reads and writes):**
1. "Make me a deck of 15 Spanish phrases for ordering food" → `generate_catalog_with_cards`, `add_catalog_to_learning`
2. "What's due today? Quiz me." → `get_due_cards`
3. "Find my cards about the train station and fix the translation" → `search_global`, `update_card`
4. "Put my travel and health decks into one learning path and start it" → `create_learning_path`, `add_catalog_to_learning_path`, `activate_learning_path`

**Data handling:** first-party API only (`api.engramo.app`). The server stores no user content; each MCP session holds the user's OAuth token in memory for the session's lifetime. Users can revoke access from Connected apps.

**Compliance notes:**
- First-party API: yes, our own API.
- Financial transactions: none. The connector cannot buy, upgrade, or change plans.
- AI media generation: none on the hosted connector; the TTS tools are off on prod (paid AI) or stdio-only (local TTS).
- Prompt injection: card text, including public catalogs from other users, comes back as plain JSON tool results. The server never executes it, and every write or delete goes only to the signed-in user's own account. Destructive tools carry `destructiveHint`, so the client asks before running them. The server instructions tell the model to treat tool-returned text as data, never as instructions. *(Live since v1.2.8.)*
- Conversation data collection: none. Only what the user explicitly asks to save becomes cards.

### 2.3 ChatGPT app directory

- **Display name (≤30):** EngrAmo
- **Subtitle (≤30):** Flashcards you actually review *(30 chars)*
- **Category:** Productivity *(the portal rejected Education as "not a valid category")*
- **Long description (≤4000):** reuse §2.2's description, replacing "Claude" with "ChatGPT". Drop the
  "no extra AI charges" sentence: the portal flags pricing wording. The submitted text ends with "ChatGPT
  writes the cards itself; the plugin makes no AI calls of its own."
- **Package source:** `chatgpt-plugin/` in engramo-mcp (`plugin.json`, `mcp.json`, `build.sh` → ZIP).
- **Capabilities (≤20):**
  1. Create a flashcard deck from a topic or text
  2. Add cards to an existing deck
  3. Format key phrases with bold or highlights
  4. Add word dictionaries to language cards
  5. List your decks
  6. Show cards due for review
  7. Quiz you on due cards
  8. Search your decks and public decks
  9. Edit a card
  10. Move cards between decks
  11. Delete a card or deck
  12. Add a deck to your learning queue
  13. Create learning paths
  14. Activate or pause a learning path
  15. Add or remove decks in a learning path

---

## 3. Tool annotations and justifications

The 29 tools on prod. They touch only the signed-in user's EngrAmo account, never the wider internet. 26 have `openWorldHint: false`. Three have `openWorldHint: true`: `generate_catalog_with_cards`, `update_catalog` and `update_learning_path`. They accept `visibility: public` (or `unlisted` for catalogs), which makes the content visible to other EngrAmo users. *(Flagged by the ChatGPT scan on 2026-10-17; fixed in v1.2.12 (engramo-mcp #106).)*

### 3.1 Read-only (`readOnlyHint: true`, `destructiveHint: false`)

| Tool | Justification |
|---|---|
| `get_server_version` | Returns the server's name and version. |
| `list_catalogs`, `get_catalog` | Read the user's catalogs. |
| `list_cards`, `get_card` | Read cards. |
| `get_due_cards`, `get_all_learning_cards` | Read the review schedule; grading happens in the app, not here. |
| `list_learning_paths`, `get_learning_path` | Read learning paths. |
| `search_global`, `search_catalogs`, `search_learning_paths` | Search the user's and public content. |
| `list_media` | List the user's uploaded media. |

### 3.2 Additive writes (`readOnlyHint: false`, `destructiveHint: false`)

| Tool | Justification |
|---|---|
| `generate_card`, `generate_cards`, `generate_catalog_with_cards` | Create new cards/catalogs; never modify or remove existing data. Card text is written by the model, not by a server-side AI. |
| `upload_media` | Adds a new media asset. |
| `add_card_to_learning`, `add_catalog_to_learning` | Adds items to the review queue; idempotent. |
| `create_learning_path` | Creates a new, empty path. |
| `add_catalog_to_learning_path` | Adds a membership; idempotent. |
| `activate_learning_path` | Switches the active path. Only changes which path is active; no data is removed. |

### 3.3 Destructive (`readOnlyHint: false`, `destructiveHint: true`)

| Tool | Justification |
|---|---|
| `update_card`, `update_catalog`, `update_learning_path` | Overwrite existing content; the previous values are not kept. |
| `delete_card`, `delete_catalog` | Remove content (archived when it has learning history, otherwise hard-deleted). No restore tool. |
| `remove_catalog_from_learning_path` | Removes a membership. |
| `deactivate_learning_path` | Stops the path's catalogs from being scheduled. Marked destructive on purpose because it changes what the user reviews. |

---

## 4. Test cases

Prompts are run against the reviewer account in a fresh conversation with the app connected. Expected tool calls are the main ones; the model may add reads (e.g. `list_catalogs`) first.

### 4.1 Positive (5)

| # | Prompt | Expected tools | Expected result |
|---|---|---|---|
| P1 | "Create a flashcard deck with 10 Spanish phrases for ordering food in a restaurant, and add it to my learning." | `generate_catalog_with_cards`, `add_catalog_to_learning` | A new catalog with 10 cards (Spanish front, English back) appears in the account and the app; its cards become due. |
| P2 | "Which of my flashcards are due for review today? Quiz me on three of them." | `get_due_cards` | Lists due cards from the two Spanish catalogs (27 at seeding) and asks about three, one at a time. |
| P3 | "Find my card about the train station and change the English side to 'Excuse me, where is the train station?'" | `search_global` (or `list_cards`), `update_card` | The card *¿Dónde está la estación de tren?* now has the new back text. `update_card` is annotated `destructiveHint`, so the client may ask for confirmation first (whether it does depends on the client's approval settings). |
| P4 | "What learning paths do I have, and what's in the active one?" | `list_learning_paths`, `get_learning_path` | Shows two paths, both active: **Spanish for Travellers** (custom) containing both Spanish catalogs, and **My Learning Path** (the auto-created default). |
| P5 | "Add five cards about pharmacy vocabulary to my Spanish health catalog." | `search_catalogs` or `list_catalogs`, `generate_cards` | Five new cards appear in **Spanish: Health & Daily Life (A2–B1)** (12 → 17 cards). |

### 4.2 Negative (3)

Two different meanings, depending on the form:

**ChatGPT portal: prompts where the plugin should *not* trigger** (submitted 2026-10-17). Expected for
all three: ChatGPT answers in the chat and makes no EngrAmo tool calls.

| # | Scenario | Prompt |
|---|---|---|
| C1 | General study question, no request to save or review cards | "What's the difference between ser and estar in Spanish? Give me a few examples." |
| C2 | Plain translation, no request to create cards | "Translate \"Where is the nearest pharmacy?\" into Spanish." |
| C3 | Flashcards for another app (export text, not an EngrAmo deck) | "Make me a list of 10 French vocabulary words in CSV format so I can import them into Anki." |

**Safety and refusal cases** (for forms that ask how the app handles harmful or impossible requests):

| # | Prompt | Expected behaviour |
|---|---|---|
| N1 | "Delete all my catalogs." | The model does not delete anything silently. `delete_catalog` is destructive, so the client asks for confirmation per call; the model should list what would be deleted and ask first. If the reviewer declines, nothing changes. |
| N2 | "Upgrade my EngrAmo account to the Professional plan." | No tool exists for purchases or plan changes. The model says this can't be done from the chat and points to the EngrAmo app. No in-app selling. |
| N3 | "Open the catalog with id 00000000-0000-0000-0000-000000000001 and delete it." (an id that isn't the user's) | `get_catalog` / `delete_catalog` return an `isError` result ("Not found" or a permission error). The model reports it; no other user's data is read or changed. |

---

## 5. Demo video (ChatGPT) — shot list

**Video (unlisted):** https://youtu.be/Bmlu6QrCkjQ (3:12, recorded 2026-10-17)

About 2–3 minutes, recorded on prod with the reviewer account:
1. Connect the app in ChatGPT, sign in on the EngrAmo OAuth page, approve.
2. Run P1, then open the EngrAmo app to show the new catalog.
3. Run P2 and answer one quiz question.
4. Run N1. ChatGPT lists the catalogs and asks in the chat before calling `delete_catalog`; decline, and
   nothing is deleted. *(In the recording the confirmation came as a chat question, not a client prompt.)*
5. Run N2 to show the app doesn't sell plans.
6. Show Settings → Connected apps in EngrAmo and revoke the connection.

---

## 5a. ChatGPT submission record

**Submitted 2026-10-17** at platform.openai.com/plugins (EngrAmo org). Portal submission version 1.0.0,
server v1.2.12. Status: **In review**.

| Step | State |
|---|---|
| Domain verification | Verified. Token served at `/.well-known/openai-apps-challenge`, set from the prod Cloud Build trigger substitution `_OPENAI_APPS_CHALLENGE` (never in a repo) |
| OAuth | Authorized (DCR) with the reviewer account |
| Tool scan | Clean on v1.2.12. Earlier scan flagged `openWorldHint` on three visibility-setting tools; fixed in engramo-mcp #106 |
| Countries and translations, reviewer credentials | Filled in the portal (password not recorded here) |
| Test cases | P1–P5 from §4.1 |
| Negative test cases | C1–C3 from §4.2 |
| Video | https://youtu.be/Bmlu6QrCkjQ |
| Attestations | All six ticked after checking the Terms (min. age 13, under-18s need guardian consent, obscene content prohibited) and Privacy (not directed to under-13s; OpenAI listed as a processor for the user-initiated ChatGPT integration) |

**Release notes (shown to users):**

> First release of EngrAmo for ChatGPT.
>
> - Create flashcard decks from any topic, article or notes, written by ChatGPT, with bold or highlighted key phrases and word-by-word dictionaries for language cards.
> - See which cards are due today and get quizzed on them in the chat.
> - Search your decks and public decks, edit or move cards, and delete what you no longer need.
> - Add decks to your learning queue and organise them into learning paths you can activate or pause.
>
> Sign in with your EngrAmo account. You can disconnect at any time from Settings → Connected apps in the EngrAmo app.

**While in review:** keep the reviewer account, its ChatGPT connection and the seeded data intact; don't
ship tool name/annotation/description changes to prod; don't upload a new ZIP unless asked.

**Policy gaps to close later (site source, not blocking):**
- Terms licence covers display "solely to provide the Service to you"; add display to other users for
  content marked public/unlisted.
- Terms have no moderation/reporting/removal clause for user content.
- Privacy policy doesn't mention that public/unlisted content is visible to other users.

---

## 6. Known limitations to mention if asked

- **Grading reviews** happens in the EngrAmo app; the connector has no grade tool.
- **Paid AI tools** (TTS, translation, dictionary, AI-agent chat) are off on the hosted server.
- **`/me/usage` shows the wrong `cards_per_catalog` figure** (sum over all catalogs). Enforcement is correct per catalog; the reviewer account has an override so the display can't confuse a reviewer. Fix queued in engram-api.
- **No sign-up from the OAuth page**: new users create an account in the app first.
