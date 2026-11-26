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
use golem_api_grpc::proto::golem::worker::v1::worker_execution_error;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    InvocationHandleOperation as Operation, invocation_handle_response as response,
    invocation_handle_success as success,
};
use golem_common::model::OwnedAgentId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_worker_executor::worker::{WorkerDeletionHook, WorkerDeletionStage};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use test_r::test;

struct GateBeforeStage {
    target: OwnedAgentId,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    gated: AtomicBool,
}

#[async_trait::async_trait]
impl WorkerDeletionHook for GateBeforeStage {
    async fn before_stage(
        &self,
        target: &OwnedAgentId,
        stage: WorkerDeletionStage,
    ) -> Result<(), WorkerExecutorError> {
        if target == &self.target
            && stage == WorkerDeletionStage::CacheRemoved
            && !self.gated.swap(true, Ordering::AcqRel)
        {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        Ok(())
    }
}

#[test]
#[timeout("4m")]
async fn deleting_cached_handle_requires_authority_before_disclosing_not_found(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
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
    let server = agent_key!("ScheduledInvocationServer", "deleting-handle-authority");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let starter = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "deleting-handle-starter"),
        )
        .await?;
    let other = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "deleting-handle-other"),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let worker = executor.active_agent(&target).await.unwrap().primary();
    let key = InvocationId(uuid::Uuid::new_v4());
    worker
        .schedule_invocation(
            AgentInvocation::AgentMethod {
                idempotency_key: key,
                method_name: "inc_global_by".to_string(),
                input: data_value!(23u64).into_parts().1.try_into().unwrap(),
                allow_streams: false,
                invocation_context: InvocationContextStack::fresh(),
                caller: callers::agent_in(
                    starter.agent_id,
                    AgentTypeName("ScheduledInvocationClient".to_string()),
                    executor
                        .active_agent(&starter)
                        .await
                        .unwrap()
                        .primary()
                        .get_initial_worker_metadata()
                        .fingerprint,
                    starter.environment_id,
                ),
                scope_card: None,
            },
            jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1),
        )
        .await?;
    drop(worker);
    assert!(matches!(
        handle_operation(&executor, &target, &starter.agent_id, key, Operation::Status).await?,
        response::Result::Success(value)
            if value.result == Some(success::Result::Status(InvocationLifecycleState::Scheduled as i32))
    ));
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        let result = handle_operation(&executor, &target, &other.agent_id, key, operation).await?;
        assert!(
            matches!(&result, response::Result::Failure(error)
            if matches!(error.error, Some(worker_execution_error::Error::PermissionDenied(_)))),
            "the unrelated caller must be denied before deletion: {operation:?}: {result:?}"
        );
    }

    let gate = Arc::new(GateBeforeStage {
        target: target.clone(),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
        gated: AtomicBool::new(false),
    });
    executor.set_worker_deletion_hook(gate.clone());
    let mut deleting = tokio::spawn({
        let executor = executor.clone();
        let target = target.clone();
        async move { executor.delete_worker(&target).await }
    });

    // Collect observations without assertions or early returns from the test: even a failed
    // request must release the gate and join deletion before it reports the failure.
    let observations: anyhow::Result<_> = async {
        tokio::time::timeout(Duration::from_secs(30), gate.entered.acquire())
            .await??
            .forget();
        let cached_before = executor.worker_is_cached(&target).await;
        let mut denied = Vec::new();
        let mut allowed = Vec::new();
        for authorized in [false, true] {
            management.store(authorized, Ordering::SeqCst);
            for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
                let result = tokio::time::timeout(
                    Duration::from_secs(10),
                    handle_operation(&executor, &target, &other.agent_id, key, operation),
                )
                .await??;
                if authorized {
                    allowed.push((operation, result));
                } else {
                    denied.push((operation, result));
                }
            }
        }
        let cached_after = executor.worker_is_cached(&target).await;
        Ok((cached_before, cached_after, denied, allowed))
    }
    .await;
    gate.release.add_permits(1);
    let deletion = tokio::time::timeout(Duration::from_secs(30), &mut deleting).await;
    if deletion.is_err() {
        deleting.abort();
        let _ = deleting.await;
    }
    deletion???;
    let (cached_before, cached_after, denied, allowed) = observations?;
    assert!(
        cached_before && cached_after,
        "the retired owner must remain cached throughout the gated operations"
    );
    for (operation, result) in denied {
        assert!(
            matches!(&result, response::Result::Failure(error)
            if matches!(error.error, Some(worker_execution_error::Error::PermissionDenied(_)))),
            "a stopped authority actor must not disclose deletion before authorization: {operation:?}: {result:?}"
        );
    }
    // CacheRemoved follows durable removal and actor shutdown. The cached generation answers
    // authorized reads as not found, not as a live invocation or a deletion-in-progress refusal.
    for (operation, result) in allowed {
        assert!(
            matches!(&result, response::Result::Success(value)
            if matches!(value.result, Some(success::Result::NotFound(_)))),
            "an authorized operation on the removed generation must be typed not-found: {operation:?}: {result:?}"
        );
    }
    assert!(!executor.worker_is_cached(&target).await);
    Ok(())
}
