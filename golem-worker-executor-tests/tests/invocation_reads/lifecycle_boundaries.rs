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

use super::*;
use golem_api_grpc::proto::golem::worker::invocation_session_result;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    InvocationHandleOperation as Operation, invocation_handle_response as response,
    invocation_handle_success as success,
};
use golem_common::model::{AgentStatus, OwnedAgentId, OwnedInvocationId, PromiseId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_worker_executor::services::rpc::Rpc;
use pretty_assertions::assert_eq;
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use std::time::Duration;
use test_r::{test, timeout};

fn denied_management_with_small_recent_index() -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.invocation_results.recent_capacity = 1;
        })),
        create_direct_invocation_auth: Some(Arc::new(|| {
            Arc::new(HandleManagementAuth(Arc::new(AtomicBool::new(false))))
        })),
        ..Default::default()
    }
}

#[test]
#[timeout("4m")]
async fn retained_starter_authority_survives_result_index_eviction_and_executor_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = denied_management_with_small_recent_index();
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_key!("ScheduledInvocationServer", "retained-starter-boundary");
    let target = executor.start_agent(&component.id, agent.clone()).await?;
    let owner = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "retained-starter-owner"),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent, "get_global_value", data_value!())
        .await?;
    let worker = executor.active_agent(&target).await.unwrap().primary();
    let fingerprint = executor
        .active_agent(&owner)
        .await
        .unwrap()
        .primary()
        .get_initial_worker_metadata()
        .fingerprint;
    let invocation = |key, amount| AgentInvocation::AgentMethod {
        idempotency_key: key,
        method_name: "inc_global_by".to_string(),
        input: data_value!(amount).into_parts().1.try_into().unwrap(),
        allow_streams: false,
        invocation_context: InvocationContextStack::fresh(),
        caller: callers::agent_in(
            owner.agent_id,
            AgentTypeName("ScheduledInvocationClient".to_string()),
            fingerprint,
            target.environment_id,
        ),
        scope_card: None,
    };
    let completed = InvocationId(uuid::Uuid::new_v4());
    worker
        .clone()
        .invoke_and_await(invocation(completed, 23u64))
        .await?;
    let cancelled = InvocationId(uuid::Uuid::new_v4());
    worker
        .schedule_invocation(
            invocation(cancelled, 61u64),
            jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1),
        )
        .await?;
    assert!(matches!(
        handle_operation(&executor, &target, &owner.agent_id, cancelled, Operation::Cancel).await?,
        response::Result::Success(value)
            if value.result == Some(success::Result::Cancelled(true))
    ));
    drop(worker);
    let mut terminals = Vec::new();
    for key in [completed, cancelled] {
        terminals.push(
            handle_operation(&executor, &target, &owner.agent_id, key, Operation::Await).await?,
        );
    }
    assert!(matches!(&terminals[0], response::Result::Success(value)
        if matches!(value.result, Some(success::Result::Output(_)))));
    assert!(matches!(&terminals[1], response::Result::Success(value)
        if matches!(value.result, Some(success::Result::InvocationFailure(_)))));
    // These are fresh, mutating requests on this target, not repeated read-cache hits.
    for amount in [2u64, 5, 11, 17] {
        executor
            .invoke_and_await_agent(&component, &agent, "inc_global_by", data_value!(amount))
            .await?;
    }
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    for ((key, terminal), state) in [completed, cancelled].into_iter().zip(terminals).zip([
        InvocationLifecycleState::Completed,
        InvocationLifecycleState::Cancelled,
    ]) {
        assert!(matches!(
            handle_operation(&executor, &target, &owner.agent_id, key, Operation::Status).await?,
            response::Result::Success(value)
                if value.result == Some(success::Result::Status(state as i32))
        ));
        assert_eq!(
            handle_operation(&executor, &target, &owner.agent_id, key, Operation::Await).await?,
            terminal
        );
        assert!(matches!(
            handle_operation(&executor, &target, &owner.agent_id, key, Operation::Cancel).await?,
            response::Result::Success(value)
                if value.result == Some(success::Result::Cancelled(false))
        ));
        for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
            assert!(matches!(
                handle_operation(&executor, &target, &target.agent_id, key, operation).await?,
                response::Result::Failure(error)
                    if matches!(error.error, Some(
                        golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PermissionDenied(_)))
            ));
        }
    }
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "get_global_value", data_value!())
            .await?
            .into_typed::<u64>()?,
        58,
        "neither result observation nor cancelled work may add another effect"
    );
    let entries = executor.stored_oplog(&target.agent_id).await;
    for (key, expected) in [(completed, 1), (cancelled, 0)] {
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry,
                golem_common::model::oplog::OplogEntry::AgentInvocationStarted { invocation_id, .. }
                    if *invocation_id == key))
                .count(),
            expected
        );
    }
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_handle_in_another_real_environment_cannot_find_or_create_the_original_owner(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let captured: Arc<Mutex<Option<Arc<dyn Rpc>>>> = Arc::new(Mutex::new(None));
    let mut overrides = denied_management_with_small_recent_index();
    overrides.wrap_rpc = Some(Arc::new({
        let captured = captured.clone();
        move |rpc| {
            *captured.lock().unwrap() = Some(rpc.clone());
            rpc
        }
    }));
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let other_environment = golem_common::model::environment::EnvironmentId::new();
    let other_component = executor
        .component_dep(&other_environment, fixture)
        .store()
        .await?;
    let agent = agent_key!("ScheduledInvocationServer", "cross-environment-handle");
    let target = executor.start_agent(&component.id, agent.clone()).await?;
    let other = executor
        .start_agent(&other_component.id, agent.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent, "inc_global_by", data_value!(37u64))
        .await?;
    executor
        .invoke_and_await_agent(
            &other_component,
            &agent,
            "inc_global_by",
            data_value!(83u64),
        )
        .await?;
    let worker = executor.active_agent(&target).await.unwrap().primary();
    let key = InvocationId(uuid::Uuid::new_v4());
    worker
        .clone()
        .invoke_and_await(AgentInvocation::AgentMethod {
            idempotency_key: key,
            method_name: "get_global_value".to_string(),
            input: data_value!().into_parts().1.try_into().unwrap(),
            allow_streams: false,
            invocation_context: InvocationContextStack::fresh(),
            caller: callers::agent_in(
                target.agent_id,
                AgentTypeName("ScheduledInvocationServer".to_string()),
                worker.get_initial_worker_metadata().fingerprint,
                target.environment_id,
            ),
            scope_card: None,
        })
        .await?;
    drop(worker);
    let original =
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Await).await?;
    assert!(matches!(&original, response::Result::Success(value)
        if matches!(value.result, Some(success::Result::Output(_)))));
    let same_address_elsewhere = OwnedAgentId::new(other_environment, &target.agent_id);
    assert_ne!(other, same_address_elsewhere);
    assert!(
        executor
            .get_worker_metadata(&same_address_elsewhere)
            .await
            .is_err()
    );
    let rpc = captured.lock().unwrap().take().unwrap();
    // OwnedInvocationId has no environment. The direct API must select the caller's
    // environment, even when its agent address equals the recorded starter's address.
    let handle = OwnedInvocationId::new(target.agent_id, key);
    // Each caller carries the recorded starter's incarnation, so only the environment differs
    // from the starter that the target recorded.
    let starter_fingerprint = executor.get_worker_metadata(&target).await?.fingerprint;
    for caller in [&other, &same_address_elsewhere] {
        let caller = callers::agent_in(
            caller.agent_id,
            AgentTypeName("ScheduledInvocationServer".to_string()),
            starter_fingerprint,
            caller.environment_id,
        )
        .immediate;
        for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
            let result = rpc
                .invocation_handle(&handle, &caller, operation, &AuthCtx::System)
                .await;
            assert!(
                matches!(result, Err(WorkerExecutorError::PermissionDenied { .. })),
                "a handle must not escape the caller environment: {result:?}"
            );
            assert!(!executor.worker_is_cached(&same_address_elsewhere).await);
            assert!(
                executor
                    .get_worker_metadata(&same_address_elsewhere)
                    .await
                    .is_err()
            );
        }
    }
    assert_eq!(
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Await).await?,
        original
    );
    for (component, expected) in [(&component, 37u64), (&other_component, 83u64)] {
        assert_eq!(
            executor
                .invoke_and_await_agent(component, &agent, "get_global_value", data_value!())
                .await?
                .into_typed::<u64>()?,
            expected
        );
    }
    Ok(())
}

