//! Pure helpers for the catalog filter params (front/back language, topics, levels).
//!
//! The API stores front languages and topics together in one `learn` list (`lang:<iso>` entries
//! plus topic keys); the MCP surface keeps them apart. No I/O and no `client` import — everything
//! here is unit-tested. The API re-validates every value (membership in the language / topic
//! lists stays with it); these checks only produce errors that name the MCP param.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::dto::{FacetCountDto, FacetCountsDto, FilterSchemaDto};
use crate::error::{ApiError, preview};

pub const NO_LANGUAGE: &str = "zxx";
pub const LANG_PREFIX: &str = "lang:";
// Mirror the API caps so errors can name MCP params; the API re-validates.
pub const MAX_WRITE_BACK: usize = 2;
pub const MAX_WRITE_LEARN: usize = 3;
pub const MAX_SEARCH_BACK: usize = 2;
pub const MAX_SEARCH_LEARN: usize = 8;
pub const MAX_SEARCH_TAGS: usize = 8;
pub const MAX_WRITE_TAGS: usize = 20;

/// A CEFR level, or a bucket that expands to two of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub enum LevelParam {
    A1,
    A2,
    B1,
    B2,
    C1,
    C2,
    /// A1, A2
    #[serde(rename = "beginner")]
    Beginner,
    /// B1, B2
    #[serde(rename = "intermediate")]
    Intermediate,
    /// C1, C2
    #[serde(rename = "advanced")]
    Advanced,
}

impl LevelParam {
    fn cefr(self) -> &'static [&'static str] {
        match self {
            Self::A1 => &["A1"],
            Self::A2 => &["A2"],
            Self::B1 => &["B1"],
            Self::B2 => &["B2"],
            Self::C1 => &["C1"],
            Self::C2 => &["C2"],
            Self::Beginner => &["A1", "A2"],
            Self::Intermediate => &["B1", "B2"],
            Self::Advanced => &["C1", "C2"],
        }
    }
}

/// Expands buckets to CEFR strings; deduplicated and sorted.
pub fn expand_levels(levels: &[LevelParam]) -> Vec<String> {
    levels
        .iter()
        .flat_map(|l| l.cefr().iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// "es", "ES", "es-ES", "es_419", "lang:es" -> "es". Rejects anything that is not 2–3 ASCII
/// letters after that. `allow_zxx` is true only for `back_languages`.
pub fn normalize_lang(raw: &str, field: &'static str, allow_zxx: bool) -> Result<String, String> {
    let trimmed = raw.trim();
    let body = trimmed
        .get(..LANG_PREFIX.len())
        .filter(|p| p.eq_ignore_ascii_case(LANG_PREFIX))
        .map_or(trimmed, |_| &trimmed[LANG_PREFIX.len()..]);
    let code = body.split(['-', '_']).next().unwrap_or_default();
    let valid = (2..=3).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphabetic());
    if !valid {
        return Err(format!(
            "{field}: '{}' is not a language code (use a 2-letter ISO 639-1 code such as 'es')",
            preview(raw)
        ));
    }
    let code = code.to_ascii_lowercase();
    if code == NO_LANGUAGE && !allow_zxx {
        return Err(format!(
            "{field}: '{NO_LANGUAGE}' (no language) is only valid in back_languages"
        ));
    }
    Ok(code)
}

/// Normalizes each code, deduplicates (first occurrence wins), then enforces `max`.
pub fn normalize_langs(
    raw: &[String],
    field: &'static str,
    allow_zxx: bool,
    max: usize,
) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for r in raw {
        let code = normalize_lang(r, field, allow_zxx)?;
        if !out.contains(&code) {
            out.push(code);
        }
    }
    if out.len() > max {
        return Err(format!("{field}: at most {max} allowed, got {}", out.len()));
    }
    Ok(out)
}

