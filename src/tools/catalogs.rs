use rmcp::{model::CallToolResult, schemars};
use serde::Deserialize;
use uuid::Uuid;

use crate::catalog_filters::{
    LevelParam, MAX_SEARCH_BACK, MAX_SEARCH_LEARN, MAX_SEARCH_TAGS, MAX_WRITE_BACK,
    MAX_WRITE_LEARN, expand_levels, join_learn, normalize_langs, normalize_topics,
};
use crate::dto::{AccessLevel, CatalogClassification, CatalogFilter, CatalogListQuery};
use crate::error::{ApiError, preview};

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ListCatalogsParams {
    #[schemars(
        description = "Maximum number of catalogs to return (default 20; values are clamped to 1..=50)"
    )]
    pub limit: Option<i64>,
    #[schemars(
        description = "Pagination cursor from a previous response. Pass it back with the same filters and query."
    )]
    pub cursor: Option<String>,
    #[schemars(
        description = "If true, return only catalogs with shadowing turned on. Keep the same value for every page: a cursor from one value is rejected with the other."
    )]
    pub speaking_only: Option<bool>,
    #[schemars(
        description = "Optional: 'own' lists only catalogs you created; 'shared' lists only catalogs other users shared with you; 'public' lists only public catalogs by any user. Omit to include all three: your own, shared with you, and public catalogs. Keep the same value for every page."
    )]
    pub filter: Option<CatalogFilter>,
    #[schemars(
        description = "Optional free text matched against catalog name, description and short ID (max 200 characters and 512 bytes)."
    )]
    pub query: Option<String>,
    #[schemars(
        description = "Front-of-card (learning) languages as ISO 639-1 codes, e.g. ['es']. Matches catalogs whose cards are in any of them. Catalogs with no front language set never match."
    )]
    pub front_languages: Option<Vec<String>>,
    #[schemars(
        description = "Back-of-card (definition/translation) languages as ISO 639-1 codes, at most two. Matches any of them; language-free catalogs (code 'zxx') always match. Catalogs with no back language set never match."
    )]
    pub back_languages: Option<Vec<String>>,
    #[schemars(
        description = "Topic keys from get_catalog_filter_options (e.g. 'medicine', 'software_dev'). Matches any of them. Combined with front_languages using AND."
    )]
    pub topics: Option<Vec<String>>,
    #[schemars(
        description = "CEFR levels A1-C2, or 'beginner' (A1,A2), 'intermediate' (B1,B2), 'advanced' (C1,C2). Matches any."
    )]
    pub levels: Option<Vec<LevelParam>>,
    #[schemars(
        description = "Tags; a catalog must have ALL of them (case-insensitive). At most 8."
    )]
    pub tags: Option<Vec<String>>,
}

/// Same filter inputs as [`ListCatalogsParams`] (no paging): narrows the facet counts that
/// `get_catalog_filter_options` returns, e.g. which topics have Spanish decks.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct CatalogFilterOptionsParams {
    #[schemars(
        description = "Optional: 'own', 'shared' or 'public': count only that source. Omit for all visible catalogs."
    )]
    pub filter: Option<CatalogFilter>,
    #[schemars(
        description = "Optional free text matched against catalog name, description and short ID (max 200 characters and 512 bytes)."
    )]
    pub query: Option<String>,
    #[schemars(
        description = "Optional: count only catalogs whose cards are in these languages (ISO 639-1 codes, e.g. ['es'])."
    )]
    pub front_languages: Option<Vec<String>>,
    #[schemars(
        description = "Optional: count only catalogs with these back (definition/translation) languages (ISO 639-1 codes, at most two; 'zxx' = language-free)."
    )]
    pub back_languages: Option<Vec<String>>,
    #[schemars(description = "Optional: count only catalogs with these topic keys.")]
    pub topics: Option<Vec<String>>,
    #[schemars(
        description = "Optional: CEFR levels A1-C2, or 'beginner', 'intermediate', 'advanced'."
    )]
    pub levels: Option<Vec<LevelParam>>,
    #[schemars(description = "Optional: tags a catalog must all have (at most 8).")]
    pub tags: Option<Vec<String>>,
}

/// Borrowed view of the filter inputs shared by `list_catalogs` and `get_catalog_filter_options`.
pub(crate) struct FilterInputs<'a> {
    pub query: Option<&'a str>,
    pub filter: Option<CatalogFilter>,
    pub front_languages: Option<&'a [String]>,
    pub back_languages: Option<&'a [String]>,
    pub topics: Option<&'a [String]>,
    pub levels: Option<&'a [LevelParam]>,
    pub tags: Option<&'a [String]>,
}

/// Max length of the free-text `query` (the API's `MAX_Q_CHARS`).
const MAX_QUERY_CHARS: usize = 200;

/// Byte bound the HTTP client enforces on the `q` wire param (`MAX_QUERY_PARAM_LEN`).
const MAX_QUERY_BYTES: usize = 512;

/// Normalizes the filter inputs into a wire query (search caps) with no request made. Returns
/// the query and whether any filter param beyond `filter` was given (decides the 400 hint).
/// A blank `query` is not sent.
pub(crate) fn build_catalog_query(f: FilterInputs<'_>) -> Result<(CatalogListQuery, bool), String> {
    let query = f.query.map(str::trim).filter(|q| !q.is_empty());
    if let Some(q) = query
        && (q.chars().count() > MAX_QUERY_CHARS || q.len() > MAX_QUERY_BYTES)
    {
        return Err(format!(
            "query is too long: at most {MAX_QUERY_CHARS} characters ({MAX_QUERY_BYTES} bytes)."
        ));
    }
    let front = normalize_langs(
        f.front_languages.unwrap_or_default(),
        "front_languages",
        false,
        MAX_SEARCH_LEARN,
    )?;
    let back = normalize_langs(
        f.back_languages.unwrap_or_default(),
        "back_languages",
        true,
        MAX_SEARCH_BACK,
    )?;
    let topics = normalize_topics(f.topics.unwrap_or_default())?;
    let learn = join_learn(&front, &topics, MAX_SEARCH_LEARN)?;
    let levels = expand_levels(f.levels.unwrap_or_default());
    let tags = normalize_search_tags(f.tags.unwrap_or_default())?;
    let has_filters = query.is_some()
        || !(learn.is_empty() && back.is_empty() && levels.is_empty() && tags.is_empty());
    Ok((
        CatalogListQuery {
            filter: f.filter,
            q: query.map(str::to_owned),
            speak: back,
            learn,
            levels,
            tags,
            ..Default::default()
        },
        has_filters,
    ))
}

