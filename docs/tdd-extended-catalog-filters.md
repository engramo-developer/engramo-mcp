# TDD: Catalog filters in MCP (front/back language, topics, levels, tags)

Status: **designed, not started** (2026-10-31). Nothing is committed and there is no branch yet. Read this whole
document before you start any phase.

Backend design: `engram-ws/tdds/extended-catalog-filters.md` (§15 and its revisions 12 to 22 are the current ones).
Backend code: `engram-api` `main` @ `d7b97fc` ("Feat/extended catalog filters (#15)"). `git diff main
feat/extended-catalog-filters` is empty, so `main` holds all the filter work (B35, B37/B38, B39). The checked-out
branch `feat/onboarding` adds only the onboarding commits (A1–A3) on top, and its working tree is clean. **Where the
code and the design doc disagree, this TDD follows the code** (see §2.7).

---

## 1. Summary, goals, non-goals

**Goal.** An AI client should be able to:

1. **Find** catalogs by front (learning) language, back (definition) language, topic, level and tags, together with
   free text. This is the "Card catalogs" filter sheet of the app, done through `list_catalogs`.
2. **Set** those fields when it creates a catalog (`generate_catalog_with_cards`) or changes one (`update_catalog`).
3. **Discover** the allowed values. Topics live in the database and moderators can change them without a deploy, so
   the client must be able to read the current list.

**Recommendation (catalog metadata).** Add four optional, additive params, `front_languages`, `back_languages`,
`topics` and `levels`, to the **existing** `update_catalog` and `generate_catalog_with_cards` tools. Do not add
dedicated `set_catalog_*` tools. Add **one** new read-only discovery tool, `get_catalog_filter_options`, and a matching
resource, `engramo://catalog-filters`. Rationale:

- The API models all four fields as plain lists on the same `PATCH /catalogs/{id}` and `POST /catalogs/with-cards`
  bodies, with replace-per-field semantics and the same `version` lock that `update_catalog` already carries. With one
  call the client sets everything and pays for one version round trip. With per-field tools it would pay for 2 to 4
  round trips and 2 to 4 chances of a Conflict.
- Optional params keep every existing tool name and every existing call valid. That matters for the Claude and ChatGPT
  directory listings, which review tool names and descriptions.
- `set_catalog_shadowing` is a separate tool only because shadowing is owner-only and has its own on/off semantics.
  These fields have no such special rule: anyone who can edit the catalog can set them (`db/src/repo/catalogs.rs`
  UPDATE `WHERE author_id = $8 OR editor`).
- Topic keys can change at runtime and there are 183 languages, so a JSON-schema enum would be either stale (topics)
  or about 1.5k tokens repeated in three tool schemas (languages). Levels are a fixed set of six, so they are an enum.
  For everything else the schema descriptions say what the values look like, and `get_catalog_filter_options` returns
  the live lists. A tool is needed and not just a resource, because many clients never read resources
  (`docs/tdd-subscription-usage.md` §2). The tool and resource pair follows the
  `get_subscription_usage` + `engramo://subscription` precedent.

**Search.** `GET /catalogs` is the only route that has the filters, together with `q`. `GET /search` and
`GET /search/catalogs` take only `q` (and `filter`) (§2.5). So `list_catalogs` gains `query` plus the filter params
and becomes the filtered search. `search_global` and `search_catalogs` keep their params. Their descriptions point to
`list_catalogs` for filtering, and their catalog output gains the new fields.

**Non-goals.**

- Facet counts (`GET /catalogs/facets`) and tag prefix search (`GET /catalogs/facets/tags`). These are UI helpers.
  An LLM can narrow with `query` or `tags` and retry. See Q2.
- Filter params on `search_global`. The backend stage B36 / D67 is designed but not built. See the follow-up in §7.
- The search-only `language` pseudo-key (D59: "a language deck with no subject"). Rev. 17 (D80) removed it from the
  app search. See Q3.
- Saved filters (`/catalogs/saved-filters`), topic management (`/catalog-subjects`, moderator only), and filter
  fields on `translate_batch_import` (paid AI, off on prod; see Q4).
- Renaming any tool.

---

## 2. Verified backend facts (engram-api `main` @ d7b97fc)

### 2.1 Field model and meaning

| Wire field (catalog JSON, camelCase) | Column | Meaning (D85, final labels) | Values | Write cap | Search param / cap |
|---|---|---|---|---|---|
| `learn` keys `lang:<iso>` | `catalogs.learn text[]` | **Learning language (front of card)**, the language the cards are in | `lang:` + one of 183 ISO 639-1 codes (`db/src/models/facets.rs` `LANGUAGES`, 183 `opt(..)` entries) | `learn` total ≤ **3** (langs + subjects) | `learn`, total ≤ **8** |
| `learn` subject keys | same column | **Topic** | active rows of `catalog_subjects`, moderator-managed (D54) | (shared cap above) | (shared cap above) |
| `speakLangs` | `catalogs.speak_langs text[]` | **Definition language (back of card)** | ISO 639-1 codes, plus `zxx` = no language needed | ≤ **2** | `speak`, ≤ **2** |
| `levels` | `catalogs.levels text[]` | CEFR level | `A1 A2 B1 B2 C1 C2` (buckets: beginner = A1,A2; intermediate = B1,B2; advanced = C1,C2) | ≤ 6 | `levels`, ≤ 6 |
| `tags` | `catalogs.tags` | free tags | trimmed, lowercased, ≤ 40 chars, no comma or control chars | ≤ **20** | `tags`, ≤ **8**, **AND** |

