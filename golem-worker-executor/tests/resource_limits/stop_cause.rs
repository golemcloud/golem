use super::*;
use futures::FutureExt;
use golem_common::model::Timestamp;
use golem_common::model::agent::Principal;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::{HasOplog, UsesAllDeps};
use golem_worker_executor::worker::Worker;
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
#[timeout("2m")]
async fn waiting_stop_freezes_supersession_and_first_terminal_without_permit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_concurrent_agent_limit(deps, &context, 1).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(&component.id, agent_id!("Counter", "cause-seed"))
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .unwrap()
        .primary();
    let held = executor
        .acquire_account_concurrent_agent_permit(seed_id)
        .await;
    let suspend = InterruptKind::Suspend(Timestamp::now_utc());
    let interrupt = InterruptKind::Interrupt(Timestamp::now_utc());
    for (index, first, second, expected) in [
        (0, InterruptKind::Restart, suspend, suspend),
        (1, InterruptKind::Jump, interrupt, interrupt),
        (2, suspend, interrupt, suspend),
        (3, interrupt, suspend, interrupt),
    ] {
        let name = agent_id!("Counter", format!("waiting-cause-{index}"));
        let id = golem_common::model::AgentId {
            component_id: component.id,
            agent_id: name.to_string(),
        };
        let worker = Worker::get_or_create_suspended(
            seed.all(),
            &OwnedAgentId::new(context.default_environment_id, &id),
            None,
            vec![],
            None,
            None,
            &InvocationContextStack::fresh(),
            Principal::anonymous(),
        )
        .await?;
        Worker::start_if_needed(worker.clone()).await?;
        let mut raw = worker.raw_interrupt_for_test();
        let mut existing = worker.await_interrupt_for_test();
        let (_, entered, release) = worker.pause_next_stop_driver_for_test();
        let (published, release_publication) = worker.pause_next_stop_publication_for_test();
        let mut receipt = worker
            .set_interrupting(first)
            .await?
            .expect("accepted first stop");
        entered.await?;
        let second_receipt = worker.set_interrupting(second).await?;
        assert_eq!(second_receipt.is_some(), index < 2);
        drop(second_receipt);
        assert!(matches!(
            raw.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert!(existing.as_mut().now_or_never().is_none());
        let late_pending = worker.await_interrupt_for_test();
        release.send(false).unwrap();
        published.await?;
        assert_eq!(worker.frozen_stop_for_test(), Some(expected));
        assert_eq!(raw.recv().await?, expected);
        assert_eq!(existing.await, expected);
        assert_eq!(late_pending.await, expected);
        assert_eq!(worker.await_interrupt_for_test().await, expected);
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert_eq!(worker.resident_generation_for_test(), 0);
        assert!(!worker.concurrent_agent_permit_is_held().await);
        release_publication.send(false).unwrap();
        tokio::time::timeout(Duration::from_secs(5), receipt.recv()).await??;
        worker.join_accepted_stops_for_test().await?;
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        let entries = worker
            .oplog()
            .read_exact(
                OplogIndex::INITIAL,
                worker.oplog().current_oplog_index().await.as_u64(),
            )
            .await;
        assert_eq!(
            entries
                .values()
                .filter(|entry| matches!(entry, OplogEntry::Interrupted { .. }))
                .count(),
            usize::from(matches!(expected, InterruptKind::Interrupt(_)))
        );
        assert_eq!(
            entries
                .values()
                .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
                .count(),
            usize::from(matches!(expected, InterruptKind::Suspend(_)))
        );
        assert!(
            !entries
                .values()
                .any(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
        );
    }
    drop(held);
    Ok(())
}