/// Trims and lowercases search tags (the API stores them that way), drops blanks and
/// duplicates, rejects a tag with a comma (the list goes out comma-joined) and caps the count.
fn normalize_search_tags(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for tag in raw {
        let tag = tag.trim().to_lowercase();
        if tag.is_empty() {
            continue;
        }
        if tag.contains(',') {
            return Err(format!(
                "tags: '{}' must not contain a comma.",
                preview(&tag)
            ));
        }
        if !out.contains(&tag) {
            out.push(tag);
        }
    }
    if out.len() > MAX_SEARCH_TAGS {
        return Err(format!("tags: at most {MAX_SEARCH_TAGS} tags per search."));
    }
    Ok(out)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetCatalogParams {
    #[schemars(description = "UUID of the catalog")]
    pub catalog_id: String,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct UpdateCatalogParams {
    #[schemars(description = "UUID of the catalog to update")]
    pub catalog_id: String,
    #[schemars(description = "New name")]
    pub name: Option<String>,
    #[schemars(description = "New description")]
    pub description: Option<String>,
    #[schemars(
        description = "New tags (lowercased; at most 20, each up to 40 characters). Replaces the stored tags."
    )]
    pub tags: Option<Vec<String>>,
    #[schemars(
        description = "New visibility: 'public', 'private', or 'unlisted' (hidden from listings/search, but reachable by anyone who has its exact short_id). Setting 'public' or 'unlisted' may require publish permission — non-moderators can get a permission error."
    )]
    pub visibility: Option<String>,
    #[schemars(
        description = "New cover image: a media_id from upload_media (an image you own). Omit to keep the current cover. A cover can be replaced but not removed."
    )]
    pub image_id: Option<String>,
    #[schemars(
        description = "Languages the card FRONTS are in (the language being learned), as ISO 639-1 codes, e.g. ['es']. Replaces the stored list; [] clears it; omit to keep it. front_languages and topics together: at most 3 values."
    )]
    pub front_languages: Option<Vec<String>>,
    #[schemars(
        description = "Languages of the card BACKS (translations/definitions), as ISO 639-1 codes, at most 2; 'zxx' = no language needed (flags, symbols). Replaces the stored list; [] clears it; omit to keep it."
    )]
    pub back_languages: Option<Vec<String>>,
    #[schemars(
        description = "Topic keys from get_catalog_filter_options (the list changes over time; don't guess). Replaces the stored list; [] clears it; omit to keep it. More specific subjects belong in tags."
    )]
    pub topics: Option<Vec<String>>,
    #[schemars(
        description = "CEFR levels A1-C2, or 'beginner' (A1,A2), 'intermediate' (B1,B2), 'advanced' (C1,C2). Replaces the stored list; [] clears it; omit to keep it."
    )]
    pub levels: Option<Vec<LevelParam>>,
    #[schemars(description = "Current version of the catalog (required for optimistic locking)")]
    pub version: i32,
}

/// Normalized catalog classification for a write (`update_catalog`, `generate_catalog_with_cards`).
/// Every field keeps the wire semantics: `None` = omitted (unchanged), `Some(vec![])` = clear.
/// The API stores front languages and topics together in `learn`, so [`Self::learn`] needs the
/// catalog's current classification when only one of the two was given.
#[derive(Debug, Default)]
pub(crate) struct ClassificationWrite {
    pub front: Option<Vec<String>>,
    pub topics: Option<Vec<String>>,
    pub speak_langs: Option<Vec<String>>,
    pub levels: Option<Vec<String>>,
}

impl ClassificationWrite {
    /// Normalizes the four params with the write caps; no I/O, so an error means no request.
    pub fn new(
        front: Option<&[String]>,
        back: Option<&[String]>,
        topics: Option<&[String]>,
        levels: Option<&[LevelParam]>,
    ) -> Result<Self, String> {
        let front = front
            .map(|v| normalize_langs(v, "front_languages", false, MAX_WRITE_LEARN))
            .transpose()?;
        let topics = topics.map(normalize_topics).transpose()?;
        if let (Some(f), Some(t)) = (&front, &topics) {
            join_learn(f, t, MAX_WRITE_LEARN)?;
        }
        Ok(Self {
            front,
            topics,
            speak_langs: back
                .map(|v| normalize_langs(v, "back_languages", true, MAX_WRITE_BACK))
                .transpose()?,
            levels: levels.map(expand_levels),
        })
    }

    /// True when any of the four fields was given (decides the 400 hint).
    pub fn any(&self) -> bool {
        self.front.is_some()
            || self.topics.is_some()
            || self.speak_langs.is_some()
            || self.levels.is_some()
    }

    /// True when exactly one of front languages / topics was given, so the other half has to
    /// be read from the catalog before `learn` can be written.
    pub fn needs_current(&self) -> bool {
        self.front.is_some() != self.topics.is_some()
    }

    /// The `learn` value to send. `current` is required only when [`Self::needs_current`].
    pub fn learn(
        &self,
        current: Option<&CatalogClassification>,
    ) -> Result<Option<Vec<String>>, String> {
        match (&self.front, &self.topics) {
            (None, None) => Ok(None),
            (Some(f), Some(t)) => join_learn(f, t, MAX_WRITE_LEARN).map(Some),
            (Some(f), None) => {
                let t = current.map(|c| c.topics.as_slice()).unwrap_or_default();
                join_learn(f, t, MAX_WRITE_LEARN).map(Some)
            }
            (None, Some(t)) => {
                let f = current
                    .map(|c| c.front_languages.as_slice())
                    .unwrap_or_default();
                join_learn(f, t, MAX_WRITE_LEARN).map(Some)
            }
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteCatalogParams {
    #[schemars(description = "UUID of the catalog to delete")]
    pub catalog_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetCatalogShadowingParams {
    #[schemars(description = "UUID of the catalog (must be one you own)")]
    pub catalog_id: String,
    #[schemars(
        description = "true turns shadowing on (or changes its language); false turns it off"
    )]
    pub enabled: bool,
    #[schemars(
        description = "Language of the card FRONTS (faces) as a BCP-47 tag, e.g. 'es', 'es-ES', 'pt-BR', 'sr-Latn'. Required when enabled is true; ignored when enabled is false."
    )]
    pub language: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ShareCatalogParams {
    #[schemars(description = "UUID of the catalog to share (must be one you own)")]
    pub catalog_id: String,
    #[schemars(
        description = "EngrAmo user ids (UUIDs, not emails) to grant access to: 1 to 100 per call"
    )]
    pub user_ids: Vec<String>,
    #[schemars(
        description = "'viewer' (default) can read and study the catalog; 'editor' can also edit its cards and details (not share it, delete it, or change shadowing)"
    )]
    pub access_level: Option<AccessLevel>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UnshareCatalogParams {
    #[schemars(description = "UUID of the catalog to stop sharing (must be one you own)")]
    pub catalog_id: String,
    #[schemars(
        description = "EngrAmo user ids (UUIDs, not emails) to remove access from: 1 to 100 per call"
    )]
    pub user_ids: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListCatalogSharesParams {
    #[schemars(description = "UUID of the catalog")]
    pub catalog_id: String,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub(crate) fn ok_json<T: serde::Serialize>(value: &T) -> CallToolResult {
    match serde_json::to_string_pretty(value) {
        Ok(json) => CallToolResult::success(vec![rmcp::model::ContentBlock::text(json)]),
        Err(e) => CallToolResult::error(vec![rmcp::model::ContentBlock::text(format!(
            "Serialization error: {e}"
        ))]),
    }
}

pub(crate) fn ok_text(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![rmcp::model::ContentBlock::text(text.into())])
}

pub(crate) fn err_result(e: impl std::fmt::Display) -> CallToolResult {
    let message = e.to_string();
    // WARN, not ERROR: this is the single choke point for every failed tool call
    // (~87 call sites), most of which are routine 4xx-shaped outcomes (not found,
    // bad input, quota, conflict) rather than bugs. Genuine system failures (backend
    // 5xx, network errors) are logged at ERROR at their own point of origin (see
    // `ApiError::from_response` and `impl From<reqwest::Error> for ApiError` in
    // `error.rs`), which is what feeds GCP Error Reporting.
    tracing::warn!(error = %message, "tool call returned an error");
    CallToolResult::error(vec![rmcp::model::ContentBlock::text(message)])
}

pub(crate) fn parse_uuid(s: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|_| format!("Invalid UUID: '{}'", preview(s)))
}

/// Upper bound on user ids per `share_catalog` / `unshare_catalog` call (after dedupe).
pub(crate) const MAX_SHARE_USER_IDS: usize = 100;

fn too_many_user_ids() -> String {
    format!(
        "Too many user ids: at most {MAX_SHARE_USER_IDS} per call; split the list into smaller calls."
    )
}