Constants: `db/src/models/catalog_filters.rs:18-29` (`MAX_CATALOG_SPEAK=2`, `MAX_CATALOG_LEARN=3`,
`MAX_CATALOG_LEVELS=6`, `MAX_QUERY_SPEAK=2`, `MAX_QUERY_LEARN=8`, `MAX_QUERY_LEVELS=6`, `MAX_QUERY_TAGS=8`). Also
`MAX_FILTER_PARAM_LEN=512` raw bytes per query param (`:22`), `LEVELS`/`BUCKETS` (`:38-56`), and `facets.rs:7-9`
(`MAX_TAG_FILTERS=8`, `MAX_TAG_LEN=40`, `MAX_CATALOG_TAGS=20`).

Normalization (`catalog_filters.rs:165-218`): `speak` and `learn` values are lowercased, levels are uppercased,
duplicates are removed, and the lists are sorted (tags keep author order, `facets.rs:237`
`normalize_tags`). An unknown value is a **400**, never silently dropped.

The DTO doc comment on `speak_langs` ("ISO codes the catalog is written in", `db/src/models/dtos.rs:191`) is
**stale**. D58 and D85 define it as the definition (back) language.

### 2.2 Current topics (DB-managed)

Migration `db/migrations/20261025120000_set_catalog_subjects_topic_list.up.sql` sets **11** active topics:
`software_dev` (Software development), `software_testing`, `geography`, `math`, `law`, `medicine`, `arts`
(Arts & music), `history`, `physics`, `chemistry`, `biology`. It deactivates the rest. The design doc's D42 list
(14 subjects incl. `interviews`, `exams`, `kids`, `general`, `other`) is **superseded by this migration**. An
**inactive** topic is hidden from the schema and still valid in search filters. A write may keep an inactive topic the
catalog already has, but may not add one (`FILTER_INACTIVE_SUBJECT`, `catalog_filters.rs:199-209`).

### 2.3 `GET /catalogs` (list = the filtered search)

`app/src/routes/catalogs.rs:74-121`, params `db/src/models/pagination.rs:128-160` (`CatalogListParams`).

| Param | Encoding | Semantics (as implemented, `db/src/repo/catalogs.rs:111-118`) |
|---|---|---|
| `limit`, `cursor` | as today | the cursor fingerprint includes every filter, so changing a filter mid-pagination gives 400 `cursor was issued for different list parameters; restart pagination` (`repo/catalogs.rs:57`) |
| `filter` | comma list of `all\|public\|shared\|own`; `all` cannot be combined (`pagination.rs:27-75`) | sources |
| `speakingOnly` | bool | unchanged |
| `q` | ≤ 200 chars (`pagination.rs:257-265`, `MAX_Q_CHARS`) | `search_tsv @@` over name + description + short id |
| `speak` | **comma-separated** `ru,en` | `speak_langs && {sel…, zxx}`. **Strict** (D81): a catalog with empty `speak_langs` never matches |
| `learn` | **comma-separated** `lang:es,medicine` | split into languages L and subjects S. Matches `(L=∅ or learn && L) AND (S=∅ or learn && S)`. Strict: no `lang:` key → never matches an L selection |
| `levels` | comma-separated | overlap (OR) |
| `tags` | comma-separated | containment (**AND**) |

The list is comma-separated, not a repeated `?k=a&k=b` param (`split_csv`, `catalog_filters.rs:128-140`). Response:
`PagedResponse<CatalogWithPinnedDto>`, whose items carry `speakLangs`, `learn` and `levels` (`dtos.rs:210-244`).

