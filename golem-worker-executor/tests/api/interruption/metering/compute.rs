use super::*;
use anyhow::Context as _;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::resource_limits::ResourceUsageFlusher;
use golem_worker_executor::services::{HasOplog, HasOplogService, UsesAllDeps};
use golem_worker_executor::worker::Worker;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::AsyncReadExt;

#[test]
#[timeout("2m")]
async fn durable_compute_reservation_survives_until_policy_exhaustion_then_suspends(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    pending_compute(last_unique_id, deps, host_api_tests, AgentMode::Durable).await
}

#[test]
#[timeout("2m")]
async fn ephemeral_compute_reservation_survives_until_policy_exhaustion_then_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    pending_compute(last_unique_id, deps, host_api_tests, AgentMode::Ephemeral).await
}

async fn pending_compute(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    mode: AgentMode,
) -> anyhow::Result<()> {
    const BUDGET: u64 = 1_000_000_000;
    let policy = MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: BUDGET,
        available_memory_gb_seconds: 0,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: 0,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: 0,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    };
    let metering = ResourceUsageMeteringConfig {
        compute: true,
        memory: false,
        filesystem: false,
    };
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let shutdown = CancellationToken::new();
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
        Arc::new(move |config| {
            config.agent_sockets.enabled = true;
            config.resource_usage_metering = metering;
            config.limits.fuel_to_borrow = BUDGET;
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("Networking", "pending-compute")
    } else {
        agent_id!("EphemeralNetworking", "pending-compute")
    };
    let key = IdempotencyKey::fresh();
    let (id, boundary) = if durable {
        let id = executor.start_agent(&component.id, name.clone()).await?;
        wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
        let boundary = executor.oplog_max_index(&id).await?;
        (id, boundary)
    } else {
        let finalized = name
            .clone()
            .with_ephemeral_invocation_phantom(&key)
            .map_err(|e| anyhow!(e))?;
        (
            AgentId::from_agent_id(component.id, &finalized).map_err(|e| anyhow!(e))?,
            OplogIndex::INITIAL,
        )
    };
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let worker = if durable {
        let worker = executor
            .production_active_agent(&owned)
            .await
            .unwrap()
            .primary();
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.eviction_class().await.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        worker
    } else {
        let seed_id = executor
            .start_agent(&component.id, agent_id!("Networking", "compute-seed"))
            .await?;
        let seed = executor
            .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
            .await
            .unwrap()
            .primary();
        wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while seed.eviction_class().await.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        Worker::get_or_create_suspended(
            seed.all(),
            &owned,
            None,
            vec![],
            None,
            None,
            &InvocationContextStack::fresh(),
            Principal::anonymous(),
        )
        .await?
    };
    // Observe calls to the registered usage dependency, without attributing them to a timer.
    let (flushed, mut flushes) = tokio::sync::mpsc::unbounded_channel();
    let usage_source: Arc<dyn ResourceUsageFlusher> = Arc::new(ObservedUsageFlusher {
        owner: Arc::downgrade(&worker),
        send: flushed,
    });
    worker
        .owner_runtime_resources()
        .register_resource_usage_flusher(Arc::downgrade(&usage_source));
    let mut capacity_updates = limits
        .subscribe_capacity_updates_for_test(context.account_id)
        .await?;
    capacity_updates.borrow_and_update();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let invocation = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
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
                    "tcp_collect_p3",
                    data_value!(port),
                )
                .await
        }
    }));
    let (mut socket, _) =
        tokio::time::timeout(Duration::from_secs(30), listener.accept()).await??;
    let starts = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            executor.commit_oplog(&id).await?;
            let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            let starts: Vec<_> = entries
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(start)
                        if entry.oplog_index > boundary
                            && matches!(
                                start.function_name.as_str(),
                                "sockets::types::tcp-socket::receive"
                                    | "sockets::types::tcp-socket::receive-chunk"
                            ) =>
                    {
                        Some(entry.oplog_index)
                    }
                    _ => None,
                })
                .collect();
            if starts.len() == 2 {
                assert_open(&entries, &starts);
                assert_eq!(
                    count_agent_invocation_pair_since(&entries, boundary),
                    if durable { (1, 0) } else { (2, 1) }
                );
                return Ok::<_, anyhow::Error>(starts);
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    let oplog = worker.all().oplog_service();
    let read_stored = async || {
        let last = oplog.get_last_index(&owned, mode).await?;
        oplog
            .read_exact(&owned, mode, OplogIndex::INITIAL, last.as_u64())
            .await
    };
    let prefix = read_stored().await?;
    let prefix_last = *prefix
        .last_key_value()
        .context("committed invocation prefix is empty")?
        .0;
    let account = limits.initialize_account(context.account_id).await?;
    assert!(
        account.monthly_capacity_is_exhausted_for_test(mode),
        "the Store prepaid the remaining account fuel"
    );
    assert!(!invocation.is_finished());

    let mut raw = worker.raw_interrupt_for_test();
    tokio::time::timeout(Duration::from_secs(35), flushes.recv())
        .await?
        .context("usage dependency was not flushed")?;
    assert!(!capacity_updates.has_changed()?);
    drop(capacity_updates);
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(!invocation.is_finished());
    let mut exhausted = policy.clone();
    exhausted.available_fuel = 0;
    registry.set_policy(exhausted);
    // This is authoritative external account exhaustion, not fuel burned by the parked guest.
    let refresh = applied_refresh(&limits, &registry, context.account_id).await?;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(30), raw.recv()).await??,
        InterruptKind::Suspend(_)
    ));
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), socket.read(&mut byte)).await??,
        0
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        while worker.is_loaded().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    worker.join_accepted_stops_for_test().await?;
    worker.retained_cleanup_for_test().await?;
    refresh.await?;
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await?;
    let stopped_raw = read_stored().await?;
    assert_eq!(
        stopped_raw.range(..=prefix_last).collect::<Vec<_>>(),
        prefix.iter().collect::<Vec<_>>()
    );
    assert!(
        !stopped_raw
            .values()
            .any(|entry| matches!(entry, OplogEntry::Jump { .. }))
    );
    let errors: Vec<_> = entries
        .values()
        .filter_map(|entry| match entry {
            OplogEntry::Error { error, .. } => Some(error),
            _ => None,
        })
        .collect();
    if durable {
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            entries
                .values()
                .any(|e| matches!(e, OplogEntry::Suspend { .. }))
        );
        let suspended = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_open(&suspended, &starts);
        assert_eq!(
            count_agent_invocation_pair_since(&suspended, boundary),
            (1, 0)
        );
        assert!(!invocation.is_finished());
        registry.set_policy(policy);
        applied_refresh(&limits, &registry, context.account_id)
            .await?
            .await?;
        executor.resume(&id, false).await?;
        let result = tokio::time::timeout(Duration::from_secs(30), invocation)
            .await???
            .into_typed::<Result<String, String>>()?;
        assert_eq!(result, Err("ErrorCode::ConnectionBroken".to_string()));
        let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let recovered_raw = read_stored().await?;
        let stopped_last = *stopped_raw.last_key_value().unwrap().0;
        assert_eq!(
            recovered_raw.range(..=stopped_last).collect::<Vec<_>>(),
            stopped_raw.iter().collect::<Vec<_>>()
        );
        assert_eq!(
            count_agent_invocation_pair_since(&recovered, boundary),
            (1, 1)
        );
        // Connect completed before suspension, so its replay must not repeat the external effect.
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    } else {
        assert!(
            matches!(errors.as_slice(), [AgentError::EphemeralFuelExhausted(_)]),
            "{errors:?}"
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(30), invocation)
                .await??
                .is_err()
        );
        assert!(
            !entries
                .values()
                .any(|e| matches!(e, OplogEntry::Suspend { .. }))
        );
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.is_loaded().await && worker.eviction_class().await.is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    limits.run_batch_for_test().await;
    let updates = registry.applied_updates();
    assert!(updates.iter().any(|u| u.fuel_delta > 0));
    let settled_fuel: i128 = updates
        .iter()
        .map(|update| i128::from(update.fuel_delta))
        .sum();
    assert!(
        settled_fuel > 0 && settled_fuel < i128::from(BUDGET),
        "unused prepaid fuel must be refunded: {settled_fuel}"
    );
    for update in updates {
        assert_eq!(update.memory_gb_seconds_delta, 0);
        assert_eq!(update.memory_byte_nanoseconds_remainder, 0);
        assert_eq!(update.durable_storage_byte_seconds_delta, 0);
        assert_eq!(update.ephemeral_storage_byte_seconds_delta, 0);
        assert_eq!(update.durable_storage_byte_nanoseconds_remainder, 0);
        assert_eq!(update.ephemeral_storage_byte_nanoseconds_remainder, 0);
    }
    shutdown.cancel();
    Ok(())
}

fn assert_open(
    entries: &[golem_common::model::oplog::PublicOplogEntryWithIndex],
    starts: &[OplogIndex],
) {
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::End(end) => assert!(!starts.contains(&end.start_index)),
            PublicOplogEntry::Cancelled(cancelled) => {
                assert!(!starts.contains(&cancelled.start_index))
            }
            _ => {}
        }
    }
}
