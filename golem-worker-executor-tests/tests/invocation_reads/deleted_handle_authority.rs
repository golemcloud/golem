use super::*;
use golem_api_grpc::proto::golem::worker::v1::worker_execution_error;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    InvocationHandleOperation as Operation, invocation_handle_response as response,
    invocation_handle_success as success,
};
use pretty_assertions::assert_eq;
use std::sync::{Arc, atomic::AtomicBool};
use test_r::test;

#[test]
#[timeout("2m")]
async fn deleted_handle_requires_surviving_authority_and_never_reaches_the_successor(
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
                move || Arc::new(HandleManagementAuth::new(management.clone()))
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let server = agent_key!("ScheduledInvocationServer", "deleted-handle-authority");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let owner = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "deleted-handle-starter"),
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
                    owner.agent_id,
                    AgentTypeName("ScheduledInvocationClient".to_string()),
                    executor
                        .active_agent(&owner)
                        .await
                        .unwrap()
                        .primary()
                        .get_initial_worker_metadata()
                        .fingerprint,
                    owner.environment_id,
                ),
                scope_card: None,
            },
            jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1),
        )
        .await?;
    drop(worker);
    let before =
        handle_operation(&executor, &target, &owner.agent_id, key, Operation::Status).await?;
    assert!(matches!(before, response::Result::Success(value)
        if value.result == Some(success::Result::Status(InvocationLifecycleState::Scheduled as i32))));
    executor.delete_worker(&target).await?;
    executor.start_agent(&component.id, server.clone()).await?;
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        let observed =
            handle_operation(&executor, &target, &owner.agent_id, key, operation).await?;
        assert!(
            matches!(observed, response::Result::Failure(error)
            if matches!(error.error, Some(worker_execution_error::Error::PermissionDenied(_)))),
            "the successor has no persisted starter evidence"
        );
    }
    management.store(true, std::sync::atomic::Ordering::SeqCst);
    for operation in [Operation::Status, Operation::Await, Operation::Cancel] {
        let observed =
            handle_operation(&executor, &target, &owner.agent_id, key, operation).await?;
        assert!(
            matches!(observed, response::Result::Success(value)
            if matches!(value.result, Some(success::Result::NotFound(_)))),
            "an authorized old-ID operation must not reach the successor"
        );
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