#[test]
#[timeout("4m")]
async fn cancellation_racing_start_reports_withdrawal_only_without_a_method_effect(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor =
        start_with_overrides(deps, &context, denied_management_with_small_recent_index()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    // Independent agents keep each race's effect observable without resetting guest state.
    for round in 0..4 {
        let agent = agent_key!("RpcBlockingCounter", format!("handle-start-race-{round}"));
        let target = executor.start_agent(&component.id, agent.clone()).await?;
        executor
            .invoke_and_await_agent(&component, &agent, "inc_by", data_value!(13u64))
            .await?;
        let gate = executor
            .invoke_and_await_agent(&component, &agent, "create_promise", data_value!())
            .await?
            .into_typed::<PromiseId>()?;
        let ready = executor
            .invoke_and_await_agent(&component, &agent, "create_promise", data_value!())
            .await?
            .into_typed::<PromiseId>()?;
        executor
            .complete_promise(&target.environment_id, &ready, vec![])
            .await?;
        let worker = executor.active_agent(&target).await.unwrap().primary();
        let invocation = |key, method: &str, promise| AgentInvocation::AgentMethod {
            idempotency_key: key,
            method_name: method.to_string(),
            input: data_value!(promise).into_parts().1.try_into().unwrap(),
            allow_streams: false,
            invocation_context: InvocationContextStack::fresh(),
            caller: callers::agent_in(
                target.agent_id,
                AgentTypeName("RpcBlockingCounter".to_string()),
                worker.get_initial_worker_metadata().fingerprint,
                target.environment_id,
            ),
            scope_card: None,
        };
        worker
            .clone()
            .invoke(invocation(
                InvocationId(uuid::Uuid::new_v4()),
                "await_promise",
                gate.clone(),
            ))
            .await?;
        executor
            .wait_for_status(&target, AgentStatus::Suspended, Duration::from_secs(20))
            .await?;
        let key = InvocationId(uuid::Uuid::new_v4());
        worker
            .clone()
            .invoke(invocation(key, "inc_after_promise", ready))
            .await?;
        assert!(matches!(
            handle_operation(&executor, &target, &target.agent_id, key, Operation::Status).await?,
            response::Result::Success(value)
                if value.result == Some(success::Result::Status(InvocationLifecycleState::Pending as i32))
        ));
        let release = executor.complete_promise(&target.environment_id, &gate, vec![]);
        let cancel = handle_operation(&executor, &target, &target.agent_id, key, Operation::Cancel);
        let (released, cancelled) = if round % 2 == 0 {
            tokio::join!(release, cancel)
        } else {
            let (cancelled, released) = tokio::join!(cancel, release);
            (released, cancelled)
        };
        released?;
        let cancelled = cancelled?;
        let withdrawn = match &cancelled {
            response::Result::Success(value)
                if value.result == Some(success::Result::Cancelled(true)) => true,
            response::Result::Success(value)
                if value.result == Some(success::Result::Cancelled(false)) => false,
            response::Result::Failure(error)
                if matches!(error.error, Some(
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::CannotCancelRunningInvocation(_))) => false,
            _ => anyhow::bail!("unexpected cancellation outcome: {cancelled:?}"),
        };
        let observed = tokio::time::timeout(
            Duration::from_secs(30),
            handle_operation(&executor, &target, &target.agent_id, key, Operation::Await),
        )
        .await??;
        let value = executor
            .invoke_and_await_agent(&component, &agent, "get_value", data_value!())
            .await?
            .into_typed::<u64>()?;
        assert_eq!(value, if withdrawn { 13 } else { 20 }, "{cancelled:?}");
        if withdrawn {
            assert!(matches!(&observed, response::Result::Success(value)
                if matches!(value.result, Some(success::Result::InvocationFailure(_)))));
        } else {
            let response::Result::Success(result) = &observed else {
                anyhow::bail!("started invocation lost its result: {observed:?}");
            };
            let Some(success::Result::Output(output)) = &result.result else {
                anyhow::bail!("started invocation has no output: {observed:?}");
            };
            assert_eq!(
                output.result,
                Some(invocation_session_result::Result::MethodResult(
                    golem_common::schema::SchemaValue::U64(20)
                        .try_into()
                        .unwrap()
                ))
            );
        }
        assert!(matches!(
            handle_operation(&executor, &target, &target.agent_id, key, Operation::Cancel).await?,
            response::Result::Success(value)
                if value.result == Some(success::Result::Cancelled(false))
        ));
        assert_eq!(
            handle_operation(&executor, &target, &target.agent_id, key, Operation::Await).await?,
            observed
        );
        assert_eq!(
            executor
                .stored_oplog(&target.agent_id)
                .await
                .iter()
                .filter(|entry| matches!(entry,
                    golem_common::model::oplog::OplogEntry::AgentInvocationStarted { invocation_id, .. }
                        if *invocation_id == key))
                .count(),
            usize::from(!withdrawn)
        );
        drop(worker);
    }
    Ok(())
}

#[test]
#[timeout("4m")]
async fn a_failed_terminal_retains_its_starter_after_eviction_and_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // The fixture needs RPC permission while it runs. Revoke it before any handle read.
    let management = Arc::new(AtomicBool::new(true));
    let mut overrides = denied_management_with_small_recent_index();
    overrides.create_direct_invocation_auth = Some(Arc::new({
        let management = management.clone();
        move || Arc::new(HandleManagementAuth(management.clone()))
    }));
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_key!("CancelTester", "retained-failed-starter");
    let target = executor.start_agent(&component.id, agent.clone()).await?;
    let owner = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "retained-failed-owner"),
        )
        .await?;
    // Complete real methods before making this agent permanently fail.
    for _ in 0..3 {
        executor
            .invoke_and_await_agent(
                &component,
                &agent,
                "inc_after_stdout",
                data_value!("retained-failure-counter", "await"),
            )
            .await?;
    }
    let fingerprint = executor
        .active_agent(&owner)
        .await
        .unwrap()
        .primary()
        .get_initial_worker_metadata()
        .fingerprint;
    let invocation = |key| AgentInvocation::AgentMethod {
        idempotency_key: key,
        method_name: "inc_after_stdout".to_string(),
        input: data_value!("retained-failure-counter", "invalid-mode")
            .into_parts()
            .1
            .try_into()
            .unwrap(),
        allow_streams: false,
        invocation_context: InvocationContextStack::fresh(),
        caller: callers::agent_in(
            owner.agent_id,
            AgentTypeName("ScheduledInvocationClient".to_string()),
            fingerprint,
            target.environment_id,
        ),
        scope_card: None,
    };
    let worker = executor.active_agent(&target).await.unwrap().primary();
    let later = [
        InvocationId(uuid::Uuid::new_v4()),
        InvocationId(uuid::Uuid::new_v4()),
        InvocationId(uuid::Uuid::new_v4()),
    ];
    for key in later {
        worker
            .schedule_invocation(
                invocation(key),
                jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1),
            )
            .await?;
    }
    let failed = InvocationId(uuid::Uuid::new_v4());
    assert!(
        worker
            .clone()
            .invoke_and_await(invocation(failed))
            .await
            .is_err()
    );
    management.store(false, std::sync::atomic::Ordering::SeqCst);
    let recorded = handle_operation(
        &executor,
        &target,
        &owner.agent_id,
        failed,
        Operation::Await,
    )
    .await?;
    assert!(matches!(&recorded, response::Result::Success(value)
        if matches!(value.result, Some(success::Result::InvocationFailure(_)))));
    // Failed agents refuse new methods. Withdraw already accepted future requests instead;
    // their real cancellation terminals evict the failed result from the one-entry index.
    for key in later {
        assert!(matches!(
            handle_operation(&executor, &target, &owner.agent_id, key, Operation::Cancel).await?,
            response::Result::Success(value)
                if value.result == Some(success::Result::Cancelled(true))
        ));
    }
    drop(worker);
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    assert!(matches!(
        handle_operation(&executor, &target, &owner.agent_id, failed, Operation::Status).await?,
        response::Result::Success(value)
            if value.result == Some(success::Result::Status(InvocationLifecycleState::Failed as i32))
    ));
    assert!(matches!(
        handle_operation(&executor, &target, &owner.agent_id, failed, Operation::Cancel).await?,
        response::Result::Success(value)
            if value.result == Some(success::Result::Cancelled(false))
    ));
    assert_eq!(
        handle_operation(
            &executor,
            &target,
            &owner.agent_id,
            failed,
            Operation::Await
        )
        .await?,
        recorded
    );
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        assert!(matches!(
            handle_operation(&executor, &target, &target.agent_id, failed, operation).await?,
            response::Result::Failure(error)
                if matches!(error.error, Some(
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PermissionDenied(_)))
        ));
    }
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &agent_key!("RpcCounter", "retained-failure-counter"),
                "get_value",
                data_value!(),
            )
            .await?
            .into_typed::<u64>()?,
        3,
        "neither the failed method nor its cancelled successors incremented the counter"
    );
    Ok(())
}

