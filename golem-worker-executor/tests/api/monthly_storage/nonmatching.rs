use super::*;
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
#[timeout("2m")]
async fn durable_pending_nonmatching_scripted_storage_remains_running(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    nonmatching_storage(last_unique_id, deps, host_api_tests, AgentMode::Durable).await
}

#[test]
#[timeout("2m")]
async fn ephemeral_pending_nonmatching_scripted_storage_remains_running(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    nonmatching_storage(last_unique_id, deps, host_api_tests, AgentMode::Ephemeral).await
}

async fn nonmatching_storage(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    mode: AgentMode,
) -> anyhow::Result<()> {
    let policy = MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: 0,
        available_memory_gb_seconds: 0,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    };
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let metering = ResourceUsageMeteringConfig {
        compute: false,
        memory: false,
        filesystem: true,
    };
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
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(
            &component.id,
            agent_id!("Networking", "nonmatching-storage-seed"),
        )
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .unwrap()
        .primary();
    wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while seed.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("seed permit release")?;
    let durable = mode == AgentMode::Durable;
    let other = if durable {
        AgentMode::Ephemeral
    } else {
        AgentMode::Durable
    };
    let name = if durable {
        agent_id!("Networking", "nonmatching-storage")
    } else {
        agent_id!("EphemeralNetworking", "nonmatching-storage")
    };
    let key = IdempotencyKey::fresh();
    let physical = if durable {
        name.clone()
    } else {
        name.clone()
            .with_ephemeral_invocation_phantom(&key)
            .unwrap()
    };
    let id = AgentId::from_agent_id(component.id, &physical).unwrap();
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let worker = Worker::get_or_create_suspended(
        seed.all(),
        &owned,
        None,
        vec![],
        None,
        None,
        &InvocationContextStack::fresh(),
        Principal::anonymous(),
    )
    .await?;
    let source = ScriptedFilesystemUsageForTest::new(FilesystemUsage::Authoritative {
        allocated_bytes: 0,
        filesystem_objects: 0,
    });
    worker
        .owner_runtime_resources()
        .set_scripted_filesystem_usage_for_test(source.clone());
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let invocation = tokio::spawn({
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
    });
    let (mut socket, _) =
        tokio::time::timeout(Duration::from_secs(30), listener.accept()).await??;
    let chunk = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            executor.commit_oplog(&id).await?;
            let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            if let Some(entry) = entries.iter().find(|entry| {
                matches!(&entry.entry, PublicOplogEntry::Start(start)
                    if start.function_name == "sockets::types::tcp-socket::receive-chunk")
            }) {
                return Ok::<_, anyhow::Error>(entry.oplog_index);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("open receive chunk")??;
    tokio::time::timeout(Duration::from_secs(10), polls.recv())
        .await?
        .unwrap();
    let initial = worker.current_monthly_proposal_for_test();
    let acquisitions = worker.permit_acquisitions_for_test();
    let mut raw = worker.raw_interrupt_for_test();
    let account = limits.initialize_account(context.account_id).await?;
    assert_eq!(initial.exhaustion, None);
    assert!(!account.monthly_capacity_is_exhausted_for_test(mode));
    assert!(!account.monthly_capacity_is_exhausted_for_test(other));

    let observations_before_allocation = source.observations();
    source.set(FilesystemUsage::Authoritative {
        allocated_bytes: 101,
        filesystem_objects: 1,
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while source.observations() == observations_before_allocation {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("positive scripted observation")?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    while polls.try_recv().is_ok() {}
    let mut opposite_exhausted = policy;
    if durable {
        opposite_exhausted.available_ephemeral_storage_byte_seconds = 0;
    } else {
        opposite_exhausted.available_durable_storage_byte_seconds = 0;
    }
    registry.set_policy(opposite_exhausted);
    let revision = registry.current_limits().monthly_policy_revision;
    assert!(revision > initial.policy_revision);
    let refresh = monthly::applied_refresh(&limits, &registry, context.account_id).await?;
    // Wait until the applied-update check finishes and rearms before advancing a separate tick.
    let update_poll = tokio::time::timeout(Duration::from_secs(10), polls.recv())
        .await
        .context("applied update checked")?
        .unwrap();
    assert_eq!(update_poll.now, Duration::ZERO);
    assert_eq!(update_poll.deadline, Duration::from_secs(30));
    assert_eq!(
        limits
            .initialized_policy_revision_for_test(context.account_id)
            .await,
        Some(revision)
    );
    assert!(account.monthly_capacity_is_exhausted_for_test(other));
    assert!(!account.monthly_capacity_is_exhausted_for_test(mode));

    // Positive matching byte-time proves the scripted filesystem meter was actually enabled.
    // These are scripted observations, not native-XFS allocation measurements.
    let updates = registry.applied_updates();
    assert!(updates.iter().any(|update| {
        update.monthly_policy_revision == initial.policy_revision
            && update.period == initial.period
            && if durable {
                update.durable_storage_byte_seconds_delta > 0
            } else {
                update.ephemeral_storage_byte_seconds_delta > 0
            }
    }));
    for update in updates {
        assert_eq!(update.fuel_delta, 0);
        assert_eq!(update.memory_gb_seconds_delta, 0);
        assert_eq!(update.memory_byte_nanoseconds_remainder, 0);
        assert_eq!(
            if durable {
                update.ephemeral_storage_byte_seconds_delta
            } else {
                update.durable_storage_byte_seconds_delta
            },
            0
        );
    }
    clock.advance(Duration::from_secs(30));
    let tick_poll = tokio::time::timeout(Duration::from_secs(10), polls.recv())
        .await?
        .unwrap();
    assert_eq!(tick_poll.now, Duration::from_secs(30));
    assert_eq!(tick_poll.deadline, Duration::from_secs(30));
    let rearmed = tokio::time::timeout(Duration::from_secs(10), polls.recv())
        .await?
        .unwrap();
    assert_eq!(rearmed.now, Duration::from_secs(30));
    assert_eq!(rearmed.deadline, Duration::from_secs(60));
    worker.drain_lifecycle_for_test().await?;
    let sampled = worker.current_monthly_proposal_for_test();
    assert_eq!(sampled.policy_revision, revision);
    assert_eq!(sampled.period, initial.period);
    assert_eq!(sampled.window_identity, initial.window_identity);
    assert_eq!(sampled.resident_generation, initial.resident_generation);
    assert_eq!(sampled.exhaustion, None);
    assert!(account.monthly_capacity_is_exhausted_for_test(other));
    assert!(!account.monthly_capacity_is_exhausted_for_test(mode));
    assert!(matches!(
        attempts.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(worker.monthly_stop_for_test(), None);
    let mut byte = [0];
    assert!(
        tokio::time::timeout(Duration::from_millis(100), socket.read(&mut byte))
            .await
            .is_err(),
        "opposite storage exhaustion must not close the silent socket"
    );
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert_eq!(clock.active_sleeps(), 1);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    assert!(!invocation.is_finished());
    executor.commit_oplog(&id).await?;
    let pending = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    assert_eq!(
        count_agent_invocation_pair_since(&pending, OplogIndex::INITIAL),
        (2, 1)
    );
    assert!(pending.iter().all(
        |entry| !matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == chunk)
    ));
    assert!(pending.iter().all(|entry| !matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == chunk)));

    executor.interrupt(&id).await?;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
        InterruptKind::Interrupt(_)
    ));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), socket.read(&mut byte)).await??,
        0
    );
    let error = tokio::time::timeout(Duration::from_secs(10), invocation)
        .await??
        .expect_err("ordinary user interruption must terminate the pending invocation");
    assert!(!error.to_string().contains("monthly"));
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    worker.join_accepted_stops_for_test().await?;
    worker.retained_cleanup_for_test().await?;
    tokio::time::timeout(Duration::from_secs(10), refresh).await??;
    assert!(attempts.try_recv().is_err());
    assert_eq!(clock.active_sleeps(), 0);
    assert_eq!(account.monthly_observer_count_for_test(), 0);
    shutdown.cancel();
    Ok(())
}
