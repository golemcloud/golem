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

// A rejection (`InvalidRequest`) of an invocation that a user cancellation had already
// withdrawn must leave the committed cancellation as the result of its key, for warm and cold
// lookups. The rejection is driven through the same steps the durable host takes: it writes the
// rejection pair (`CancelPendingInvocation` + invocation `Error`), commits it, and then calls
// `store_invocation_failure` with the rejection trap.

use crate::Tracing;
use chrono::Utc;
use golem_common::model::agent::{AgentPrincipal, Principal};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{AgentError, OplogEntry, OplogErrorKind, OplogIndex};
use golem_common::model::{AgentInvocation, InvocationId, OwnedAgentId};
use golem_common::{agent_id, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::model::{LookupResult, TrapType};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);

async fn late_rejection_after_cancel(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_rpc_rust: &PrecompiledComponent,
    overrides: TestExecutorOverrides,
    cancel_first: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server = agent_id!("ScheduledInvocationServer", "late-rejection-after-cancel");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let owner = executor
        .start_agent(
            &component.id,
            agent_id!("ScheduledInvocationClient", "late-rejection-owner"),
        )
        .await?;
    let invocation = |key: &InvocationId, value: u64| AgentInvocation::AgentMethod {
        idempotency_key: *key,
        method_name: "inc_global_by".to_string(),
        input: data_value!(value).into_parts().1,
        invocation_context: InvocationContextStack::fresh(),
        principal: Principal::Agent(AgentPrincipal {
            agent_id: owner.clone(),
        }),
        scope_card: None,
    };
    let owned = OwnedAgentId::new(component.environment_id, &target);
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    let due = Utc::now() + chrono::Duration::hours(1);

    let key = InvocationId(uuid::Uuid::new_v4());
    let receipt = worker.schedule_invocation(invocation(&key, 5), due).await?;
    if cancel_first {
        assert!(
            worker
                .cancel_scheduled_invocation(key, receipt, &owner)
                .await?
        );
    }

    // The durable host's rejection pair for the withdrawn invocation.
    let error = AgentError::InvalidRequest("Agent method 'gone' not found".to_string());
    worker
        .add_to_oplog(OplogEntry::cancel_pending_invocation(key))
        .await?;
    worker
        .add_and_commit_oplog(OplogEntry::error(
            None,
            OplogErrorKind::Invocation,
            Some(key),
            error.clone(),
            OplogIndex::INITIAL,
            None,
        ))
        .await?;

    // Other results committed by the same agent before the rejection is stored.
    // With a one-entry recent index they evict the key, so its result is found through the
    // persisted index or the oplog.
    for _ in 0..4 {
        let other = InvocationId(uuid::Uuid::new_v4());
        let receipt = worker
            .schedule_invocation(invocation(&other, 1), due)
            .await?;
        assert!(
            worker
                .cancel_scheduled_invocation(other, receipt, &owner)
                .await?
        );
    }

    worker
        .store_invocation_failure(
            &key,
            &TrapType::Error {
                error,
                retry_from: OplogIndex::INITIAL,
                semantic_trap_retry_override: None,
            },
        )
        .await;

    let expected = if cancel_first {
        WorkerExecutorError::InvocationCancelled
    } else {
        WorkerExecutorError::invalid_request("Agent method 'gone' not found")
    };
    let warm = worker.lookup_invocation_result(&key).await;
    assert_eq!(
        format!("{warm:?}"),
        format!("{:?}", LookupResult::Complete(Err(expected.clone()))),
        "warm lookup after a late rejection"
    );
    drop(worker);
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    let cold = worker.lookup_invocation_result(&key).await;
    assert_eq!(
        format!("{cold:?}"),
        format!("{:?}", LookupResult::Complete(Err(expected))),
        "cold lookup after a late rejection"
    );
    drop(worker);
    Ok(())
}

fn evicting_overrides() -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.invocation_results.recent_capacity = 1;
            config.invocation_results.bloom_bits = 8;
            // A catch-up chunk ends inside the rejection pair.
            config.invocation_results.physical_index_catch_up_chunk_size = 1;
        })),
        ..Default::default()
    }
}

#[test]
#[timeout("4m")]
async fn late_rejection_after_a_user_cancellation_stays_cancelled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    late_rejection_after_cancel(
        last_unique_id,
        deps,
        agent_rpc_rust,
        evicting_overrides(),
        true,
    )
    .await
}

#[test]
#[timeout("4m")]
async fn rejection_of_an_uncancelled_invocation_stays_its_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    late_rejection_after_cancel(
        last_unique_id,
        deps,
        agent_rpc_rust,
        evicting_overrides(),
        false,
    )
    .await
}
