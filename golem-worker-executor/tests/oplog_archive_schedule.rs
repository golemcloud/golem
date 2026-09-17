// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Counts the `ArchiveOplog` rows an ephemeral workload leaves in scheduler storage. With the oplog
//! sweep enabled there are none, while a durable agent in the same component, the control, still
//! registers one.

use crate::Tracing;

use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, scheduler_sqlite_storage_config, start, start_with_overrides,
};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// Large enough that "one row per invocation" cannot be mistaken for incidental scheduler traffic.
const INVOCATIONS: usize = 30;

/// Reads scheduler storage directly. This workload schedules nothing but `ArchiveOplog`.
async fn scheduled_action_count(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> anyhow::Result<i64> {
    let config = scheduler_sqlite_storage_config(deps, context);
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&config.database)
        .create_if_missing(false);
    let pool = sqlx::SqlitePool::connect_with(options).await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scheduled_actions")
        .fetch_one(&pool)
        .await?;
    pool.close().await;
    Ok(count)
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn ephemeral_invocations_schedule_no_oplog_archive(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("EphemeralCounter", "archive-schedule");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for _ in 0..INVOCATIONS {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    // Settling time, so a registration that merely arrived late still fails the assertion.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let count = scheduled_action_count(deps, &context).await?;
    assert_eq!(
        count, 0,
        "{INVOCATIONS} ephemeral invocations left {count} scheduled actions behind"
    );

    // The control: `Counter` is durable and in the same component, so only the agent mode differs.
    let durable_agent_id = agent_id!("Counter", "archive-schedule-durable");
    executor
        .start_agent(&component.id, durable_agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &durable_agent_id, "increment", data_value!())
        .await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let with_durable = scheduled_action_count(deps, &context).await?;
    assert!(
        with_durable > 0,
        "a durable agent must still register its archive, but the table holds {with_durable} rows"
    );
    Ok(())
}

/// With the sweep disabled nothing else would archive an ephemeral oplog stranded by a crashed
/// pod, so the registration has to happen again.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn disabling_the_sweep_restores_the_ephemeral_archive_registration(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.sweep.enabled = false;
            })),
            ..Default::default()
        },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_id!("EphemeralCounter", "archive-schedule-sweep-off");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    // Settling time, as in the positive case.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let count = scheduled_action_count(deps, &context).await?;
    assert!(
        count > 0,
        "with the sweep off an ephemeral agent must register its archive again, \
         but the table holds {count} rows"
    );
    Ok(())
}
