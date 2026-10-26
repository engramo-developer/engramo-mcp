//! The caller's plan, limits and usage as one normalized view — shared by the
//! `get_subscription_usage` tool and the `engramo://subscription` resource so the two can't
//! diverge — plus the resource/feature labels and UTC reset rules that `error.rs` reuses to
//! explain a quota or plan-feature denial.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::client::EngramoClient;
use crate::dto::{UsageEntryDto, UserSubscriptionDto};
use crate::error::{ApiError, PLAN_CHANGE_HINT};

/// Human name for a quota resource type; `None` for a type this build doesn't know yet.
pub fn resource_label(resource_type: &str) -> Option<&'static str> {
    Some(match resource_type {
        "catalogs_total" => "Catalogs",
        "cards_total" => "Cards (total)",
        "cards_per_catalog" => "Cards per catalog",
        "ai_agent_messages" => "AI chat messages",
        "tts_generations" => "Text-to-speech generations",
        "premium_tts_generations" => "Premium text-to-speech generations",
        "translation_chunks" => "Translations",
        _ => return None,
    })
}

/// Human name for a plan feature (`PlanFeature` in the API); `None` when unknown.
pub fn feature_label(feature: &str) -> Option<&'static str> {
    Some(match feature {
        "catalog_share" => "Catalog sharing",
        "tts_generate" => "Text-to-speech",
        "dictionary_generate" => "Dictionary generation",
        "api_token_create" => "API tokens",
        "ai_chat" => "AI chat",
        "translate_cards" => "Card translation",
        "premium_tts" => "Premium text-to-speech",
        "youtube_workflow" => "YouTube workflows",
        _ => return None,
    })
}

/// When a quota window that is current at `now` resets, using the API's UTC calendar
/// boundaries: `monthly` → the 1st of next month 00:00, `daily` → next 00:00, `hourly` → the
/// next full hour. `absolute` (lifetime) and unknown windows never reset → `None`.
pub fn next_reset(window: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let midnight = |d: NaiveDate| Some(Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?));
    match window {
        "monthly" => {
            let (y, m) = if now.month() == 12 {
                (now.year() + 1, 1)
            } else {
                (now.year(), now.month() + 1)
            };
            midnight(NaiveDate::from_ymd_opt(y, m, 1)?)
        }
        "daily" => midnight(now.date_naive().succ_opt()?),
        "hourly" => {
            let hour = now.date_naive().and_hms_opt(now.hour(), 0, 0)?;
            Some(Utc.from_utc_datetime(&hour) + Duration::hours(1))
        }
        _ => None,
    }
}

/// Formats a reset instant for a message, e.g. `2026-11-01 00:00 UTC`.
pub fn format_reset(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

/// The full account view returned by `get_subscription_usage`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionUsageView {
    /// The caller's own EngrAmo user id (from `GET /me`); quote it when contacting support.
    pub user_id: Uuid,
    pub plan: PlanView,
    pub usage: Vec<UsageView>,
    pub how_to_change: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
    pub id: String,
    pub name: String,
    pub starts_at: String,
    pub expires_at: Option<String>,
    pub pending_plan_id: Option<String>,
    pub lapsed_plan_id: Option<String>,
    pub deactivated_plan_id: Option<String>,
    pub features: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageView {
    pub resource: String,
    pub label: String,
    /// `absolute` (lifetime, never resets) or `monthly`; null on an older API build.
    pub window: Option<String>,
    #[serde(flatten)]
    pub counts: Counts,
    pub resets_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daily: Option<SubWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hourly: Option<SubWindow>,
    /// `cards_per_catalog` only: the fullest catalog, whose count `used` reports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<Uuid>,
}

/// A daily or hourly sub-limit on top of the resource's main window.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubWindow {
    #[serde(flatten)]
    pub counts: Counts,
    pub resets_at: Option<String>,
}

/// `used` against `limit`, normalized: `-1` → `unlimited` with null `limit`/`remaining`;
/// `0` → `included: false` (not part of the plan); otherwise `remaining = max(0, limit - used)`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub used: i64,
    pub limit: Option<i64>,
    pub remaining: Option<i64>,
    pub unlimited: bool,
    pub included: bool,
}

impl Counts {
    pub fn new(used: i64, limit: i64) -> Self {
        let unlimited = limit < 0;
        Self {
            used,
            limit: (!unlimited).then_some(limit),
            remaining: (!unlimited).then(|| limit.saturating_sub(used).max(0)),
            unlimited,
            included: limit != 0,
        }
    }
}

impl From<UsageEntryDto> for UsageView {
    fn from(e: UsageEntryDto) -> Self {
        let sub = |used: Option<i64>, limit: Option<i64>, resets_at: Option<String>| {
            limit.map(|limit| SubWindow {
                counts: Counts::new(used.unwrap_or(0), limit),
                resets_at,
            })
        };
        let label = match (e.resource_type.as_str(), resource_label(&e.resource_type)) {
            ("cards_per_catalog", _) => "Cards in your fullest catalog".to_string(),
            (_, Some(label)) => label.to_string(),
            (raw, None) => raw.to_string(),
        };
        Self {
            label,
            window: e.window,
            counts: Counts::new(e.used, e.limit),
            resets_at: e.resets_at,
            daily: sub(e.used_daily, e.limit_daily, e.resets_at_daily),
            hourly: sub(e.used_hourly, e.limit_hourly, e.resets_at_hourly),
            catalog_id: e.scope_id,
            resource: e.resource_type,
        }
    }
}

