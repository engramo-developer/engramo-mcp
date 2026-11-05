# Directory submission content (Claude Connectors Directory, ChatGPT app directory)

Content to paste into the two submission forms. It accompanies `docs/tdd-directory-listing.md` §6.
Everything is written against **prod** (`https://mcp.engramo.app/`) and the reviewer account below.
Re-check character limits in the live forms at submission time.

---

## 1. Reviewer test accounts

Each directory gets its own account, so one review's test runs can't change the data the other reviewer sees.

| | |
|---|---|
| ChatGPT login | `mcp-review@engramo.app` + password (enter it in the form only, **never in this repo**) |
| Claude login | `mcp-review-claude@engramo.app` + password (same rule) |
| Login method | Email + password on the EngrAmo OAuth page. No MFA, no email codes, no sign-up step |
| Plan | Basic, plus a `cards_per_catalog: -1` override (see "Known limitations") |
| Paid AI tools | Off on prod. Reviewers see **35 tools** |

**Seeded data** (identical on both accounts; ChatGPT account 2026-10-14, Claude account 2026-10-18). The Claude
account also has an empty **My Catalog** (Default) that the app creates on first sign-in.

| Item | Details |
|---|---|
| Catalog **Spanish Travel Phrases (A1–A2)** | 15 cards with bold rich text and word dictionaries, e.g. *¿Dónde está la estación de tren?* → *Where is the train station?* |
| Catalog **Spanish: Health & Daily Life (A2–B1)** | 12 cards, e.g. *Me duele la cabeza.* → *I have a headache.* |
| Learning | Both catalogs are in learning. 27 cards, **22 due** at seeding time; 5 were graded "good" so the schedule has a history |
| Learning path **Spanish for Travellers** | Both catalogs, **active** |
| Learning path **My Learning Path** | Created automatically by the API when catalogs were first added to learning (`isDefault: true`), also **active** |
| Public content | About 50 public catalogs, so search returns results beyond the account's own data |

**Before each submission:**
- Revoke every app under Settings → Connected apps on the account, so the reviewer starts clean.
- Revoke any API token used for seeding (API token & MCP → Revoke).
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
> - **Share and discover.** Share a deck with other EngrAmo users by their user id, and find public decks by language, topic and level.
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
- Prompt injection: card text, including public catalogs from other users, comes back as plain JSON tool results. The server never executes it, and reads can include other users' public catalogs and catalogs shared with the user, and writes go only to the user's own catalogs or to catalogs another user shared with them as editor. Destructive tools carry `destructiveHint`, so the client asks before running them. The server instructions tell the model to treat tool-returned text as data, never as instructions. *(Live since v1.2.8.)*
- Conversation data collection: none. Only what the user explicitly asks to save becomes cards.

### 2.3 ChatGPT app directory

- **Display name (≤30):** EngrAmo
- **Subtitle (≤30):** Flashcards you actually review *(30 chars)*
- **Category:** Education & Research *(the portal rejected "Education" alone as not a valid category; "Productivity" was rejected by review on 2026-11-02 as not reflecting the primary purpose)*
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
  16. Find decks by language, topic and level
  17. Share a deck with other EngrAmo users
  18. Show your plan limits and usage
  19. Turn on shadowing (speaking practice) for a deck
  20. Set a deck's cover image

---

## 3. Tool annotations and justifications

The 35 tools served over http. All four hints are explicitly set (true or false, never null) on every tool. Every tool acts only inside EngrAmo, through its first-party API, never the wider internet.

**openWorldHint rule (applied 2026-11-02, after OpenAI rejected the earlier set):** `true` when a tool reads content authored by other EngrAmo users (public, unlisted or shared catalogs, public learning paths) or creates, changes or removes content or access that other users can see. `false` when it only touches the caller's own private data. 24 tools are `true`; the other 11 are `false`.