fn handle_management(management: &Arc<AtomicBool>) -> TestExecutorOverrides {
    let management = management.clone();
    TestExecutorOverrides {
        create_direct_invocation_auth: Some(Arc::new(move || {
            Arc::new(HandleManagementAuth(management.clone()))
        })),
        ..Default::default()
    }
}

/// Schedules `inc_global_by(amount)` on `target` an hour from now, as `starter`'s call.
async fn schedule_from(
    executor: &TestWorkerExecutor,
    target: &OwnedAgentId,
    starter: &OwnedAgentId,
    amount: u64,
) -> anyhow::Result<InvocationId> {
    let fingerprint = executor.get_worker_metadata(starter).await?.fingerprint;
    let key = InvocationId(uuid::Uuid::new_v4());
    executor
        .active_agent(target)
        .await
        .expect("the target is resident")
        .primary()
        .schedule_invocation(
            AgentInvocation::AgentMethod {
                idempotency_key: key,
                method_name: "inc_global_by".to_string(),
                input: data_value!(amount).into_parts().1.try_into().unwrap(),
                allow_streams: false,
                invocation_context: InvocationContextStack::fresh(),
                caller: callers::agent_in(
                    starter.agent_id,
                    AgentTypeName("ScheduledInvocationClient".to_string()),
                    fingerprint,
                    starter.environment_id,
                ),
                scope_card: None,
            },
            jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1),
        )
        .await?;
    Ok(key)
}

