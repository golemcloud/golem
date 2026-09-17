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

//! The oplog sweep on a real executor, where its residency probe meets `ActiveAgents` and the
//! unloaded-worker TTL.

use crate::Tracing;

use golem_api_grpc::proto::golem::worker::{
    AgentInvocationMode, InvocationFreshnessDisposition, InvocationStart,
};
use golem_common::model::oplog::OplogEntry;
use golem_common::model::{AgentId, IdempotencyKey, OwnedAgentId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, sqlite_storage_config, start_with_overrides,
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

/// Twenty sweep intervals, so a probe that counted as an access would reach the worker many times
/// inside every TTL and the worker could never expire.
const TEST_TTL: Duration = Duration::from_secs(2);
const SWEEP_INTERVAL: Duration = Duration::from_millis(100);

/// Rows the ephemeral compressed layers hold for one agent, read from the executor's
/// indexed-storage database directly.
async fn ephemeral_layer_rows(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    agent_id: &AgentId,
) -> anyhow::Result<i64> {
    // The executor names the indexed storage's file after the key-value one.
    let key_value = sqlite_storage_config(deps, context);
    let database = match key_value.database.strip_suffix(".db") {
        Some(stem) => format!("{stem}-indexed.db"),
        None => format!("{}-indexed", key_value.database),
    };
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&database)
        .create_if_missing(false);
    let pool = sqlx::SqlitePool::connect_with(options).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM index_storage \
         WHERE namespace LIKE 'ephemeral-worker-c%-oplog' AND key = ?",
    )
    .bind(agent_id.to_redis_key())
    .fetch_one(&pool)
    .await?;
    pool.close().await;
    Ok(count)
}

async fn wait_until(
    message: &str,
    mut condition: impl AsyncFnMut() -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !condition().await? {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for {message}"))?
}

/// A worker built by an invocation lookup can sit over an oplog a crash stranded. The sweep leaves
/// the oplog alone while the worker is cached and archives it once the TTL evicts the worker, which
/// only happens if the per-tick residency probe does not refresh the TTL.
#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn a_lookup_created_worker_expires_and_its_stranded_oplog_is_swept(
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
                config.active_agents.ttl = TEST_TTL;
                config.oplog.sweep.interval = SWEEP_INTERVAL;
                config.durable_stream.renewal_interval = Duration::from_millis(5);
                config.durable_stream.reconciliation_interval = Duration::from_millis(5);
            })),
            ..Default::default()
        },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let parsed_agent_id = agent_id!("EphemeralCounter", "swept-after-lookup");
    executor
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;
    let idempotency_key = IdempotencyKey::fresh();
    executor
        .invoke_and_await_agent_with_key(
            &component,
            &parsed_agent_id,
            &idempotency_key,
            "increment",
            data_value!(),
        )
        .await?;
    // The DSL runs each ephemeral invocation under a phantom derived from its key.
    let agent_id = AgentId::from_agent_id(
        component.id,
        &parsed_agent_id
            .with_ephemeral_invocation_phantom(&idempotency_key)
            .map_err(anyhow::Error::msg)?,
    )
    .map_err(anyhow::Error::msg)?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &agent_id);

    // Teardown removes the worker and drains its layer.
    wait_until("the invocation's teardown", || async {
        Ok(!executor.worker_is_cached(&owned_agent_id).await
            && ephemeral_layer_rows(deps, &context, &agent_id).await? == 0)
    })
    .await?;

    executor
        .invoke_agent_session(InvocationStart {
            agent_id: Some(agent_id.clone().into()),
            method_name: None,
            input: None,
            mode: AgentInvocationMode::Lookup as i32,
            schedule_at: None,
            idempotency_key: Some(idempotency_key.into()),
            component_owner_account_id: Some(component.account_id.into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            context: None,
            principal: None,
            freshness_disposition: InvocationFreshnessDisposition::MayExist as i32,
            config: Vec::new(),
            attempt_id: None,
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            external_tool: None,
        })
        .await?;
    assert!(
        executor.worker_is_cached(&owned_agent_id).await,
        "the lookup should leave a suspended worker behind"
    );

    // No invocation runs on the suspended worker, so no teardown drains what it commits. That is
    // the state a crash leaves, with the worker already cached over it.
    executor
        .commit_oplog_entry_bypassing_worker_status(&agent_id, OplogEntry::suspend())
        .await?;
    assert!(
        ephemeral_layer_rows(deps, &context, &agent_id).await? > 0,
        "the committed entry should be in an ephemeral compressed layer"
    );

    // Ten ticks inside the TTL, which the commit above restarted, so the worker is still cached
    // and the sweep has to leave the entry where it is.
    tokio::time::sleep(SWEEP_INTERVAL * 10).await;
    assert!(
        executor.worker_is_cached(&owned_agent_id).await,
        "the suspended worker should still be cached inside its TTL"
    );
    assert!(
        ephemeral_layer_rows(deps, &context, &agent_id).await? > 0,
        "the sweep should leave the entry alone while the worker is cached"
    );

    wait_until("the sweep to archive the stranded entry", || async {
        Ok(ephemeral_layer_rows(deps, &context, &agent_id).await? == 0)
    })
    .await?;
    Ok(())
}