/// Topic keys: trimmed + lowercased; rejects empty, a `lang:` prefix, the reserved `language`
/// key, and anything not matching `^[a-z][a-z_]{0,31}$` (the API's key shape).
pub fn normalize_topics(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for r in raw {
        let key = r.trim().to_ascii_lowercase();
        if key.is_empty() {
            return Err("topics: empty topic key".to_owned());
        }
        if key.starts_with(LANG_PREFIX) {
            return Err(format!(
                "topics: '{}' is a language; use front_languages",
                preview(r)
            ));
        }
        if key == "language" {
            return Err("topics: 'language' is reserved and not a topic".to_owned());
        }
        let mut bytes = key.bytes();
        let shape_ok = key.len() <= 32
            && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes.all(|b| b.is_ascii_lowercase() || b == b'_');
        if !shape_ok {
            return Err(format!(
                "topics: '{}' is not a topic key (lowercase letters and '_', see get_catalog_filter_options)",
                preview(r)
            ));
        }
        if !out.contains(&key) {
            out.push(key);
        }
    }
    Ok(out)
}

/// `learn` = `lang:<front…>` ++ topics; errors when front + topics exceeds `max`.
pub fn join_learn(front: &[String], topics: &[String], max: usize) -> Result<Vec<String>, String> {
    let total = front.len() + topics.len();
    if total > max {
        return Err(format!(
            "front_languages and topics together allow at most {max}, got {total}"
        ));
    }
    Ok(front
        .iter()
        .map(|l| format!("{LANG_PREFIX}{l}"))
        .chain(topics.iter().cloned())
        .collect())
}

/// Inverse of [`join_learn`]; unknown non-`lang:` keys are kept as topics.
pub fn split_learn(learn: &[String]) -> (Vec<String>, Vec<String>) {
    let mut front = Vec::new();
    let mut topics = Vec::new();
    for entry in learn {
        match entry.strip_prefix(LANG_PREFIX) {
            Some(code) => front.push(code.to_owned()),
            None => topics.push(entry.clone()),
        }
    }
    (front, topics)
}

/// Appends a pointer to the discovery tool to an API 400; other variants pass through.
pub fn with_filter_hint(e: ApiError) -> ApiError {
    match e {
        ApiError::BadRequest(m) => ApiError::BadRequest(format!(
            "{m} — call get_catalog_filter_options for valid topics, levels and language codes."
        )),
        other => other,
    }
}

