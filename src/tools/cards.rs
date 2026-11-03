use rmcp::schemars;
use serde::Deserialize;
use uuid::Uuid;

use crate::dto::CardContentPatch;
use crate::tools::catalogs::parse_uuid;

/// `update_card`'s `catalog_ids` is REQUIRED and must be non-empty: the underlying API accepts
/// `catalogIds: []` without error and silently moves the card into the user's default catalog
/// ("My Catalog") instead of removing it from every catalog (see engramo-mcp#64). Reject an
/// empty list here, before any HTTP call, rather than forwarding it.
pub(crate) const EMPTY_CATALOG_IDS_ERROR: &str = "catalog_ids must contain at least one catalog \
    UUID — a card must belong to at least one catalog. Build the list from the `catalogs[].id` \
    values in `get_card`/`list_cards`, plus/minus any intended changes; an empty list would \
    silently move the card to your default catalog instead of removing it from every catalog.";

/// Validates `update_card`'s `catalog_ids`: non-empty and every entry a valid UUID.
pub(crate) fn parse_catalog_ids(raw: &[String]) -> Result<Vec<Uuid>, String> {
    if raw.is_empty() {
        return Err(EMPTY_CATALOG_IDS_ERROR.to_string());
    }
    raw.iter().map(|s| parse_uuid(s)).collect()
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListCardsParams {
    #[schemars(description = "UUID of the catalog to list cards from")]
    pub catalog_id: String,
    #[schemars(
        description = "Maximum number of cards to return (default 20; values are clamped to 1..=50)"
    )]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination cursor from a previous response")]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetCardParams {
    #[schemars(description = "UUID of the card")]
    pub card_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateCardParams {
    #[schemars(description = "UUID of the card to update")]
    pub card_id: String,
    #[schemars(
        description = "Face patch (optional). Omitted keys keep the stored value; send null to \
        clear audioId, visualId+visualType (together), dictionary, richText, or style. \
        Changing `text` without `richText` drops the stored formatting. richText styling goes \
        under a nested `style` object, e.g. {\"text\":\"gracias.\",\"style\":{\"bold\":true,\"fontColor\":\"#27AE60\"}}."
    )]
    pub face: Option<CardContentPatch>,
    #[schemars(description = "Back patch (optional). Same merge rules as `face`.")]
    pub back: Option<CardContentPatch>,
    #[schemars(
        description = "Catalog UUIDs this card should belong to — REPLACES the memberships you \
        can see. Start from `catalogs[].id` in `get_card`/`list_cards` (which lists only \
        catalogs visible to you), then add/remove as intended. Must contain at least one UUID — \
        a card must belong to at least one catalog, so an empty list is rejected (it is NOT a \
        way to remove the card from all catalogs; the API would otherwise silently move it to \
        your default catalog). If the card JSON has no `catalogs` key, its memberships are \
        unknown — call `get_card` first."
    )]
    pub catalog_ids: Vec<String>,
    #[schemars(description = "Card order number within the catalog")]
    pub order_number: i64,
    #[schemars(description = "Current version for optimistic locking — fetch the card first")]
    pub version: i32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteCardParams {
    #[schemars(description = "UUID of the catalog the card belongs to")]
    pub catalog_id: String,
    #[schemars(description = "UUID of the card to delete")]
    pub card_id: String,
}