fn is_denied(result: &response::Result) -> bool {
    matches!(result, response::Result::Failure(error)
        if matches!(error.error, Some(
            golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PermissionDenied(_))))
}

/// The retryable failure of a caller whose authority could not be established, which names
/// neither the invocation, its owner nor the cause.
fn is_unestablished_authority(result: &response::Result) -> bool {
    matches!(result, response::Result::Failure(error)
        if matches!(&error.error, Some(
            golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::RuntimeError(error))
            if error.details.contains("authority over the invocation could not be established")))
}

/// A routing failure that sends the caller to the owner's new executor.
fn is_reroute(result: &response::Result) -> bool {
    matches!(result, response::Result::Failure(error)
        if matches!(error.error, Some(
            golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ShardingNotReady(_)
            | golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidShardId(_))))
}

/// The starter is the recorded principal, which names the starter's incarnation: an agent that
/// is deleted and created again at the starter's address is another principal.
#[test]
#[timeout("4m")]
async fn a_recreated_starter_is_not_the_recorded_starter(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let management = Arc::new(AtomicBool::new(false));
    let executor = start_with_overrides(deps, &context, handle_management(&management)).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let server = agent_key!("ScheduledInvocationServer", "recreated-starter-target");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let starter_key = agent_key!("ScheduledInvocationClient", "recreated-starter");
    let starter = executor
        .start_agent(&component.id, starter_key.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let key = schedule_from(&executor, &target, &starter, 29).await?;
    assert!(matches!(
        handle_operation(&executor, &target, &starter.agent_id, key, Operation::Status).await?,
        response::Result::Success(value)
            if value.result == Some(success::Result::Status(InvocationLifecycleState::Scheduled as i32))
    ));

    let original = executor.get_worker_metadata(&starter).await?.fingerprint;
    executor.delete_worker(&starter).await?;
    let recreated = executor.start_agent(&component.id, starter_key).await?;
    assert_eq!(recreated, starter);
    assert_ne!(
        executor.get_worker_metadata(&starter).await?.fingerprint,
        original,
        "the agent at the starter's address is a new incarnation"
    );
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        let result =
            handle_operation(&executor, &target, &starter.agent_id, key, operation).await?;
        assert!(
            is_denied(&result),
            "the successor of the starter must be denied: {operation:?}: {result:?}"
        );
    }
    management.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        matches!(
            handle_operation(&executor, &target, &starter.agent_id, key, Operation::Status).await?,
            response::Result::Success(value)
                if value.result == Some(success::Result::Status(InvocationLifecycleState::Scheduled as i32))
        ),
        "the denied cancel must have left the schedule in place"
    );
    Ok(())
}