The **OpenAPI text for `speak` and `learn` on this route is stale** (`routes/catalogs.rs:89-92`: "…or with no spoken
language set", "a catalog without a `lang:` key matches every language"). The SQL and `pagination.rs:149-156` are
strict (D81). The MCP descriptions follow the code.

### 2.4 Writes

| Route | Body fields | Semantics |
|---|---|---|
| `POST /catalogs/with-cards` (`routes/catalogs.rs:364-430`, `CreateCatalogWithCardsRequest` `dtos.rs:637`) | `speakLangs?`, `learn?`, `levels?`, `tags?` (all `Option<Vec<String>>`) | validated **before** the quota is reserved (`prevalidate_filters`, `routes/catalogs.rs:56`) |
| `POST /catalogs` (`:291-352`, `CreateCatalogRequest` `dtos.rs:613-625`) | same | same; MCP has no tool for it (only `translate_batch_import` metadata uses its MCP DTO) |
| `PATCH /catalogs/{id}` (`:988-1030`, `UpdateCatalogRequest` `dtos.rs:652-669`) | `speakLangs?`, `learn?`, `levels?`, `tags?`, `version` | **per field: absent or `null` = unchanged; present = replaced wholesale; `[]` = cleared** (`repo/catalogs.rs:966-1049`, `COALESCE($12..$14, col)`). Stale `version` → 409 |

**Speaking-language sync** (D84, `catalog_filters.rs:375-386`, `repo/catalogs.rs:996-1012`): on PATCH, when the catalog
has a shadowing language and `learn` has no `lang:` key and fewer than 3 values, the API adds `lang:<code>`. It does
this only when the payload **omits** `learn`, or sets the speaking language in the same PATCH. Consequence: if you
clear the front languages of a shadowing catalog, the next PATCH that omits `learn` adds the shadowing language back.

Publishing does **not** require these fields in the API (D49 is enforced only in the owner UIs).

### 2.5 Discovery and other routes

- `GET /catalogs/facets/schema` (`app/src/routes/catalog_facets.rs:23-44`, DTO `catalog_filters.rs:399-495`). Any
  authenticated user (`AuthUser`, which accepts `X-Api-Key`). Sends `Cache-Control: private, max-age=300` and returns
  503 `SUBJECTS_UNAVAILABLE` if the topic list never loaded. Body:
  ```json
  {"schemaVersion":2,
   "languages":[{"key":"en","label":"English"}, …183],
   "subjects":[{"key":"software_dev","label":"Software development"}, …active only],
   "levels":[{"key":"A1","label":"A1","bucket":"beginner"}, …],
   "buckets":[{"key":"beginner","label":"Beginner","levels":["A1","A2"]}, …],
   "limits":{"speak":2,"learn":3,"tagsPerCatalog":20,"searchSpeak":2,"searchLearn":8}}
  ```
  `cardTypes` is **gone** (B39, migration `20261027130000_drop_catalog_card_types`), although §15.4 of the design doc
  still shows it.
- `GET /catalogs/facets` (counts) and `GET /catalogs/facets/tags` (prefix search): `catalog_facets.rs:48-156`. Out of
  scope (§1).
- `GET /search`: `GlobalSearchParams { q, filter }` only (`dtos.rs:583-588`, `routes/search.rs:38-50`). **No filter
  params** (B36 not built). `GET /search/catalogs`: `q` only. Its rows already include `speak_langs`, `learn` and
  `levels` (`db/src/repo/search.rs:109-131`).

### 2.6 Errors

Body: `{"error": "<message>", "code": "<CODE>"}` (`app/src/routes/mod.rs:769-773`). `FilterError` → 400
`BadRequestCoded`, except `SUBJECTS_UNAVAILABLE` → 503 (`mod.rs:627-636`). Messages (`catalog_filters.rs:65-87`):
`invalid language 'x'` (`FILTER_INVALID_LANGUAGE`), `unknown subject 'x'` (`FILTER_UNKNOWN_SUBJECT`),
`subject 'x' is no longer available for new catalogs` (`FILTER_INACTIVE_SUBJECT`), `invalid level 'x'; expected one
of: A1, …` (`FILTER_INVALID_LEVEL`), `'learn' accepts at most 3 values` (`FILTER_TOO_MANY_VALUES`; the message names
the **wire** field), `'speak' parameter is too long` (`FILTER_PARAM_TOO_LONG`), `invalid tag` / `tags accept at most
N values` (`TAG_INVALID` / `TAG_TOO_MANY`), and `q must be at most 200 characters` (`QUERY_TOO_LONG`). MCP already
maps 400 → `ApiError::BadRequest(message)` (`src/error.rs:338-339`, `extract_error_message` reads `error` only).

### 2.7 Where the design doc and the code disagree (code wins)

| Design doc | Code |
|---|---|
| §15.4 `cardTypes` param, field and `FILTER_CARD_TYPE_NEEDS_LANGUAGE` | removed (B39) |
| D42: 14 subjects | 11 topics (migration `20261025120000`) |
| D52/D58 lenient matching (empty matches) | strict (D81). The OpenAPI text of `GET /catalogs` is still lenient (stale) |
| §8.3 "Track M": `domain`/`facets`/`attributes` | that model was replaced in rev. 10; Track M's text is obsolete |
| D67: `/search` takes the filters | not implemented |

---

## 3. Current MCP state (engramo-mcp `main` @ 6c132a2)

| Piece | State |
|---|---|
| `ListCatalogsParams` (`src/tools/catalogs.rs:8-24`) | `limit`, `cursor`, `speaking_only`, `filter: Option<CatalogFilter>` (own/shared only) |
| `list_catalogs` tool (`src/server.rs:221-251`) | → `client.list_catalogs_filtered(limit, cursor, speaking_only, filter)` (`src/client.rs:334-349`). It never sends `q`, `speak`, `learn`, `levels` or `tags` |
| `CatalogFilter` (`src/dto.rs:126-142`) | `Own`, `Shared`. No `Public` |
| `CatalogDto` (`src/dto.rs:49-67`) | typed. `speakLangs`, `learn` and `levels` are **dropped** on deserialize, so no tool shows them |
| `UpdateCatalogParams` (`src/tools/catalogs.rs:32-52`) / `update_catalog` (`src/server.rs:277-314`) | name, description, tags, visibility, image_id, version |
| `UpdateCatalogRequest` (`src/dto.rs:180-196`) | no filter fields. Built literally in `apply_shadowing` (`src/server.rs:136-146`), `update_catalog`, and tests |
| Catalog creation | **only** `generate_catalog_with_cards` (`src/server.rs:1829-1907`, `GenerateCatalogWithCardsParams` `src/tools/generate.rs:80-103`, `CreateCatalogWithCardsApiRequest` `src/dto.rs:501-515`). No `create_catalog` tool. `client.create_catalog` is used only in tests |
| `search_global` / `search_catalogs` (`src/server.rs:1198-1254`, `SearchParams` `src/tools/search.rs`) | `query` only. `/search/catalogs` rows decode into `CatalogDto`, which drops the filter fields |
| Server instructions (`src/server.rs:1396-1421`) | no mention of filters |
| Resources (`src/resources/mod.rs:7-33,160-200`) | six resources. `engramo://catalogs` = id/name/card_count (unchanged) |
| Prompt `create_language_deck` (`src/prompts/mod.rs:197-251`) | face = `source_lang`, back = `target_lang`. Does not set catalog languages |
| Tests | in-file `#[cfg(test)] mod tests` with wiremock (`query_param`, `query_param_is_missing`, `body_partial_json`). 886 `#[test]`/`#[tokio::test]` today. `ListCatalogsParams {…}` literals ×12, `UpdateCatalogParams` ×10, `UpdateCatalogRequest` ×11, `GenerateCatalogWithCardsParams` ×17, `CreateCatalogWithCardsApiRequest` ×7 (adding fields breaks every literal; see §5.0) |
| Directory listing | 34 tools on prod (`docs/directory-submission.md:19,119`). A new tool brings the count to 35 |

---

## 4. Design

### 4.0 Naming used everywhere in MCP

| MCP name (params **and** tool output) | Wire |
|---|---|
| `front_languages: [iso]` | the `lang:<iso>` keys in `learn` |
| `topics: [key]` | the non-`lang:` keys in `learn` |
| `back_languages: [iso \| "zxx"]` | `speakLangs` / `speak` |
| `levels: [A1…C2]` | `levels` |
| `tags` | `tags` (unchanged) |

The LLM never sees `learn` or `lang:` prefixes. MCP splits and joins them. The words "front" and "back" match the card
schema (`face`/`back`) and `set_catalog_shadowing` ("Language of the card FRONTS"). The output keys are snake_case and
equal to the param names, so a value read from `get_catalog` can be passed straight back.

### 4.1 New pure module `src/catalog_filters.rs` (registered in `src/lib.rs`)

No I/O. Everything here is unit-tested.

```rust
pub const NO_LANGUAGE: &str = "zxx";
pub const LANG_PREFIX: &str = "lang:";
// Mirror the API caps (§2.1) so errors can name MCP params; the API re-validates.
pub const MAX_WRITE_BACK: usize = 2;  pub const MAX_WRITE_LEARN: usize = 3;
pub const MAX_SEARCH_BACK: usize = 2; pub const MAX_SEARCH_LEARN: usize = 8;
pub const MAX_SEARCH_TAGS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
pub enum LevelParam {           // JSON-schema enum; serde names exactly as listed
    A1, A2, B1, B2, C1, C2,
    #[serde(rename = "beginner")] Beginner,         // = A1, A2
    #[serde(rename = "intermediate")] Intermediate, // = B1, B2
    #[serde(rename = "advanced")] Advanced,         // = C1, C2
}
pub fn expand_levels(levels: &[LevelParam]) -> Vec<String>; // dedup + sort, CEFR strings

/// "es", "ES", "es-ES", "es_419", "lang:es" -> "es". Rejects anything that is not 2–3 ASCII
/// letters after that (local error naming `field`); membership in the 183-code list stays
/// with the API. `allow_zxx` is true only for back_languages.
pub fn normalize_lang(raw: &str, field: &'static str, allow_zxx: bool) -> Result<String, String>;
pub fn normalize_langs(raw: &[String], field: &'static str, allow_zxx: bool, max: usize)
    -> Result<Vec<String>, String>;              // dedup, cap after dedup

/// Topic keys: trimmed + lowercased; reject empty, a `lang:` prefix ("use front_languages"),
/// and anything not matching ^[a-z][a-z_]{0,31}$ (the API's key shape; it also covers the
/// reserved `language`, which is rejected explicitly with a short note).
pub fn normalize_topics(raw: &[String]) -> Result<Vec<String>, String>;

/// learn = lang:<front…> ++ topics; errors if front+topics > max, naming both params.
pub fn join_learn(front: &[String], topics: &[String], max: usize) -> Result<Vec<String>, String>;
pub fn split_learn(learn: &[String]) -> (Vec<String> /*front*/, Vec<String> /*topics*/);

/// Hint appended to an API 400 when the call carried any filter param.
pub fn with_filter_hint(e: ApiError) -> ApiError; // BadRequest(m) -> BadRequest(m + " — call get_catalog_filter_options for valid topics, levels and language codes."); others pass through

/// get_catalog_filter_options / engramo://catalog-filters view (built from the wire DTO, §4.2).
#[derive(Serialize)] pub struct FilterOptionsView {
    topics: Vec<KeyLabel>,                 // active only, API order
    levels: Vec<&'static str>,             // ["A1".."C2"]
    level_groups: BTreeMap<String, Vec<String>>, // beginner -> [A1,A2], … (from `buckets`)
    languages: BTreeMap<String, String>,   // "en" -> "English" (compact map, 183 entries)
    no_language_code: &'static str,        // "zxx", valid only in back_languages
    limits: Limits,                        // see below
    notes: Vec<&'static str>,              // three short usage notes (front/back meaning, AND/OR, strict matching)
}
// Limits { per_catalog: {back_languages, front_languages_plus_topics, tags},
//          search: {back_languages, front_languages_plus_topics, tags} }
// per_catalog values come from the API `limits` (speak, learn, tagsPerCatalog); search uses
// searchSpeak / searchLearn and the MCP constant MAX_SEARCH_TAGS (the API does not publish it).
pub fn filter_options_view(schema: FilterSchemaDto) -> FilterOptionsView;
```

### 4.2 DTOs and client (`src/dto.rs`, `src/client.rs`)

- **`CatalogClassification`** (new, `dto.rs`), flattened into `CatalogDto` with `#[serde(flatten)]`:
  - `Deserialize`: through a private wire struct `{ #[serde(rename="speakLangs", default)] speak_langs, #[serde(default)]
    learn, #[serde(default)] levels }` with every field `Vec<String>`. Then `split_learn` turns it into
    `front_languages`, `topics`, `back_languages` and `levels`. A missing field (older API) becomes empty.
  - `Serialize`: a hand-written `SerializeMap` that writes only the **non-empty** keys `front_languages`,
    `back_languages`, `topics` and `levels`. A catalog without metadata therefore keeps today's output.
  - The doc comment explains why: the LLM sees one naming scheme (§4.0).
  - Existing `CatalogDto` deserialization tests and fixtures must still pass. A flattened field turns off nothing that
    `CatalogDto` uses today (it has no `deny_unknown_fields`).
- **`UpdateCatalogRequest`**: add `#[serde(rename="speakLangs", skip_serializing_if="Option::is_none")] speak_langs:
  Option<Vec<String>>`, `learn: Option<Vec<String>>` and `levels: Option<Vec<String>>` (both skip if `None`).
  `Some(vec![])` serializes `[]` (= clear). Derive `Default` and change the existing literals to `..Default::default()`.
- **`CreateCatalogWithCardsApiRequest`**: the same three fields, plus `Default`.
- **`CatalogFilter`**: add `Public` (`as_str` → `"public"`). The variant is additive.
- **`CatalogListQuery`** (new, `dto.rs`, `Default`): `limit, cursor, speaking_only, filter, q: Option<String>,
  speak: Vec<String>, learn: Vec<String>, levels: Vec<String>, tags: Vec<String>`.
- **`client.list_catalogs_query(&CatalogListQuery)`** (new). It builds `page_params` and then, only for non-empty
  values: `speakingOnly=true`, `filter`, `q` (through `bounded_param`), and `speak`/`learn`/`levels`/`tags` as **one
  comma-joined value each**, also through `bounded_param` (512 bytes, equal to the API's `MAX_FILTER_PARAM_LEN`).
  `list_catalogs_filtered` becomes a thin wrapper over it, so the callers in `resources/mod.rs:299` and `client.rs`
  tests do not change.
- **`client.get_catalog_filter_schema() -> FilterSchemaDto`** (new) → `GET /catalogs/facets/schema`. The wire DTO
  `FilterSchemaDto` (camelCase; `languages`/`subjects` `[{key,label}]`, `buckets` `[{key,label,levels}]`, `limits`
  `{speak, learn, tagsPerCatalog, searchSpeak?, searchLearn?}`) lives in `dto.rs`. `searchSpeak` and `searchLearn`
  are `Option` and fall back to the MCP constants, because they were added later (D83). Unknown keys are ignored.

### 4.3 Tools

The rules from CLAUDE.md apply: never return `Err` from a handler; use `err_result`; ids go through `parse_uuid`; a
local validation failure sends **no request**.

#### `list_catalogs` (extended, same name and annotations)

New fields on `ListCatalogsParams`, all optional (schemars descriptions in quotes):

| Field | Type | Description |
|---|---|---|
| `query` | `Option<String>` | "Optional free text matched against catalog name, description and short ID (max 200 characters)." |
| `front_languages` | `Option<Vec<String>>` | "Front-of-card (learning) languages as ISO 639-1 codes, e.g. ['es']. Matches catalogs whose cards are in any of them. Catalogs with no front language set never match." |
| `back_languages` | `Option<Vec<String>>` | "Back-of-card (definition/translation) languages as ISO 639-1 codes, at most two. Matches any of them; language-free catalogs (code 'zxx') always match. Catalogs with no back language set never match." |
| `topics` | `Option<Vec<String>>` | "Topic keys from get_catalog_filter_options (e.g. 'medicine', 'software_dev'). Matches any of them. Combined with front_languages using AND." |
| `levels` | `Option<Vec<LevelParam>>` | "CEFR levels A1–C2, or 'beginner' (A1,A2), 'intermediate' (B1,B2), 'advanced' (C1,C2). Matches any." |
| `tags` | `Option<Vec<String>>` | "Tags; a catalog must have ALL of them (case-insensitive). At most 8." |

`filter` description gains "'public' lists only public catalogs by any user". `limit`/`cursor` text unchanged.

Handler: normalize with the §4.1 helpers using the **search** caps (`front_languages` + `topics` ≤ 8, back ≤ 2, tags ≤ 8,
`query` ≤ 200 chars counted locally), then call `list_catalogs_query`. If the call carried any filter param, map the
error with `with_filter_hint`. A `query` that is empty or whitespace-only is not sent.

New description (replaces the old one; keep it free of plan-specific numbers other than the page size already there):

> List or filter flashcard catalogs (the user's own, shared with them, and public ones) with cursor-based pagination.
> Returns each catalog's id, name, description, tags, cardCount, visibility and version, plus front_languages,
> back_languages, topics and levels when set, and `speakingSettings.language` for shadowing catalogs. To find
> catalogs, combine `query` (free text) with front_languages (the language the cards are in), back_languages (the
> language of the translations/definitions), topics, levels and tags. Values within one field are OR-ed (tags are
> AND-ed), and different fields narrow each other. Many personal catalogs have no languages or topic set and are
> hidden as soon as one of those filters is used, so if nothing matches, retry with fewer filters or with `query`
> only. Get valid topic keys from get_catalog_filter_options. `filter`: 'own', 'shared' or 'public'. Returns at most
> 50 items per call. Pass the returned `cursor` back with **the same filters** to get the next page (`cursor: null`
> means this is the last page).

#### `update_catalog` (extended, same name and annotations)

New optional fields on `UpdateCatalogParams`:

| Field | Type | Description |
|---|---|---|
| `front_languages` | `Option<Vec<String>>` | "Languages the card FRONTS are in (ISO 639-1, e.g. ['es']). Replaces the stored list; [] clears it; omit to keep it. front_languages + topics: at most 3 in total." |
| `back_languages` | `Option<Vec<String>>` | "Languages of the card BACKS (translations/definitions), ISO 639-1, at most 2; 'zxx' = no language needed (flags, symbols). Replaces; [] clears; omit to keep." |
| `topics` | `Option<Vec<String>>` | "Topic keys from get_catalog_filter_options. Replaces; [] clears; omit to keep. More specific subjects belong in tags." |
| `levels` | `Option<Vec<LevelParam>>` | "CEFR levels or 'beginner'/'intermediate'/'advanced'. Replaces; [] clears; omit to keep." |

`tags` description becomes "New tags (lowercased; at most 20, each up to 40 characters). Replaces the stored tags."

Handler:

1. Validate `image_id` (as today), normalize the four fields with the **write** caps, and parse `catalog_id`. On any
   error return `err_result` with no request.
2. `learn`:
   - Neither `front_languages` nor `topics` given → `learn: None`.
   - **Both** given → `join_learn(front, topics, 3)`.
   - **Exactly one** given → `client.get_catalog(id)`. If `current.version != p.version`, return the same actionable
     Conflict text that `update_catalog` returns today (re-fetch and retry), **without** a PATCH. Otherwise keep the
     other half from `current` (`CatalogClassification`) and `join_learn`. Stored inactive topics are preserved, and
     the API allows that.
3. `speak_langs` = back languages, `levels` = `expand_levels`, and both stay `None` when omitted. Send the PATCH with
   `p.version`.
4. If the call carried any filter param, map a BadRequest through `with_filter_hint`.

Description (append to the existing text):

> It also sets how the catalog is found in search: front_languages (the language the cards are in), back_languages
> (the language of translations/definitions), topics and levels. Each of these, like tags, replaces the stored list
> when given, [] clears it, and omitting it keeps it. For a public catalog, set back_languages and front_languages
> (or topics) so others can find it. A shadowing catalog with no front language gets its shadowing language added
> back automatically.

#### `generate_catalog_with_cards` (extended, same name and annotations)

It gains the same four optional fields as `update_catalog` (for creation, "Replaces/clears" becomes "Optional").
Normalize with the write caps **before** the existing card checks, and join `learn` (no read needed). Wire them into
`CreateCatalogWithCardsApiRequest`. Add one sentence to the description: "Set front_languages, back_languages, topics
and levels so the catalog can be found by language and topic (valid topics: get_catalog_filter_options)." The API
validates them before it reserves quota (§2.4), so a bad value costs no quota.

#### `get_catalog_filter_options` (new, `catalog_tools_router`)

- No params (`Parameters<()>`-free handler, like `get_subscription_usage`).
- Annotations: `read_only_hint=true, destructive_hint=false, idempotent_hint=true, open_world_hint=false`. Title
  "Get catalog filter options".
- It calls `client.get_catalog_filter_schema()` and returns `ok_json(&filter_options_view(schema))`. Errors go
  through `err_result` (the 503 `SUBJECTS_UNAVAILABLE` becomes the generic server-error text).
- Description: "List the values accepted by the catalog filter and classification params of list_catalogs,
  update_catalog and generate_catalog_with_cards: the current topic keys with labels (topics change over time; don't
  guess), the CEFR levels and their beginner/intermediate/advanced groups, the language codes with names (front and
  back languages use ISO 639-1; 'zxx' means no language and is valid only for back_languages), and the per-catalog
  and search limits." The description quotes **no digits** (same rule as `test_sharing_tools_registered_and_annotated`,
  `src/server.rs:4814-4830`), because limits can change.

#### `search_global` / `search_catalogs` (descriptions only)

Add one sentence to each: "To filter catalogs by language, topic, level or tags, use list_catalogs with `query`
instead." `search_catalogs` output now includes front_languages, back_languages, topics and levels (from the DTO
change). No new params.

#### Server instructions (`src/server.rs:1396-1421`)

Append: "Catalogs carry front_languages (the language the cards are in), back_languages (the language of
translations/definitions), topics and levels. list_catalogs filters on them, update_catalog and
generate_catalog_with_cards set them, and get_catalog_filter_options lists the valid values."