/// Validates the `user_ids` of a sharing call: non-empty, every entry a UUID (trimmed),
/// duplicates dropped silently in first-seen order (the backend rejects duplicates), at most
/// `MAX_SHARE_USER_IDS` remaining. Pure, so no HTTP call is made on failure.
pub(crate) fn parse_user_ids(raw: &[String]) -> Result<Vec<Uuid>, String> {
    if raw.is_empty() {
        return Err("`user_ids` must contain at least one EngrAmo user id (UUID).".to_string());
    }
    // Reject absurd input before doing any per-entry work (dedupe can only shrink the list, so
    // allow some headroom for duplicates).
    if raw.len() > MAX_SHARE_USER_IDS * 10 {
        return Err(too_many_user_ids());
    }
    let mut seen = std::collections::HashSet::new();
    let mut ids: Vec<Uuid> = Vec::with_capacity(raw.len().min(MAX_SHARE_USER_IDS));
    for (i, entry) in raw.iter().enumerate() {
        let id = parse_uuid(entry.trim()).map_err(|e| {
            format!("user_ids[{i}]: {e} — user ids are UUIDs, not emails or names; ask the person for their EngrAmo user id")
        })?;
        if seen.insert(id) {
            ids.push(id);
            if ids.len() > MAX_SHARE_USER_IDS {
                return Err(too_many_user_ids());
            }
        }
    }
    Ok(ids)
}

/// Rewrites the backend errors of the sharing write endpoints that need clearer wording; every
/// other error passes through unchanged.
pub(crate) fn share_write_error(e: ApiError) -> ApiError {
    match e {
        // Only the generic 403 (see `forbidden_hint`) is rewritten; a specific backend message
        // passes through unchanged.
        ApiError::PermissionDenied(msg) if crate::error::is_generic_forbidden(&msg) => {
            ApiError::PermissionDenied(
                "only the catalog's owner can share it or change who has access (editors and viewers can't); \
                 also check the catalog id: the catalog may not exist or may belong to another user."
                    .to_string(),
            )
        }
        ApiError::NotFound(msg) => {
            // Only rewrite the "unknown user" 404, whose message is exactly `[Not found: ]user
            // <uuid>`; any other 404 (e.g. catalog not found) passes through unchanged.
            let trimmed = msg.trim();
            let id = trimmed
                .strip_prefix("Not found: ")
                .unwrap_or(trimmed)
                .strip_prefix("user ")
                .and_then(|rest| rest.split_whitespace().next())
                .map(|t| t.trim_end_matches(|c: char| !c.is_ascii_hexdigit()))
                .and_then(|t| Uuid::parse_str(t).ok());
            match id {
                Some(id) => ApiError::NotFound(format!(
                    "no EngrAmo user has id {id}; nothing was shared or changed (the request is all-or-nothing). Check the id with its owner."
                )),
                None => ApiError::NotFound(msg),
            }
        }
        other => other,
    }
}

/// Rewrites the backend errors of the read-only collaborator listing; every other error passes
/// through unchanged.
pub(crate) fn share_read_error(e: ApiError) -> ApiError {
    match e {
        ApiError::PermissionDenied(msg) if crate::error::is_generic_forbidden(&msg) => {
            ApiError::PermissionDenied(
                "only the catalog's owner or its editors can see who it is shared with; also check the catalog id: the catalog may not exist or may belong to another user."
                    .to_string(),
            )
        }
        other => other,
    }
}

/// Fixes the case and separator of a BCP-47-like tag (`es_es` -> `es-ES`, `sr-latn` ->
/// `sr-Latn`) and nothing else: validation stays on the backend so the rule can't drift. The
/// region/script part is only re-cased when it is exactly 2 (region) or 4 (script) ASCII
/// characters; anything else, and anything non-ASCII, passes through for the backend to reject.
pub(crate) fn normalize_language_tag(raw: &str) -> String {
    let raw = raw.trim();
    let (lang, rest) = match raw.split_once(['-', '_']) {
        Some((lang, rest)) => (lang, Some(rest)),
        None => (raw, None),
    };
    let lang = lang.to_ascii_lowercase();
    match rest {
        None => lang,
        Some(rest) if rest.is_ascii() && rest.len() == 2 => {
            format!("{lang}-{}", rest.to_ascii_uppercase())
        }
        Some(rest) if rest.is_ascii() && rest.len() == 4 => {
            let (head, tail) = rest.split_at(1);
            format!(
                "{lang}-{}{}",
                head.to_ascii_uppercase(),
                tail.to_ascii_lowercase()
            )
        }
        Some(rest) => format!("{lang}-{rest}"),
    }
}

/// Generous upper bound for a BCP-47 tag (RFC 5646 recommends supporting at least 35 chars).
const MAX_LANGUAGE_TAG_LEN: usize = 64;

