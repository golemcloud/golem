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
use golem_common::model::invocation_context::TraceId;
use golem_common::model::oplog::{
    AgentError, OplogEntry, OplogErrorKind, OplogIndex, OplogPayload,
};
use golem_common::model::{
    AgentInvocation, AgentInvocationPayload, InvocationId, InvocationOperation, OwnedAgentId,
    Timestamp,
};
use golem_common::{agent_id, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::model::{LookupResult, TrapType};
use golem_worker_executor::worker::invocations::{
    InvocationFilter, InvocationState, MAX_INVOCATION_PAGE_SIZE,
};
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

/// The invocations of an agent are listed the same before and after its cached status is
/// evicted, and after the executor restarts. With a one-entry recent index the older results
/// come from the persisted index, which the eviction removes and catch-up rebuilds, and the
/// payloads are external, so their operations are read from the request entries.
#[test]
#[timeout("4m")]
async fn invocations_are_listed_the_same_after_eviction_and_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.invocation_results.recent_capacity = 1;
            config.invocation_results.bloom_bits = 8;
            config.invocation_results.physical_index_catch_up_chunk_size = 1;
            config.oplog.max_payload_size = 1;
        })),
        ..Default::default()
    };
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server = agent_id!("ScheduledInvocationServer", "listed-after-eviction");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let owner = executor
        .start_agent(
            &component.id,
            agent_id!("ScheduledInvocationClient", "listed-after-eviction-owner"),
        )
        .await?;
    let invocation = |key: &InvocationId| AgentInvocation::AgentMethod {
        idempotency_key: *key,
        method_name: "inc_global_by".to_string(),
        input: data_value!(1u64).into_parts().1,
        invocation_context: InvocationContextStack::fresh(),
        principal: Principal::Agent(AgentPrincipal {
            agent_id: owner.clone(),
        }),
        scope_card: None,
    };
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let owned = OwnedAgentId::new(component.environment_id, &target);
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    let due = Utc::now() + chrono::Duration::hours(1);

    let mut cancelled = Vec::new();
    for _ in 0..3 {
        let key = InvocationId(uuid::Uuid::new_v4());
        let receipt = worker.schedule_invocation(invocation(&key), due).await?;
        assert!(
            worker
                .cancel_scheduled_invocation(key, receipt, &owner)
                .await?
        );
        cancelled.push(key);
    }
    let scheduled = InvocationId(uuid::Uuid::new_v4());
    worker
        .schedule_invocation(invocation(&scheduled), due)
        .await?;

    let before = worker
        .list_invocation_page(&InvocationFilter::default(), None, MAX_INVOCATION_PAGE_SIZE)
        .await?
        .0;
    let state_of = |key: &InvocationId| {
        before
            .iter()
            .find(|entry| entry.key == *key)
            .map(|entry| (entry.state, entry.operation.clone()))
    };
    let inc = Some(InvocationOperation::Method("inc_global_by".to_string()));
    for key in &cancelled {
        assert_eq!(
            state_of(key),
            Some((InvocationState::Cancelled, inc.clone()))
        );
    }
    assert_eq!(
        state_of(&scheduled),
        Some((InvocationState::Scheduled, inc.clone()))
    );
    assert!(
        before
            .iter()
            .any(|entry| entry.state == InvocationState::Completed
                && entry.operation
                    == Some(InvocationOperation::Method("get_global_value".to_string())))
    );
    assert!(
        before
            .windows(2)
            .all(|pair| pair[0].last_change > pair[1].last_change),
        "newest change first: {before:?}"
    );

    executor.evict_cached_status(&target).await?;
    assert_eq!(
        worker
            .list_invocation_page(&InvocationFilter::default(), None, MAX_INVOCATION_PAGE_SIZE)
            .await?
            .0,
        before,
        "after eviction"
    );
    drop(worker);
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    let after: Vec<_> = worker
        .list_invocation_page(&InvocationFilter::default(), None, MAX_INVOCATION_PAGE_SIZE)
        .await?
        .0
        .into_iter()
        .filter(|entry| before.iter().any(|known| known.key == entry.key))
        .collect();
    assert_eq!(after, before, "after a restart");
    drop(worker);
    Ok(())
}