### 4.4 Resource and prompt

- `engramo://catalog-filters` (`src/resources/mod.rs`): the same JSON as `get_catalog_filter_options`. Name "Catalog
  Filter Options", `application/json`. Add a `URI_CATALOG_FILTERS` const, a `list_all` entry, a `read_resource` arm,
  and the module doc line.
- `create_language_deck` prompt (`src/prompts/mod.rs:197-251`): add one line to the "create a new catalog" path: "Set
  `front_languages` to the ISO 639-1 code of {source_lang} and `back_languages` to that of {target_lang} (and `levels`
  if the user gave one)." The prompt args are free text, so the model maps them to codes.

### 4.5 Edge cases decided

| Case | Behavior |
|---|---|
| `front_languages: ["zxx"]` | local error: "omit front_languages for language-free decks; set back_languages to ['zxx']" |
| `topics: ["lang:es"]` | local error: "put languages in front_languages" |
| `topics: ["language"]` | local error on writes ("reserved key"). On `list_catalogs` too, until Q3 is decided |
| `levels: ["beginner","A1"]` | → `["A1","A2"]` (dedup) |
| `front_languages + topics > cap` | local error naming both params and the cap |
| JSON `null` for a field | same as omitted (serde `Option`) |
| Unknown topic or inactive topic added | API 400 + `with_filter_hint` |
| Only `topics` given and the GET fails | return the GET's error and do not PATCH |

