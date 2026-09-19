use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use chrono::{DateTime, Datelike, Utc};
use golem_cli::{fs, versions};
use golem_common::model::account_usage::{
    AccountUsageMetering, AccountUsagePeriod, MeteringStatus,
};
use indoc::{formatdoc, indoc};
use serde::Deserialize;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

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
    period: AccountUsagePeriod,
    as_of: DateTime<Utc>,
    metering: AccountUsageMetering,
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

fn previous_period(period: AccountUsagePeriod) -> AccountUsagePeriod {
    if period.month == 1 {
        AccountUsagePeriod {
            year: period.year - 1,
            month: 12,
        }
    } else {
        AccountUsagePeriod {
            year: period.year,
            month: period.month - 1,
        }
    }
}

async fn seed_account_usage(
    ctx: &TestContext,
) -> (AccountUsagePeriod, AccountUsagePeriod, AccountUsagePeriod) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(ctx.data_dir.path().join("registry.db"))
                .journal_mode(SqliteJournalMode::Wal)
                .foreign_keys(true),
        )
        .await
        .expect("failed to open registry database");
    let account_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT account_id FROM accounts WHERE email = 'initial@user'",
    )
    .fetch_one(&pool)
    .await
    .expect("initial account is missing");

    let now = Utc::now();
    let current = AccountUsagePeriod {
        year: now.year(),
        month: now.month(),
    };
    let previous = previous_period(current);
    let zero_period = previous_period(previous);
    let gb_month = 1024_i64.pow(3) * 730 * 3600;

    for (period, values, metering) in [
        (
            current,
            [(2, 1_500_000), (10, gb_month), (11, 2 * gb_month), (12, 14)],
            (true, true, true),
        ),
        (
            previous,
            [
                (2, 2_500_000),
                (10, 3 * gb_month),
                (11, 4 * gb_month),
                (12, 21),
            ],
            (true, false, true),
        ),
        (
            zero_period,
            [(2, 0), (10, 0), (11, 0), (12, 0)],
            (false, true, false),
        ),
    ] {
        let usage_key = period.to_string();
        for (usage_type, value) in values {
            sqlx::query(
                "INSERT INTO account_usage_stats \
                 (account_id, usage_type, usage_key, value, updated_at) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(account_id)
            .bind(usage_type)
            .bind(&usage_key)
            .bind(value)
            .bind(now)
            .execute(&pool)
            .await
            .expect("failed to seed account usage");
        }
        sqlx::query(
            "INSERT INTO account_usage_metering_state \
             (account_id, usage_key, compute_enabled, memory_enabled, \
              filesystem_enabled, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(account_id)
        .bind(usage_key)
        .bind(metering.0)
        .bind(metering.1)
        .bind(metering.2)
        .bind(now)
        .execute(&pool)
        .await
        .expect("failed to seed account usage metering state");
    }

    (current, previous, zero_period)
}

#[test]
#[timeout("1m")]
async fn account_usage_and_limits_use_live_cli_wire_path(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    let (current_period, previous_period, zero_period) = seed_account_usage(&ctx).await;

    let output = ctx
        .cli([cmd::ACCOUNT, "usage", "show", flag::FORMAT, "json"])
        .await;
    assert!(output.success_or_dump());
    let usage = output
        .stdout_json::<AccountUsageView>()
        .into_iter()
        .next()
        .expect("account usage show produced no JSON output");
    assert_eq!(usage.kind, "account.usage.show");
    assert_eq!(usage.usage.compute_gcu, 1.5);
    assert_eq!(usage.usage.memory_gb_seconds, 14);
    assert_eq!(usage.usage.durable_storage_gb_month, 1.0);
    assert_eq!(usage.usage.ephemeral_storage_gb_month, 2.0);
    assert!(usage.usage.as_of <= Utc::now());
    assert_eq!(usage.usage.metering.compute, MeteringStatus::Enabled);
    assert_eq!(usage.usage.metering.memory, MeteringStatus::Enabled);
    assert_eq!(
        usage.usage.metering.durable_storage,
        MeteringStatus::Enabled
    );
    assert_eq!(
        usage.usage.metering.ephemeral_storage,
        MeteringStatus::Enabled
    );
    assert_eq!(usage.usage.period, current_period);

    let output = ctx.cli([cmd::ACCOUNT, "usage", "show"]).await;
    assert!(output.success_or_dump());
    assert!(output.stdout_contains("1.5 GCU"));
    assert!(output.stdout_contains("14 GB-seconds"));
    assert!(output.stdout_contains("1 GB-month"));
    assert!(output.stdout_contains("2 GB-month"));

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
    assert_eq!(history.usage.len(), 2);
    assert_eq!(history.usage[0].period, previous_period);
    assert_eq!(history.usage[0].compute_gcu, 2.5);
    assert_eq!(history.usage[0].memory_gb_seconds, 21);
    assert_eq!(history.usage[0].durable_storage_gb_month, 3.0);
    assert_eq!(history.usage[0].ephemeral_storage_gb_month, 4.0);
    assert_eq!(history.usage[0].metering.compute, MeteringStatus::Enabled);
    assert_eq!(history.usage[0].metering.memory, MeteringStatus::Disabled);
    assert_eq!(
        history.usage[0].metering.durable_storage,
        MeteringStatus::Enabled
    );
    assert_eq!(
        history.usage[0].metering.ephemeral_storage,
        MeteringStatus::Enabled
    );
    assert_eq!(history.usage[1].period, zero_period);
    assert_eq!(history.usage[1].compute_gcu, 0.0);
    assert_eq!(history.usage[1].memory_gb_seconds, 0);
    assert_eq!(history.usage[1].durable_storage_gb_month, 0.0);
    assert_eq!(history.usage[1].ephemeral_storage_gb_month, 0.0);
    assert_eq!(history.usage[1].metering.compute, MeteringStatus::Disabled);
    assert_eq!(history.usage[1].metering.memory, MeteringStatus::Enabled);
    assert_eq!(
        history.usage[1].metering.durable_storage,
        MeteringStatus::Disabled
    );
    assert_eq!(
        history.usage[1].metering.ephemeral_storage,
        MeteringStatus::Disabled
    );

    let output = ctx.cli([cmd::ACCOUNT, "usage", "history"]).await;
    assert!(output.success_or_dump());
    assert!(output.stdout_contains("2.5 GCU"));
    assert!(
        output.stdout_contains_ordered(["21", "GB-seconds", "(metering", "disabled)"]),
        "disabled metering must not hide accrued usage"
    );
    assert!(output.stdout_contains("3 GB-month"));
    assert!(output.stdout_contains("4 GB-month"));
    assert!(output.stdout_contains(zero_period.to_string()));
    assert!(output.stdout_contains_ordered(["0 GCU", "(metering", "disabled)"]));
    assert!(output.stdout_contains_ordered([zero_period.to_string(), "GB-seconds".to_string()]));

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

#[test]
#[timeout("5m")]
async fn account_usage_reports_sparse_allocated_memory(_tracing: &Tracing) {
    // The standalone server uses the 60s ResourceLimitsGrpcConfig default. Allow
    // two complete flush intervals so a delayed tick does not make the test flaky.
    const BILLING_REPORT_TIMEOUT: Duration = Duration::from_secs(120);

    let mut ctx = TestContext::new();
    ctx.add_env_var("GOLEM__RESOURCE_USAGE_METERING__MEMORY", "true");
    ctx.start_server().await;

    let app_name = "memory-billing";
    let output = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "rust"])
        .await;
    assert!(output.success_or_dump());
    ctx.cd(app_name);
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: memory-billing

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              memory-billing:rust-main:
                templates: rust
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("src/lib.rs"),
        "mod sparse_memory_agent;\n\npub use sparse_memory_agent::*;\n",
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("src/sparse_memory_agent.rs"),
        indoc! {r#"
            use golem_rust::{agent_definition, agent_implementation};

            #[agent_definition]
            pub trait SparseMemoryAgent {
                fn new(name: String) -> Self;
                fn allocate_and_work(&self) -> u32;
            }

            struct SparseMemoryAgentImpl;

            #[agent_implementation]
            impl SparseMemoryAgent for SparseMemoryAgentImpl {
                fn new(_name: String) -> Self {
                    Self
                }

                fn allocate_and_work(&self) -> u32 {
                    let previous_pages = core::arch::wasm32::memory_grow::<0>(4096);
                    assert_ne!(previous_pages, usize::MAX, "sparse memory growth failed");
                    let started = std::time::Instant::now();
                    let mut work = 0u64;
                    while started.elapsed() < std::time::Duration::from_secs(5) {
                        for _ in 0..10_000_000 {
                            work = std::hint::black_box(work.wrapping_add(1));
                        }
                    }
                    previous_pages as u32
                }
            }
        "#},
    )
    .unwrap();

    let output = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(output.success_or_dump());
    let output = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "SparseMemoryAgent(\"memory-billing\")",
            "allocate_and_work",
        ])
        .await;
    assert!(output.success_or_dump());

    let deadline = tokio::time::Instant::now() + BILLING_REPORT_TIMEOUT;
    loop {
        let output = ctx
            .cli([cmd::ACCOUNT, "usage", "show", flag::FORMAT, "json"])
            .await;
        assert!(output.success_or_dump());
        let usage = output
            .stdout_json::<AccountUsageView>()
            .into_iter()
            .next()
            .expect("account usage show produced no JSON output");
        if usage.usage.memory_gb_seconds > 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "CLI reported zero Memory GB-seconds after sparse allocation"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
