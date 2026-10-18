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
        Arc::new(move |config| {
            config.agent_sockets.enabled = true;
            config.resource_usage_metering = metering;
        }),
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
    let capacity_version = *capacity_updates.borrow_and_update();
    let mut attempts =
        crate::monthly_acceptance::observe(worker.get_initial_worker_metadata().fingerprint);
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
    let initial = worker.current_monthly_proposal_for_test();
    let mut next_flush = async |revision| {
        loop {
            let (window, observed_revision) =
                flushes.recv().await.context("owner usage flusher closed")?;
            if window == initial.diagnostic_window_id && observed_revision == revision {
                return Ok::<_, anyhow::Error>(());
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), next_flush(initial.policy_revision))
        .await
        .context("initial monitor pre-wait flush")??;
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
    let acquisitions = worker.permit_acquisitions_for_test();
    let mut raw = worker.raw_interrupt_for_test();
    let account = limits.initialize_account(context.account_id).await?;
    assert_eq!(account.monthly_observer_count_for_test(), 2);
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
    let mut opposite_exhausted = policy;
    if durable {
        opposite_exhausted.available_ephemeral_storage_byte_seconds = 0;
    } else {
        opposite_exhausted.available_durable_storage_byte_seconds = 0;
    }
    let update_started = tokio::time::Instant::now();
    registry.set_policy(opposite_exhausted);
    let revision = registry.current_limits().monthly_policy_revision;
    assert!(revision > initial.policy_revision);
    let refresh = applied_refresh(&limits, &registry, context.account_id).await?;
    tokio::time::timeout(Duration::from_secs(10), capacity_updates.changed())
        .await
        .context("opposite storage policy was not applied")??;
    assert_eq!(
        *capacity_updates.borrow_and_update(),
        capacity_version.wrapping_add(1)
    );
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
    // Healthy iterations flush after wake and before the next wait. The first new-
    // revision observation can already be pre-wait; six observations cover at least
    // two completed checks. If the first wake was a tick racing the policy update,
    // they also include the pending update check and a later complete periodic check.
    // Memory metering is off, and no further capacity update may supply those wakes.
    // These are dependency observations, not assertions that a sleep was polled.
    for _ in 0..6 {
        next_flush(revision).await?;
    }
    assert!(update_started.elapsed() >= Duration::from_secs(30));
    assert!(!capacity_updates.has_changed()?);
    drop(capacity_updates);
    worker.drain_lifecycle_for_test().await?;
    let sampled = worker.current_monthly_proposal_for_test();
    assert_eq!(sampled.policy_revision, revision);
    assert_eq!(sampled.period, initial.period);
    assert_eq!(sampled.diagnostic_window_id, initial.diagnostic_window_id);
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
    assert!(worker.monthly_window_active_for_test());
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
    assert!(!worker.monthly_window_active_for_test());
    assert_eq!(account.monthly_observer_count_for_test(), 0);
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
    shutdown.cancel();
    Ok(())
}