/// The language to store for `set_catalog_shadowing`: `None` (turn off) when `enabled` is false,
/// otherwise the normalized tag. `enabled: true` without a usable language is a local error, so
/// a model that forgets `language` can never silently turn shadowing off.
pub(crate) fn shadowing_target(
    enabled: bool,
    language: Option<&str>,
) -> Result<Option<String>, String> {
    if !enabled {
        return Ok(None);
    }
    match language.map(str::trim) {
        Some(l) if l.len() > MAX_LANGUAGE_TAG_LEN => Err(format!(
            "`language` is too long (max {MAX_LANGUAGE_TAG_LEN} bytes): '{}'",
            preview(l)
        )),
        Some(l) if !l.is_empty() => Ok(Some(normalize_language_tag(l))),
        _ => Err(
            "`language` is required when `enabled` is true (e.g. 'es', 'es-ES', 'pt-BR', \
                  'sr-Latn')"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::EngramoClient;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mock_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    fn make_server(base_url: &str) -> crate::server::EngramoMcpServer {
        crate::server::EngramoMcpServer::new(EngramoClient::new(base_url, "engramo_test"), false)
    }

    #[tokio::test]
    async fn test_list_catalogs_ok() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "name": "Rust", "version": 1}],
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: None,
                speaking_only: None,
                filter: None,
                ..Default::default()
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    fn tool_text(result: &rmcp::model::CallToolResult) -> &str {
        result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("")
    }

    #[tokio::test]
    async fn test_list_catalogs_speaking_only_param_reaches_api() {
        use rmcp::handler::server::wrapper::Parameters;
        for (flag, expect_param) in [(Some(true), true), (Some(false), false), (None, false)] {
            let server = MockServer::start().await;
            let mock = Mock::given(method("GET")).and(path("/catalogs"));
            let mock = if expect_param {
                mock.and(query_param("speakingOnly", "true"))
            } else {
                mock.and(query_param_is_missing("speakingOnly"))
            };
            mock.respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [], "nextCursor": null
            })))
            .expect(1)
            .mount(&server)
            .await;

            let result = make_server(&server.uri())
                .list_catalogs(Parameters(ListCatalogsParams {
                    limit: None,
                    cursor: None,
                    speaking_only: flag,
                    filter: None,
                    ..Default::default()
                }))
                .await
                .unwrap();
            assert!(!result.is_error.unwrap_or(false), "{flag:?}");
        }
    }

    #[tokio::test]
    async fn test_list_catalogs_output_includes_speaking_settings_only_when_set() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    {"id": mock_id(), "name": "Shadow", "version": 1,
                     "speakingSettings": {"language": "es-ES"}},
                    {"id": mock_id(), "name": "Plain", "version": 1, "speakingSettings": {}}
                ],
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: None,
                speaking_only: None,
                filter: None,
                ..Default::default()
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(tool_text(&result)).unwrap();
        assert_eq!(v["data"][0]["speakingSettings"]["language"], "es-ES");
        assert!(v["data"][1].get("speakingSettings").is_none(), "{v}");
    }

    #[tokio::test]
    async fn test_get_catalog_output_includes_speaking_settings_only_when_set() {
        use rmcp::handler::server::wrapper::Parameters;
        for (body, expected) in [
            (
                json!({"speakingSettings": {"language": "es-ES"}}),
                Some("es-ES"),
            ),
            (json!({}), None),
        ] {
            let server = MockServer::start().await;
            let mut catalog = json!({"id": mock_id(), "name": "Rust", "version": 1});
            catalog
                .as_object_mut()
                .unwrap()
                .extend(body.as_object().unwrap().clone());
            Mock::given(method("GET"))
                .and(path(format!("/catalogs/{}", mock_id())))
                .respond_with(ResponseTemplate::new(200).set_body_json(catalog))
                .mount(&server)
                .await;

            let result = make_server(&server.uri())
                .get_catalog(Parameters(GetCatalogParams {
                    catalog_id: mock_id().to_string(),
                }))
                .await
                .unwrap();
            let v: serde_json::Value = serde_json::from_str(tool_text(&result)).unwrap();
            match expected {
                Some(lang) => assert_eq!(v["speakingSettings"]["language"], lang),
                None => assert!(v.get("speakingSettings").is_none(), "{v}"),
            }
        }
    }

    /// Regression test for issue #34: the tool output's `cursor` field must reflect the
    /// API's real `nextCursor` value, not always be `null`.
    #[tokio::test]
    async fn test_list_catalogs_cursor_reaches_tool_output() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [],
                "nextCursor": "abc"
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: None,
                speaking_only: None,
                filter: None,
                ..Default::default()
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["cursor"], "abc", "{text}");
    }

    #[tokio::test]
    async fn test_list_catalogs_unauthorized_returns_error_content_not_panic() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        // CRITICAL: must return Ok(CallToolResult) with is_error=true, NOT panic or Err()
        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: None,
                speaking_only: None,
                filter: None,
                ..Default::default()
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false), "expected is_error=true");
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(
            text.contains("nauthorized") || text.contains("token"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn test_get_catalog_invalid_uuid_returns_error_content() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        // No mock needed — UUID parse fails before HTTP call
        let result = make_server(&server.uri())
            .get_catalog(Parameters(GetCatalogParams {
                catalog_id: "not-a-uuid".to_string(),
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(text.contains("Invalid UUID"), "{text}");
    }

    #[tokio::test]
    async fn test_update_catalog_conflict_returns_actionable_message() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/catalogs/{}", mock_id())))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .update_catalog(Parameters(UpdateCatalogParams {
                catalog_id: mock_id().to_string(),
                name: Some("New".to_string()),
                description: None,
                tags: None,
                visibility: None,
                image_id: None,
                version: 1,
                ..Default::default()
            }))
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(text.contains("Fetch the latest version"), "{text}");
    }

    #[tokio::test]
    async fn test_delete_catalog_ok() {
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!("/catalogs/{}", mock_id())))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "deleted"})))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .delete_catalog(Parameters(DeleteCatalogParams {
                catalog_id: mock_id().to_string(),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[test]
    fn test_parse_uuid_valid() {
        let id = parse_uuid("00000000-0000-0000-0000-000000000001");
        assert!(id.is_ok());
    }

    #[test]
    fn test_parse_uuid_invalid() {
        let id = parse_uuid("not-a-uuid");
        assert!(id.is_err());
        assert!(id.unwrap_err().contains("Invalid UUID"));
    }

    /// Regression for F10: a huge, malformed LLM-supplied value must not be echoed in full
    /// into the error text (and thus into the WARN log via `err_result`) — `preview` caps it.
    #[test]
    fn test_parse_uuid_huge_input_error_is_capped() {
        let huge = "x".repeat(1024 * 1024);
        let err = parse_uuid(&huge).unwrap_err();
        assert!(
            err.len() < 100,
            "error message not capped: {} bytes",
            err.len()
        );
    }

    #[tokio::test]
    async fn test_decode_error_body_values_do_not_reach_logs_via_err_result() {
        use crate::error::test_support::{BufWriter, TRACING_CAPTURE_LOCK};
        use rmcp::handler::server::wrapper::Parameters;

        // See `error::test_support::TRACING_CAPTURE_LOCK`: held for the capturing
        // subscriber's entire lifetime, including across the `.await`s below — a
        // `tokio::sync::Mutex` guard is fine to hold across an await (unlike a std one,
        // which clippy rejects).
        let _lock = TRACING_CAPTURE_LOCK.lock().await;

        let buf = BufWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        // Rebuild so the thread-local subscriber above actually sees the event, even if a
        // callsite was previously cached as "no subscriber interested" by another test. Safe
        // now that TRACING_CAPTURE_LOCK guarantees no other capturing subscriber is active
        // concurrently.
        tracing::callsite::rebuild_interest_cache();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": mock_id(), "name": "x", "version": "SECRET_USER_TEXT"}],
                "nextCursor": null
            })))
            .mount(&server)
            .await;

        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: None,
                speaking_only: None,
                filter: None,
                ..Default::default()
            }))
            .await
            .unwrap();
        drop(guard);

        assert_eq!(result.is_error, Some(true));
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(
            text.contains("Unexpected response from the Engramo API"),
            "{text}"
        );
        assert!(!text.contains("SECRET_USER_TEXT"), "{text}");
        let logged = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(!logged.contains("SECRET_USER_TEXT"), "{logged}");
    }

    #[tokio::test]
    async fn test_list_catalogs_network_failure_returns_error_without_url() {
        use rmcp::handler::server::wrapper::Parameters;
        // Port 1 (tcpmux) has no listener, so the connection is refused deterministically. A
        // dropped MockServer's port would race with other tests' servers reusing it.
        let uri = "http://127.0.0.1:1".to_string();

        let result = make_server(&uri)
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: None,
                cursor: Some("SECRET_CURSOR".to_string()),
                speaking_only: None,
                filter: None,
                ..Default::default()
            }))
            .await
            .unwrap(); // must NOT be Err — MCP contract
        assert_eq!(result.is_error, Some(true), "{result:?}");
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert!(text.contains("Network error"), "{text}");
        assert!(!text.contains("SECRET_CURSOR"), "{text}");
        assert!(!text.contains(&uri), "{text}");
    }

    #[test]
    fn test_normalize_language_tag() {
        for (input, expected) in [
            ("es_es", "es-ES"),
            (" PT-br ", "pt-BR"),
            ("sr-latn", "sr-Latn"),
            ("SR_LATN", "sr-Latn"),
            ("es", "es"),
            ("ES", "es"),
            ("spanish", "spanish"),
            ("zh-hans-cn", "zh-hans-cn"),
            ("es-é", "es-é"),
            ("ésp-es", "ésp-ES"),
        ] {
            assert_eq!(normalize_language_tag(input), expected, "{input}");
        }
    }

    #[test]
    fn test_shadowing_target_length_limit() {
        let ok = "a".repeat(64);
        assert_eq!(shadowing_target(true, Some(&ok)), Ok(Some(ok.clone())));
        assert_eq!(
            shadowing_target(true, Some(&format!("  {ok}  "))),
            Ok(Some(ok.clone()))
        );
        let long = "a".repeat(65);
        let err = shadowing_target(true, Some(&long)).unwrap_err();
        assert!(err.contains("too long") && err.contains("64"), "{err}");
        assert_eq!(shadowing_target(false, Some(&"a".repeat(500))), Ok(None));
    }

    #[test]
    fn test_shadowing_target() {
        assert_eq!(shadowing_target(false, None), Ok(None));
        assert_eq!(shadowing_target(false, Some("es_es")), Ok(None));
        assert_eq!(
            shadowing_target(true, Some("es_es")),
            Ok(Some("es-ES".to_string()))
        );
        for bad in [None, Some(""), Some("   ")] {
            assert!(shadowing_target(true, bad).is_err(), "{bad:?}");
        }
    }

    // ── sharing tools ────────────────────────────────────────────────────────

    use rmcp::handler::server::wrapper::Parameters;
    use wiremock::matchers::body_json;

    const CAT: &str = "00000000-0000-0000-0000-000000000001";
    const U1: &str = "11111111-1111-1111-1111-111111111111";
    const U2: &str = "22222222-2222-2222-2222-222222222222";

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn share_params(ids: &[&str], level: Option<AccessLevel>) -> ShareCatalogParams {
        ShareCatalogParams {
            catalog_id: CAT.into(),
            user_ids: strs(ids),
            access_level: level,
        }
    }

    async fn mount_collab_get(server: &MockServer, status: u16, body: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path(format!("/catalogs/{CAT}/collaborators")))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(server)
            .await;
    }

    async fn mount_collab_post(
        server: &MockServer,
        body: Option<serde_json::Value>,
        status: u16,
        resp: serde_json::Value,
    ) {
        let m = Mock::given(method("POST")).and(path(format!("/catalogs/{CAT}/collaborators")));
        let m = match body {
            Some(b) => m.and(body_json(b)),
            None => m,
        };
        m.respond_with(ResponseTemplate::new(status).set_body_json(resp))
            .mount(server)
            .await;
    }

    #[test]
    fn test_parse_user_ids_rules() {
        assert!(parse_user_ids(&[]).unwrap_err().contains("at least one"));
        let err = parse_user_ids(&strs(&[U1, "bob@example.com"])).unwrap_err();
        assert!(err.contains("user_ids[1]"), "{err}");
        assert!(err.contains("not emails"), "{err}");
        let padded = format!("  {U1}\n");
        assert_eq!(
            parse_user_ids(&[padded]).unwrap(),
            vec![Uuid::parse_str(U1).unwrap()]
        );
        let dup = parse_user_ids(&strs(&[U2, U1, U2, U1])).unwrap();
        assert_eq!(
            dup,
            vec![Uuid::parse_str(U2).unwrap(), Uuid::parse_str(U1).unwrap()]
        );
        let many =
            |n: u128| -> Vec<String> { (1..=n).map(|i| Uuid::from_u128(i).to_string()).collect() };
        assert_eq!(parse_user_ids(&many(100)).unwrap().len(), 100);
        assert!(parse_user_ids(&many(101)).unwrap_err().contains("at most"));
    }

    #[tokio::test]
    async fn test_share_catalog_default_viewer_posts_then_lists() {
        let server = MockServer::start().await;
        mount_collab_post(
            &server,
            Some(json!({"changes": [
                {"userId": U1, "accessLevel": "viewer"},
                {"userId": U2, "accessLevel": "viewer"}
            ]})),
            200,
            json!({"id": CAT, "shared": "successfully"}),
        )
        .await;
        mount_collab_get(
            &server,
            200,
            json!([{"userId": U1, "accessLevel": "viewer"}, {"userId": U2, "accessLevel": "viewer"}]),
        )
        .await;
        let r = make_server(&server.uri())
            .share_catalog(Parameters(share_params(&[U1, U2], None)))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false), "{r:?}");
        let v: serde_json::Value = serde_json::from_str(tool_text(&r)).unwrap();
        assert_eq!(v["granted"], json!([U1, U2]));
        assert_eq!(v["accessLevel"], "viewer");
        assert_eq!(v["collaborators"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_share_catalog_editor_and_dedupe() {
        let server = MockServer::start().await;
        mount_collab_post(
            &server,
            Some(json!({"changes": [{"userId": U1, "accessLevel": "editor"}]})),
            200,
            json!({"id": CAT}),
        )
        .await;
        mount_collab_get(
            &server,
            200,
            json!([{"userId": U1, "accessLevel": "editor"}]),
        )
        .await;
        let r = make_server(&server.uri())
            .share_catalog(Parameters(share_params(
                &[U1, U1],
                Some(AccessLevel::Editor),
            )))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false), "{r:?}");
        assert!(tool_text(&r).contains("editor"));
    }

    #[tokio::test]
    async fn test_sharing_tools_local_validation_makes_no_request() {
        let server = MockServer::start().await;
        let s = make_server(&server.uri());
        let r = s
            .share_catalog(Parameters(ShareCatalogParams {
                catalog_id: "nope".into(),
                user_ids: strs(&[U1]),
                access_level: None,
            }))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        for ids in [vec![], vec!["bob@example.com"]] {
            let r = s
                .share_catalog(Parameters(share_params(&ids, None)))
                .await
                .unwrap();
            assert_eq!(r.is_error, Some(true), "{ids:?}");
            let r = s
                .unshare_catalog(Parameters(UnshareCatalogParams {
                    catalog_id: CAT.into(),
                    user_ids: strs(&ids),
                }))
                .await
                .unwrap();
            assert_eq!(r.is_error, Some(true), "{ids:?}");
        }
        let r = s
            .unshare_catalog(Parameters(UnshareCatalogParams {
                catalog_id: "nope".into(),
                user_ids: strs(&[U1]),
            }))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        let r = s
            .list_catalog_shares(Parameters(ListCatalogSharesParams {
                catalog_id: "nope".into(),
            }))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        let too_many: Vec<String> = (1..=101u128)
            .map(|i| Uuid::from_u128(i).to_string())
            .collect();
        let r = s
            .share_catalog(Parameters(ShareCatalogParams {
                catalog_id: CAT.into(),
                user_ids: too_many,
                access_level: None,
            }))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_share_catalog_error_messages() {
        for (status, body, needles) in [
            (
                403,
                json!({"error": "Forbidden"}),
                vec!["Permission denied", "only the catalog's owner"],
            ),
            (
                404,
                json!({"error": format!("Not found: user {U2}")}),
                vec![
                    "no EngrAmo user has id",
                    U2,
                    "all-or-nothing",
                    "nothing was shared",
                ],
            ),
            (
                404,
                json!({"error": format!("Not found: Catalog with id {CAT} not found")}),
                vec!["Catalog with id"],
            ),
            (
                400,
                json!({"error": "Cannot share a catalog with yourself"}),
                vec!["Bad request", "yourself"],
            ),
            (
                429,
                json!({"error": "limit", "resource_type": "collaborators_per_catalog",
                       "used": 4, "limit": 5, "requested": 3, "window": "absolute"}),
                vec!["Quota exceeded"],
            ),
        ] {
            let server = MockServer::start().await;
            mount_collab_post(&server, None, status, body).await;
            let r = make_server(&server.uri())
                .share_catalog(Parameters(share_params(&[U1], None)))
                .await
                .unwrap();
            assert_eq!(r.is_error, Some(true), "{status}");
            for n in needles {
                assert!(
                    tool_text(&r).contains(n),
                    "{status}: {n} in {}",
                    tool_text(&r)
                );
            }
        }
    }

    #[tokio::test]
    async fn test_share_catalog_reread_failure_still_succeeds() {
        let server = MockServer::start().await;
        mount_collab_post(&server, None, 200, json!({"id": CAT})).await;
        mount_collab_get(&server, 500, json!({"error": "boom"})).await;
        let r = make_server(&server.uri())
            .share_catalog(Parameters(share_params(&[U1], None)))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false), "{r:?}");
        let v: serde_json::Value = serde_json::from_str(tool_text(&r)).unwrap();
        assert_eq!(v["granted"], json!([U1]));
        assert!(v["note"].as_str().unwrap().contains("could not be fetched"));
        assert!(v.get("collaborators").is_none());
    }

    #[tokio::test]
    async fn test_unshare_catalog_posts_null_levels_then_lists() {
        let server = MockServer::start().await;
        mount_collab_post(
            &server,
            Some(json!({"changes": [
                {"userId": U1, "accessLevel": null},
                {"userId": U2, "accessLevel": null}
            ]})),
            200,
            json!({"id": CAT}),
        )
        .await;
        mount_collab_get(&server, 200, json!([])).await;
        let r = make_server(&server.uri())
            .unshare_catalog(Parameters(UnshareCatalogParams {
                catalog_id: CAT.into(),
                user_ids: strs(&[U1, U2, U1]),
            }))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false), "{r:?}");
        let v: serde_json::Value = serde_json::from_str(tool_text(&r)).unwrap();
        assert_eq!(v["requestedRemoval"], json!([U1, U2]));
        assert_eq!(v["collaborators"], json!([]));
    }

    #[tokio::test]
    async fn test_unshare_catalog_forbidden_is_owner_only() {
        let server = MockServer::start().await;
        mount_collab_post(&server, None, 403, json!({"error": "Forbidden"})).await;
        let r = make_server(&server.uri())
            .unshare_catalog(Parameters(UnshareCatalogParams {
                catalog_id: CAT.into(),
                user_ids: strs(&[U1]),
            }))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(tool_text(&r).contains("only the catalog's owner"));
    }

    #[tokio::test]
    async fn test_list_catalog_shares_happy_empty_and_forbidden() {
        let p = || {
            Parameters(ListCatalogSharesParams {
                catalog_id: CAT.into(),
            })
        };
        let server = MockServer::start().await;
        mount_collab_get(
            &server,
            200,
            json!([{"userId": U1, "accessLevel": "editor"}]),
        )
        .await;
        let r = make_server(&server.uri())
            .list_catalog_shares(p())
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false));
        let v: serde_json::Value = serde_json::from_str(tool_text(&r)).unwrap();
        assert_eq!(v["collaborators"][0]["userId"], U1);
        assert_eq!(v["collaborators"][0]["accessLevel"], "editor");

        let server = MockServer::start().await;
        mount_collab_get(&server, 200, json!([])).await;
        let r = make_server(&server.uri())
            .list_catalog_shares(p())
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(tool_text(&r)).unwrap();
        assert_eq!(v["collaborators"], json!([]));

        let server = MockServer::start().await;
        mount_collab_get(&server, 403, json!({"error": "Forbidden"})).await;
        let r = make_server(&server.uri())
            .list_catalog_shares(p())
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(tool_text(&r).contains("owner or its editors"));
    }

    #[tokio::test]
    async fn test_unshare_catalog_reread_failure_still_succeeds() {
        let server = MockServer::start().await;
        mount_collab_post(&server, None, 200, json!({"id": CAT})).await;
        mount_collab_get(&server, 500, json!({"error": "boom"})).await;
        let r = make_server(&server.uri())
            .unshare_catalog(Parameters(UnshareCatalogParams {
                catalog_id: CAT.into(),
                user_ids: strs(&[U1]),
            }))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false), "{r:?}");
        let v: serde_json::Value = serde_json::from_str(tool_text(&r)).unwrap();
        assert_eq!(v["requestedRemoval"], json!([U1]));
        assert!(v["note"].as_str().unwrap().contains("could not be fetched"));
        assert!(v.get("collaborators").is_none());
    }

    #[tokio::test]
    async fn test_unshare_catalog_error_passthrough() {
        for (status, body, needles) in [
            (
                404,
                json!({"error": format!("Not found: user {U2}")}),
                vec!["no EngrAmo user has id", U2],
            ),
            (500, json!({"error": "boom"}), vec!["Server error"]),
        ] {
            let server = MockServer::start().await;
            mount_collab_post(&server, None, status, body).await;
            let r = make_server(&server.uri())
                .unshare_catalog(Parameters(UnshareCatalogParams {
                    catalog_id: CAT.into(),
                    user_ids: strs(&[U2]),
                }))
                .await
                .unwrap();
            assert_eq!(r.is_error, Some(true), "{status}");
            for n in needles {
                assert!(tool_text(&r).contains(n), "{status}: {}", tool_text(&r));
            }
        }
    }

    #[tokio::test]
    async fn test_list_catalog_shares_not_found_and_server_error() {
        let p = || {
            Parameters(ListCatalogSharesParams {
                catalog_id: CAT.into(),
            })
        };
        let server = MockServer::start().await;
        mount_collab_get(&server, 404, json!({"error": "Not found: Catalog"})).await;
        let r = make_server(&server.uri())
            .list_catalog_shares(p())
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(tool_text(&r).contains("Not found"), "{}", tool_text(&r));
        assert!(!tool_text(&r).contains("owner or its editors"));

        let server = MockServer::start().await;
        mount_collab_get(&server, 500, json!({"error": "boom"})).await;
        let r = make_server(&server.uri())
            .list_catalog_shares(p())
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));

        let server = MockServer::start().await;
        let uri = server.uri();
        drop(server);
        let r = make_server(&uri).list_catalog_shares(p()).await.unwrap();
        assert_eq!(r.is_error, Some(true));
    }

    #[test]
    fn test_share_write_error_rewrites_and_passes_through() {
        match share_write_error(ApiError::NotFound(format!("Not found: user {U2}"))) {
            ApiError::NotFound(m) => {
                assert!(m.contains("no EngrAmo user has id") && m.contains(U2))
            }
            other => panic!("{other:?}"),
        }
        for msg in [
            "Not found: user ",
            "Catalog X not found",
            "catalog not found for user bob",
            "Catalog 22222222-2222-2222-2222-222222222222 not found for user 11111111-1111-1111-1111-111111111111",
        ] {
            assert!(
                matches!(share_write_error(ApiError::NotFound(msg.into())), ApiError::NotFound(m) if m == msg),
                "{msg}"
            );
        }
        assert!(matches!(
            share_write_error(ApiError::BadRequest("x".into())),
            ApiError::BadRequest(m) if m == "x"
        ));
        assert!(matches!(
            share_write_error(ApiError::Internal),
            ApiError::Internal
        ));
        match share_write_error(ApiError::PermissionDenied(
            crate::error::GENERIC_FORBIDDEN.into(),
        )) {
            ApiError::PermissionDenied(m) => {
                assert!(m.contains("only the catalog's owner") && m.contains("catalog id"))
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            share_write_error(ApiError::PermissionDenied("not allowed: plan".into())),
            ApiError::PermissionDenied(m) if m == "not allowed: plan"
        ));
        assert!(matches!(
            share_read_error(ApiError::PermissionDenied("not allowed: plan".into())),
            ApiError::PermissionDenied(m) if m == "not allowed: plan"
        ));
        assert!(matches!(
            share_read_error(ApiError::PermissionDenied(crate::error::GENERIC_FORBIDDEN.into())),
            ApiError::PermissionDenied(m) if m.contains("owner or its editors")
        ));
        assert!(matches!(
            share_read_error(ApiError::Internal),
            ApiError::Internal
        ));
    }

    #[test]
    fn test_share_write_error_user_prefix_with_non_uuid_passes_through() {
        for msg in [
            "Not found: user bob",
            "user bob@example.com",
            "Not found: user 1234-not-a-uuid.",
        ] {
            assert!(
                matches!(share_write_error(ApiError::NotFound(msg.into())), ApiError::NotFound(m) if m == msg),
                "{msg}"
            );
        }
    }

    #[tokio::test]
    async fn test_share_catalog_feature_not_entitled_passes_through() {
        let server = MockServer::start().await;
        mount_collab_post(
            &server,
            None,
            403,
            json!({"error":"Forbidden","code":"FEATURE_NOT_ENTITLED","feature":"catalog_share","plan_id":"free"}),
        )
        .await;
        let r = make_server(&server.uri())
            .share_catalog(Parameters(share_params(&[U1], None)))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        let t = tool_text(&r);
        assert!(t.contains("Catalog sharing"), "{t}");
        assert!(t.ends_with(crate::error::PLAN_CHANGE_HINT), "{t}");
        assert!(!t.contains("only the catalog's owner"), "{t}");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[test]
    fn test_share_errors_do_not_rewrite_feature_not_entitled() {
        let f = || ApiError::FeatureNotEntitled {
            feature: "catalog_share".into(),
            plan_id: Some("free".into()),
        };
        assert!(matches!(
            share_write_error(f()),
            ApiError::FeatureNotEntitled { .. }
        ));
        assert!(matches!(
            share_read_error(f()),
            ApiError::FeatureNotEntitled { .. }
        ));
    }

    #[tokio::test]
    async fn test_share_and_unshare_network_failure_is_error_result() {
        // Port 1 has no listener; a dropped MockServer's port could be reused by a parallel test.
        let s = make_server("http://127.0.0.1:1");
        let r = s
            .share_catalog(Parameters(share_params(&[U1], None)))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true), "{r:?}");
        let r = s
            .unshare_catalog(Parameters(UnshareCatalogParams {
                catalog_id: CAT.into(),
                user_ids: strs(&[U1]),
            }))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true), "{r:?}");
    }

    #[test]
    fn test_parse_user_ids_cap_applies_after_dedupe() {
        let mut raw: Vec<String> = (1..=100u128)
            .map(|i| Uuid::from_u128(i).to_string())
            .collect();
        raw.extend((1..=50u128).map(|i| Uuid::from_u128(i).to_string()));
        let ids = parse_user_ids(&raw).unwrap();
        assert_eq!(ids.len(), MAX_SHARE_USER_IDS);
        assert_eq!(ids[0], Uuid::from_u128(1));
    }

    #[test]
    fn test_share_write_error_unknown_user_with_trailing_punctuation() {
        for msg in [
            format!("Not found: user {U2}."),
            format!("Not found: user {U2}, aborting"),
            format!("user {U2}"),
        ] {
            match share_write_error(ApiError::NotFound(msg.clone())) {
                ApiError::NotFound(m) => assert!(
                    m.contains("no EngrAmo user has id") && m.contains(U2),
                    "{msg} -> {m}"
                ),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn test_parse_user_ids_rejects_absurd_length_early() {
        let raw = vec!["not-a-uuid".to_string(); MAX_SHARE_USER_IDS * 10 + 1];
        let err = parse_user_ids(&raw).unwrap_err();
        assert!(err.contains("Too many user ids"), "{err}");
    }

    #[test]
    fn test_sharing_params_deserialize_enums() {
        let p: ListCatalogsParams = serde_json::from_value(json!({"filter": "shared"})).unwrap();
        assert_eq!(p.filter, Some(CatalogFilter::Shared));
        assert!(serde_json::from_value::<ListCatalogsParams>(json!({"filter": "all"})).is_err());
        let p: ShareCatalogParams = serde_json::from_value(
            json!({"catalog_id": CAT, "user_ids": [U1], "access_level": "editor"}),
        )
        .unwrap();
        assert_eq!(p.access_level, Some(AccessLevel::Editor));
        assert!(
            serde_json::from_value::<ShareCatalogParams>(
                json!({"catalog_id": CAT, "user_ids": [U1], "access_level": "owner"})
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn test_list_catalogs_filter_param_reaches_api() {
        for (filter, expected) in [
            (Some(CatalogFilter::Own), Some("own")),
            (Some(CatalogFilter::Shared), Some("shared")),
            (None, None),
        ] {
            let server = MockServer::start().await;
            let mock = Mock::given(method("GET")).and(path("/catalogs"));
            let mock = match expected {
                Some(v) => mock.and(query_param("filter", v)),
                None => mock.and(query_param_is_missing("filter")),
            };
            mock.respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"data": [], "nextCursor": null})),
            )
            .expect(1)
            .mount(&server)
            .await;
            let r = make_server(&server.uri())
                .list_catalogs(Parameters(ListCatalogsParams {
                    limit: None,
                    cursor: None,
                    speaking_only: None,
                    filter,
                    ..Default::default()
                }))
                .await
                .unwrap();
            assert_eq!(r.is_error, Some(false), "{filter:?}");
        }
    }

    // ── Catalog filters: list_catalogs + get_catalog_filter_options ───────────

    fn opt_strs(v: &[&str]) -> Option<Vec<String>> {
        Some(v.iter().map(|x| (*x).to_owned()).collect())
    }

    fn empty_list_page() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"data": [], "nextCursor": null}))
    }

    /// A server that must never be called: proves a local error sent no request.
    async fn no_request_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(empty_list_page())
            .expect(0)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn test_list_catalogs_filters_reach_api_in_wire_form() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .and(query_param("learn", "lang:es,medicine"))
            .and(query_param("speak", "en,zxx"))
            .and(query_param("levels", "A1,A2,C1"))
            .and(query_param("tags", "anatomy,exam"))
            .and(query_param("q", "heart"))
            .and(query_param("filter", "public"))
            .respond_with(empty_list_page())
            .expect(1)
            .mount(&server)
            .await;
        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                query: Some("  heart ".into()),
                filter: Some(CatalogFilter::Public),
                front_languages: opt_strs(&["ES"]),
                back_languages: opt_strs(&["en-US", "ZXX"]),
                topics: opt_strs(&["Medicine"]),
                levels: Some(vec![LevelParam::Beginner, LevelParam::A1, LevelParam::C1]),
                tags: opt_strs(&[" Anatomy", "exam", "ANATOMY"]),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false), "{result:?}");
    }

    #[tokio::test]
    async fn test_list_catalogs_blank_query_and_empty_lists_are_not_sent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .and(query_param_is_missing("q"))
            .and(query_param_is_missing("learn"))
            .and(query_param_is_missing("speak"))
            .and(query_param_is_missing("levels"))
            .and(query_param_is_missing("tags"))
            .respond_with(empty_list_page())
            .expect(1)
            .mount(&server)
            .await;
        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                query: Some("   ".into()),
                front_languages: Some(vec![]),
                tags: Some(vec![]),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false), "{result:?}");
    }

    #[tokio::test]
    async fn test_list_catalogs_cursor_and_filters_together() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .and(query_param("cursor", "abc"))
            .and(query_param("limit", "5"))
            .and(query_param("learn", "lang:es"))
            .and(query_param("speakingOnly", "true"))
            .respond_with(empty_list_page())
            .expect(1)
            .mount(&server)
            .await;
        let result = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                limit: Some(5),
                cursor: Some("abc".into()),
                speaking_only: Some(true),
                front_languages: opt_strs(&["es"]),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false), "{result:?}");
    }

    fn list_params_cases() -> Vec<(&'static str, ListCatalogsParams, &'static str)> {
        vec![
            (
                "too many front+topics",
                ListCatalogsParams {
                    front_languages: opt_strs(&["en", "es", "fr", "de", "it"]),
                    topics: opt_strs(&["math", "law", "arts", "history"]),
                    ..Default::default()
                },
                "front_languages and topics together",
            ),
            (
                "too many back",
                ListCatalogsParams {
                    back_languages: opt_strs(&["en", "es", "fr"]),
                    ..Default::default()
                },
                "back_languages: at most",
            ),
            (
                "too many tags",
                ListCatalogsParams {
                    tags: opt_strs(&["a", "b", "c", "d", "e", "f", "g", "h", "i"]),
                    ..Default::default()
                },
                "tags: at most",
            ),
            (
                "long query",
                ListCatalogsParams {
                    query: Some("x".repeat(201)),
                    ..Default::default()
                },
                "query is too long",
            ),
            (
                "zxx in front",
                ListCatalogsParams {
                    front_languages: opt_strs(&["zxx"]),
                    ..Default::default()
                },
                "only valid in back_languages",
            ),
            (
                "lang: in topics",
                ListCatalogsParams {
                    topics: opt_strs(&["lang:es"]),
                    ..Default::default()
                },
                "is a language",
            ),
            (
                "comma in tag",
                ListCatalogsParams {
                    tags: opt_strs(&["a,b"]),
                    ..Default::default()
                },
                "must not contain a comma",
            ),
        ]
    }

    #[tokio::test]
    async fn test_list_catalogs_local_validation_errors_send_no_request() {
        let server = no_request_server().await;
        for (name, params, expect) in list_params_cases() {
            let result = make_server(&server.uri())
                .list_catalogs(Parameters(params))
                .await
                .unwrap();
            assert_eq!(result.is_error, Some(true), "{name}: {result:?}");
            assert!(
                tool_text(&result).contains(expect),
                "{name}: {}",
                tool_text(&result)
            );
        }
    }

    #[tokio::test]
    async fn test_get_catalog_filter_options_local_validation_errors_send_no_request() {
        let server = no_request_server().await;
        let result = make_server(&server.uri())
            .get_catalog_filter_options(Parameters(CatalogFilterOptionsParams {
                front_languages: opt_strs(&["zxx"]),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
    }

    #[tokio::test]
    async fn test_list_catalogs_api_400_with_filters_gets_hint_without_filters_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs"))
            .respond_with(ResponseTemplate::new(400).set_body_json(
                json!({"error": "unknown subject 'foo'", "code": "FILTER_UNKNOWN_SUBJECT"}),
            ))
            .mount(&server)
            .await;
        let with = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams {
                topics: opt_strs(&["foo"]),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(with.is_error, Some(true));
        let text = tool_text(&with);
        assert!(text.contains("unknown subject 'foo'"), "{text}");
        assert!(text.contains("get_catalog_filter_options"), "{text}");

        let without = make_server(&server.uri())
            .list_catalogs(Parameters(ListCatalogsParams::default()))
            .await
            .unwrap();
        assert_eq!(without.is_error, Some(true));
        assert!(
            !tool_text(&without).contains("get_catalog_filter_options"),
            "{}",
            tool_text(&without)
        );
    }

    #[test]
    fn test_list_catalogs_params_deserialize_new_fields() {
        let p: ListCatalogsParams = serde_json::from_value(json!({
            "filter": "public", "query": "q", "front_languages": ["es"],
            "back_languages": ["en"], "topics": ["math"],
            "levels": ["A1", "beginner"], "tags": ["t"]
        }))
        .unwrap();
        assert_eq!(p.filter, Some(CatalogFilter::Public));
        assert_eq!(p.levels.unwrap().len(), 2);
        assert!(serde_json::from_value::<ListCatalogsParams>(json!({"levels": ["Z9"]})).is_err());
        // null == omitted
        let p: ListCatalogsParams = serde_json::from_value(json!({"topics": null})).unwrap();
        assert!(p.topics.is_none());
    }

    const SCHEMA_JSON: &str = r#"{"schemaVersion":2,
        "languages":[{"key":"en","label":"English"},{"key":"es","label":"Spanish"}],
        "subjects":[{"key":"medicine","label":"Medicine"},{"key":"math","label":"Math"}],
        "levels":[{"key":"A1","label":"A1","bucket":"beginner"}],
        "buckets":[{"key":"beginner","label":"Beginner","levels":["A1","A2"]}],
        "limits":{"speak":2,"learn":3,"tagsPerCatalog":20,"searchSpeak":2,"searchLearn":8}}"#;

    async fn mount_schema(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/catalogs/facets/schema"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(SCHEMA_JSON, "application/json"))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn test_get_catalog_filter_options_returns_schema_and_facets() {
        let server = MockServer::start().await;
        mount_schema(&server).await;
        Mock::given(method("GET"))
            .and(path("/catalogs/facets"))
            .and(query_param("learn", "lang:es"))
            .and(query_param("filter", "public"))
            .and(query_param_is_missing("limit"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "total": 3,
                "speak": [{"key": "en", "count": 3}],
                "learn": [{"key": "lang:es", "count": 3}, {"key": "medicine", "count": 2}],
                "levels": [{"key": "A1", "count": 1}],
                "buckets": [{"key": "beginner", "count": 1}],
                "tags": [{"key": "anatomy", "count": 2}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let result = make_server(&server.uri())
            .get_catalog_filter_options(Parameters(CatalogFilterOptionsParams {
                filter: Some(CatalogFilter::Public),
                front_languages: opt_strs(&["ES"]),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(tool_text(&result)).unwrap();
        assert_eq!(v["topics"][0]["key"], "medicine");
        assert_eq!(v["topics"][0]["count"], 2);
        assert_eq!(v["topics"][1]["count"], 0);
        assert_eq!(v["languages"]["es"], "Spanish");
        assert_eq!(v["no_language_code"], "zxx");
        assert_eq!(v["level_groups"]["beginner"], json!(["A1", "A2"]));
        assert_eq!(v["facets"]["total"], 3);
        assert_eq!(v["facets"]["front_languages"][0]["key"], "es");
        assert_eq!(v["facets"]["top_tags"][0]["key"], "anatomy");
        assert_eq!(v["notes"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn test_get_catalog_filter_options_facets_failure_degrades_to_schema_with_note() {
        let server = MockServer::start().await;
        mount_schema(&server).await;
        Mock::given(method("GET"))
            .and(path("/catalogs/facets"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let result = make_server(&server.uri())
            .get_catalog_filter_options(Parameters(CatalogFilterOptionsParams::default()))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(tool_text(&result)).unwrap();
        assert!(v.get("facets").is_none());
        assert!(v["topics"][0].get("count").is_none());
        let notes = v["notes"].as_array().unwrap();
        assert_eq!(notes.len(), 4);
        assert!(notes[3].as_str().unwrap().contains("unavailable"));
    }

    #[tokio::test]
    async fn test_get_catalog_filter_options_facets_400_without_filters_degrades() {
        let server = MockServer::start().await;
        mount_schema(&server).await;
        Mock::given(method("GET"))
            .and(path("/catalogs/facets"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": "bad"})))
            .mount(&server)
            .await;
        // `filter` alone is not a filter param for the hint/err decision.
        let result = make_server(&server.uri())
            .get_catalog_filter_options(Parameters(CatalogFilterOptionsParams {
                filter: Some(CatalogFilter::Public),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false), "{result:?}");
        let v: serde_json::Value = serde_json::from_str(tool_text(&result)).unwrap();
        assert!(v.get("facets").is_none());
        assert_eq!(v["notes"].as_array().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn test_get_catalog_filter_options_facets_400_is_reported_with_hint() {
        let server = MockServer::start().await;
        mount_schema(&server).await;
        Mock::given(method("GET"))
            .and(path("/catalogs/facets"))
            .respond_with(ResponseTemplate::new(400).set_body_json(
                json!({"error": "unknown subject 'foo'", "code": "FILTER_UNKNOWN_SUBJECT"}),
            ))
            .mount(&server)
            .await;
        let result = make_server(&server.uri())
            .get_catalog_filter_options(Parameters(CatalogFilterOptionsParams {
                topics: opt_strs(&["foo"]),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(tool_text(&result).contains("unknown subject"));
    }

    #[tokio::test]
    async fn test_get_catalog_filter_options_schema_503_is_tool_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalogs/facets/schema"))
            .respond_with(ResponseTemplate::new(503).set_body_json(
                json!({"error": "topics unavailable", "code": "SUBJECTS_UNAVAILABLE"}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/catalogs/facets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"total": 0})))
            .mount(&server)
            .await;
        let result = make_server(&server.uri())
            .get_catalog_filter_options(Parameters(CatalogFilterOptionsParams::default()))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
    }

    #[test]
    fn test_build_catalog_query_char_cap_blank_tags_and_query_only_has_filters() {
        fn inputs<'a>(query: Option<&'a str>, tags: Option<&'a [String]>) -> FilterInputs<'a> {
            FilterInputs {
                query,
                filter: None,
                front_languages: None,
                back_languages: None,
                topics: None,
                levels: None,
                tags,
            }
        }
        let q200 = "я".repeat(200);
        let (_, has) = build_catalog_query(inputs(Some(&q200), None))
            .expect("200 chars accepted even at 400 bytes");
        assert!(has, "query alone counts as a filter");
        let q201 = "я".repeat(201);
        assert!(build_catalog_query(inputs(Some(&q201), None)).is_err());
        // 200 chars of 3-byte text exceed the client's 512-byte bound: refused locally.
        let wide = "界".repeat(200);
        let err = build_catalog_query(inputs(Some(&wide), None)).unwrap_err();
        assert!(err.contains("query is too long"), "{err}");
        let tags = vec!["  ".to_string(), "A".to_string()];
        let (q, _) = build_catalog_query(inputs(None, Some(tags.as_slice()))).unwrap();
        assert_eq!(q.tags, vec!["a"]);
    }
}