---

## 5. Implementation phases

### 5.0 Rules for every phase

- Branch `feat/catalog-filters` from `main`. The orchestrator cuts it (Phase 0). No commits inside phases.
- Each subagent runs the full gate in **one foreground call** before it reports:
  `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | tail -30`.
  The report lists the files changed, the test count against the baseline (886), and any deviation from this TDD.
- Tests live in the same file as the code. Phases that touch the same file run **sequentially**.
- Struct literals: when you add fields, give the struct `Default` (DTOs) or keep the params structs `Deserialize`-only
  and add `Default`. Fix every existing literal with `..Default::default()`. Do not leave a phase that fails to
  compile.

### Phase 1: pure module (`src/catalog_filters.rs`, `src/lib.rs`)

§4.1 except `filter_options_view`, which needs Phase 2's `FilterSchemaDto`.

Tests:
- `normalize_lang`: `es`, `ES`, `es-ES`, `es_419`, `lang:es` → `es`; `zxx` is rejected when `!allow_zxx` and accepted
  for back; `""`, `e`, `espa`, `e1`, and non-ASCII are rejected; the error is capped with `preview`.
- `normalize_langs`: dedup happens before the cap (`["es","ES"]` with cap 1 is OK); over the cap → the error names the
  field.
- `normalize_topics`: lowercases; rejects `lang:` keys, `language`, empty strings and bad shapes.
- `join_learn`/`split_learn`: round trip; the cap error names `front_languages` and `topics`; `split_learn` keeps
  unknown non-`lang:` keys as topics.
