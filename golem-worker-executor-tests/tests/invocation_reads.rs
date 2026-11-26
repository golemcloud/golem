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

// The executor's invocation reads: `GetInvocation`, `ListInvocations` and `LookupInvocations`.

use crate::Tracing;

use golem_api_grpc::proto::golem::worker::Cursor;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    InvocationKind, InvocationLifecycleState, InvocationSummary, ListInvocationsRequest,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{AgentInvocation, IdempotencyKey, InvocationId};
use golem_common::{agent_key, data_value};
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, callers, start, start_with_overrides,
};
use pretty_assertions::assert_eq;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_sdk_rust")]
    PrecompiledComponent
);

fn id_of(summary: &InvocationSummary) -> InvocationId {
    summary.invocation_id.unwrap().into()
}

fn state_of(summary: &InvocationSummary) -> InvocationLifecycleState {
    InvocationLifecycleState::try_from(summary.state).unwrap()
}

mod authority_surfaces;
mod deleted_handle_authority;
mod deleting_handle_authority;
mod lifecycle_boundaries;

/// Grants the management permissions of invocation handles while its flag is set, and denies
/// every check otherwise.
pub(crate) struct HandleManagementAuth(pub(crate) std::sync::Arc<std::sync::atomic::AtomicBool>);

#[async_trait::async_trait]
impl golem_worker_executor::services::direct_invocation_auth::DirectInvocationAuthService
    for HandleManagementAuth
{
    async fn check(
        &self,
        account: AccountId,
        _: &golem_common::model::OwnedAgentId,
        _: golem_common::model::component::ComponentId,
        _: Option<AgentTypeName>,
        _: golem_common::model::card::AgentVerb,
        _: golem_common::model::card::AgentResourcePattern,
        _: &AuthCtx,
    ) -> Result<
        golem_worker_executor::services::direct_invocation_auth::EnvironmentOwnerAccountId,
        golem_worker_executor::services::rpc::RpcError,
    > {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(
                golem_worker_executor::services::direct_invocation_auth::EnvironmentOwnerAccountId(
                    account,
                ),
            )
        } else {
            Err(golem_worker_executor::services::rpc::RpcError::Denied {
                details: "no management permission".to_string(),
            })
        }
    }
}

/// The principal that the executor gives the current incarnation of `agent` as a caller: the
/// pair of its address and fingerprint, which is what a starter check compares. An agent that
/// does not exist gets the principal of no incarnation.
pub(crate) async fn agent_principal(
    executor: &TestWorkerExecutor,
    agent: &golem_common::model::OwnedAgentId,
) -> anyhow::Result<golem_common::model::caller::Principal> {
    let fingerprint = match executor.get_worker_metadata_opt(agent).await? {
        Some(metadata) => metadata.fingerprint,
        None => golem_common::model::AgentFingerprint(uuid::Uuid::nil()),
    };
    Ok(callers::agent_in(
        agent.agent_id,
        AgentTypeName("caller".to_string()),
        fingerprint,
        agent.environment_id,
    )
    .immediate)
}

pub(crate) async fn handle_operation(
    executor: &TestWorkerExecutor,
    target: &golem_common::model::OwnedAgentId,
    caller: &golem_common::model::AgentId,
    key: InvocationId,
    operation: golem_api_grpc::proto::golem::workerexecutor::v1::InvocationHandleOperation,
) -> anyhow::Result<
    golem_api_grpc::proto::golem::workerexecutor::v1::invocation_handle_response::Result,
> {
    let caller = agent_principal(
        executor,
        &golem_common::model::OwnedAgentId::new(target.environment_id, caller),
    )
    .await?;
    let response = executor
        .client
        .clone()
        .invocation_handle(
            golem_api_grpc::proto::golem::workerexecutor::v1::InvocationHandleRequest {
                agent_id: Some(target.agent_id.into()),
                environment_id: Some(target.environment_id.into()),
                invocation_id: Some(key.0.into()),
                caller: Some(caller.into()),
                auth_ctx: Some(AuthCtx::System.into()),
                operation: operation as i32,
            },
        )
        .await?
        .into_inner();
    Ok(response.result.expect("a handle operation result"))
}