/// A starter check that cannot read the invocation's request answers that authority could not
/// be established, to the starter and to anyone else without management permission: it neither
/// denies the starter for good nor tells another caller the cause. A manager does not depend on
/// the check. Once the request is readable, the starter is allowed and another caller is denied.
#[test]
#[timeout("4m")]
async fn a_starter_proof_outage_is_an_operational_failure_not_a_denial(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::{OplogEntry, OplogPayload};

    let context = TestContext::new(last_unique_id);
    let management = Arc::new(AtomicBool::new(false));
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| config.oplog.max_payload_size = 1)),
        ..handle_management(&management)
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let server = agent_key!("ScheduledInvocationServer", "proof-outage-target");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let starter = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "proof-outage-starter"),
        )
        .await?;
    let other = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "proof-outage-other"),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let key = schedule_from(&executor, &target, &starter, 31).await?;
    let request_payload = executor
        .stored_oplog(&target.agent_id)
        .await
        .into_iter()
        .find_map(|entry| match entry {
            OplogEntry::PendingAgentInvocation {
                invocation_id,
                payload: OplogPayload::External { payload_id, .. },
                ..
            } if invocation_id == key => Some(payload_id),
            _ => None,
        })
        .expect("the scheduled request must be stored outside the oplog");
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    executor.set_oplog_download_outage_for_payload(&target.agent_id, request_payload.clone());
    for caller in [&starter, &other] {
        for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
            let result =
                handle_operation(&executor, &target, &caller.agent_id, key, operation).await?;
            assert!(
                is_unestablished_authority(&result),
                "an unreadable request is neither a denial nor a missing invocation: {operation:?}: {result:?}"
            );
        }
    }
    management.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        matches!(
            handle_operation(&executor, &target, &other.agent_id, key, Operation::Status).await?,
            response::Result::Success(value)
                if value.result == Some(success::Result::Status(InvocationLifecycleState::Scheduled as i32))
        ),
        "a manager reads the status while the starter check cannot"
    );
    management.store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(
        executor
            .oplog_download_outage_hits(&target.agent_id)
            .contains(&request_payload)
    );
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    assert!(matches!(
        handle_operation(&executor, &target, &starter.agent_id, key, Operation::Status).await?,
        response::Result::Success(value)
            if value.result == Some(success::Result::Status(InvocationLifecycleState::Scheduled as i32))
    ));
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        let result = handle_operation(&executor, &target, &other.agent_id, key, operation).await?;
        assert!(is_denied(&result), "{operation:?}: {result:?}");
    }
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
            .await?
            .into_typed::<u64>()?,
        0,
        "no operation ran or withdrew the scheduled method"
    );
    Ok(())
}

/// An await whose owner leaves this executor with its shard ends with a routing failure. It
/// does not hang, and it does not answer the invocation as finished or missing.
#[test]
#[timeout("4m")]
async fn an_await_ends_when_its_owner_loses_the_shard(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::shardmanager::{ShardEpochEntry, ShardId};
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        AssignShardsRequest, RevokeShardsRequest, assign_shards_response, revoke_shards_response,
    };
    const TEST_SHARD_MANAGER: &str = "5eed0000-0000-4000-8000-000000000001";

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let mut client = executor.client.clone();
    let assignment = client
        .assign_shards(AssignShardsRequest {
            number_of_shards: 1,
            shard_epochs: vec![ShardEpochEntry {
                shard_id: Some(ShardId { value: 0 }),
                epoch: 1,
            }],
            revision: 1,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
            executor_count: 1,
        })
        .await?
        .into_inner();
    assert!(matches!(
        assignment.result,
        Some(assign_shards_response::Result::Success(_))
    ));
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let server = agent_key!("ScheduledInvocationServer", "shard-loss-target");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let starter = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "shard-loss-starter"),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let key = schedule_from(&executor, &target, &starter, 37).await?;
    let waiting = handle_operation(&executor, &target, &starter.agent_id, key, Operation::Await);
    tokio::pin!(waiting);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut waiting)
            .await
            .is_err(),
        "the await waits for the scheduled invocation"
    );
    let revoked = client
        .revoke_shards(RevokeShardsRequest {
            shard_ids: vec![ShardId { value: 0 }],
            revision: 1,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
            executor_count: 1,
        })
        .await?
        .into_inner();
    assert!(matches!(
        revoked.result,
        Some(revoke_shards_response::Result::Success(_))
    ));
    let ended = tokio::time::timeout(Duration::from_secs(30), waiting).await??;
    assert!(
        is_reroute(&ended),
        "a lost shard is a routing failure, not an answer: {ended:?}"
    );
    Ok(())
}

