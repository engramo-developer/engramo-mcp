use rmcp::schemars;
use serde::Deserialize;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DueCardsParams {
    #[schemars(
        description = "Maximum number of cards to return (default 20; values are clamped to 1..=50)"
    )]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination cursor from a previous response")]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddCardToLearningParams {
    #[schemars(description = "UUID of the card to add to learning")]
    pub card_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddCatalogToLearningParams {
    #[schemars(description = "UUID of the catalog — adds all its cards to learning")]
    pub catalog_id: String,
}