impl From<UserSubscriptionDto> for PlanView {
    fn from(s: UserSubscriptionDto) -> Self {
        Self {
            id: s.plan_id,
            name: s.plan_name,
            starts_at: s.starts_at,
            expires_at: s.expires_at,
            pending_plan_id: s.pending_plan_id,
            lapsed_plan_id: s.lapsed_plan_id,
            deactivated_plan_id: s.deactivated_plan_id,
            features: s.features,
        }
    }
}

/// Fetches `/me`, `/me/subscription` and `/me/usage` concurrently and normalizes them. Any
/// failure fails the whole view — a partial answer about limits is worse than a clear error.
pub async fn subscription_usage(client: &EngramoClient) -> Result<SubscriptionUsageView, ApiError> {
    let (me, subscription, usage) = tokio::try_join!(
        client.get_me(),
        client.get_subscription(),
        client.get_usage()
    )?;
    Ok(SubscriptionUsageView {
        user_id: me.user_id,
        plan: subscription.into(),
        usage: usage.usage.into_iter().map(Into::into).collect(),
        how_to_change: PLAN_CHANGE_HINT,
    })
}

/// Real `GET /me`, `/me/subscription` and `/me/usage` bodies, copied from the API's DTOs
/// (`db/src/models/quota.rs`), so a shape drift fails loudly.
#[cfg(test)]
pub(crate) mod fixtures {
    use serde_json::{Value, json};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    pub const USER_ID: &str = "f4c1d1e2-0000-4000-8000-000000000001";
    pub const FULLEST_CATALOG: &str = "0b5c0000-0000-4000-8000-0000000000aa";

    pub fn subscription() -> Value {
        json!({
            "planId": "free",
            "planName": "Free",
            "limits": {"catalogs_total": 5, "cards_total": 500},
            "startsAt": "2026-01-01T00:00:00Z",
            "expiresAt": null,
            "features": ["api_token_create"]
        })
    }

    /// With the window / reset / scope fields (engram-api #26).
    pub fn usage() -> Value {
        json!({
            "plan_id": "free",
            "usage": [
                {"resource_type": "catalogs_total", "used": 3, "limit": 5, "window": "absolute"},
                {"resource_type": "cards_per_catalog", "used": 80, "limit": 100,
                 "window": "absolute", "scope_id": FULLEST_CATALOG},
                {"resource_type": "ai_agent_messages", "used": 0, "limit": 0,
                 "used_daily": 0, "limit_daily": 0, "window": "monthly",
                 "resets_at": "2026-11-01T00:00:00Z", "resets_at_daily": "2026-10-27T00:00:00Z"},
                {"resource_type": "translation_chunks", "used": 12, "limit": -1,
                 "used_hourly": 4, "limit_hourly": 20, "window": "monthly",
                 "resets_at": "2026-11-01T00:00:00Z", "resets_at_hourly": "2026-10-26T14:00:00Z"},
                {"resource_type": "future_thing", "used": 1, "limit": 2, "window": "monthly",
                 "resets_at": "2026-11-01T00:00:00Z"}
            ]
        })
    }

    /// An API build from before engram-api #26: no window / reset / scope fields.
    pub fn usage_before_windows() -> Value {
        json!({
            "plan_id": "free",
            "usage": [{"resource_type": "cards_total", "used": 42, "limit": 500}]
        })
    }

