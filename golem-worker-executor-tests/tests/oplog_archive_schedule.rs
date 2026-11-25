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

use futures::FutureExt;
use golem_common::model::agent::AgentMode;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{LogLevel, OplogEntry};
use golem_common::model::{AgentInvocation, InvocationId, ShardLeaseClock, ShardLeaseRevision};
use golem_common::{agent_key, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::derive_disjoint_sqlite_config;
use golem_worker_executor::services::oplog::ArchiveWait;
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor::services::{HasOplog, HasOplogService, HasWorkerActivator};
use golem_worker_executor::storage::indexed::IndexedStorage;
use golem_worker_executor::storage::indexed::sqlite::SqliteIndexedStorage;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, callers, scheduler_sqlite_storage_config,
    sqlite_storage_config, start, start_with_overrides,
};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
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
    let agent_id = agent_key!("EphemeralCounter", "archive-schedule");
    executor
        .create_unkeyed_agent(
            &component.id,
            None,
            agent_id.clone(),
            std::collections::HashMap::new(),
        )
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
    let durable_agent_id = agent_key!("Counter", "archive-schedule-durable");
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
    let agent_id = agent_key!("EphemeralCounter", "archive-schedule-sweep-off");
    executor
        .create_unkeyed_agent(
            &component.id,
            None,
            agent_id.clone(),
            std::collections::HashMap::new(),
        )
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

