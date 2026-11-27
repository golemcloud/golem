//! Interrupting a streaming invocation that still has an open input ends its session with one
//! `Finished` record, whichever path delivers the interrupt; a lost shard writes none.
use super::*;
use golem_service_base::error::worker_executor::InterruptKind;
use pretty_assertions::assert_eq;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);

const SESSION_END_TIMEOUT: Duration = Duration::from_secs(20);
const TEST_SHARD_MANAGER: &str = "5eed0000-0000-4000-8000-000000000001";

#[derive(Clone, Copy, Debug)]
enum InterruptPath {
    /// The management interrupt endpoint, which retires the interrupted agent.
    Management,
    /// Deleting the agent's environment, which retires every agent in it.
    EnvironmentDeletion,
    /// An interrupt queued on the loaded worker, which does not retire it.
    LoadedWorker,
}

struct OpenInputSession {
    executor: TestWorkerExecutor,
    target: OwnedAgentId,
    // The open transport: dropping either half would end the session before the interrupt.
    _responses: tonic::Streaming<InvocationResponse>,
    _frames: mpsc::Sender<InvocationRequest>,
}

async fn open_input_session(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    fixture: &PrecompiledComponent,
    name: &str,
) -> anyhow::Result<OpenInputSession> {
    let executor = start(deps, context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let target = executor
        .start_agent(&component.id, agent_key!("StreamingRpcTarget", name))
        .await?;
    let caller = crate::agent::keyed_agent_id(
        component.environment_id,
        &agent_key!("StreamingRpcCaller", name),
    );
    let fingerprint = executor.get_worker_metadata(&target).await?.fingerprint.0;
    let start = origin_observer_start(
        &executor,
        &component,
        &target,
        &caller,
        fingerprint,
        &IdempotencyKey::fresh(),
        "consume",
    );
    let (frames, responses, _accepted) = open_raw_session(&executor, start).await?;
    executor
        .wait_for_status(&target, AgentStatus::Running, SESSION_END_TIMEOUT)
        .await?;
    Ok(OpenInputSession {
        executor,
        target,
        _responses: responses,
        _frames: frames,
    })
}

async fn interrupt(session: &OpenInputSession, path: InterruptPath) -> anyhow::Result<()> {
    match path {
        InterruptPath::Management => session.executor.interrupt(&session.target).await,
        InterruptPath::EnvironmentDeletion => {
            session
                .executor
                .active_agents()
                .unload_environment(session.target.environment_id)
                .await;
            Ok(())
        }
        InterruptPath::LoadedWorker => {
            session
                .executor
                .interrupt_loaded_worker(
                    &session.target.agent_id,
                    InterruptKind::Interrupt(golem_common::model::Timestamp::now_utc()),
                )
                .await
        }
    }
}

/// The results of the session `Finished` records in the agent's stored oplog.
async fn finished_session_records(
    executor: &TestWorkerExecutor,
    target: &OwnedAgentId,
) -> anyhow::Result<Vec<Result<(), Vec<u8>>>> {
    Ok(executor
        .stored_stream_session_records(&target.agent_id)
        .await?
        .into_iter()
        .filter_map(|record| match record {
            StreamSessionRecord::Finished(record) => Some(record.result),
            _ => None,
        })
        .collect())
}

async fn wait_until_unloaded(
    executor: &TestWorkerExecutor,
    target: &OwnedAgentId,
) -> anyhow::Result<()> {
    tokio::time::timeout(SESSION_END_TIMEOUT, async {
        while executor.worker_is_loaded(target).await {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("the interrupted worker stayed loaded"))
}

async fn interrupt_ends_session_with_one_finished_record(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    fixture: &PrecompiledComponent,
    path: InterruptPath,
) -> anyhow::Result<()> {
    let session = open_input_session(deps, context, fixture, &format!("{path:?}")).await?;

    interrupt(&session, path).await?;
    wait_until_unloaded(&session.executor, &session.target).await?;

    // Live: the stop has unwound the invocation, so its session terminal is written by now.
    let live = finished_session_records(&session.executor, &session.target).await?;
    assert_eq!(
        live.len(),
        1,
        "{path:?}: the interrupted session must have one Finished record"
    );
    assert!(live[0].is_err(), "{path:?}: an interrupt fails the session");

    // After a restart: reconstruction neither adds nor drops the session's terminal.
    let OpenInputSession {
        executor, target, ..
    } = session;
    drop(executor);
    let executor = start(deps, context).await?;
    executor.resume(&target, false).await?;
    // The resumed `consume` reads the failed input and panics, which fails the agent.
    executor
        .wait_for_status(&target, AgentStatus::Failed, SESSION_END_TIMEOUT)
        .await?;
    assert_eq!(
        finished_session_records(&executor, &target).await?,
        live,
        "{path:?}: reconstruction changed the session's Finished records"
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
async fn management_interrupt_finishes_an_open_input_session_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    interrupt_ends_session_with_one_finished_record(
        deps,
        &context,
        agent_rpc_rust,
        InterruptPath::Management,
    )
    .await
}

#[test]
#[timeout("2 minutes")]
async fn environment_deletion_finishes_an_open_input_session_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    interrupt_ends_session_with_one_finished_record(
        deps,
        &context,
        agent_rpc_rust,
        InterruptPath::EnvironmentDeletion,
    )
    .await
}

#[test]
#[timeout("2 minutes")]
async fn loaded_worker_interrupt_finishes_an_open_input_session_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    interrupt_ends_session_with_one_finished_record(
        deps,
        &context,
        agent_rpc_rust,
        InterruptPath::LoadedWorker,
    )
    .await
}

/// A lost shard leaves the session to the shard's new owner: this executor records nothing.
#[test]
#[timeout("2 minutes")]
async fn shard_revocation_leaves_an_open_input_session_unfinished(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::shardmanager::ShardId;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        RevokeShardsRequest, revoke_shards_response,
    };

    let context = TestContext::new(last_unique_id);
    let session = open_input_session(deps, &context, agent_rpc_rust, "shard-revocation").await?;

    let revoked = session
        .executor
        .client
        .clone()
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
    wait_until_unloaded(&session.executor, &session.target).await?;

    assert_eq!(
        finished_session_records(&session.executor, &session.target).await?,
        Vec::new(),
        "a lost shard must not finish the session that the new owner resumes"
    );
    Ok(())
}