    /// Mounts all three endpoints with 200s.
    pub async fn mount_account(server: &MockServer) {
        for (p, body) in [
            (
                "/me",
                json!({"user_id": USER_ID, "canManageSubjects": false}),
            ),
            ("/me/subscription", subscription()),
            ("/me/usage", usage()),
        ] {
            Mock::given(method("GET"))
                .and(path(p))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(server)
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn next_reset_follows_utc_calendar_boundaries() {
        let now = at("2026-10-26T13:45:10Z");
        assert_eq!(next_reset("monthly", now), Some(at("2026-11-01T00:00:00Z")));
        assert_eq!(next_reset("daily", now), Some(at("2026-10-27T00:00:00Z")));
        assert_eq!(next_reset("hourly", now), Some(at("2026-10-26T14:00:00Z")));
        assert_eq!(next_reset("absolute", now), None);
        assert_eq!(next_reset("weekly", now), None);
    }

    #[test]
    fn next_reset_rolls_over_year_month_and_day_ends() {
        let now = at("2026-12-31T23:59:59Z");
        assert_eq!(next_reset("monthly", now), Some(at("2027-01-01T00:00:00Z")));
        assert_eq!(next_reset("daily", now), Some(at("2027-01-01T00:00:00Z")));
        assert_eq!(next_reset("hourly", now), Some(at("2027-01-01T00:00:00Z")));
    }

    #[test]
    fn counts_normalize_unlimited_not_included_and_overuse() {
        let c = Counts::new(7, -1);
        assert!(c.unlimited && c.included);
        assert_eq!((c.limit, c.remaining), (None, None));

        let c = Counts::new(0, 0);
        assert!(!c.unlimited && !c.included);
        assert_eq!((c.limit, c.remaining), (Some(0), Some(0)));

        let c = Counts::new(520, 500);
        assert_eq!(c.remaining, Some(0), "over-consumed window clamps to 0");
        assert_eq!(Counts::new(3, 5).remaining, Some(2));
    }

    #[tokio::test]
    async fn subscription_usage_normalizes_the_three_responses() {
        use serde_json::json;
        let server = wiremock::MockServer::start().await;
        fixtures::mount_account(&server).await;
        let client = EngramoClient::new(server.uri(), "tok");
        let v = serde_json::to_value(subscription_usage(&client).await.unwrap()).unwrap();

        assert_eq!(v["userId"], fixtures::USER_ID);
        assert_eq!(
            v["plan"],
            json!({
                "id": "free", "name": "Free",
                "startsAt": "2026-01-01T00:00:00Z", "expiresAt": null,
                "pendingPlanId": null, "lapsedPlanId": null, "deactivatedPlanId": null,
                "features": ["api_token_create"]
            })
        );
        assert!(
            v["plan"].get("limits").is_none(),
            "raw limits JSON is not passed through"
        );
        assert_eq!(v["howToChange"], PLAN_CHANGE_HINT);

        let usage = v["usage"].as_array().unwrap();
        assert_eq!(
            usage[0],
            json!({
                "resource": "catalogs_total", "label": "Catalogs", "window": "absolute",
                "used": 3, "limit": 5, "remaining": 2, "unlimited": false, "included": true,
                "resetsAt": null
            })
        );
        assert_eq!(usage[1]["label"], "Cards in your fullest catalog");
        assert_eq!(usage[1]["catalogId"], fixtures::FULLEST_CATALOG);
        assert_eq!(usage[1]["remaining"], 20);

        let ai = &usage[2];
        assert_eq!(
            (ai["included"].clone(), ai["remaining"].clone()),
            (json!(false), json!(0))
        );
        assert_eq!(ai["resetsAt"], "2026-11-01T00:00:00Z");
        assert_eq!(
            ai["daily"],
            json!({"used": 0, "limit": 0, "remaining": 0, "unlimited": false,
                   "included": false, "resetsAt": "2026-10-27T00:00:00Z"})
        );
        assert!(ai.get("hourly").is_none());

        let tr = &usage[3];
        assert_eq!(tr["unlimited"], true);
        assert_eq!(
            (tr["limit"].clone(), tr["remaining"].clone()),
            (json!(null), json!(null))
        );
        assert_eq!(tr["hourly"]["remaining"], 16);
        assert_eq!(tr["hourly"]["resetsAt"], "2026-10-26T14:00:00Z");

        assert_eq!(usage[4]["resource"], "future_thing");
        assert_eq!(
            usage[4]["label"], "future_thing",
            "unknown types pass through"
        );
    }

    #[tokio::test]
    async fn subscription_usage_against_an_api_without_windows() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me/usage"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(fixtures::usage_before_windows()),
            )
            .mount(&server)
            .await;
        fixtures::mount_account(&server).await;
        let client = EngramoClient::new(server.uri(), "tok");
        let v = serde_json::to_value(subscription_usage(&client).await.unwrap()).unwrap();
        let e = &v["usage"][0];
        assert_eq!(e["resource"], "cards_total");
        assert_eq!(e["remaining"], 458);
        assert_eq!(
            (e["window"].clone(), e["resetsAt"].clone()),
            (serde_json::Value::Null, serde_json::Value::Null)
        );
    }

    #[tokio::test]
    async fn subscription_usage_fails_whole_when_any_call_fails() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        for (failing, status) in [("/me", 401), ("/me/subscription", 500), ("/me/usage", 401)] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(failing))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;
            fixtures::mount_account(&server).await;
            let client = EngramoClient::new(server.uri(), "tok");
            let err = subscription_usage(&client).await.unwrap_err();
            match status {
                401 => assert!(matches!(err, ApiError::Unauthorized), "{failing}: {err:?}"),
                _ => assert!(matches!(err, ApiError::Internal), "{failing}: {err:?}"),
            }
        }
    }

    #[test]
    fn every_known_resource_and_feature_has_a_label() {
        for rt in [
            "catalogs_total",
            "cards_total",
            "cards_per_catalog",
            "ai_agent_messages",
            "tts_generations",
            "premium_tts_generations",
            "translation_chunks",
        ] {
            assert!(resource_label(rt).is_some(), "{rt}");
        }
        for f in [
            "catalog_share",
            "tts_generate",
            "dictionary_generate",
            "api_token_create",
            "ai_chat",
            "translate_cards",
            "premium_tts",
            "youtube_workflow",
        ] {
            assert!(feature_label(f).is_some(), "{f}");
        }
        assert_eq!(resource_label("future_thing"), None);
    }
}