/// An await that is waiting when an operator interrupts the owner keeps waiting, as the
/// interrupted invocation is still accepted, and follows the owner to the generation that
/// resumes it: it ends with the result that generation commits.
#[test]
#[timeout("3m")]
async fn an_await_across_an_operator_interrupt_ends_with_the_resumed_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_direct_invocation_auth: Some(Arc::new(|| {
                Arc::new(HandleManagementAuth(Arc::new(AtomicBool::new(true))))
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_key!("RpcBlockingCounter", "await-across-interrupt");
    let target = executor.start_agent(&component.id, agent.clone()).await?;
    let promise = executor
        .invoke_and_await_agent(&component, &agent, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let request_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent,
            &request_key,
            "inc_after_promise",
            data_value!(promise.clone()),
        )
        .await?;
    executor
        .wait_for_status(&target, AgentStatus::Suspended, Duration::from_secs(20))
        .await?;
    let entries = executor
        .lookup_invocations(
            &target.agent_id,
            target.environment_id,
            &request_key,
            Some("inc_after_promise"),
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(entries.len(), 1);
    let key = id_of(&entries[0]);

    // The await starts while the owner is suspended, before the interrupt.
    let observing = handle_operation(&executor, &target, &target.agent_id, key, Operation::Await);
    tokio::pin!(observing);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut observing)
            .await
            .is_err()
    );

    executor.interrupt(&target).await?;
    executor
        .wait_for_status(&target, AgentStatus::Interrupted, Duration::from_secs(20))
        .await?;
    // An interrupted invocation is still accepted: the wait must not end here.
    if let Ok(early) = tokio::time::timeout(Duration::from_millis(500), &mut observing).await {
        anyhow::bail!("the await ended on an operator interrupt: {early:?}");
    }
    assert_eq!(
        executor.get_worker_metadata(&target).await?.status,
        AgentStatus::Interrupted,
        "the await must not resume the interrupted owner"
    );

    executor
        .complete_promise(&target.environment_id, &promise, vec![])
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "get_value", data_value!())
            .await?
            .into_typed::<u64>()?,
        7
    );
    let observed = tokio::time::timeout(Duration::from_secs(30), observing)
        .await
        .map_err(|_| anyhow::anyhow!("the await did not end after the invocation completed"))??;
    let response::Result::Success(value) = &observed else {
        anyhow::bail!("observation failed: {observed:?}");
    };
    let Some(success::Result::Output(output)) = &value.result else {
        anyhow::bail!("expected a completed result: {value:?}");
    };
    assert_eq!(
        output.result,
        Some(invocation_session_result::Result::MethodResult(
            golem_common::schema::SchemaValue::U64(7)
                .try_into()
                .unwrap()
        ))
    );
    Ok(())
}

