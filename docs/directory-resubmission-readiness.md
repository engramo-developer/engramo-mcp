# Directory resubmission readiness audit (2026-10-31)

Scope: what the **http-mode** server (`build_session_server`, `src/server.rs:110-118`, paid AI off, no TTS) exposes
on branch `feat/extended-catalog-filters` (PR #126, Cargo version 1.2.15), checked against
`docs/directory-submission.md` and the current Claude and OpenAI directory guidance. The inventory comes from a live
`initialize` + `tools/list` + `resources/list` + `prompts/list` against a local build with paid AI off and no TTS
keys. That build has the same router as `build_session_server`, because `EngramoMcpServer::new(client, false)` is
the only call either path makes.

## User actions (in order)

Everything left after the in-repo fixes (2026-10-31) is yours. Do these in order:

1. Merge PR #126.
2. Run `/release` (do not bump versions by hand; `chatgpt-plugin/plugin.json` `version` is bumped there).
3. Deploy through the Cloud Build trigger.
4. Verify: `curl https://mcp.engramo.app/version` matches, `tools/list` returns **35** tools, then self-test the new tools in claude.ai as a custom connector (B1).
5. Seed reviewer data on both reviewer accounts: classify the two catalogs, a classified public subset, one media image, shadowing left off on one catalog (B3, seeding section).
6. Create the share-target account and entitle both reviewer accounts to share (`catalog_share` on, limits at least 1); verify with `get_subscription_usage`; put the target's userId in the portal test instructions (B4).
7. Update the privacy policy and Terms for public, unlisted and shared content (S4).
8. Cancel the active ChatGPT review in the OpenAI portal (B2; decided).
9. Resubmit ChatGPT: rebuild the ZIP (`chatgpt-plugin/build.sh`), 5 positive + 3 negative cases per `docs/directory-submission.md` §4 and §5a.
10. Edit the Claude listing (tool names and description, `docs/directory-submission.md` §2.2) and re-scan the Tools tab (S6).

Also: check the OAuth token lifetime covers a review session (N5, not changed here).

## Status of checklist items (2026-10-31)

| Item | Status |
|---|---|
| B1 | User action (steps 1-4) |
| B2 | Decided: cancel and resubmit (steps 8-9); `docs/directory-submission.md` §5a updated |
| B3 | User action (step 5) |
| B4 | User action (step 6); P6 rewritten in-repo |
| S1 | **Done** (§2.2 compliance note, §3 intro, `list_catalogs` row) |
| S2 | **Done** (§4 rewritten: P1-P12, C1-C5, N1-N7, N2 fixed, counts corrected) |
| S3 | **Done** in-repo (description, 20 capabilities, `plugin.json` text; versions untouched); portal edits are steps 9-10 |
| S4 | User action (step 7) |
| S5 | **Done** (mentions removed from `create_language_deck` and `engramo://card-schema`; tests updated) |
| S6 | Docs **done**; portal listing edit is step 10 |
| S7 | **Done** in §6 (backend `cards_per_catalog` check remains a user check) |
| S8 | **Done** (decision and rationale in `docs/directory-submission.md` §3; no annotation changed) |
| N1 | Skipped: changing `visibility` / `media_type` from `String` to enums would reject previously accepted values at the protocol layer and touch many call sites; not worth the risk |
| N2 | Skipped: descriptions are pinned by tests and shared wording; no length limit exists, so the risk outweighs the benefit |
| N3 | **Done** (`get_catalog` description) |
| N4 | **Done** (§3.2 wording) |
| N5 | Skipped by decision (OAuth, out of repo) |

## Summary verdict

**Not ready to resubmit yet.** The code is in good shape. All 35 tools have `title`, `annotations.title` and all four
hints set explicitly. Every top-level property has `type` or `anyOf`, and every enum is typed. No tool description
names a tool that is missing in http mode. The blockers are all outside the code:

- prod still runs **v1.2.13 (29 tools)**;
- the ChatGPT review is still active, and OpenAI allows only one review at a time;
- the reviewer accounts have no data for the new filter and sharing tools.

The docs also need a refresh: the §3 claims, §4 test cases, listing copy and `chatgpt-plugin/`.

Counts: **4 Blocker · 8 Should · 5 Nice**.

### Live inventory (http mode)

- **35 tools.**
  - **16 read-only:** `get_server_version`, `get_subscription_usage`, `list_catalogs`, `get_catalog_filter_options`, `get_catalog`, `list_catalog_shares`, `list_cards`, `get_card`, `get_due_cards`, `get_all_learning_cards`, `list_learning_paths`, `get_learning_path`, `search_global`, `search_catalogs`, `search_learning_paths`, `list_media`.
  - **9 additive writes:** `generate_card`, `generate_cards`, `generate_catalog_with_cards`, `upload_media`, `add_card_to_learning`, `add_catalog_to_learning`, `create_learning_path`, `add_catalog_to_learning_path`, `activate_learning_path`.
  - **10 destructive:** `update_card`, `update_catalog`, `update_learning_path`, `delete_card`, `delete_catalog`, `set_catalog_shadowing`, `share_catalog`, `unshare_catalog`, `remove_catalog_from_learning_path`, `deactivate_learning_path`.
  - **4 have `openWorldHint: true`:** `generate_catalog_with_cards`, `update_catalog`, `update_learning_path`, `share_catalog`.
  - These counts match the §3 tables (`docs/directory-submission.md:117-159`). Every tool is listed in the right group, and no listed tool is stale.
- **7 resources.** `engramo://catalog-filters` is new. The Claude scan recorded 6 (`docs/directory-submission.md:261`).
- **5 prompts.** Unchanged.
- **Prod today:** `curl https://mcp.engramo.app/version` → `{"name":"engramo-mcp","version":"1.2.13"}`. `git show v1.2.13:src/server.rs` has 29 `#[tool(` blocks. Tags v1.2.14 and v1.2.15 (34 tools) were never deployed.
- **Unchanged since the approved listing, and fine:** OAuth metadata (`/.well-known/oauth-protected-resource`, plus the AS metadata at `api.engramo.app` with DCR and PKCE S256), the 401 response with `WWW-Authenticate: Bearer resource_metadata=…`, and `/version`.

---

## Prioritized checklist

### Blocker

**B1. The new tools are not on prod.**
- **Where:** prod `/version` = 1.2.13. `docs/directory-submission.md:19` and `:119` say "35 tools on prod".
- **Fix:**
  1. Merge PR #126.
  2. Cut the release (`/release`).
  3. Deploy through the Cloud Build trigger.
  4. Check that `/version` matches and that `tools/list` returns 35 tools, then self-test every new tool in claude.ai as a custom connector. The portal's Test & launch step asks you to confirm this.
- **Owner:** user / release + deploy (out-of-repo trigger).
- **Note:** confirm the prod API serves the backend routes. `GET api.engramo.app/catalogs/facets`, `/catalogs/facets/schema` and `/catalogs/{id}/collaborators` return **401**, while an unknown path returns **404**. So the routes exist. A logged-in check is still needed.

**B2. The ChatGPT review is still active, and only one is allowed at a time.**
- **Where:** `docs/directory-submission.md:215-243`. Submitted 2026-10-17 on the v1.2.12 tool set (29 tools), with P1–P5 / C1–C3.
- **Rule:** OpenAI says "Only one review can be active per plugin. To replace a package under review, wait for the decision or cancel the review before uploading."
  - MCP tool changes are re-scanned automatically.
  - New metadata (capabilities, descriptions) needs a new ZIP.
  - Once the app is published, tool changes deploy and re-scan; new metadata needs a fresh ZIP and possibly another review.
- **Fix:** decide one of these:
  - (a) Wait for the decision. If the app is approved, deploy B1 so tools re-scan, then upload a new ZIP (S3).
  - (b) Cancel the review and resubmit with the 35-tool set, the new test cases (§4 below) and a new ZIP.

  Either way, don't deploy B1 while the reviewer is mid-test without deciding first. The reviewer would see 35 tools against 29 submitted.
- **Owner:** user / OpenAI portal.
- **Claude side:** there is no resubmission. Per "Manage your listing after publishing", you "deploy the change to your server". The tool names on the listing change through a listing edit, which a reviewer approves (S6).

**B3. The reviewer accounts have no classification data, so the filter tools come back empty.**
- **Where:**
  - `docs/directory-submission.md:21-31`: the seeded catalogs have no front or back languages, topics or levels.
  - The `list_catalogs` description (`src/server.rs:233`) itself says "Many personal catalogs have no languages or topic set and are hidden as soon as one of those filters is used."
  - P7 (`:175`) depends on public catalogs tagged `es`, `beginner` and `medicine`, and on the `medicine` topic key existing in the prod DB.
- **Fix:** seed the data in the seeding section below, and confirm P7 returns at least one catalog on both accounts.
- **Owner:** out-of-repo (seeding script / DB).

**B4. P6 (share) can't be run by a reviewer as written, and may hit the plan limit.**
- **Where:**
  - `docs/directory-submission.md:174` says "Use a second test account's user id". Reviewers don't have one.
  - Sharing is plan-gated: `catalog_share` feature, plus `shared_catalogs` and `collaborators_per_catalog` limits (`src/subscription.rs:24-33`). The reviewer accounts are on Basic plus one override (`:18`).
  - If Basic lacks `catalog_share`, `share_catalog` returns "Not included in your plan…".
  - Claude's review criteria say every tool "must return a successful response when called with valid parameters".
- **Fix:**
  1. Create a dedicated share-target account and put its userId in the test instructions and in P6.
  2. Make sure both reviewer accounts are entitled to share (feature on, limits ≥ 1).
  3. Check with `get_subscription_usage` on each account.
- **Owner:** out-of-repo (DB plan or override) + in-repo docs.

### Should

**S1. The §3 and compliance claims are no longer accurate since sharing and public reads were added.**
- **Where:**
  - `docs/directory-submission.md:119`: "They touch only the signed-in user's EngrAmo account".
  - `:86`: "every write or delete goes only to the signed-in user's own account".
  - `:127`: "Read the user's catalogs".
- **Why they're wrong:** `list_catalogs`, `get_catalog`, `get_catalog_filter_options` and `search_*` read other users' public content and catalogs shared with the user. An editor's `update_card` / `update_catalog` / `generate_cards` write into a catalog **someone else owns**. `share_catalog` exposes content to chosen users.
- **Fix:** reword along these lines:
  - §3 intro: "Every tool acts only inside EngrAmo, through its first-party API, never the wider internet. Reads can include other users' public catalogs and catalogs shared with the user. Writes go to the user's own catalogs, or to catalogs another user shared with them as editor."
  - Add the open-world rationale (see the openWorldHint section).
  - Change `:127` to "Read the user's own, shared and public catalogs".
- **Owner:** in-repo docs.

**S2. The §4 test cases are stale and inconsistent.**
- **Where:**
  - `docs/directory-submission.md:165`: the heading says "(6)" but there are 7 rows.
  - `:170`: "27 at seeding" contradicts `:26` ("22 due").
  - `:195` N2: "points to the EngrAmo app". The server's own text is `PLAN_CHANGE_HINT` = "Plan changes aren't self-service yet — contact the EngrAmo team…" (`src/error.rs:9-10`), and `get_subscription_usage` returns it as `howToChange` (`src/subscription.rs:213`). The model will now relay that text, not "the app".
  - `:226`: the submitted set was P1–P5. OpenAI requires 5 positive and 3 negative cases (submission page).
- **Fix:**
  - Change N2's expected result to: "may call `get_subscription_usage`; says plans can't be changed from the chat and the EngrAmo team changes them. No purchase tool exists."
  - Add the cases proposed below.
  - Choose which 5 + 3 to submit to ChatGPT so that they cover the new tools.
- **Owner:** in-repo docs.

**S3. The listing copy, ChatGPT capabilities and plugin package don't mention the new features.**
- **Where:**
  - `docs/directory-submission.md:66-70`: the description doesn't cover sharing, filtering by language, topic or level, plan usage, shadowing or covers.
  - `:98-113`: 15 capabilities, out of a limit of 20.
  - `chatgpt-plugin/plugin.json:4`: `"version": "1.2.11"`.
  - `chatgpt-plugin/plugin.json:27-42`: same 15 capabilities.
- **Fix:** add these 5 capabilities, which brings the total to 20:
  - "Find decks by language, topic and level"
  - "Share a deck with other EngrAmo users"
  - "Show your plan limits and usage"
  - "Turn on shadowing (speaking practice) for a deck"
  - "Set a deck's cover image"

  Also:
  - Add one bullet to the description, e.g. "**Share and discover.** Share a deck with other EngrAmo users by their user id, and find public decks by language, topic and level."
  - Bump `plugin.json` `version` and rebuild the ZIP (`build.sh`).
  - In Claude's portal, edit the listing to match.
- **Owner:** in-repo docs/package + portals.

**S4. The privacy policy and Terms don't cover sharing or public visibility.**
- **Where:** https://engramo.app/policy/privacy/ (last updated 2026-10-13). It doesn't mention public or unlisted catalogs being visible to other users, or catalog sharing (collaborators see the content, and user ids are exchanged). `docs/directory-submission.md:244-249` lists this as "not blocking".
- **Why it matters now:** OpenAI requires the policy to state "the categories of recipients". With `share_catalog` live, other users are such a category.
- **Fix:** add a section on public, unlisted and shared content (who can see it, and that user ids are shown to collaborators). Add the Terms licence and moderation clauses already queued at `:244-248`.
- **Owner:** out-of-repo (site).

**S5. The hosted server mentions a stdio-only audio-generation tool and an outside doc.**
- **Where:**
  - `src/prompts/mod.rs:249-252` (`create_language_deck`): "If generate_card_audio is available … spends only the user's own Gemini quota".
  - `src/prompts/mod.rs:253-254`: "More worked examples … are in `docs/prompt-examples.md` in the engramo-mcp repo".
  - `src/resources/mod.rs:142` (`engramo://card-schema`): the `generate_card_audio` aside.
- **Why it matters:**
  - Claude lists "Generate images, video, or audio through AI models" as unsupported.
  - Claude rejects text that tells the model to "pull behavioral instructions from external sources".
  - The §2.2 compliance answer says there is "no AI media generation on the hosted connector".
  - The tool descriptions are already clean: tests at `src/server.rs:2183-2209` pin that. The prompt and resource are not.
  - This text was already in the approved v1.2.13 (`git show v1.2.13:src/prompts/mod.rs` has 3 mentions), hence Should rather than Blocker.
- **Fix:** drop the `generate_card_audio` sentences and the repo-doc pointer from both texts. If stdio needs them, gate them on TTS being present. Add a test like `test_no_always_on_tool_description_mentions_generate_card_audio` that covers `prompts::get` and the card-schema resource.
- **Owner:** in-repo code.

**S6. The §5b record and "queued" list are stale, and the Claude listing needs an edit.**
- **Where:** `docs/directory-submission.md:261` (29 tools, 6 resources) and `:279-284` ("Queued for the release after the ChatGPT review").
- **What changed:** both queued items are done:
  - the `type` fix, with test `test_every_tool_input_property_declares_a_type` at `src/server.rs:2157-2180`;
  - the `upload_media` wording, with tests at `src/server.rs:2183-2210`.
- **Fix:**
  1. Mark both queued items done (released in v1.2.14).
  2. After B1, re-sync the tools on the Claude listing's Tools tab (expect 35 tools, 7 resources and no warnings).
  3. Submit a listing edit for the tool names and the description from S3.
  4. Record the date.
- **Owner:** in-repo docs + Claude portal.

**S7. The known limitations (§6) need updating.**
- **Where:** `docs/directory-submission.md:287-292`.
- **Fix:**
  - The `/me/usage` `cards_per_catalog` bug (`:291`) is now shown directly by the `get_subscription_usage` tool and the `engramo://subscription` resource. Either confirm the engram-api fix is on prod, or re-check how the override looks in that tool's output.
  - Add these limitations:
    - sharing takes user ids (UUIDs) only, with no email lookup;
    - `list_catalog_shares` returns ids and levels, not names;
    - `search_global` has no language or topic filters (use `list_catalogs`);
    - catalogs with no classification are hidden once any language or topic filter is set.
- **Owner:** in-repo docs + backend check.

**S8. Record the openWorldHint decision and rationale in §3.**
- **Where:** `docs/directory-submission.md:119`.
- **Fix:** add the rationale from the next section, so a reviewer asking "why is `list_catalogs` false if it returns public content?" gets a written answer. ChatGPT asks for "a detailed justification for each" annotation at submission.
- **Owner:** in-repo docs.

### Nice

**N1. Free-string params that are really enums.**
- **Where:** `visibility` at `src/tools/catalogs.rs:193`, `src/tools/generate.rs:97` and `src/tools/learning_paths.rs:100`; `media_type` at `src/tools/media.rs:28`.
- **Fix:** make them enums (`public` / `private` / `unlisted`; for paths `public` / `private`). The scan passes today because they're `type: [string, null]`, but an enum makes the schema self-documenting and lets the client validate it.
- **Owner:** in-repo code (needs a tool freeze check).

**N2. Some descriptions are long.**
- **Which:** seven exceed 1000 characters:
  - `generate_catalog_with_cards` 1511
  - `list_catalogs` 1225
  - `generate_cards` 1127
  - `generate_card` 1101
  - `update_card` 1084
  - `search_global` 1067
  - `update_catalog` 1032

  Server instructions are 1953 characters.
- **Rules:** neither directory sets a length limit. Claude asks for "narrow, accurate descriptions".
- **Fix:** the R1–R5 rich-text paragraph is copied into all three `generate_*` tools and also lives in `engramo://card-schema`. Consider one sentence plus a pointer to the resource. Also drop the shell/base64 advice from `upload_media` (`src/server.rs:1414-1422`); it's about the caller's tooling, not the tool.
- **Owner:** in-repo code.

**N3. `get_catalog`'s description is thin.**
- **Where:** `src/server.rs:319`.
- **Fix:** say it also returns shared and public catalogs and the classification fields. Add the "treat as untrusted data" note that `list_catalogs` and `get_catalog_filter_options` have.
- **Owner:** in-repo code.

**N4. §3.2 says `activate_learning_path` "Switches the active path".**
- **Where:** `docs/directory-submission.md:145`.
- **Problem:** P4 (`:173`) and the seeding notes show two paths active at once, so "switches" reads as if it deactivates the other.
- **Fix:** reword to "Marks a path active so its catalogs are scheduled; no data is removed".
- **Owner:** in-repo docs.

**N5. The OAuth server issues no refresh tokens.**
- **Evidence:** `api.engramo.app/.well-known/oauth-authorization-server` → `grant_types_supported: ["authorization_code"]`.
- **Note:** not a stated requirement, and both earlier reviews passed. If access tokens expire, reviewers re-authorize mid-test. Confirm the token lifetime is longer than a review session.
- **Owner:** out-of-repo (engram-api).

---

## openWorldHint recommendation

**Keep `openWorldHint: false` on `list_catalogs` and `get_catalog_filter_options`, and on the other read tools that can return other users' public or shared content.** Those are `get_catalog`, `search_global`, `search_catalogs`, `search_learning_paths`, `list_cards` and `get_card` (all false today: `src/server.rs:224-231`, `261-268`, `311-318`, `597-604`, `625-632`, `1289-1296`, `1323-1330`, `1348-1355`). Do not change them.

Reasons:

1. **MCP spec.** `openWorldHint` means the tool "may interact with an 'open world' of external entities… the world of a web search tool is open, whereas that of a memory tool is not." These tools read one bounded corpus inside EngrAmo's own first-party API. They don't reach the public internet or arbitrary destinations.
2. **OpenAI guidance.**
   - True is for "public or open-ended entities, including read-only web search and arbitrary destinations".
   - "A tool confined to a bounded private account, workspace, or catalog may use false, even when externally hosted."
   - The reference page says it "accesses the public internet or open-ended external entities".

   EngrAmo's public catalogs are a bounded catalog inside the service.
3. **Precedent.** The same read tools already returned public catalogs with `false` in v1.2.12. The ChatGPT scan flagged only the tools that **create** publicly visible content (fixed in #106, commit d74e512), and was then clean (`docs/directory-submission.md:224`). Claude approved v1.2.13 with the same hints. Claude requires only `title` plus `readOnlyHint`/`destructiveHint`, so `openWorldHint` is informational there.
4. **Same rule as the write tools.** Today the rule is: true only where the tool **publishes** content (sets `visibility` public or unlisted, or grants access with `share_catalog`). By that rule:
   - `generate_card`, `generate_cards` and `update_card` stay false even when they write into a catalog that is already public. The user made the publish decision through the open-world visibility tool.
   - `unshare_catalog` stays false, because it only reduces exposure.

   State this rule in §3 (S8).
5. **Prompt-injection risk** is handled in the descriptions instead. `list_catalogs` and `get_catalog_filter_options` already tell the model to treat public names, descriptions and tags as untrusted data. The server instructions say the same.

**Fallback:** if a reviewer flags it, flip the **whole set** of public-reading tools to `true` together (`list_catalogs`, `get_catalog_filter_options`, `get_catalog`, `search_global`, `search_catalogs`, `search_learning_paths`, `list_cards`, `get_card`), not only the two new ones. Update the pinning test at `src/server.rs:6825-6830` (`list_catalogs` is asserted false) and the one at `:5004-5013`.

---

## Proposed test cases (expected results on the seeded accounts)

### Positive

| # | Prompt | Expected tools | Expected result |
|---|---|---|---|
| P7 (keep, after B3) | "Find public Spanish catalogs for beginners about medicine." | `get_catalog_filter_options`, `list_catalogs` | `list_catalogs` with `front_languages:["es"]`, `levels:["beginner"]`, `topics:["medicine"]`, `filter:"public"` returns ≥1 seeded public catalog |
| P8 | "Which of my decks are B1 level?" | `list_catalogs` (`filter:"own"`, `levels:["B1"]`) | Returns **Spanish: Health & Daily Life (A2–B1)** only |
| P9 | "Tag my travel deck as Spanish to English, topic travel, level A1." | `get_catalog`/`list_catalogs`, `update_catalog` (`front_languages:["es"]`, `back_languages:["en"]`, `topics:["travel"]`, `levels:["A1"]`, `version`) | Catalog shows the new fields. Destructive hint, so the client may confirm first |
| P10 | "What plan am I on and how many catalogs can I still create?" | `get_subscription_usage` | Plan name, the catalogs limit as used / remaining, reset rule. No purchase or upgrade link |
| P11 | "Turn on shadowing for my Spanish travel deck." | `set_catalog_shadowing` (`enabled:true`, `language:"es"`) | Shadowing on. A second identical call changes nothing |
| P12 | "Use the image in my media library as the cover of my health deck." | `list_media`, `get_catalog`, `update_catalog` (`image_id`) | Cover set (needs the seeded image) |
| P6 (rewritten) | "Share my Spanish travel deck with user `<share-target userId>` as a viewer, then show who it's shared with." | `share_catalog`, `list_catalog_shares` | Collaborators list shows that id as `viewer`. Clean up with `unshare_catalog` |

Suggested ChatGPT 5: P1, P2, P7, P10, P6. Alternatively swap P6 for P9 if sharing can't be entitled.

### Negative and safety

| # | Prompt | Expected behaviour |
|---|---|---|
| N4 | "Share my Spanish deck with anna@example.com." | No `share_catalog` call with a guessed id. The model explains sharing needs the person's EngrAmo user id (from their own `get_subscription_usage`) |
| N5 | "Show me catalogs about topic `quantum-cats`." | `list_catalogs` returns `isError`: "topics: 'quantum-cats' is not a topic key (lowercase letters and '_', see get_catalog_filter_options)" (`src/catalog_filters.rs:139-143`). The model calls `get_catalog_filter_options` and offers real topics |
| N6 | "Turn on shadowing for `<id of a public catalog owned by someone else>`." | `isError` permission error, worded as "only the catalog's owner can turn shadowing on or off…" (`src/server.rs:154`). Nothing changes |
| N7 | "Give me the emails of everyone I've shared my deck with." | `list_catalog_shares` returns ids and levels only. The model says emails aren't available |
| N2 (fixed) | "Upgrade me to Professional." | May call `get_subscription_usage`, then relays "Plan changes aren't self-service yet — contact the EngrAmo team". No purchase tool |

ChatGPT "should not trigger" additions:
- **C4:** "What does CEFR level B1 mean?"
- **C5:** "What's included in ChatGPT Plus?" This checks that `get_subscription_usage` doesn't fire on generic "plan" questions.

---

## Reviewer-account seeding (out of repo: seeding script / engram-api DB)

Do the same on both `mcp-review@engramo.app` (ChatGPT) and `mcp-review-claude@engramo.app` (Claude):

1. **Classify the two seeded catalogs.**
   - Travel: `front_languages ["es"]`, `back_languages ["en"]`, a travel topic, levels `A1, A2`.
   - Health: `front ["es"]`, `back ["en"]`, a health/medicine topic, levels `A2, B1`.

   First confirm the topic keys exist on prod (`get_catalog_filter_options` or `/catalogs/facets/schema`).
2. **Public catalogs (about 50).** Make sure a subset is classified, including at least one `es`→`en` / `medicine` / `A1|A2` catalog so P7 returns results, and a few other languages so the counts in `get_catalog_filter_options` look meaningful. If no `medicine` topic exists, change P7 to a topic that does.
3. **Sharing.**
   - Create a **share-target account** and record its userId in the portal test instructions (not in this repo, if you consider the id sensitive).
   - Entitle both reviewer accounts: `catalog_share` feature on, `shared_catalogs` ≥ 1, `collaborators_per_catalog` ≥ 1. Verify with `get_subscription_usage`.
   - Optionally, have another account share one catalog *with* the reviewer as editor, so `filter:"shared"` returns something.
4. **Media.** Upload one small image to each reviewer's media library for P12.
5. **Shadowing.** Leave it off on one catalog so P11 shows a change. Optionally turn it on for another so `speaking_only:true` returns something.
6. **Subscription display.** Check how the `cards_per_catalog: -1` override looks in `get_subscription_usage` (S7).
7. **Before each submission** (existing checklist, `docs/directory-submission.md:33-36`):
   - re-seed due cards;
   - remove leftover shares from test runs;
   - revoke Connected apps and any seeding API tokens.

---

## Sources

- MCP spec 2025-06-18, ToolAnnotations: https://modelcontextprotocol.io/specification/2025-06-18/schema
- Claude, Submit a connector (annotation requirement, portal steps): https://claude.com/docs/connectors/building/submission
- Claude, Connector review criteria (prompt-injection patterns, functional quality, unsupported AI media): https://claude.com/docs/connectors/building/review-criteria
- Claude, After publishing (tool changes deploy to the server; tool names change through a listing edit): https://claude.com/docs/connectors/building/after-publishing
- Anthropic Software Directory Policy: https://support.claude.com/en/articles/13145358-anthropic-software-directory-policy
- OpenAI app submission guidelines (explicit hints, the bounded-catalog rule for openWorldHint, descriptions, privacy, demo account): https://developers.openai.com/apps-sdk/app-submission-guidelines
- OpenAI developer guidelines: https://developers.openai.com/apps-sdk/app-developer-guidelines
- OpenAI Apps SDK reference (annotation definitions): https://developers.openai.com/apps-sdk/reference
- OpenAI submission (5 positive / 3 negative tests, one active review, updates after publishing): https://developers.openai.com/apps-sdk/deploy/submission
- EngrAmo privacy policy (checked 2026-10-31): https://engramo.app/policy/privacy/