/// A sealed owner must lose its cached Worker, not just its Store. Reusing that Worker after
/// restoration at the same shard epoch must refuse both invocation admission and oplog writes.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn sealed_cached_worker_is_reacquired_without_reusing_its_old_handle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.indexed_storage_layers = 1;
                config.oplog.blob_storage_layers = 1;
                config.oplog.entry_count_limit = 1_000_000;
                config.oplog.shard_lease_max_ttl = Duration::from_secs(300);
                config.oplog.skew_margin = Duration::from_secs(1);
            })),
            wrap_shard_service: Some(Arc::new({
                let shard = shard.clone();
                move |inner| {
                    assert!(shard.set(inner.clone()).is_ok());
                    inner
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_key!("GolemWasiHttp", "seal-cached-worker");
    let owned = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    for _ in 0..2 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
            .await?
            .into_typed::<u64>()?,
        2
    );
    let old = executor.active_agents().try_get(&owned).await.unwrap();
    let old_oplog = old.oplog();
    let fingerprint = old.get_initial_worker_metadata().fingerprint;
    tokio::time::timeout(Duration::from_secs(30), async {
        while executor.worker_is_loaded(&owned).await {
            executor.stop_worker_if_idle(&owned).await?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    assert!(executor.worker_is_cached(&owned).await);
    assert!(Arc::ptr_eq(
        &old,
        &executor.active_agents().try_get(&owned).await.unwrap()
    ));

    // Replace the harness's unexpiring grant with a real lease without changing its shard epochs.
    let shard = shard.get().unwrap();
    let assignment = shard.current_assignment()?;
    shard.update_lease(
        &assignment.shard_epochs,
        ShardLeaseClock {
            sent_at: Instant::now(),
            wall_anchor: SystemTime::now(),
            ttl: Duration::from_secs(300),
        },
        ShardLeaseRevision {
            incarnation: uuid::Uuid::new_v4(),
            number: 1,
        },
        assignment.executor_count,
    )?;
    let epoch = old_oplog.shard_epoch().expect("durable oplog has an epoch");
    assert!(
        shard
            .lease_valid_until(&owned.agent_id, epoch, Duration::from_secs(1))
            .is_some()
    );
    let indexed_config =
        derive_disjoint_sqlite_config(&sqlite_storage_config(deps, &context), "indexed");
    let indexed = SqliteIndexedStorage::configured(&indexed_config).await?;
    let resident = indexed.observe_agent("test", "before_seal", &owned).await?;
    assert!(resident.has_data);
    let history = executor.stored_oplog(&owned.agent_id).await;
    let oplog_service = old.oplog_service();
    let last = oplog_service
        .get_last_index(&owned, AgentMode::Durable)
        .await?;
    let activator = old.worker_activator();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match activator
                .archive_oplog(&owned, last, ArchiveWait::Finished)
                .await?
            {
                Some(true) => tokio::time::sleep(Duration::from_millis(10)).await,
                Some(false) => break,
                None => anyhow::bail!("the current cached generation was not archived"),
            }
        }
        anyhow::Ok(())
    })
    .await??;

    let sealed = indexed.observe_agent("test", "after_seal", &owned).await?;
    assert!(!sealed.has_data, "seal must remove all indexed authority");
    assert_eq!(sealed.durable_generation, None);
    assert_eq!(sealed.ephemeral_generation, None);
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&indexed_config.database)
            .create_if_missing(false),
    )
    .await?;
    let key = owned.storage_key();
    let epochs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM indexed_key_epoch WHERE key = ? OR (key >= ? || '#' AND key < ? || '$')",
    )
    .bind(&key)
    .bind(&key)
    .bind(&key)
    .fetch_one(&pool)
    .await?;
    pool.close().await;
    assert_eq!(epochs, 0, "seal must release every indexed epoch row too");
    assert!(!executor.worker_is_cached(&owned).await);
    assert!(old_oplog.is_retired());
    assert_eq!(executor.stored_oplog(&owned.agent_id).await, history);

    // An archive redelivery observes frozen history without constructing or unsealing an owner.
    assert_eq!(
        activator
            .archive_oplog(&owned, last, ArchiveWait::Finished)
            .await?,
        None
    );
    assert!(!executor.worker_is_cached(&owned).await);
    assert!(
        !indexed
            .observe_agent("test", "archive_redelivery", &owned)
            .await?
            .has_data
    );
    let invocation = || AgentInvocation::AgentMethod {
        idempotency_key: InvocationId(uuid::Uuid::new_v4()),
        method_name: "increment".to_string(),
        input: data_value!().into_parts().1.try_into().unwrap(),
        allow_streams: false,
        invocation_context: InvocationContextStack::fresh(),
        caller: callers::anonymous(),
        scope_card: None,
    };
    assert!(matches!(
        old.clone().invoke(invocation()).await,
        Err(WorkerExecutorError::ShardingNotReady)
    ));
    assert_eq!(executor.stored_oplog(&owned.agent_id).await, history);

    // Ordinary invocation must restore presence, construct a different Worker and replay its state.
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
            .await?
            .into_typed::<u64>()?,
        2
    );
    let new = executor.active_agents().try_get(&owned).await.unwrap();
    assert!(!Arc::ptr_eq(&old, &new));
    assert_eq!(new.get_initial_worker_metadata().fingerprint, fingerprint);
    assert_eq!(new.oplog().shard_epoch(), Some(epoch));
    let restored = indexed
        .observe_agent("test", "after_unseal", &owned)
        .await?;
    assert!(restored.has_data);
    assert!(restored.durable_generation.is_some());
    assert_ne!(restored.durable_generation, resident.durable_generation);
    // Await settlement before comparing history around refused stale-generation operations.
    tokio::time::timeout(Duration::from_secs(30), async {
        while executor.worker_is_loaded(&owned).await {
            executor.stop_worker_if_idle(&owned).await?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    let continued = executor.stored_oplog(&owned.agent_id).await;
    assert!(continued.len() > history.len());
    assert_eq!(&continued[..history.len()], history.as_slice());

    assert!(matches!(
        old.clone().invoke(invocation()).await,
        Err(WorkerExecutorError::ShardingNotReady)
    ));
    assert!(
        std::panic::AssertUnwindSafe(async {
            old_oplog
                .add(OplogEntry::log(
                    None,
                    LogLevel::Info,
                    "stale-generation".to_string(),
                    "must not be written".to_string(),
                    None,
                ))
                .await
        })
        .catch_unwind()
        .await
        .is_err(),
        "restoration at the same epoch must not revive the old oplog handle"
    );
    assert_eq!(executor.stored_oplog(&owned.agent_id).await, continued);
    executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
            .await?
            .into_typed::<u64>()?,
        3
    );
    Ok(())
}
