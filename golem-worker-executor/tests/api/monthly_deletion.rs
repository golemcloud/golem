use super::*;
use anyhow::Context as _;
use golem_common::model::Timestamp;
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::AsyncReadExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FirstStop {
    Deletion,
    Quota,
    User,
}

macro_rules! stop_order_test {
    ($name:ident, $first:expr) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            for durable in [true, false] {
                stop_order(last_unique_id, deps, host_api_tests, $first, durable).await?;
            }
            Ok(())
        }
    };
}

stop_order_test!(
    deletion_claim_rejects_later_monthly_stop,
    FirstStop::Deletion
);
stop_order_test!(monthly_stop_survives_later_deletion, FirstStop::Quota);
stop_order_test!(user_stop_rejects_later_monthly_stop, FirstStop::User);

async fn stop_order(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    first: FirstStop,
    durable: bool,
) -> anyhow::Result<()> {
    let mut policy = MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: u64::MAX,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    };
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let shutdown = CancellationToken::new();
    let metering = ResourceUsageMeteringConfig {
        compute: false,
        memory: true,
        filesystem: false,
    };
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown.clone(),
    );
    let context = TestContext::new(last_unique_id);
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let name = if durable {
        agent_id!("Networking", format!("stop-order-{first:?}"))
    } else {
        agent_id!("EphemeralNetworking", format!("stop-order-{first:?}"))
    };
    let key = IdempotencyKey::fresh();
    let id = if durable {
        executor.start_agent(&component.id, name.clone()).await?
    } else {
        AgentId::from_agent_id(
            component.id,
            &name
                .clone()
                .with_ephemeral_invocation_phantom(&key)
                .unwrap(),
        )
        .unwrap()
    };
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let invocation = tokio::spawn({
        let key = key.clone();
        let executor = executor.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    "tcp_collect_p3",
                    data_value!(port),
                )
                .await
        }
    });
    let (mut socket, _) =
        tokio::time::timeout(Duration::from_secs(10), listener.accept()).await??;
    let worker = executor
        .production_active_agent(&owned)
        .await
        .unwrap()
        .primary();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            executor.commit_oplog(&id).await?;
            let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            if let Some(start) = entries.iter().find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(start)
                    if start.function_name == "sockets::types::tcp-socket::receive-chunk" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            }) {
                assert!(!entries.iter().any(|entry| match &entry.entry {
                    PublicOplogEntry::End(end) => end.start_index == start,
                    PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
                    _ => false,
                }));
                break Ok::<_, anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    let generation = worker.resident_generation_for_test();
    let acquisitions = worker.permit_acquisitions_for_test();
    let mut raw = worker.raw_interrupt_for_test();
    let helper = worker.await_interrupt_for_test();
    let (_, driver_entered, release_driver) = worker.pause_next_stop_driver_for_test();
    let mut deleting = None;
    let mut fence = (first != FirstStop::User)
        .then(|| worker.pause_next_deletion_stage_for_test(WorkerDeletionStage::ExecutionFenced));
    let mut before_remove = None;
    let delete = || {
        tokio::spawn({
            let executor = executor.clone();
            let id = id.clone();
            async move { executor.delete_worker(&id).await }
        })
    };
    if first == FirstStop::Deletion {
        deleting = Some(delete());
        tokio::time::timeout(Duration::from_secs(10), &mut fence.as_mut().unwrap().0)
            .await
            .context("deletion claim")??;
    }
    let user = InterruptKind::Interrupt(Timestamp::now_utc());
    if first == FirstStop::User {
        worker.set_interrupting(user).await.unwrap();
    }
    let (proposal_entered, release_proposal) = worker.pause_next_monthly_acceptance_for_test();
    policy.available_memory_gb_seconds = 0;
    registry.set_policy(policy);
    let refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), proposal_entered).await??;
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(worker.monthly_stop_for_test(), None);
    release_proposal.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), worker.drain_lifecycle_for_test()).await??;
    let accepted_monthly = worker.monthly_stop_for_test();
    if first == FirstStop::Quota {
        deleting = Some(delete());
        tokio::time::timeout(Duration::from_secs(10), &mut fence.as_mut().unwrap().0)
            .await
            .context("deletion claim after quota")??;
    }
    if let Some((_, release_fence)) = fence.take() {
        before_remove = Some(
            worker.pause_next_deletion_stage_for_test(WorkerDeletionStage::DurableStateRemoved),
        );
        release_fence.send(()).unwrap();
    }
    tokio::time::timeout(Duration::from_secs(10), driver_entered)
        .await
        .context("stop driver entry")??;
    assert_eq!(worker.frozen_stop_for_test(), None);
    let mut receipt = worker.accepted_stop_receipt_for_test();
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    release_driver.send(false).unwrap();
    tokio::time::timeout(Duration::from_secs(10), receipt.recv())
        .await
        .context("stop receipt")??;
    let cause = tokio::time::timeout(Duration::from_secs(10), raw.recv())
        .await
        .context("raw interrupt")??;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), helper)
            .await
            .context("helper interrupt")?,
        cause
    );
    tokio::time::timeout(
        Duration::from_secs(10),
        worker.join_accepted_stops_for_test(),
    )
    .await
    .context("join accepted stops")??;
    if first != FirstStop::User {
        tokio::time::timeout(
            Duration::from_secs(10),
            &mut before_remove.as_mut().unwrap().0,
        )
        .await
        .context("storage removal stage")??;
    }
    let mut byte = [0];
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(10), socket.read(&mut byte)).await?,
        Ok(0) | Err(_)
    ));
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    worker.retained_cleanup_for_test().await?;
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    if let Some(deleting) = deleting {
        before_remove.take().unwrap().1.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), deleting).await???;
    }
    refresh.await?;
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert_eq!(
        limits
            .initialize_account(context.account_id)
            .await?
            .monthly_observer_count_for_test(),
        0
    );
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    assert!(
        tokio::time::timeout(Duration::from_secs(10), invocation)
            .await??
            .is_err()
    );
    shutdown.cancel();

    assert_eq!(entries.values().filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { idempotency_key, .. } if idempotency_key == &key)).count(), 1);
    assert_eq!(
        entries
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        2
    );
    assert_eq!(
        entries
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        1
    );
    let errors: Vec<_> = entries
        .values()
        .filter_map(|e| match e {
            OplogEntry::Error { error, .. } => Some(error),
            _ => None,
        })
        .collect();
    let suspends = entries
        .values()
        .filter(|e| matches!(e, OplogEntry::Suspend { .. }))
        .count();
    let interrupts = entries
        .values()
        .filter(|e| matches!(e, OplogEntry::Interrupted { .. }))
        .count();
    if first == FirstStop::Quota {
        assert_eq!(accepted_monthly, Some(cause));
        assert!(matches!(cause, InterruptKind::Suspend(_)));
        assert_eq!(interrupts, 0);
        assert_eq!(suspends, usize::from(durable));
        if durable {
            assert!(errors.is_empty(), "{errors:?}");
        } else {
            assert!(
                matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(_)]),
                "{errors:?}"
            );
        }
    } else {
        assert_eq!(
            accepted_monthly, None,
            "quota was accepted after {first:?} already owned the stop"
        );
        assert!(matches!(cause, InterruptKind::Interrupt(_)));
        if first == FirstStop::User {
            assert_eq!(cause, user);
        }
        assert_eq!(interrupts, 1);
        assert_eq!(suspends, 0);
        assert!(errors.is_empty(), "{errors:?}");
    }
    Ok(())
}