- `expand_levels`: buckets, dedup, sort. `LevelParam` serde: `"A1"` and `"beginner"` parse, `"a1"` and `"expert"` fail.
- `with_filter_hint`: BadRequest gets the hint appended once; other variants are unchanged.

Acceptance: the module is pure (no `client` import), and the gate is green.

### Phase 2: wire layer (`src/dto.rs`, `src/client.rs`, plus mechanical literal fixes in `src/server.rs`)

Depends on Phase 1. §4.2 plus `filter_options_view` (add it to `catalog_filters.rs`, which stays Phase 1's file; this
phase may append the function and its tests there).

Tests (wiremock):
- `CatalogDto` decodes a payload with `speakLangs`, `learn: ["lang:es","medicine"]` and `levels`, and re-serializes
  `front_languages:["es"]`, `topics:["medicine"]`, `back_languages`, `levels`. An empty or absent payload emits none of
  the keys. The legacy fixture without the fields still decodes.
- `UpdateCatalogRequest`: `None` → key absent; `Some(vec![])` → `"learn":[]`.
- `list_catalogs_query`: every param reaches the API as one comma-joined value (`query_param("learn","lang:es,medicine")`);
  empty vectors and `None` → `query_param_is_missing`; an over-512-byte joined value → BadRequest with no request
  (`.expect(0)`); `filter=public`.