#[test]
#[timeout("4m")]
async fn invocation_handles_observe_schedules_and_authorize_the_persisted_starter(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        InvocationHandleOperation as Operation, invocation_handle_response as response,
        invocation_handle_success as success,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let context = TestContext::new(last_unique_id);
    let management = Arc::new(AtomicBool::new(false));
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_direct_invocation_auth: Some(Arc::new({
                let management = management.clone();
                move || Arc::new(HandleManagementAuth(management.clone()))
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let server = agent_key!("ScheduledInvocationServer", "handle-observer");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let owner = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "handle-owner"),
        )
        .await?;
    let other = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "handle-other"),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let worker = executor.active_agent(&target).await.unwrap().primary();
    let owner_fingerprint = executor
        .active_agent(&owner)
        .await
        .unwrap()
        .primary()
        .get_initial_worker_metadata()
        .fingerprint;
    let key = InvocationId(uuid::Uuid::new_v4());
    worker
        .schedule_invocation(
            AgentInvocation::AgentMethod {
                idempotency_key: key,
                method_name: "inc_global_by".to_string(),
                input: data_value!(17u64).into_parts().1.try_into().unwrap(),
                allow_streams: false,
                invocation_context: InvocationContextStack::fresh(),
                caller: callers::agent_in(
                    owner.agent_id,
                    AgentTypeName("ScheduledInvocationClient".to_string()),
                    owner_fingerprint,
                    target.environment_id,
                ),
                scope_card: None,
            },
            jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1),
        )
        .await?;
    drop(worker);

    assert_eq!(
        handle_operation(&executor, &target, &owner.agent_id, key, Operation::Status).await?,
        response::Result::Success(
            golem_api_grpc::proto::golem::workerexecutor::v1::InvocationHandleSuccess {
                result: Some(success::Result::Status(
                    InvocationLifecycleState::Scheduled as i32
                )),
            }
        )
    );
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        assert!(
            matches!(
                handle_operation(&executor, &target, &other.agent_id, key, operation).await?,
                response::Result::Failure(error)
                    if matches!(error.error, Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PermissionDenied(_)))
            ),
            "an unrelated ID holder must be denied"
        );
    }

    // Poll the observer before cancellation. It must neither run the future entry nor return
    // a pending invocation as not-found. Cancelling a second observer only detaches that read.
    let observing = handle_operation(&executor, &target, &owner.agent_id, key, Operation::Await);
    tokio::pin!(observing);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), &mut observing)
            .await
            .is_err()
    );
    {
        let detached = handle_operation(&executor, &target, &owner.agent_id, key, Operation::Await);
        tokio::pin!(detached);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), &mut detached)
                .await
                .is_err()
        );
    }
    let value = executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(value, 0, "observation must not execute future work");
    assert!(
        matches!(handle_operation(&executor, &target, &owner.agent_id, key, Operation::Cancel).await?,
        response::Result::Success(value) if value.result == Some(success::Result::Cancelled(true)))
    );
    assert!(matches!(observing.await?, response::Result::Success(value)
        if matches!(value.result, Some(success::Result::InvocationFailure(_)))));
    assert!(
        matches!(handle_operation(&executor, &target, &owner.agent_id, key, Operation::Cancel).await?,
        response::Result::Success(value) if value.result == Some(success::Result::Cancelled(false)))
    );

    management.store(true, Ordering::SeqCst);
    assert!(
        matches!(handle_operation(&executor, &target, &other.agent_id, key, Operation::Status).await?,
        response::Result::Success(value) if value.result == Some(success::Result::Status(InvocationLifecycleState::Cancelled as i32)))
    );
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        assert!(
            matches!(handle_operation(&executor, &target, &owner.agent_id,
            InvocationId(uuid::Uuid::new_v4()), operation).await?, response::Result::Success(value)
            if matches!(value.result, Some(success::Result::NotFound(_))))
        );
    }
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
            .await?
            .into_typed::<u64>()?,
        0
    );
    let deleted_key = InvocationId(uuid::Uuid::new_v4());
    executor
        .active_agent(&target)
        .await
        .unwrap()
        .primary()
        .schedule_invocation(
            AgentInvocation::AgentMethod {
                idempotency_key: deleted_key,
                method_name: "inc_global_by".to_string(),
                input: data_value!(31u64).into_parts().1.try_into().unwrap(),
                allow_streams: false,
                invocation_context: InvocationContextStack::fresh(),
                caller: callers::agent_in(
                    owner.agent_id,
                    AgentTypeName("ScheduledInvocationClient".to_string()),
                    owner_fingerprint,
                    target.environment_id,
                ),
                scope_card: None,
            },
            jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1),
        )
        .await?;
    let deleted_wait = handle_operation(
        &executor,
        &target,
        &owner.agent_id,
        deleted_key,
        Operation::Await,
    );
    tokio::pin!(deleted_wait);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), &mut deleted_wait)
            .await
            .is_err()
    );
    executor.delete_worker(&target).await?;
    let deleted = tokio::time::timeout(std::time::Duration::from_secs(10), deleted_wait).await??;
    assert!(
        matches!(&deleted, response::Result::Success(value)
            if matches!(value.result, Some(success::Result::NotFound(_)))),
        "a wait that the owner's deletion ends answers the starter not found: {deleted:?}"
    );
    assert!(executor.get_worker_metadata(&target).await.is_err());
    executor.start_agent(&component.id, server.clone()).await?;
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        assert!(matches!(
            handle_operation(&executor, &target, &owner.agent_id, deleted_key, operation).await?,
            response::Result::Success(value)
                if matches!(value.result, Some(success::Result::NotFound(_)))
        ));
    }
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
            .await?
            .into_typed::<u64>()?,
        0
    );
    Ok(())
}

