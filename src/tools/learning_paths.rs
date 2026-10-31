use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::dto::LearningPathDto;

/// Upper bound on `catalog_ids` accepted by `create_learning_path` in a single call. Keeps one
/// MCP call from turning into an unbounded run of sequential upstream
/// `POST /learning-paths/{id}/catalogs/{cid}` requests (mirrors `generate::MAX_BATCH_CARDS`).
pub const MAX_PATH_CATALOG_IDS: usize = 50;

/// One `catalog_ids` entry that failed to add in `create_learning_path`.
#[derive(Debug, Serialize)]
pub struct CatalogAddFailure {
    pub catalog_id: String,
    pub error: String,
}

/// `create_learning_path`'s response when `catalog_ids` is set: the created path's own fields,
/// flattened, plus which catalogs were added/failed.
#[derive(Debug, Serialize)]
pub struct CreatedLearningPathWithCatalogs<'a> {
    #[serde(flatten)]
    pub path: &'a LearningPathDto,
    pub catalogs_added: Vec<String>,
    pub catalogs_failed: Vec<CatalogAddFailure>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListLearningPathsParams {
    #[schemars(
        description = "Maximum number of paths to return (default 20; values are clamped to 1..=50)"
    )]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination cursor from a previous response")]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetLearningPathParams {
    #[schemars(description = "UUID of the learning path")]
    pub path_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateLearningPathParams {
    #[schemars(description = "Name of the learning path")]
    pub name: String,
    #[schemars(description = "Optional description")]
    pub description: Option<String>,
    #[schemars(
        length(max = MAX_PATH_CATALOG_IDS),
        description = "Optional UUIDs of catalogs to add to the new path right after it's created. Duplicates are ignored; the per-call maximum is given by maxItems. Each catalog is added in its own request after the path exists (non-atomic) — the response includes `catalogs_added`/`catalogs_failed` when this is set, so failed ids can be retried with add_catalog_to_learning_path."
    )]
    pub catalog_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ActivateLearningPathParams {
    #[schemars(description = "UUID of the learning path to activate")]
    pub path_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddCatalogToLearningPathParams {
    #[schemars(description = "UUID of the learning path")]
    pub path_id: String,
    #[schemars(description = "UUID of the catalog to add to the path")]
    pub catalog_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RemoveCatalogFromLearningPathParams {
    #[schemars(description = "UUID of the learning path")]
    pub path_id: String,
    #[schemars(description = "UUID of the catalog to remove from the path")]
    pub catalog_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateLearningPathParams {
    #[schemars(description = "UUID of the learning path to update")]
    pub path_id: String,
    #[schemars(description = "New name")]
    pub name: Option<String>,
    #[schemars(description = "New description")]
    pub description: Option<String>,
    #[schemars(description = "New tags")]
    pub tags: Option<Vec<String>>,
    #[schemars(
        description = "New visibility: 'public' or 'private'. 'unlisted' is catalog-only and is rejected for learning paths."
    )]
    pub visibility: Option<String>,
    #[schemars(
        description = "Current version of the learning path (required for optimistic locking)"
    )]
    pub version: i32,
}
