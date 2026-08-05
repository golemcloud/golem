use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_common::model::account_usage::StorageUsagePeriod;
use serde::Deserialize;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountUsageView {
    #[serde(rename = "$type")]
    kind: String,
    #[serde(flatten)]
    usage: AccountUsageItemView,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountUsageItemView {
    compute_gcu: f64,
    memory_gb_seconds: u64,
    durable_storage_gb_month: f64,
    ephemeral_storage_gb_month: f64,
    period: StorageUsagePeriod,
}

#[derive(Debug, Deserialize)]
struct AccountUsageListView {
    #[serde(rename = "$type")]
    kind: String,
    usage: Vec<AccountUsageItemView>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountLimitsView {
    #[serde(rename = "$type")]
    kind: String,
    effective_value: u64,
    plan_default: u64,
    override_value: Option<u64>,
    ceiling: u64,
    user_configurable: bool,
}

#[test]
fn account_usage_history_items_have_no_discriminator() {
    let history: AccountUsageListView = serde_json::from_value(serde_json::json!({
        "$type": "account.usage.history",
        "usage": [{
            "computeGcu": 1.5,
            "memoryGbSeconds": 4,
            "durableStorageGbMonth": 2.5,
            "ephemeralStorageGbMonth": 3.5,
            "period": { "year": 2026, "month": 4 }
        }]
    }))
    .unwrap();

    assert_eq!(history.usage.len(), 1);
    assert_eq!(history.usage[0].compute_gcu, 1.5);
    assert_eq!(history.usage[0].memory_gb_seconds, 4);
    assert_eq!(history.usage[0].period.month, 4);
}

#[test]
#[timeout("1m")]
async fn account_storage_usage_and_limits_use_live_cli_wire_path(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    ctx.start_server().await;

    let period_before_request = StorageUsagePeriod::current();
    let output = ctx
        .cli([cmd::ACCOUNT, "usage", "show", flag::FORMAT, "json"])
        .await;
    let period_after_request = StorageUsagePeriod::current();
    assert!(output.success_or_dump());
    let usage = output
        .stdout_json::<AccountUsageView>()
        .into_iter()
        .next()
        .expect("account usage show produced no JSON output");
    assert_eq!(usage.kind, "account.usage.show");
    assert_eq!(usage.usage.compute_gcu, 0.0);
    assert_eq!(usage.usage.memory_gb_seconds, 0);
    assert_eq!(usage.usage.durable_storage_gb_month, 0.0);
    assert_eq!(usage.usage.ephemeral_storage_gb_month, 0.0);
    assert!(
        usage.usage.period == period_before_request || usage.usage.period == period_after_request,
        "usage period should match the current period during the request"
    );

    let output = ctx
        .cli([cmd::ACCOUNT, "usage", "history", flag::FORMAT, "json"])
        .await;
    assert!(output.success_or_dump());
    let history = output
        .stdout_json::<AccountUsageListView>()
        .into_iter()
        .next()
        .expect("account usage history produced no JSON output");
    assert_eq!(history.kind, "account.usage.history");
    assert!(history.usage.is_empty());

    let output = ctx
        .cli([cmd::ACCOUNT, "limits", "show", flag::FORMAT, "json"])
        .await;
    assert!(output.success_or_dump());
    let limits = output
        .stdout_json::<AccountLimitsView>()
        .into_iter()
        .next()
        .expect("account limits show produced no JSON output");
    assert_eq!(limits.kind, "account.limits.show");
    assert_eq!(limits.effective_value, u64::MAX);
    assert_eq!(limits.plan_default, u64::MAX);
    assert_eq!(limits.override_value, None);
    assert_eq!(limits.ceiling, u64::MAX);
    assert!(limits.user_configurable);

    let output = ctx
        .cli([
            cmd::ACCOUNT,
            "limits",
            "set",
            "1048576",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(output.success_or_dump());
    let limits = output
        .stdout_json::<AccountLimitsView>()
        .into_iter()
        .next()
        .expect("account limits set produced no JSON output");
    assert_eq!(limits.kind, "account.limits.show");
    assert_eq!(limits.effective_value, 1_048_576);
    assert_eq!(limits.override_value, Some(1_048_576));

    let output = ctx
        .cli([cmd::ACCOUNT, "limits", "unset", flag::FORMAT, "json"])
        .await;
    assert!(output.success_or_dump());
    let limits = output
        .stdout_json::<AccountLimitsView>()
        .into_iter()
        .next()
        .expect("account limits unset produced no JSON output");
    assert_eq!(limits.kind, "account.limits.show");
    assert_eq!(limits.effective_value, u64::MAX);
    assert_eq!(limits.override_value, None);
}