- `list_catalogs_filtered` still behaves the same (the existing tests are unchanged).
- `get_catalog_filter_schema`: decodes the §2.5 sample. Without `searchSpeak`/`searchLearn` it still decodes. 503 →
  error. `filter_options_view` maps buckets to `level_groups`, languages to a map, and limits as specified.

Acceptance: no tool behavior changes yet (except that the catalog output gains the four keys when the API sends
them), and the gate is green.

### Phase 3a: `list_catalogs` + `get_catalog_filter_options` + search descriptions + instructions (`src/tools/catalogs.rs`, `src/server.rs`)

Depends on Phase 2.

Tests:
- Each new param reaches `GET /catalogs` in wire form (`front_languages:["ES"]` + `topics:["medicine"]` →
  `learn=lang:es,medicine`; `back_languages` → `speak`; `levels:["beginner"]` → `levels=A1,A2`; `tags` → comma-joined;
  `query` → `q`; blank `query` → missing).
- Local errors make zero requests: 9 front+topics, 3 back, 9 tags, a 201-char `query`, `front_languages:["zxx"]`,
  `topics:["lang:es"]`.
- An API 400 (`{"error":"unknown subject 'foo'","code":"FILTER_UNKNOWN_SUBJECT"}`) → `is_error` and the text contains
  the hint. A 400 without filter params has no hint.
- `get_catalog_filter_options`: happy path JSON shape; 503 → `is_error` without panic; registered in the stdio server
  and in `build_session_server`; annotations `(true,false,true,false)`; the description has no ASCII digits.
- The `search_global`/`search_catalogs` descriptions mention `list_catalogs`; the instructions mention
  `get_catalog_filter_options`.
- `filter: "public"` deserializes and reaches the API.

### Phase 3b: `update_catalog` + `generate_catalog_with_cards` writes (`src/tools/catalogs.rs`, `src/tools/generate.rs`, `src/server.rs`)

Depends on Phase 3a (same files).

Tests (`body_partial_json` and request counting):
- `update_catalog` with both `front_languages` and `topics` → one PATCH with `learn`, no GET.
- Only `topics` → a GET and then a PATCH whose `learn` keeps the stored `lang:` keys. Only `front_languages` → it keeps
  the stored topics (including a stored topic the GET returns that is not in any list).
- Only `topics` with a stale `version` (the GET returns a newer one) → Conflict text and **no PATCH**. A GET 404 → error
  and no PATCH.
- `[]` → `"speakLangs":[]` / `"learn":[]` is sent (the clear case). An omitted field → key absent
  (`test_update_catalog_never_sends_speaking_settings` style).
- `levels:["advanced"]` → `["C1","C2"]`. Over the cap → zero requests.
- `apply_shadowing` still sends no filter keys (existing test plus one assertion).
- `generate_catalog_with_cards` with all four → the POST body has `speakLangs`, `learn` and `levels`; a local error →
  zero requests; an API 400 → the hint.
- Descriptions contain "front_languages" and "get_catalog_filter_options".

### Phase 4: resource + prompt (`src/resources/mod.rs`, `src/prompts/mod.rs`)

Depends on Phase 2 only. Its files are **disjoint from 3a/3b**, so it may run in parallel with them in the same tree if
the orchestrator verifies sequentially. The simpler choice is to run it after 3b.

Tests: `list_all` includes `engramo://catalog-filters` (update the resource-count assertions); a read returns the view
JSON; an API error → the existing error mapping; `create_language_deck` text contains `front_languages` and
`back_languages`.

### Phase 5: docs (`README.md`, `docs/directory-submission.md`, `CLAUDE.md`, `docs/prompt-examples.md`)

