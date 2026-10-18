use super::*;
use crate::monthly_acceptance::Acceptance;
use golem_worker_executor::services::oplog::CommitLevel;
use golem_worker_executor::services::resource_limits::ResourceUsageFlusher;
use golem_worker_executor::worker::MonthlyProposalForTest;
use golem_worker_executor::workerctx::default::Context;
use golem_worker_executor_test_utils::TestWorkerExecutor;
use test_r::test;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scenario {
    Quota { completion_first: bool },
    CheckpointOnly,
    RefundBeforeCaller,
}

#[test]
#[timeout("2m")]
async fn actual_grow_memory_predecessors_allow_original_rpc_pending(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    pending_rpc_scenario(
        last_unique_id,
        deps,
        fixture,
        Resource::Compute,
        AgentMode::Durable,
        true,
        Scenario::CheckpointOnly,
    )
    .await
}

#[test]
#[timeout("2m")]
async fn prepaid_target_refund_rejects_queued_caller_stop_then_rpc_recovers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    pending_rpc_scenario(
        last_unique_id,
        deps,
        fixture,
        Resource::Compute,
        AgentMode::Durable,
        true,
        Scenario::RefundBeforeCaller,
    )
    .await
}

pub(super) fn observe_usage(
    worker: &Arc<Worker<Context>>,
) -> (
    Arc<dyn ResourceUsageFlusher>,
    mpsc::UnboundedReceiver<(u64, u64)>,
) {
    let (send, receive) = mpsc::unbounded_channel();
    let source: Arc<dyn ResourceUsageFlusher> = Arc::new(ObservedUsageFlusher {
        owner: Arc::downgrade(worker),
        send,
    });
    worker
        .owner_runtime_resources()
        .register_resource_usage_flusher(Arc::downgrade(&source));
    (source, receive)
}

pub(super) async fn usage_flushes(
    flushes: &mut mpsc::UnboundedReceiver<(u64, u64)>,
    proposal: MonthlyProposalForTest,
    count: usize,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut seen = 0;
        while seen < count {
            let current = flushes.recv().await.context("owner usage flusher closed")?;
            if current == (proposal.diagnostic_window_id, proposal.policy_revision) {
                seen += 1;
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}

pub(super) fn exhausted_proposal(
    initial: MonthlyProposalForTest,
    observed: MonthlyProposalForTest,
) -> anyhow::Result<()> {
    let mut expected = initial;
    expected.policy_revision += 1;
    expected.exhaustion = Some("monthly compute exhausted");
    ensure!(
        observed == expected,
        "changed prepaid owner identity: {observed:?}, expected {expected:?}"
    );
    Ok(())
}

pub(super) fn no_attempt(attempts: &mut mpsc::UnboundedReceiver<Acceptance>) -> anyhow::Result<()> {
    ensure!(
        matches!(attempts.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "unexpected monthly attempt or closed acceptance observer"
    );
    Ok(())
}

pub(super) async fn entry_count(
    worker: &Arc<Worker<Context>>,
    intercepted: &OplogEntry,
    expected: usize,
) -> anyhow::Result<()> {
    worker
        .commit_oplog_and_update_state(CommitLevel::Always)
        .await?;
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await?;
    // Raw oplog timestamps have millisecond precision on persistence.
    let persisted = intercepted.clone().rounded();
    let count = entries
        .values()
        .cloned()
        .map(OplogEntry::rounded)
        .filter(|entry| entry == &persisted)
        .count();
    ensure!(
        count == expected,
        "intercepted GrowMemory append count {count}, expected {expected}: {intercepted:?}"
    );
    Ok(())
}

pub(super) async fn physical_cleanup(worker: &Arc<Worker<Context>>) -> anyhow::Result<()> {
    tokio::time::timeout(STOP_BOUND, async {
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
        worker.join_accepted_stops_for_test().await?;
        worker.retained_cleanup_for_test().await?;
        ensure!(worker.unload_succeeded_for_test());
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("RPC Store and permit cleanup exceeded the existing stop bound")??;
    Ok(())
}

pub(super) async fn compute_suspension(worker: &Arc<Worker<Context>>) -> anyhow::Result<()> {
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await?;
    let mut suspends = 0;
    for entry in entries.values() {
        match entry {
            OplogEntry::Suspend { .. } => suspends += 1,
            OplogEntry::Error { .. }
            | OplogEntry::Interrupted { .. }
            | OplogEntry::Exited { .. } => bail!("wrong target compute terminal: {entry:?}"),
            _ => {}
        }
    }
    ensure!(
        suspends == 1,
        "expected one target compute Suspend, got {suspends}"
    );
    Ok(())
}

pub(super) async fn cleanup(
    executor: &TestWorkerExecutor,
    workers: [&Arc<Worker<Context>>; 2],
    ids: [&AgentId; 2],
) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    for (worker, id) in workers.into_iter().zip(ids) {
        let interrupt = tokio::time::timeout(Duration::from_secs(5), executor.interrupt(id)).await;
        if !matches!(interrupt, Ok(Ok(()))) {
            errors.push(format!("{id} cleanup interrupt: {interrupt:?}"));
        }
        // Public Interrupt leaves already-idle agents loaded. Use their real idle eviction.
        worker.stop_if_idle().await;
        if let Err(error) = physical_cleanup(worker).await {
            errors.push(format!("{id} physical cleanup: {error:#}"));
        }
    }
    ensure!(errors.is_empty(), "{}", errors.join("; "));
    Ok(())
}