#[test]
#[timeout("4m")]
async fn invocation_handle_wait_does_not_resume_an_interrupted_target(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::worker::invocation_session_result;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        InvocationHandleOperation as Operation, invocation_handle_response as response,
        invocation_handle_success as success,
    };
    use golem_common::model::{AgentStatus, PromiseId};
    use std::sync::{Arc, atomic::AtomicBool};
    use std::time::Duration;

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
    let agent = agent_key!("RpcBlockingCounter", "interrupted-handle-observer");
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
    assert_eq!(state_of(&entries[0]), InvocationLifecycleState::Waiting);
    assert!(matches!(
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Cancel).await?,
        response::Result::Failure(error) if matches!(error.error, Some(
            golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::CannotCancelRunningInvocation(_)))
    ));
    executor.interrupt(&target).await?;
    executor
        .wait_for_status(&target, AgentStatus::Interrupted, Duration::from_secs(20))
        .await?;
    let loads = executor.instance_load_count(&target.agent_id);
    let observing = handle_operation(&executor, &target, &target.agent_id, key, Operation::Await);
    tokio::pin!(observing);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut observing)
            .await
            .is_err(),
        "an interrupted invocation is still accepted, not a recorded failure"
    );
    assert_eq!(
        executor.get_worker_metadata(&target).await?.status,
        AgentStatus::Interrupted
    );
    assert_eq!(executor.instance_load_count(&target.agent_id), loads);
    // Normal promise completion, not the observation, is what resumes the target.
    executor
        .complete_promise(&target.environment_id, &promise, vec![])
        .await?;
    let observed = tokio::time::timeout(Duration::from_secs(30), observing).await??;
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
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "get_value", data_value!())
            .await?
            .into_typed::<u64>()?,
        7
    );
    assert!(matches!(
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Cancel).await?,
        response::Result::Success(value)
            if value.result == Some(success::Result::Cancelled(false))
    ));
    assert_eq!(
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Await).await?,
        observed
    );
    Ok(())
}