Depends on the final descriptions from 3a/3b.

- README tool table: add `get_catalog_filter_options`, add the new params to `list_catalogs`/`update_catalog`, and add
  the new resource.
- `directory-submission.md`: 34 → **35 tools** (line 19 and §3 intro), `openWorldHint: false` 30 → 31, add
  `get_catalog_filter_options` to §3.1, and add a test-case row ("Find public Spanish catalogs for beginners about
  medicine" → `get_catalog_filter_options`?, `list_catalogs`).
- `CLAUDE.md` tree: add `catalog_filters.rs` and the resource list.
- `prompt-examples.md`: one example that covers search, then classification.

### Phase 6: `/review` loop on the branch. Phase 7: commit, push (engramo SSH identity), open PR (orchestrator). The release is user-driven.

### Progress

| Phase | Status | Files | Tests (before → after) | Notes |
|---|---|---|---|---|
| 0 branch | done | — | | `feat/extended-catalog-filters` |
| 1 pure module | done | `src/catalog_filters.rs`, `src/lib.rs` | → 956 total (942 lib + 14 integration) after phase 5 | |
| 2 wire layer | done | `src/dto.rs`, `src/client.rs`, (`src/catalog_filters.rs`, `src/server.rs` literals) | | |
| 3a list + options tool | done | `src/tools/catalogs.rs`, `src/server.rs` | | incl. facets (Decision 2) |
| 3b writes | done | `src/tools/catalogs.rs`, `src/tools/generate.rs`, `src/server.rs` | | |
| 3c translate_batch_import | done | `src/tools/ai.rs` | | Decision 4 |
| 4 resource + prompt | done | `src/resources/mod.rs`, `src/prompts/mod.rs` | | static resource, no counts; prompt sets front/back languages, topics, levels on the new-catalog path only |
| 5 docs | done | `README.md`, `docs/directory-submission.md`, `CLAUDE.md`, `docs/prompt-examples.md` | 956 passed, 0 failed | 34 → 35 tools (verified in code) |
| 6 review | done | src/error.rs, src/server.rs, src/tools/{catalogs,ai,generate}.rs, src/catalog_filters.rs | 964 passed, 0 failed | 14 findings fixed (1 High: 3-letter target_lang default); final pass 0 security, 2 Low doc nits fixed inline; open_world_hint deferred to user |
| 7 PR | | — | | |

---

## 6. Risks

| Risk | Mitigation |
|---|---|
| Read-modify-write on `learn` races with another editor | The PATCH carries the caller's `version`, so a concurrent change gives 409 and the existing Conflict text |
| The LLM over-filters and gets an empty list (dev: 302 of 310 private catalogs have no languages) | The description says so and tells it to retry with fewer filters |
| The topic list changes at runtime | Never hard-code it. Examples in descriptions only; `get_catalog_filter_options` reads it live |
| The directory re-scan sees a new tool and changed descriptions | The tool is read-only and closed-world, and the changes are additive. The user decides when to release (Q1) |
| The API adds a field (e.g. card types back) | `CatalogClassification` ignores unknown keys. Additive later |

## 7. Follow-ups (not in this TDD)

- When engram-api ships B36 (D67, filters on `GET /search`), add the same five params to `search_global`.
- Facet counts and tag suggestions as a tool (Q2).

## 8. Open questions

1. **New tool vs. directory status.** The ChatGPT review is open (memory 2026-10-18), and adding
   `get_catalog_filter_options` makes the count 35. Is a new tool acceptable now, or should this ship with the
   resource only and topic examples in the descriptions, adding the tool later?
2. **Counts and tag suggestions.** Should `get_catalog_filter_options` take optional filter params and also return
   `GET /catalogs/facets` counts ("which topics have Spanish decks?") and top tags? That would be useful for
   discovery, but it makes the tool bigger.
3. **"Language learning only" search.** Should `list_catalogs` expose the API's `language` pseudo-key (language decks
   that have no topic), for example as `language_decks_only: bool`? The app search dropped it in rev. 17 (D80).
4. **`translate_batch_import`.** Should it set `back_languages = [target_lang]` (and accept `front_languages`)
   automatically? It is paid AI and off on prod.

---

## Decisions from review (2026-10-31) — these override anything above

1. **Q1 — new tool OK.** Ship `get_catalog_filter_options` as the 35th tool now, plus the resource.
2. **Q2 — yes, include facets.** `get_catalog_filter_options` also returns `GET /catalogs/facets` counts and top
   tags, and it accepts the same filter inputs as `list_catalogs`, so the model can ask "which topics have Spanish
   decks?". Remove facets from Non-goals. The tag prefix search (`/catalogs/facets/tags`) stays out of scope.
   Phase 2 adds the facets DTO and client method; phase 3a wires it into the tool. The static
   `engramo://catalog-filters` resource stays static (allowed values only, no counts).
3. **Q3 — not exposed.** The `language` pseudo-key stays a non-goal (the app dropped it in D80). `normalize_topics`
   keeps rejecting it.
4. **Q4 — yes.** `translate_batch_import` (src/tools/ai.rs, paid AI) sets `back_languages = [target_lang]` on the
   catalog it creates, unless the caller passes `back_languages` explicitly. It also accepts optional
   `front_languages`, `topics` and `levels`. Remove it from Non-goals. This is a new phase **3c** (files:
   `src/tools/ai.rs`, plus `src/client.rs`/`src/dto.rs` only if the request shape needs it). It runs after 3b.
5. **3c defaults `back_languages` only.** `translate_batch_import` defaults `back_languages` to `[target_lang]` unless the caller passes it, and sets no front-language default, because the tool has no source-language input.