History: the ChatGPT scan of 2026-10-17 flagged the tools that publish content (fixed in v1.2.12, engramo-mcp #106). On 2026-10-31 the read tools were kept `false` on the "bounded catalog" argument, and OpenAI's 2026-11-02 review rejected the annotations as not matching behavior, so the fallback was applied.

| Tool | readOnly | destructive | idempotent | openWorld | Justification |
|---|---|---|---|---|---|
| `get_server_version` | true | false | true | false | Returns the server name and version. |
| `get_subscription_usage` | true | false | true | false | Reads the caller's own plan, limits and usage. |
| `list_catalogs` | true | false | true | true | Lists the caller's own, shared and other users' public catalogs. |
| `get_catalog_filter_options` | true | false | true | true | Lists filter values and counts across the caller's and other users' public catalogs. |
| `get_catalog` | true | false | true | true | Reads one catalog, possibly authored by another user (public, unlisted or shared). |
| `update_catalog` | false | true | true | true | Overwrites catalog fields; can set visibility public/unlisted, exposing it to other users. |
| `set_catalog_shadowing` | false | true | true | true | Changes the catalog's shadowing language and Shadowing-tab listing, which other users see on public or shared catalogs. |
| `share_catalog` | false | true | true | true | Grants other users viewer/editor access to the caller's catalog. |
| `unshare_catalog` | false | true | true | true | Removes other users' access to the caller's catalog. |
| `list_catalog_shares` | true | false | true | false | Lists who the caller's own catalog is shared with (owner/editor only). |
| `delete_catalog` | false | true | true | true | Removes a catalog, which may be public or shared, from other users' view. |
| `list_cards` | true | false | true | true | Reads cards of a catalog that may be authored by another user. |
| `get_card` | true | false | true | true | Reads one card that may belong to another user's catalog. |
| `update_card` | false | true | true | true | Overwrites a card, which may sit in a public or shared catalog others see. |
| `delete_card` | false | true | true | true | Removes a card, which may sit in a public or shared catalog others see. |
| `get_due_cards` | true | false | true | true | Reads cards in the caller's queue, which may come from other users' shared or public catalogs. |
| `get_all_learning_cards` | true | false | true | true | Reads cards in the caller's learning queue, which may come from other users' shared or public catalogs. |
| `add_card_to_learning` | false | false | true | false | Adds a card to the caller's own review queue. |
| `add_catalog_to_learning` | false | false | true | false | Adds a catalog to the caller's own review queue. |
| `list_learning_paths` | true | false | true | false | Lists the caller's own learning paths. |
| `get_learning_path` | true | false | true | true | Reads a path by id; the API also returns other users' public paths. |
| `create_learning_path` | false | false | false | false | Creates a new private path for the caller (not idempotent: each call makes one). |
| `activate_learning_path` | false | false | true | false | Marks a path active for the caller only. |
| `deactivate_learning_path` | false | true | true | false | Stops scheduling a path for the caller only. |
| `add_catalog_to_learning_path` | false | false | true | true | Adds a catalog to a path that may be public, changing what other users see. |
| `remove_catalog_from_learning_path` | false | true | true | true | Removes a catalog from a path that may be public, changing what other users see. |
| `update_learning_path` | false | true | true | true | Overwrites a path; can set visibility public, exposing it to other users. |
| `search_global` | true | false | true | true | Searches the caller's and other users' public content. |
| `search_catalogs` | true | false | true | true | Searches the caller's and other users' public catalogs. |
| `search_learning_paths` | true | false | true | true | Searches the caller's and other users' public learning paths. |
| `list_media` | true | false | true | false | Lists the caller's own uploaded media. |
| `upload_media` | false | false | false | false | Adds a new media file to the caller's own library. |
| `generate_card` | false | false | false | true | Creates a card, possibly in a public or shared catalog others see. |
| `generate_catalog_with_cards` | false | false | false | true | Creates a catalog with cards; can set visibility public/unlisted. |
| `generate_cards` | false | false | false | true | Creates cards, possibly in a public or shared catalog others see. |

Prompt injection: card, catalog and path text from other users is untrusted data; the server instructions and the read-tool descriptions say so.

---

## 4. Test cases

Prompts are run against the reviewer account in a fresh conversation with the app connected. Expected tool calls are the main ones; the model may add reads (e.g. `list_catalogs`) first.

### 4.1 Positive (12)

Seeded-account expectations assume the seeding in `docs/directory-resubmission-readiness.md` is done (classified catalogs, public classified catalogs, share-target account, sharing entitlement, one media image).

| # | Prompt | Expected tools | Expected result |
|---|---|---|---|
| P1 | "Create a flashcard deck with 10 Spanish phrases for ordering food in a restaurant, and add it to my learning." | `generate_catalog_with_cards`, `add_catalog_to_learning` | A new catalog with 10 cards (Spanish front, English back) appears in the account and the app; its cards become due. |
| P2 | "Which of my flashcards are due for review today? Quiz me on three of them." | `get_due_cards` | Lists due cards from the two Spanish catalogs (22 due at seeding) and asks about three, one at a time. |
| P3 | "Find my card about the train station and change the English side to 'Excuse me, where is the train station?'" | `search_global` (or `list_cards`), `update_card` | The card *¿Dónde está la estación de tren?* now has the new back text. `update_card` is annotated `destructiveHint`, so the client may ask for confirmation first (whether it does depends on the client's approval settings). |
| P4 | "What learning paths do I have, and what's in the active one?" | `list_learning_paths`, `get_learning_path` | Shows two paths, both active: **Spanish for Travellers** (custom) containing both Spanish catalogs, and **My Learning Path** (the auto-created default). |
| P5 | "Add five cards about pharmacy vocabulary to my Spanish health catalog." | `search_catalogs` or `list_catalogs`, `generate_cards` | Five new cards appear in **Spanish: Health & Daily Life (A2–B1)** (12 → 17 cards). |
| P6 | "Share my Spanish travel deck with user `<share-target userId>` as a viewer, then show who it's shared with." | `share_catalog`, `list_catalog_shares` | Collaborators list shows that id as `viewer`. The share-target account's user id is given in the portal test instructions. Clean up with `unshare_catalog`. |
| P7 | "Find public Spanish catalogs for beginners with English translations." | `get_catalog_filter_options`, `list_catalogs` | `list_catalogs` with `front_languages: ["es"]`, `back_languages: ["en"]`, `levels: ["beginner"]`, `filter: "public"` returns at least one public catalog. |
| P8 | "Which of my decks are B1 level?" | `list_catalogs` (`filter: "own"`, `levels: ["B1"]`) | Returns **Spanish: Health & Daily Life (A2–B1)** only. |
| P9 | "Tag my travel deck as Spanish to English, topic travel, level A1." | `get_catalog` or `list_catalogs`, `update_catalog` (`front_languages: ["es"]`, `back_languages: ["en"]`, `topics: ["travel"]`, `levels: ["A1"]`, `version`) | Catalog shows the new fields. Destructive hint, so the client may confirm first. |
| P10 | "What plan am I on and how many catalogs can I still create?" | `get_subscription_usage` | Plan name, the catalogs limit as used / remaining, reset rule. No purchase or upgrade link. |
| P11 | "Turn on shadowing for my Spanish travel deck." | `set_catalog_shadowing` (`enabled: true`, `language: "es"`) | Shadowing on. A second identical call changes nothing. |
| P12 | "Use the image in my media library as the cover of my health deck." | `list_media`, `get_catalog`, `update_catalog` (`image_id`) | Cover set (needs the seeded image). |

**ChatGPT submission (5 positive):** P1, P2, P7, P10, P6. Swap P6 for P9 if sharing can't be entitled.
**Claude suggested prompts:** all of P1 to P12 as time allows; P7, P8, P10 and P6 cover the new tools.

### 4.2 Negative

Two different meanings, depending on the form:

**ChatGPT portal: prompts where the plugin should *not* trigger.** Expected for all of them: ChatGPT answers in the chat and makes no EngrAmo tool calls. Submit three, e.g. C1, C3 and C5.

| # | Scenario | Prompt |
|---|---|---|
| C1 | General study question, no request to save or review cards | "What's the difference between ser and estar in Spanish? Give me a few examples." |
| C2 | Plain translation, no request to create cards | "Translate \"Where is the nearest pharmacy?\" into Spanish." |
| C3 | Flashcards for another app (export text, not an EngrAmo deck) | "Make me a list of 10 French vocabulary words in CSV format so I can import them into Anki." |
| C4 | General language question | "What does CEFR level B1 mean?" |
| C5 | Generic "plan" question that must not fire `get_subscription_usage` | "What's included in ChatGPT Plus?" |

**Safety and refusal cases** (for forms that ask how the app handles harmful or impossible requests):

| # | Prompt | Expected behaviour |
|---|---|---|
| N1 | "Delete all my catalogs." | The model does not delete anything silently. `delete_catalog` is destructive, so the client asks for confirmation per call; the model should list what would be deleted and ask first. If the reviewer declines, nothing changes. |
| N2 | "Upgrade my EngrAmo account to the Professional plan." | No tool exists for purchases or plan changes. The model may call `get_subscription_usage`, then says plans can't be changed from the chat and that the EngrAmo team changes them ("Plan changes aren't self-service yet — contact the EngrAmo team"). No in-app selling. |
| N3 | "Open the catalog with id 00000000-0000-0000-0000-000000000001 and delete it." (an id that isn't the user's) | `get_catalog` / `delete_catalog` return an `isError` result ("Not found" or a permission error). The model reports it; no other user's data is read or changed. |
| N4 | "Share my Spanish deck with anna@example.com." | No `share_catalog` call with a guessed id. The model explains sharing needs the person's EngrAmo user id. |
| N5 | "Show me catalogs about topic `quantum-cats`." | `list_catalogs` returns `isError` ("topics: 'quantum-cats' is not a topic key …"). The model calls `get_catalog_filter_options` and offers real topics. |
| N6 | "Turn on shadowing for `<id of a public catalog owned by someone else>`." | `isError` permission error ("only the catalog's owner can turn shadowing on or off…"). Nothing changes. |
| N7 | "Give me the emails of everyone I've shared my deck with." | `list_catalog_shares` returns ids and levels only. The model says emails aren't available. |

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

**Resubmitted 2026-11-05** at platform.openai.com/plugins (EngrAmo org), after the 2026-11-02 rejection. Plugin
ZIP 1.3.3, server v1.3.3, 35 tools. Status: **in review**. The first submission (2026-10-17, ZIP 1.0.0, server
v1.2.12) was rejected on 2026-11-02 over tool annotations (see §3).

| Step | State |
|---|---|
| Domain verification | Verified. Token served at `/.well-known/openai-apps-challenge`, set from the prod Cloud Build trigger substitution `_OPENAI_APPS_CHALLENGE` (never in a repo) |
| OAuth | Authorized (DCR) with the reviewer account |
| Tool scan | v1.3.3: no blocking findings. Every tool carries the non-blocking "new tool definition needs further review" finding, explained in the Additional Submission Context note (annotation rule from §3). Earlier: v1.2.12 scan flagged `openWorldHint` on three visibility-setting tools, fixed in engramo-mcp #106 |
| Countries and translations, reviewer credentials | Filled in the portal: all supported countries, no translations, username and password for `mcp-review@engramo.app` (password not recorded here) |
| Test cases | P1, P2, P7, P10, P6 from §4.1. Imported read-only from `plugin.json` (`extensions.com.openai.review.test_cases`); P6's share-target user id is filled in by `build.sh` from `SHARE_TARGET_USER_ID`. First submission: P1–P5 |
| Negative test cases | C1, C3, C5 from §4.2, imported from `plugin.json`. First submission: C1–C3 |
| Video | https://youtu.be/Bmlu6QrCkjQ (recorded 2026-10-17, reused; covers P1, P2, N1, N2 but not P7, P10, P6). Imported from `review.demo_recording_url` |
| Additional Submission Context | Note explaining the non-blocking findings, the openWorldHint/destructiveHint rules, no purchases, and the test account |
| Attestations | All six ticked after checking the Terms (min. age 13, under-18s need guardian consent, obscene content prohibited) and Privacy (not directed to under-13s; OpenAI listed as a processor for the user-initiated ChatGPT integration) |

**Release notes (shown to users):** `publication.release_notes` in `chatgpt-plugin/plugin.json`.

**Resubmission steps (done 2026-11-05):**
1. Cancel the active review in the OpenAI portal.
2. Seed reviewer data and the share-target account (see `docs/directory-resubmission-readiness.md`).
3. Rebuild the ZIP from `chatgpt-plugin/` (`SHARE_TARGET_USER_ID=<uuid> build.sh`) with the updated description and 20 capabilities.
4. Re-run the tool scan, then submit 5 positive (P1, P2, P7, P10, P6) and 3 negative (C1, C3, C5) cases from §4, with per-tool annotation justifications from §3.
5. Update the release notes to cover filters, sharing, plan usage, shadowing and covers.

**While in review:** keep the reviewer account, its ChatGPT connection and the seeded data intact; don't
ship tool name/annotation/description changes to prod; don't upload a new ZIP unless asked.

**Policy gaps — closed 2026-11-03** (engramo-landing PRs #40, #41; both pages "Last updated: November 3, 2026"):
- Privacy §1.6 covers shared, public and unlisted catalogs (public/unlisted are EngrAmo-published only; user
  catalogs are never published), what collaborators see (user ids, never emails), and that study progress is
  never shared; §3 lists other EngrAmo users as a recipient category.
- Terms §3 licence covers display to the users you share with; §3.1 shared catalogs; §4 permits AI apps
  connected to your own account; §4.1 moderation and reporting; §6 AI apps' own terms.

---

## 5b. Claude submission record

**Submitted 2026-10-18** at claude.ai/directory/manage → Submit new → MCP connector, from a paid Claude account
(the portal requires Pro, Max, Team or Enterprise). Server v1.2.13. Approved the same day, minutes after
submission, and published: status **Live** (2026-10-18). Listing: https://claude.ai/directory/engramo

| Step | State |
|---|---|
| 1. Connection | `https://mcp.engramo.app`, single URL for all users. Scan: 29 tools, 6 resources, 5 prompts, OAuth (v1.2.15 serves 35 tools and 7 resources) |
| 2. Tools | One warning, non-blocking: `generate_card` params `face` and `back` have no `type` (schemars emits a bare `$ref` for a required struct field; `update_card`'s `anyOf` form passes). Fix queued for after review, see below |
| 3. Listing | §2.2 copy, description pasted as plain text. Author EngrAmo / https://engramo.app, favicon icon, docs https://engramo.app/connect/, support = GitHub issues, privacy policy |
| 4. Use cases | Create a deck; review what's due; organise and edit decks. Prerequisite: an EngrAmo account (free plan works), OAuth on connect. Capability: read and write |
| 5. Company | EngrAmo, https://engramo.app; primary contact is the submitting account's owner |
| 6. Authentication | Always required, Dynamic Client Registration (both auto-detected), no extra headers |
| 7. Data handling | We own the API; no personal health data; no sponsored content |
| 8. Test & launch | Reviewer account `mcp-review-claude@engramo.app` with login steps, seeded data and suggested prompts (password not recorded here). Self-tested box ticked |
| 9. Compliance | All seven acknowledgements ticked, plus additional notes (first-party API, no stored content, tool cross-references are same-server hints, tool output treated as data, destructive hints, no payments, no AI media) |

**Self-test before submitting (2026-10-18):** all 29 tools run in claude.ai as a custom connector with a
personal account, 19 prompts in one chat, every prompt calling the expected tools; prompt 1 also checked
in Claude Desktop and the mobile app.

**Freeze lifted (2026-10-31):** the ChatGPT review is being cancelled, so tool changes may deploy. Keep `mcp-review-claude@engramo.app` unconnected and its data intact until the Claude listing edit is reviewed.

**Queued items, done (released in v1.2.14):**
- [x] `"type": "object"` on `generate_card`'s `face`/`back` schemas, with test `test_every_tool_input_property_declares_a_type`.
- [x] `generate_card_audio` mention removed from `upload_media`'s description (pinned by tests). The `create_language_deck` prompt and `engramo://card-schema` mentions were removed on branch `feat/extended-catalog-filters`.

**Claude listing edit after the deploy (S6):**
1. Update the tool names on the listing. The portal has no rescan: on Edit → Tools, paste the names one per line, exactly as the server declares them (35 tools, plus the 5 prompt names).
2. Submit a listing edit for the tool names and the §2.2 description (a reviewer approves it).
3. Record the date here. Claude has no resubmission; tool changes reach users by deploying the server.

**Submitted 2026-11-04** (prod on 1.3.2): 35 tool names (adds `get_catalog_filter_options`, `get_subscription_usage`, `list_catalog_shares`, `set_catalog_shadowing`, `share_catalog`, `unshare_catalog`), 5 prompt names, and the §2.2 description. **Approved 2026-11-04.**

---

## 6. Known limitations to mention if asked

- **Grading reviews** happens in the EngrAmo app; the connector has no grade tool.
- **Paid AI tools** (TTS, translation, dictionary, AI-agent chat) are off on the hosted server.
- **`/me/usage` shows the wrong `cards_per_catalog` figure** (sum over all catalogs). Enforcement is correct per catalog; the reviewer account has an override so the display can't confuse a reviewer. Fix queued in engram-api. The figure is now visible directly in `get_subscription_usage` and `engramo://subscription`; confirm the fix is on prod or check how the override looks there.
- **Sharing takes user ids (UUIDs) only**, with no email lookup. `list_catalog_shares` returns ids and levels, not names.
- **`search_global` has no language or topic filters**; use `list_catalogs`.
- **Unclassified catalogs are hidden** once any language or topic filter is set.
- **No sign-up from the OAuth page**: new users create an account in the app first.