#[test]
#[timeout("4m")]
async fn invocation_handle_result_outage_is_not_a_recorded_failure_or_missing_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        InvocationHandleOperation as Operation, invocation_handle_response as response,
        invocation_handle_success as success,
    };
    use golem_common::model::oplog::{OplogEntry, OplogPayload};
    use std::sync::Arc;
    use std::time::Duration;

    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| config.oplog.max_payload_size = 1)),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_key!("ScheduledInvocationServer", "handle-result-outage");
    let target = executor.start_agent(&component.id, agent.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "get_global_value", data_value!())
        .await?;
    let worker = executor.active_agent(&target).await.unwrap().primary();
    let key = InvocationId(uuid::Uuid::new_v4());
    worker
        .clone()
        .invoke(AgentInvocation::AgentMethod {
            idempotency_key: key,
            method_name: "inc_global_by".to_string(),
            input: data_value!(19u64).into_parts().1.try_into().unwrap(),
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
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if worker.get_invocation(&key).await?.is_some_and(|entry| {
                entry.state
                    == golem_worker_executor::worker::invocations::InvocationState::Completed
            }) {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let result_payload = executor
        .stored_oplog(&target.agent_id)
        .await
        .into_iter()
        .find_map(|entry| match entry {
            OplogEntry::AgentInvocationFinished {
                invocation_id,
                result: OplogPayload::External { payload_id, .. },
                ..
            } if invocation_id == key => Some(payload_id),
            _ => None,
        })
        .expect("the result must be stored outside the oplog");
    drop(worker);
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    executor.set_oplog_download_outage_for_payload(&target.agent_id, result_payload.clone());
    let unavailable =
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Await).await?;
    assert!(
        matches!(&unavailable, response::Result::Failure(error)
            if !matches!(error.error, Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PermissionDenied(_))
                | Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::AgentNotFound(_)))),
        "an unreadable result is an operational failure, not a denial or a missing invocation: {unavailable:?}"
    );
    assert!(
        executor
            .oplog_download_outage_hits(&target.agent_id)
            .contains(&result_payload)
    );
    assert!(matches!(
        handle_operation(&executor, &target, &target.agent_id,
            InvocationId(uuid::Uuid::new_v4()), Operation::Await).await?,
        response::Result::Success(value) if matches!(value.result, Some(success::Result::NotFound(_)))
    ));
    // The same record becomes readable again; an outage must not replace its terminal.
    executor.set_oplog_download_outage(&target.agent_id, false);
    let available =
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Await).await?;
    assert!(matches!(available,
        response::Result::Success(value) if matches!(value.result, Some(success::Result::Output(_)))));
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "get_global_value", data_value!())
            .await?
            .into_typed::<u64>()?,
        19
    );
    Ok(())
}

