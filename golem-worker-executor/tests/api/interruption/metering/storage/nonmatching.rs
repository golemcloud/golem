use super::*;
use pretty_assertions::assert_eq;

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
async fn durable_pending_nonmatching_managed_xfs_storage_remains_running(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    nonmatching_storage(last_unique_id, deps, host_api_tests, AgentMode::Durable).await
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
async fn ephemeral_pending_nonmatching_managed_xfs_storage_remains_running(
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
    let filesystem = managed_filesystem_config(metering)?;
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| {
            config.agent_sockets.enabled = true;
            config.resource_usage_metering = metering;
            config.filesystem_storage.mode = filesystem.clone().unwrap();
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
        while seed.eviction_class().await.is_none() {
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
    let initial_revision = registry.current_limits().monthly_policy_revision;
    let initial_period = policy.period;
    let mut next_flush = async |revision| {
        loop {
            let observed_revision = flushes.recv().await.context("owner usage flusher closed")?;
            if observed_revision == revision {
                return Ok::<_, anyhow::Error>(());
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(35), next_flush(initial_revision))
        .await
        .context("initial usage flush")??;
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
    let mut raw = worker.raw_interrupt_for_test();
    let account = limits.initialize_account(context.account_id).await?;
    assert!(!account.monthly_capacity_is_exhausted_for_test(mode));
    assert!(!account.monthly_capacity_is_exhausted_for_test(other));

    allocate_storage(&owned).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            limits.run_batch_for_test().await;
            if registry.applied_updates().iter().any(|update| {
                if durable {
                    update.durable_storage_byte_seconds_delta > 0
                } else {
                    update.ephemeral_storage_byte_seconds_delta > 0
                }
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("positive managed XFS storage usage")?;
    let capacity_version = *capacity_updates.borrow_and_update();
    let mut opposite_exhausted = policy;
    if durable {
        opposite_exhausted.available_ephemeral_storage_byte_seconds = 0;
    } else {
        opposite_exhausted.available_durable_storage_byte_seconds = 0;
    }
    registry.set_policy(opposite_exhausted);
    let revision = registry.current_limits().monthly_policy_revision;
    assert!(revision > initial_revision);
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

    // Positive matching byte-time proves the managed XFS filesystem meter was enabled.
    let updates = registry.applied_updates();
    assert!(updates.iter().any(|update| {
        update.monthly_policy_revision == initial_revision
            && update.period == initial_period
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
    // A dependency flush observes the applied policy; it does not identify a monitor tick.
    tokio::time::timeout(Duration::from_secs(35), next_flush(revision)).await??;
    assert!(!capacity_updates.has_changed()?);
    drop(capacity_updates);
    assert!(account.monthly_capacity_is_exhausted_for_test(other));
    assert!(!account.monthly_capacity_is_exhausted_for_test(mode));
    assert!(matches!(
        raw.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    let mut byte = [0];
    assert!(
        tokio::time::timeout(Duration::from_millis(100), socket.read(&mut byte))
            .await
            .is_err(),
        "opposite storage exhaustion must not close the silent socket"
    );
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
        while worker.is_loaded().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    worker.join_accepted_stops_for_test().await?;
    worker.retained_cleanup_for_test().await?;
    tokio::time::timeout(Duration::from_secs(10), refresh).await??;
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
