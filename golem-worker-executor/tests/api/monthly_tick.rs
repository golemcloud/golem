use super::*;
use anyhow::Context as _;
use golem_common::model::oplog::PublicOplogEntry;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::worker::{MonthlyClockForTest, MonthlyTimerPollForTest};
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::AsyncReadExt;

#[test]
#[timeout("60s")]
async fn monthly_tick_retries_rejected_acceptance_on_the_same_billed_window(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let available = MonthlyResourcePolicy {
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
    let registry = Arc::new(MutableResourceLimitsRegistry::new(available.clone()));
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
    let name = agent_id!("Networking", "monthly-controlled-tick");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let active = executor.production_active_agent(&owned).await.unwrap();
    let worker = active.primary();
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("initial idle release")?;
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let account = limits.initialize_account(context.account_id).await?;
    assert_eq!(clock.active_sleeps(), 0);
    assert_eq!(account.monthly_observer_count_for_test(), 0);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (connected_tx, connected_rx) = tokio::sync::oneshot::channel();
    let (closed_tx, mut closed_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = connected_tx.send(());
        let mut byte = [0];
        let _ = closed_tx.send(matches!(socket.read(&mut byte).await, Ok(0)));
    });
    let invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let name = name.clone();
        async move {
            executor
                .invoke_and_await_agent(&component, &name, "tcp_collect_p3", data_value!(port))
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(10), connected_rx).await??;
    // Connection acceptance precedes the guest's receive call. Require its open durable chunk.
    let chunk = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            executor.commit_oplog(&id).await?;
            let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            if let Some(entry) = entries.iter().find(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(start)
                        if start.function_name == "sockets::types::tcp-socket::receive-chunk"
                )
            }) {
                return Ok::<_, anyhow::Error>(entry.oplog_index);
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    expect_poll(&mut polls, 0, 30).await?;
    assert_eq!(clock.active_sleeps(), 1);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    let acquisitions = worker.permit_acquisitions_for_test();
    let initial = worker.current_monthly_proposal_for_test();
    let mut raw = worker.raw_interrupt_for_test();
    clock.advance(Duration::from_secs(11));
    expect_poll(&mut polls, 11, 30).await?;

    let (entered, release_acceptance) = worker.pause_next_monthly_acceptance_for_test();
    let mut exhausted = available.clone();
    exhausted.available_memory_gb_seconds = 0;
    registry.set_policy(exhausted);
    let refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), entered)
        .await
        .context("Registry proposal at actor acceptance")??;
    let missed = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
        .await?
        .context("Registry-triggered attempt")?;
    let interrupts = worker.hold_interrupt_lock_for_test().await;
    let current = worker.current_monthly_proposal_for_test();
    assert_eq!(current, missed.proposal);
    assert_eq!(current.window_identity, initial.window_identity);
    assert_eq!(current.start_attempt, initial.start_attempt);
    assert_eq!(current.resident_generation, initial.resident_generation);
    assert_eq!(current.exhaustion, Some("monthly memory exhausted"));
    release_acceptance.send(()).unwrap();
    assert!(
        !tokio::time::timeout(Duration::from_secs(10), missed.completed)
            .await
            .context("contended attempt rejection")??
    );
    expect_poll(&mut polls, 11, 41).await?;
    assert_eq!(worker.monthly_stop_for_test(), None);
    assert_eq!(worker.frozen_stop_for_test(), None);
    assert_eq!(worker.owner_stop_for_test().await, None);
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        closed_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    let usage_after_update = account.hard_limit_memory_overshoot_for_test();
    eprintln!(
        "[monthly-tick] rejected Registry check before acceptance, same window {current:?}, overshoot={usage_after_update}"
    );

    // Applied updates restart the existing select-loop sleep; the old 30s deadline is gone.
    clock.advance(Duration::from_secs(19));
    expect_poll(&mut polls, 30, 41).await?;
    clock.advance(Duration::from_secs(10));
    expect_poll(&mut polls, 40, 41).await?;
    worker.drain_lifecycle_for_test().await?;
    assert!(matches!(
        attempts.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    assert_eq!(worker.monthly_stop_for_test(), None);
    assert_eq!(worker.current_monthly_proposal_for_test(), current);
    assert!(worker.concurrent_agent_permit_is_held().await);

    clock.advance(Duration::from_secs(1));
    expect_poll(&mut polls, 41, 41).await?;
    let missed_tick = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
        .await?
        .context("contended tick proposal")?;
    assert_eq!(missed_tick.proposal, current);
    assert!(!tokio::time::timeout(Duration::from_secs(10), missed_tick.completed).await??);
    expect_poll(&mut polls, 41, 71).await?;
    assert_eq!(worker.monthly_stop_for_test(), None);
    assert_eq!(worker.frozen_stop_for_test(), None);
    assert_eq!(worker.owner_stop_for_test().await, None);
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        closed_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    assert_eq!(clock.active_sleeps(), 1);
    let usage_after_miss = account.hard_limit_memory_overshoot_for_test();
    assert!(usage_after_miss > usage_after_update);
    eprintln!(
        "[monthly-tick] rejected first tick at 41s while interrupt lock remains held, overshoot={usage_after_miss}"
    );
    drop(interrupts);
    assert_eq!(worker.pending_stop_for_test().await, None);

    clock.advance(Duration::from_secs(29));
    expect_poll(&mut polls, 70, 71).await?;
    worker.drain_lifecycle_for_test().await?;
    assert!(matches!(
        attempts.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    assert_eq!(worker.monthly_stop_for_test(), None);
    assert_eq!(worker.current_monthly_proposal_for_test(), current);
    let (_, accepted, release_driver) = worker.pause_next_stop_driver_for_test();
    clock.advance(Duration::from_secs(1));
    expect_poll(&mut polls, 71, 71).await?;
    let retried = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
        .await?
        .context("next tick proposal after contention ends")?;
    assert_eq!(retried.proposal, current);
    assert!(tokio::time::timeout(Duration::from_secs(10), retried.completed).await??);
    tokio::time::timeout(Duration::from_secs(10), accepted).await??;
    expect_poll(&mut polls, 71, 101).await?;
    let monthly = worker.monthly_stop_for_test().expect("accepted tick stop");
    assert!(matches!(monthly, InterruptKind::Suspend(_)));
    assert_eq!(worker.pending_stop_for_test().await, Some(monthly));
    assert_eq!(worker.frozen_stop_for_test(), None);
    assert_eq!(worker.owner_stop_for_test().await, None);
    assert_eq!(worker.current_monthly_proposal_for_test(), current);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    assert_eq!(clock.active_sleeps(), 1);
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert!(!invocation.is_finished());
    assert!(matches!(
        closed_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    let usage_at_tick = account.hard_limit_memory_overshoot_for_test();
    assert!(
        usage_at_tick > usage_after_miss,
        "the still-held window must settle more byte-time on the retry tick"
    );
    eprintln!(
        "[monthly-tick] accepted next tick at 71s, identical target and capacity, overshoot={usage_at_tick}"
    );

    let mut receipt = worker.accepted_stop_receipt_for_test();
    release_driver.send(false).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
        monthly
    );
    assert!(tokio::time::timeout(Duration::from_secs(10), closed_rx).await??);
    tokio::time::timeout(Duration::from_secs(10), async {
        worker.join_accepted_stops_for_test().await?;
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
        worker.retained_cleanup_for_test().await
    })
    .await
    .context("joined physical cleanup")??;
    tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    assert_eq!(clock.active_sleeps(), 0);
    assert_eq!(account.monthly_observer_count_for_test(), 0);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert_eq!(
        worker.resident_generation_for_test(),
        current.resident_generation
    );
    let settled = account.hard_limit_memory_overshoot_for_test();
    assert!(
        settled > usage_at_tick,
        "accounting continues through physical release, not only stop acceptance"
    );
    clock.advance(Duration::from_secs(60));
    assert_eq!(clock.active_sleeps(), 0);
    assert!(matches!(
        polls.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        attempts.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    tokio::time::timeout(Duration::from_secs(10), refresh).await??;
    limits.run_batch_for_test().await;
    assert_eq!(account.hard_limit_memory_overshoot_for_test(), settled);
    let billed = registry.applied_memory_byte_nanoseconds();
    assert!(billed > 0, "pre-exhaustion memory remains billable");
    limits.run_batch_for_test().await;
    assert_eq!(registry.applied_memory_byte_nanoseconds(), billed);
    assert_eq!(account.hard_limit_memory_overshoot_for_test(), settled);
    eprintln!(
        "[monthly-tick] joined monitor, no timer/subscription/permit, settled overshoot={settled}, billed={billed}"
    );

    executor
        .wait_for_status(&id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    let oplog = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    assert_eq!(
        count_agent_invocation_pair_since(&oplog, OplogIndex::INITIAL),
        (2, 1)
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Suspend(_)))
            .count(),
        1
    );
    assert!(oplog.iter().all(|entry| !matches!(
        &entry.entry,
        PublicOplogEntry::Interrupted(_) | PublicOplogEntry::Error(_)
    )));
    assert!(oplog.iter().all(
        |entry| !matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == chunk)
    ));
    assert!(oplog.iter().all(|entry| !matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == chunk)));
    server.await?;

    registry.set_policy(available);
    limits.run_batch_for_test().await;
    executor.resume(&id, false).await?;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), invocation)
            .await???
            .into_typed::<Result<String, String>>()?,
        Err("ErrorCode::InvalidState".to_string())
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert_eq!(clock.active_sleeps(), 0);
    assert_eq!(account.monthly_observer_count_for_test(), 0);
    shutdown.cancel();
    Ok(())
}

async fn expect_poll(
    polls: &mut tokio::sync::mpsc::UnboundedReceiver<MonthlyTimerPollForTest>,
    now: u64,
    deadline: u64,
) -> anyhow::Result<()> {
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), polls.recv())
            .await
            .with_context(|| format!("timer poll at {now}s for deadline {deadline}s"))?
            .context("timer poll")?,
        MonthlyTimerPollForTest {
            now: Duration::from_secs(now),
            deadline: Duration::from_secs(deadline),
        }
    );
    Ok(())
}