/// Get and list name each invocation with its state, kind and operation; pages of two list
/// every invocation once, newest change first; cursors and filters are checked.
#[test]
#[timeout("4m")]
async fn invocations_are_read_by_id_and_listed_in_pages(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server = agent_key!("ScheduledInvocationServer", "invocation-reads");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let owner = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "invocation-reads-owner"),
        )
        .await?;
    let env = component.environment_id;
    let invocation = |key: &InvocationId| AgentInvocation::AgentMethod {
        idempotency_key: *key,
        method_name: "inc_global_by".to_string(),
        input: data_value!(1u64).into_parts().1.try_into().unwrap(),
        allow_streams: false,
        invocation_context: InvocationContextStack::fresh(),
        caller: callers::agent(
            owner.agent_id,
            AgentTypeName("ScheduledInvocationClient".to_string()),
        ),
        scope_card: None,
    };
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let owned = target.clone();
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    let due = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1);

    let cancelled = InvocationId(uuid::Uuid::new_v4());
    worker
        .schedule_invocation(invocation(&cancelled), due)
        .await?;
    assert_eq!(
        worker.cancel_invocation(cancelled).await?,
        golem_worker_executor::worker::CancelInvocationOutcome::Withdrawn
    );
    let scheduled = InvocationId(uuid::Uuid::new_v4());
    worker
        .schedule_invocation(invocation(&scheduled), due)
        .await?;
    drop(worker);

    // Get names the state, kind and operation; an unknown ID is no invocation, not an error.
    let got = executor
        .get_invocation(&target.agent_id, env, scheduled)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
        .expect("the scheduled invocation");
    assert_eq!(state_of(&got), InvocationLifecycleState::Scheduled);
    assert_eq!(got.kind, InvocationKind::Method as i32);
    assert_eq!(got.operation.as_deref(), Some("inc_global_by"));
    let cancelled_got = executor
        .get_invocation(&target.agent_id, env, cancelled)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
        .expect("the cancelled invocation");
    assert_eq!(
        state_of(&cancelled_got),
        InvocationLifecycleState::Cancelled
    );
    assert_eq!(
        executor
            .get_invocation(&target.agent_id, env, InvocationId(uuid::Uuid::new_v4()))
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        None
    );

    // The whole list: the constructor run, the awaited call, the cancelled and the scheduled one.
    let all = executor
        .list_invocations(
            &target.agent_id,
            env,
            ListInvocationsRequest {
                page_size: 100,
                ..Default::default()
            },
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(all.next_cursor, None);
    let ids: Vec<InvocationId> = all.invocations.iter().map(id_of).collect();
    assert_eq!(ids.len(), 4, "{:?}", all.invocations);
    assert_eq!(ids[0], scheduled);
    assert_eq!(ids[1], cancelled);
    let fingerprint = executor.get_worker_metadata(&target).await?.fingerprint;
    let constructor_id = InvocationId(uuid::Uuid::new_v5(&fingerprint.0, b"constructor"));
    assert!(all.invocations.iter().any(|summary| summary.kind
        == InvocationKind::Constructor as i32
        && id_of(summary) == constructor_id
        && state_of(summary) == InvocationLifecycleState::Completed
        && summary.operation.is_none()));
    let constructor = executor
        .get_invocation(&target.agent_id, env, constructor_id)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
        .expect("the constructor is readable by its fingerprint-derived ID");
    assert_eq!(constructor.kind, InvocationKind::Constructor as i32);
    assert_eq!(state_of(&constructor), InvocationLifecycleState::Completed);

    // Pages of two give the same invocations in the same order, each once.
    let mut paged = Vec::new();
    let mut cursor: Option<Cursor> = None;
    loop {
        let page = executor
            .list_invocations(
                &target.agent_id,
                env,
                ListInvocationsRequest {
                    page_size: 2,
                    cursor: cursor.clone(),
                    ..Default::default()
                },
            )
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        assert!(page.invocations.len() <= 2);
        paged.extend(page.invocations.iter().map(id_of));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(paged, ids);

    // Filters by state and kind.
    let methods_in_flight = executor
        .list_invocations(
            &target.agent_id,
            env,
            ListInvocationsRequest {
                page_size: 100,
                states: vec![
                    InvocationLifecycleState::Scheduled as i32,
                    InvocationLifecycleState::Cancelled as i32,
                ],
                kind: Some(InvocationKind::Method as i32),
                ..Default::default()
            },
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(
        methods_in_flight
            .invocations
            .iter()
            .map(id_of)
            .collect::<Vec<_>>(),
        vec![scheduled, cancelled]
    );

    // A cursor of another agent, and a malformed one, are invalid requests.
    let first_page = executor
        .list_invocations(
            &target.agent_id,
            env,
            ListInvocationsRequest {
                page_size: 1,
                ..Default::default()
            },
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let foreign = first_page.next_cursor.expect("more than one invocation");
    for bad in [
        foreign,
        Cursor {
            value: "not-a-cursor".to_string(),
        },
    ] {
        let refused = executor
            .list_invocations(
                &owner.agent_id,
                env,
                ListInvocationsRequest {
                    page_size: 1,
                    cursor: Some(bad.clone()),
                    ..Default::default()
                },
            )
            .await?;
        let error = refused.expect_err("an invalid cursor is refused");
        assert!(
            format!("{error:?}").contains("cursor"),
            "{bad:?}: {error:?}"
        );
    }

    // A deleted agent has no invocations to read.
    executor.delete_worker(&target).await?;
    let gone = executor
        .get_invocation(&target.agent_id, env, scheduled)
        .await?;
    assert!(format!("{gone:?}").contains("AgentNotFound"), "{gone:?}");
    Ok(())
}

/// Lookup finds the caller's keyed invocations on every operation, narrows to one operation,
/// and finds nothing for another caller or a caller without a scope.
#[test]
#[timeout("4m")]
async fn invocations_are_looked_up_by_the_callers_key(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server = agent_key!("ScheduledInvocationServer", "invocation-lookup");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let env = component.environment_id;

    let key = IdempotencyKey::fresh();
    executor
        .invoke_and_await_agent_with_key(
            &component,
            &server,
            &key,
            "inc_global_by",
            data_value!(3u64),
        )
        .await?;
    executor
        .invoke_and_await_agent_with_key(
            &component,
            &server,
            &key,
            "get_global_value",
            data_value!(),
        )
        .await?;

    let operations = |found: &[InvocationSummary]| {
        let mut operations: Vec<String> = found
            .iter()
            .map(|summary| {
                assert_eq!(state_of(summary), InvocationLifecycleState::Completed);
                summary.operation.clone().unwrap()
            })
            .collect();
        operations.sort();
        operations
    };
    let found = executor
        .lookup_invocations(&target.agent_id, env, &key, None)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(
        operations(&found),
        vec!["get_global_value", "inc_global_by"]
    );
    let narrowed = executor
        .lookup_invocations(&target.agent_id, env, &key, Some("inc_global_by"))
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(operations(&narrowed), vec!["inc_global_by"]);
    // Each found invocation is the one get reads by its ID.
    let by_id = executor
        .get_invocation(&target.agent_id, env, id_of(&narrowed[0]))
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(by_id.as_ref(), Some(&narrowed[0]));
    // A key nobody used finds nothing.
    assert_eq!(
        executor
            .lookup_invocations(&target.agent_id, env, &IdempotencyKey::fresh(), None)
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        Vec::new()
    );

    let AuthCtx::User(user) = executor.auth_ctx() else {
        anyhow::bail!("the test executor authenticates as a user");
    };
    let mut other = user.clone();
    other.account_id = AccountId::new();
    for caller in [AuthCtx::User(other), AuthCtx::System] {
        let found = executor
            .as_caller(caller.clone())
            .lookup_invocations(&target.agent_id, env, &key, None)
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        assert_eq!(found, Vec::new(), "{caller:?}");
    }

    // After a delete and re-create the key finds nothing: the new incarnation derives other IDs.
    executor.delete_worker(&target).await?;
    executor.start_agent(&component.id, server.clone()).await?;
    assert_eq!(
        executor
            .lookup_invocations(&target.agent_id, env, &key, None)
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        Vec::new()
    );
    Ok(())
}

/// An ephemeral agent's finished invocation is listed after its instance is gone: the agent is
/// reconstructed from its oplog, and its few results stay in the exact recent results.
#[test]
#[timeout("4m")]
async fn a_finished_ephemeral_invocation_is_listed_after_its_instance_is_gone(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let ephemeral = agent_key!("ScheduledEphemeralTarget", "invocation-reads-ephemeral");
    let key = IdempotencyKey::fresh();
    let result = executor
        .invoke_and_await_agent_with_key(&component, &ephemeral, &key, "inc", data_value!(0u64))
        .await?;
    let agent = result.agent_id().clone();
    let env = component.environment_id;

    let listed = executor
        .list_invocations(
            &agent.agent_id,
            env,
            ListInvocationsRequest {
                page_size: 100,
                ..Default::default()
            },
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(listed.invocations.len(), 2, "{:?}", listed.invocations);
    let constructor = &listed.invocations[1];
    assert_eq!(constructor.kind, InvocationKind::Constructor as i32);
    assert_eq!(state_of(constructor), InvocationLifecycleState::Completed);
    let inc = &listed.invocations[0];
    assert_eq!(state_of(inc), InvocationLifecycleState::Completed);
    assert_eq!(inc.operation.as_deref(), Some("inc"));
    let found = executor
        .lookup_invocations(&agent.agent_id, env, &key, None)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(found, vec![inc.clone()]);
    Ok(())
}

#[test]
#[timeout("4m")]
async fn cancelling_a_failed_invocation_handle_preserves_its_recorded_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        InvocationHandleOperation as Operation, invocation_handle_response as response,
        invocation_handle_success as success,
    };
    use std::sync::{Arc, atomic::AtomicBool};

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
    let agent = agent_key!("CancelTester", "failed-handle");
    let target = executor.start_agent(&component.id, agent.clone()).await?;
    // This existing fixture deliberately panics for an unknown RPC mode.
    let failed = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "inc_after_stdout",
            data_value!("failure-counter", "invalid-mode"),
        )
        .await;
    assert!(failed.is_err(), "the fixture must have failed");
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
    let invocation = listed
        .invocations
        .iter()
        .find(|entry| entry.operation.as_deref() == Some("inc_after_stdout"))
        .expect("the accepted failed invocation");
    assert_eq!(state_of(invocation), InvocationLifecycleState::Failed);
    let key = id_of(invocation);
    let recorded_failure =
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Await).await?;
    assert!(matches!(
        &recorded_failure,
        response::Result::Success(value)
            if matches!(value.result, Some(success::Result::InvocationFailure(_)))
    ));
    let cancelled =
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Cancel).await?;
    assert!(
        matches!(cancelled,
        response::Result::Success(ref value)
            if value.result == Some(success::Result::Cancelled(false))),
        "a recorded failed terminal must be a cancellation no-op, got {cancelled:?}"
    );
    assert_eq!(
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Status).await?,
        response::Result::Success(
            golem_api_grpc::proto::golem::workerexecutor::v1::InvocationHandleSuccess {
                result: Some(success::Result::Status(
                    InvocationLifecycleState::Failed as i32
                )),
            }
        )
    );
    assert_eq!(
        handle_operation(&executor, &target, &target.agent_id, key, Operation::Await).await?,
        recorded_failure
    );
    Ok(())
}