/// A topic the API currently accepts, with its live catalog count once facets are merged in.
#[derive(Debug, Serialize)]
pub struct TopicOption {
    pub key: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct LimitSet {
    pub back_languages: usize,
    pub front_languages_plus_topics: usize,
    pub tags: usize,
}

#[derive(Debug, Serialize)]
pub struct Limits {
    pub per_catalog: LimitSet,
    pub search: LimitSet,
}

/// Counts from `GET /catalogs/facets` under the filters of the call, in MCP naming. Absent from
/// the view unless [`FilterOptionsView::with_facets`] was called.
#[derive(Debug, Serialize)]
pub struct FacetsView {
    /// Catalogs matching the filters.
    pub total: i64,
    pub front_languages: Vec<FacetCountDto>,
    pub back_languages: Vec<FacetCountDto>,
    pub topics: Vec<FacetCountDto>,
    pub levels: Vec<FacetCountDto>,
    pub level_groups: Vec<FacetCountDto>,
    /// Most used tags among the matches that are not already filtered on.
    pub top_tags: Vec<FacetCountDto>,
}

/// `get_catalog_filter_options` / `engramo://catalog-filters` output: the allowed values (from
/// the schema) and, optionally, live counts (from facets).
#[derive(Debug, Serialize)]
pub struct FilterOptionsView {
    pub topics: Vec<TopicOption>,
    pub levels: Vec<String>,
    pub level_groups: BTreeMap<String, Vec<String>>,
    /// ISO 639-1 code -> English name.
    pub languages: BTreeMap<String, String>,
    pub no_language_code: &'static str,
    pub limits: Limits,
    pub notes: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<FacetsView>,
}

/// Builds the view from the schema; the API's `limits` win, the MCP constants fill what an
/// older API omits (and the tag search cap, which it never publishes).
pub fn filter_options_view(schema: FilterSchemaDto) -> FilterOptionsView {
    let limits = &schema.limits;
    FilterOptionsView {
        topics: schema
            .subjects
            .into_iter()
            .map(|s| TopicOption {
                key: s.key,
                label: s.label,
                count: None,
            })
            .collect(),
        levels: schema.levels.into_iter().map(|l| l.key).collect(),
        level_groups: schema
            .buckets
            .into_iter()
            .map(|b| (b.key, b.levels))
            .collect(),
        languages: schema
            .languages
            .into_iter()
            .map(|l| (l.key, l.label))
            .collect(),
        no_language_code: NO_LANGUAGE,
        limits: Limits {
            per_catalog: LimitSet {
                back_languages: limits.speak.unwrap_or(MAX_WRITE_BACK),
                front_languages_plus_topics: limits.learn.unwrap_or(MAX_WRITE_LEARN),
                tags: limits.tags_per_catalog.unwrap_or(MAX_WRITE_TAGS),
            },
            search: LimitSet {
                back_languages: limits.search_speak.unwrap_or(MAX_SEARCH_BACK),
                front_languages_plus_topics: limits.search_learn.unwrap_or(MAX_SEARCH_LEARN),
                tags: MAX_SEARCH_TAGS,
            },
        },
        notes: vec![
            "front_languages = language of the card fronts (what is being learned); back_languages = language of the backs (translations/definitions); 'zxx' = no language, back only.",
            "In searches, values within one field are OR-ed, tags are AND-ed, and different fields narrow each other.",
            "Matching is strict: a catalog with no value set for a filtered field never matches.",
        ],
        facets: None,
    }
}

impl FilterOptionsView {
    /// Merges facet counts: topics gain a `count` (topics with no matches get 0) and a
    /// `facets` section is added. `learn` keys split into front languages (`lang:`) and topics.
    pub fn with_facets(mut self, counts: FacetCountsDto) -> Self {
        let mut front_languages = Vec::new();
        let mut topic_counts = Vec::new();
        for c in counts.learn {
            match c.key.strip_prefix(LANG_PREFIX) {
                Some(code) => front_languages.push(FacetCountDto {
                    key: code.to_owned(),
                    count: c.count,
                }),
                None => topic_counts.push(c),
            }
        }
        for t in &mut self.topics {
            t.count = Some(
                topic_counts
                    .iter()
                    .find(|c| c.key == t.key)
                    .map_or(0, |c| c.count),
            );
        }
        self.facets = Some(FacetsView {
            total: counts.total,
            front_languages,
            back_languages: counts.speak,
            topics: topic_counts,
            levels: counts.levels,
            level_groups: counts.buckets,
            top_tags: counts.tags,
        });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn test_normalize_lang_accepts_variants() {
        for raw in ["es", "ES", "es-ES", "es_419", "lang:es", " es "] {
            assert_eq!(normalize_lang(raw, "front_languages", false).unwrap(), "es");
        }
    }

    #[test]
    fn test_normalize_lang_zxx_only_when_allowed() {
        assert!(normalize_lang("zxx", "front_languages", false).is_err());
        assert_eq!(
            normalize_lang("zxx", "back_languages", true).unwrap(),
            "zxx"
        );
    }

    #[test]
    fn test_normalize_lang_rejects_bad_shapes() {
        for raw in ["", "e", "espa", "e1", "ñé", "lang:", "-es"] {
            let err = normalize_lang(raw, "front_languages", false).unwrap_err();
            assert!(err.contains("front_languages"), "{err}");
        }
    }

    #[test]
    fn test_normalize_lang_error_is_capped() {
        let err = normalize_lang(&"x".repeat(10_000), "front_languages", false).unwrap_err();
        assert!(err.len() < 300, "{}", err.len());
    }

    #[test]
    fn test_normalize_langs_dedups_before_cap() {
        assert_eq!(
            normalize_langs(&s(&["es", "ES"]), "front_languages", false, 1).unwrap(),
            s(&["es"])
        );
    }

    #[test]
    fn test_normalize_langs_over_cap_names_field() {
        let err = normalize_langs(&s(&["es", "en", "fr"]), "back_languages", true, 2).unwrap_err();
        assert!(err.contains("back_languages"), "{err}");
    }

    #[test]
    fn test_normalize_topics_lowercases_and_dedups() {
        assert_eq!(
            normalize_topics(&s(&[" Medicine ", "software_dev", "medicine"])).unwrap(),
            s(&["medicine", "software_dev"])
        );
    }

    #[test]
    fn test_normalize_topics_rejects_bad_input() {
        for raw in [
            "lang:es", "LANG:es", "language", "Language", "", "  ", "a b", "1a", "a-b",
        ] {
            assert!(normalize_topics(&s(&[raw])).is_err(), "{raw}");
        }
        assert!(normalize_topics(&s(&[&"a".repeat(33)])).is_err());
        assert!(normalize_topics(&s(&[&"a".repeat(32)])).is_ok());
    }

    #[test]
    fn test_join_learn_split_learn_round_trip() {
        let learn = join_learn(&s(&["es", "en"]), &s(&["medicine"]), 3).unwrap();
        assert_eq!(learn, s(&["lang:es", "lang:en", "medicine"]));
        assert_eq!(split_learn(&learn), (s(&["es", "en"]), s(&["medicine"])));
    }

    #[test]
    fn test_join_learn_cap_names_both_params() {
        let err = join_learn(&s(&["es", "en"]), &s(&["a", "b"]), 3).unwrap_err();
        assert!(
            err.contains("front_languages") && err.contains("topics"),
            "{err}"
        );
    }

    #[test]
    fn test_split_learn_keeps_unknown_keys_as_topics() {
        let (front, topics) = split_learn(&s(&["lang:es", "mystery", "medicine"]));
        assert_eq!(front, s(&["es"]));
        assert_eq!(topics, s(&["mystery", "medicine"]));
        assert_eq!(split_learn(&[]), (vec![], vec![]));
    }

    #[test]
    fn test_expand_levels_buckets_dedup_sort() {
        use LevelParam::*;
        assert_eq!(
            expand_levels(&[Advanced, A2, Beginner, B1, A1]),
            s(&["A1", "A2", "B1", "C1", "C2"])
        );
        assert_eq!(expand_levels(&[Intermediate]), s(&["B1", "B2"]));
        assert!(expand_levels(&[]).is_empty());
    }

    #[test]
    fn test_level_param_serde() {
        let ok: LevelParam = serde_json::from_str("\"A1\"").unwrap();
        assert_eq!(ok, LevelParam::A1);
        let b: LevelParam = serde_json::from_str("\"beginner\"").unwrap();
        assert_eq!(b, LevelParam::Beginner);
        assert!(serde_json::from_str::<LevelParam>("\"a1\"").is_err());
        assert!(serde_json::from_str::<LevelParam>("\"expert\"").is_err());
    }

    #[test]
    fn test_with_filter_hint_only_touches_bad_request() {
        match with_filter_hint(ApiError::BadRequest("bad topic".into())) {
            ApiError::BadRequest(m) => {
                assert!(m.starts_with("bad topic"));
                assert_eq!(m.matches("get_catalog_filter_options").count(), 1);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            with_filter_hint(ApiError::Unauthorized),
            ApiError::Unauthorized
        ));
    }

    fn schema(extra_limits: serde_json::Value) -> FilterSchemaDto {
        let mut limits = serde_json::json!({"speak":2,"learn":3,"tagsPerCatalog":20});
        limits
            .as_object_mut()
            .unwrap()
            .extend(extra_limits.as_object().unwrap().clone());
        serde_json::from_value(serde_json::json!({
            "schemaVersion": 2,
            "languages": [{"key":"en","label":"English"},{"key":"es","label":"Spanish"}],
            "subjects": [{"key":"medicine","label":"Medicine"},{"key":"math","label":"Math"}],
            "levels": [{"key":"A1","label":"A1","bucket":"beginner"},{"key":"A2","label":"A2","bucket":"beginner"}],
            "buckets": [{"key":"beginner","label":"Beginner","levels":["A1","A2"]}],
            "limits": limits
        }))
        .unwrap()
    }

    #[test]
    fn test_filter_options_view_maps_schema() {
        let v = serde_json::to_value(filter_options_view(schema(
            serde_json::json!({"searchSpeak":2,"searchLearn":8}),
        )))
        .unwrap();
        assert_eq!(
            v["topics"][0],
            serde_json::json!({"key":"medicine","label":"Medicine"})
        );
        assert_eq!(v["levels"], serde_json::json!(["A1", "A2"]));
        assert_eq!(
            v["level_groups"]["beginner"],
            serde_json::json!(["A1", "A2"])
        );
        assert_eq!(v["languages"]["es"], "Spanish");
        assert_eq!(v["no_language_code"], "zxx");
        assert_eq!(v["limits"]["per_catalog"]["front_languages_plus_topics"], 3);
        assert_eq!(v["limits"]["per_catalog"]["tags"], 20);
        assert_eq!(v["limits"]["search"]["front_languages_plus_topics"], 8);
        assert_eq!(v["limits"]["search"]["tags"], 8);
        assert_eq!(v["notes"].as_array().unwrap().len(), 3);
        assert!(v.get("facets").is_none(), "{v}");
    }

    #[test]
    fn test_filter_options_view_search_limits_fall_back_to_constants() {
        let v = serde_json::to_value(filter_options_view(schema(serde_json::json!({})))).unwrap();
        assert_eq!(v["limits"]["search"]["back_languages"], 2);
        assert_eq!(v["limits"]["search"]["front_languages_plus_topics"], 8);
        assert_eq!(v["limits"]["search"]["tags"], 8);
    }

    #[test]
    fn test_filter_options_view_with_facets_merges_counts() {
        let counts: FacetCountsDto = serde_json::from_value(serde_json::json!({
            "total": 12,
            "speak": [{"key":"en","count":9}],
            "learn": [{"key":"lang:es","count":7},{"key":"medicine","count":4}],
            "levels": [{"key":"A1","count":3}],
            "buckets": [{"key":"beginner","count":3}],
            "tags": [{"key":"anatomy","count":2}]
        }))
        .unwrap();
        let v = serde_json::to_value(
            filter_options_view(schema(serde_json::json!({}))).with_facets(counts),
        )
        .unwrap();
        assert_eq!(v["topics"][0]["count"], 4);
        assert_eq!(v["topics"][1]["count"], 0, "no matches -> 0");
        assert_eq!(v["facets"]["total"], 12);
        assert_eq!(
            v["facets"]["front_languages"],
            serde_json::json!([{"key":"es","count":7}])
        );
        assert_eq!(
            v["facets"]["topics"],
            serde_json::json!([{"key":"medicine","count":4}])
        );
        assert_eq!(v["facets"]["back_languages"][0]["key"], "en");
        assert_eq!(v["facets"]["level_groups"][0]["key"], "beginner");
        assert_eq!(v["facets"]["top_tags"][0]["key"], "anatomy");
    }

    #[test]
    fn test_filter_options_view_per_catalog_limits_fall_back_to_constants() {
        let schema: FilterSchemaDto = serde_json::from_value(serde_json::json!({})).unwrap();
        let v = serde_json::to_value(filter_options_view(schema)).unwrap();
        assert_eq!(v["limits"]["per_catalog"]["back_languages"], MAX_WRITE_BACK);
        assert_eq!(
            v["limits"]["per_catalog"]["front_languages_plus_topics"],
            MAX_WRITE_LEARN
        );
        assert_eq!(v["limits"]["per_catalog"]["tags"], MAX_WRITE_TAGS);
        assert!(v["topics"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_normalize_lang_prefix_is_case_insensitive() {
        for raw in ["LANG:es", "Lang:ES", "lAnG:es-MX"] {
            assert_eq!(normalize_lang(raw, "front_languages", false).unwrap(), "es");
        }
    }
}