/// An await that followed its owner across an operator interrupt answers not found once the
/// owner is deleted.
#[test]
#[timeout("3m")]
async fn an_await_across_an_operator_interrupt_ends_as_not_found_when_the_owner_is_deleted(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_direct_invocation_auth: Some(Arc::new(|| {
                Arc::new(HandleManagementAuth(Arc::new(AtomicBool::new(true))))
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_key!("RpcBlockingCounter", "await-across-interrupt-deleted");
    let target = executor.start_agent(&component.id, agent.clone()).await?;
    let promise = executor
        .invoke_and_await_agent(&component, &agent, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let request_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent,
            &request_key,
            "inc_after_promise",
            data_value!(promise.clone()),
        )
        .await?;
    executor
        .wait_for_status(&target, AgentStatus::Suspended, Duration::from_secs(20))
        .await?;
    let entries = executor
        .lookup_invocations(
            &target.agent_id,
            target.environment_id,
            &request_key,
            Some("inc_after_promise"),
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(entries.len(), 1);
    let key = id_of(&entries[0]);

    // The await starts while the owner is suspended, before the interrupt.
    let observing = handle_operation(&executor, &target, &target.agent_id, key, Operation::Await);
    tokio::pin!(observing);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut observing)
            .await
            .is_err()
    );

    executor.interrupt(&target).await?;
    executor
        .wait_for_status(&target, AgentStatus::Interrupted, Duration::from_secs(20))
        .await?;
    // An interrupted invocation is still accepted: the wait must not end here.
    if let Ok(early) = tokio::time::timeout(Duration::from_millis(500), &mut observing).await {
        anyhow::bail!("the await ended on an operator interrupt: {early:?}");
    }
    assert_eq!(
        executor.get_worker_metadata(&target).await?.status,
        AgentStatus::Interrupted,
        "the await must not resume the interrupted owner"
    );

    executor.delete_worker(&target).await?;
    let observed = tokio::time::timeout(Duration::from_secs(30), observing)
        .await
        .map_err(|_| anyhow::anyhow!("the await did not end after the owner was deleted"))??;
    assert!(
        matches!(&observed, response::Result::Success(value)
            if matches!(value.result, Some(success::Result::NotFound(_)))),
        "the deleted owner's invocation is not found: {observed:?}"
    );
    Ok(())
}

/// A finished invocation whose withdrawal is written again after its result left the recent
/// results keeps its starter: the starter check reads the request from the older result index,
/// since the withdrawal names none.
#[test]
#[timeout("4m")]
async fn a_withdrawal_written_again_keeps_the_starters_authority(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::OplogEntry;

    let context = TestContext::new(last_unique_id);
    let management = Arc::new(AtomicBool::new(false));
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.invocation_results.recent_capacity = 1;
            config.invocation_results.bloom_bits = 8;
            config.invocation_results.physical_index_catch_up_chunk_size = 1;
            config.invocation_results.hydrated_cache_capacity = 0;
        })),
        ..handle_management(&management)
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let server = agent_key!("ScheduledInvocationServer", "withdrawn-again-target");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let starter = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "withdrawn-again-starter"),
        )
        .await?;
    let other = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "withdrawn-again-other"),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let caller = callers::agent_in(
        starter.agent_id,
        AgentTypeName("ScheduledInvocationClient".to_string()),
        executor.get_worker_metadata(&starter).await?.fingerprint,
        starter.environment_id,
    );
    let invocation = |key: InvocationId| AgentInvocation::AgentMethod {
        idempotency_key: key,
        method_name: "inc_global_by".to_string(),
        input: data_value!(1u64).into_parts().1.try_into().unwrap(),
        allow_streams: false,
        invocation_context: InvocationContextStack::fresh(),
        caller: caller.clone(),
        scope_card: None,
    };
    let worker = executor.active_agent(&target).await.unwrap().primary();
    let finished = InvocationId(uuid::Uuid::new_v4());
    worker
        .clone()
        .invoke_and_await(invocation(finished))
        .await?;
    // Later results evict the finished one from the one-entry recent results.
    let due = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1);
    for _ in 0..3 {
        let other_key = InvocationId(uuid::Uuid::new_v4());
        worker
            .schedule_invocation(invocation(other_key), due)
            .await?;
        assert_eq!(
            worker.cancel_invocation(other_key).await?,
            golem_worker_executor::worker::CancelInvocationOutcome::Withdrawn
        );
    }
    let completed = |result: &response::Result| {
        matches!(result, response::Result::Success(value)
            if value.result == Some(success::Result::Status(InvocationLifecycleState::Completed as i32)))
    };
    let before = handle_operation(
        &executor,
        &target,
        &starter.agent_id,
        finished,
        Operation::Status,
    )
    .await?;
    assert!(completed(&before), "before the withdrawal: {before:?}");

    // The withdrawal of a key that only the Bloom filter may hold names no request.
    worker
        .add_and_commit_oplog(OplogEntry::cancel_pending_invocation(finished))
        .await?;
    drop(worker);

    let status = handle_operation(
        &executor,
        &target,
        &starter.agent_id,
        finished,
        Operation::Status,
    )
    .await?;
    assert!(completed(&status), "after the withdrawal: {status:?}");
    let awaited = handle_operation(
        &executor,
        &target,
        &starter.agent_id,
        finished,
        Operation::Await,
    )
    .await?;
    assert!(
        matches!(&awaited, response::Result::Success(value)
            if matches!(value.result, Some(success::Result::Output(_)))),
        "the starter awaits the recorded result: {awaited:?}"
    );
    let denied = handle_operation(
        &executor,
        &target,
        &other.agent_id,
        finished,
        Operation::Status,
    )
    .await?;
    assert!(is_denied(&denied), "{denied:?}");
    Ok(())
}

