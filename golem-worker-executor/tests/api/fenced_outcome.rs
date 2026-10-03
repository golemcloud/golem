use super::*;
use golem_common::model::ShardEpoch;
use golem_common::model::oplog::OplogEntry;
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
#[timeout("2m")]
async fn selected_success_fence_releases_and_removes_old_generation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    selected_writer_fence(last_unique_id, deps, agent_counters, false).await
}

#[test]
#[timeout("2m")]
async fn selected_failure_fence_releases_and_removes_old_generation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    selected_writer_fence(last_unique_id, deps, agent_counters, true).await
}

async fn selected_writer_fence(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_counters: &PrecompiledComponent,
    failure: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let name = agent_id!("FailingCounter", format!("fenced-selected-{failure}"));
    let id = executor.start_agent(&component.id, name.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &name, "add", data_value!(1u64))
        .await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    let fingerprint = worker.get_initial_worker_metadata().fingerprint;
    let (selected, release) = worker.pause_next_outcome_for_test(true);
    let key = IdempotencyKey::fresh();
    let caller = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let name = name.clone();
        let key = key.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    "add",
                    data_value!(if failure { 11u64 } else { 2u64 }),
                )
                .await
        }
    }));
    tokio::time::timeout(Duration::from_secs(10), selected).await??;
    assert!(worker.concurrent_agent_permit_is_held().await);
    let acquisitions = worker.permit_acquisitions_for_test();
    // Persist the selected invocation's prefix before the new owner takes over.
    executor.commit_oplog(&id).await?;
    let entries = agent_oplog_length(deps, &context, &owned).await?;
    let prefix = worker
        .oplog()
        .read_exact(OplogIndex::INITIAL, entries)
        .await;
    assert_eq!(prefix.values().filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
    let finished = prefix
        .values()
        .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
        .count();
    assert!(
        !prefix
            .values()
            .any(|entry| matches!(entry, OplogEntry::Error { .. }))
    );
    take_agent_oplog_over_at_epoch(deps, &context, &owned, 1).await?;
    release.send(false).unwrap();

    let answer = tokio::time::timeout(Duration::from_secs(10), caller).await??;
    let rendered = format!(
        "{:#}",
        answer.expect_err("a refused outcome must not be published")
    );
    assert!(
        rendered.contains("Sharding not ready") || rendered.contains("fenced"),
        "caller must reroute, got {rendered}"
    );
    // Keep the old Arc alive: release and removal must belong to this exact generation.
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.concurrent_agent_permit_is_held().await
            || executor.worker_is_cached(&owned).await
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| {
        anyhow!("the selected writer's fenced generation retained its permit or cache entry")
    })?;
    worker.join_accepted_stops_for_test().await?;
    worker.retained_cleanup_for_test().await?;
    assert!(!worker.is_loaded().await);
    assert!(worker.unload_succeeded_for_test());
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert_eq!(
        agent_oplog_length(deps, &context, &owned).await?,
        entries,
        "ShardLost must append no terminal or lifecycle entry"
    );
    assert_eq!(
        worker
            .oplog()
            .read_exact(OplogIndex::INITIAL, entries)
            .await,
        prefix
    );

    revoke_shard_zero(&executor).await?;
    take_agent_oplog_over_at_epoch(deps, &context, &owned, 2).await?;
    use golem_api_grpc::proto::golem::shardmanager::{ShardEpochEntry, ShardId};
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        AssignShardsRequest, assign_shards_response,
    };
    let assigned = executor
        .client
        .clone()
        .assign_shards(AssignShardsRequest {
            shard_epochs: vec![ShardEpochEntry {
                shard_id: Some(ShardId { value: 0 }),
                epoch: 2,
            }],
            number_of_shards: 1,
            revision: 2,
            incarnation_id: TEST_SHARD_MANAGER.to_string(),
        })
        .await?
        .into_inner();
    assert!(matches!(
        assigned.result,
        Some(assign_shards_response::Result::Success(_))
    ));
    let retried = executor
        .invoke_and_await_agent_with_key(
            &component,
            &name,
            &key,
            "add",
            data_value!(if failure { 11u64 } else { 2u64 }),
        )
        .await;
    if failure {
        let rendered = format!(
            "{:#}",
            retried.expect_err("the new owner must record the guest failure")
        );
        assert!(
            rendered.contains("Component trapped"),
            "the fresh owner must publish the actual guest trap, got {rendered}"
        );
    } else {
        retried?;
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &name, "get", data_value!())
                .await?
                .into_typed::<u64>()?,
            3,
            "the new owner reconstructs the seed effect and executes the uncommitted invocation once"
        );
    }
    let replacement = executor.active_agent(&owned).await.unwrap().primary();
    assert!(!Arc::ptr_eq(&worker, &replacement));
    assert_eq!(
        replacement.get_initial_worker_metadata().fingerprint,
        fingerprint
    );
    assert_eq!(replacement.oplog().shard_epoch(), Some(ShardEpoch(2)));
    let history = replacement
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            replacement.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(history.values().filter(|entry| matches!(entry, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 1, "retry must continue the original durable invocation");
    assert_eq!(
        history
            .values()
            .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        finished + if failure { 0 } else { 2 }
    );
    assert_eq!(
        history
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Error { .. }))
            .count(),
        usize::from(failure)
    );
    Ok(())
}