/// The invocation loop withdraws a queued duplicate of a finished invocation. The withdrawal is
/// not the result of the key, so once the finished result has left the recent results the list
/// still shows that result.
#[test]
#[timeout("4m")]
async fn a_withdrawn_duplicate_of_an_older_result_is_listed_as_that_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.invocation_results.recent_capacity = 1;
            config.invocation_results.bloom_bits = 8;
            config.invocation_results.physical_index_catch_up_chunk_size = 1;
        })),
        ..Default::default()
    };
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server = agent_id!("ScheduledInvocationServer", "listed-duplicate");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let owner = executor
        .start_agent(
            &component.id,
            agent_id!("ScheduledInvocationClient", "listed-duplicate-owner"),
        )
        .await?;
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner.clone(),
    });
    let invocation = |key: &InvocationId| AgentInvocation::AgentMethod {
        idempotency_key: *key,
        method_name: "inc_global_by".to_string(),
        input: data_value!(1u64).into_parts().1,
        invocation_context: InvocationContextStack::fresh(),
        principal: principal.clone(),
        scope_card: None,
    };
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let owned = OwnedAgentId::new(component.environment_id, &target);
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    let due = Utc::now() + chrono::Duration::hours(1);

    let finished = InvocationId(uuid::Uuid::new_v4());
    worker
        .clone()
        .invoke_and_await(invocation(&finished))
        .await?;
    let state_of =
        |listed: &[golem_worker_executor::worker::invocations::InvocationListEntry]| {
            listed
                .iter()
                .find(|entry| entry.key == finished)
                .map(|entry| entry.state)
        };
    assert_eq!(
        state_of(
            &worker
                .list_invocation_page(&InvocationFilter::default(), None, MAX_INVOCATION_PAGE_SIZE)
                .await?
                .0
        ),
        Some(InvocationState::Completed)
    );

    // Later results evict the finished one from the one-entry recent results.
    for _ in 0..3 {
        let other = InvocationId(uuid::Uuid::new_v4());
        let receipt = worker.schedule_invocation(invocation(&other), due).await?;
        assert!(
            worker
                .cancel_scheduled_invocation(other, receipt, &owner)
                .await?
        );
    }
    assert_eq!(
        state_of(
            &worker
                .list_invocation_page(&InvocationFilter::default(), None, MAX_INVOCATION_PAGE_SIZE)
                .await?
                .0
        ),
        Some(InvocationState::Completed),
        "before the duplicate"
    );

    // A queued duplicate of the finished key, withdrawn as `discard_pending_duplicate` does.
    worker
        .add_to_oplog(OplogEntry::pending_agent_invocation(
            Some(Timestamp::from(due.timestamp_millis() as u64)),
            finished,
            OplogPayload::Inline(Box::new(AgentInvocationPayload::AgentMethod {
                method_name: "inc_global_by".to_string(),
                input: data_value!(1u64).into_parts().1,
                principal: principal.clone(),
                scope_card: None,
            })),
            TraceId::generate(),
            Vec::new(),
            Vec::new(),
        ))
        .await?;
    worker
        .add_and_commit_oplog(OplogEntry::cancel_pending_invocation(finished))
        .await?;

    let listed = worker
        .list_invocation_page(&InvocationFilter::default(), None, MAX_INVOCATION_PAGE_SIZE)
        .await?
        .0;
    let lookup = worker.lookup_invocation_result(&finished).await;
    assert_eq!(
        state_of(&listed),
        Some(InvocationState::Completed),
        "after the withdrawn duplicate (lookup of the key: {lookup:?}): {listed:?}"
    );
    drop(worker);
    Ok(())
}