/// A revert keeps an invocation that had not started in the reverted history pending, so an
/// await of it keeps waiting. An invocation that started there leaves the agent's history, and
/// its await answers the authorized caller that it is not found.
#[test]
#[timeout("4m")]
async fn an_await_answers_not_found_when_a_revert_removes_its_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let management = Arc::new(AtomicBool::new(true));
    let executor = start_with_overrides(deps, &context, handle_management(&management)).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let revert_to = |cut: u64| {
        golem_common::model::agent::RevertWorkerTarget::RevertToOplogIndex(
            golem_common::model::agent::RevertToOplogIndex {
                last_oplog_index: golem_common::model::oplog::OplogIndex::from_u64(cut),
            },
        )
    };

    // A scheduled invocation that has not started stays pending across the revert.
    let server = agent_key!("ScheduledInvocationServer", "revert-keeps-target");
    let scheduled_target = executor.start_agent(&component.id, server.clone()).await?;
    let starter = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "revert-keeps-starter"),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let cut = executor.get_oplog_last_index(&scheduled_target).await?;
    let scheduled = schedule_from(&executor, &scheduled_target, &starter, 41).await?;
    let mut waiting = Box::pin(handle_operation(
        &executor,
        &scheduled_target,
        &starter.agent_id,
        scheduled,
        Operation::Await,
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut waiting)
            .await
            .is_err()
    );
    executor.revert(&scheduled_target, revert_to(cut)).await?;
    if let Ok(early) = tokio::time::timeout(Duration::from_secs(2), &mut waiting).await {
        anyhow::bail!("the await of an invocation still pending ended: {early:?}");
    }
    drop(waiting);
    assert!(matches!(
        handle_operation(&executor, &scheduled_target, &starter.agent_id, scheduled, Operation::Status).await?,
        response::Result::Success(value)
            if value.result == Some(success::Result::Status(InvocationLifecycleState::Scheduled as i32))
    ));

    // An invocation that started after the cut is removed with the reverted history.
    let agent = agent_key!("RpcBlockingCounter", "revert-removes-started");
    let target = executor.start_agent(&component.id, agent.clone()).await?;
    let promise = executor
        .invoke_and_await_agent(&component, &agent, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let cut = executor.get_oplog_last_index(&target).await?;
    let request_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent,
            &request_key,
            "inc_after_promise",
            data_value!(promise),
        )
        .await?;
    executor
        .wait_for_status(&target, AgentStatus::Suspended, Duration::from_secs(20))
        .await?;
    let entries = executor
        .lookup_invocations(
            &target.agent_id,
            target.environment_id,
            &request_key,
            Some("inc_after_promise"),
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(entries.len(), 1);
    let key = id_of(&entries[0]);
    let waiting = handle_operation(&executor, &target, &target.agent_id, key, Operation::Await);
    tokio::pin!(waiting);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut waiting)
            .await
            .is_err(),
        "the await waits for the suspended invocation"
    );
    executor.revert(&target, revert_to(cut)).await?;
    let ended = tokio::time::timeout(Duration::from_secs(30), waiting)
        .await
        .map_err(|_| anyhow::anyhow!("the await did not end after the revert"))??;
    assert!(
        matches!(&ended, response::Result::Success(value)
            if matches!(value.result, Some(success::Result::NotFound(_)))),
        "the reverted invocation is not found: {ended:?}"
    );
    let status =
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Status).await?;
    assert!(
        matches!(&status, response::Result::Success(value)
            if matches!(value.result, Some(success::Result::NotFound(_)))),
        "the status agrees: {status:?}"
    );
    Ok(())
}

#[test]
#[timeout("4m")]
async fn an_await_of_a_background_failed_invocation_returns_its_result_with_the_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_direct_invocation_auth: Some(Arc::new(|| {
                Arc::new(HandleManagementAuth(Arc::new(AtomicBool::new(true))))
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_key!("BackgroundWork", "handle-background-failure");
    let target = executor.start_agent(&component.id, agent.clone()).await?;
    let release = executor
        .invoke_and_await_agent(&component, &agent, "create_release", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let returned = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "trap_after_release",
            data_value!(release.clone()),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(returned, "returned");
    let listed = executor
        .list_invocations(
            &target.agent_id,
            target.environment_id,
            ListInvocationsRequest {
                page_size: 100,
                ..Default::default()
            },
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let key = id_of(
        listed
            .invocations
            .iter()
            .find(|entry| entry.operation.as_deref() == Some("trap_after_release"))
            .expect("the accepted invocation"),
    );

    executor
        .complete_promise(&component.environment_id, &release, vec![1])
        .await?;
    executor
        .wait_for_status(&target, AgentStatus::Failed, Duration::from_secs(30))
        .await?;

    // The agent gave up after the invocation recorded its result: the await answers that result
    // together with the background failure.
    let awaited =
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Await).await;
    let response::Result::Success(awaited) = awaited? else {
        panic!("the await must succeed");
    };
    let Some(success::Result::Output(output)) = awaited.result else {
        panic!("the await must answer the recorded result, got {awaited:?}");
    };
    assert!(
        output.background_failure.is_some(),
        "the await carries the background failure"
    );
    assert!(matches!(
        output.result,
        Some(invocation_session_result::Result::MethodResult(_))
    ));
    assert_eq!(
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Status).await?,
        response::Result::Success(
            golem_api_grpc::proto::golem::workerexecutor::v1::InvocationHandleSuccess {
                result: Some(success::Result::Status(
                    InvocationLifecycleState::BackgroundFailed as i32
                )),
            }
        )
    );
    assert_eq!(
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Cancel).await?,
        response::Result::Success(
            golem_api_grpc::proto::golem::workerexecutor::v1::InvocationHandleSuccess {
                result: Some(success::Result::Cancelled(false)),
            }
        )
    );
    Ok(())
}
