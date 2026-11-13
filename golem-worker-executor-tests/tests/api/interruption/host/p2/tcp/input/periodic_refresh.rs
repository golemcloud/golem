use super::*;
use test_r::test;

#[test]
#[timeout("2m")]
async fn durable_periodic_refresh_stops_overage_tcp_after_owner_selects_hard_limit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    periodic_refresh_stops_overage_tcp(last_unique_id, deps, component, AgentMode::Durable).await
}

#[test]
#[timeout("2m")]
async fn ephemeral_periodic_refresh_stops_overage_tcp_after_owner_selects_hard_limit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    periodic_refresh_stops_overage_tcp(last_unique_id, deps, component, AgentMode::Ephemeral).await
}

async fn periodic_refresh_stops_overage_tcp(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    mode: AgentMode,
) -> anyhow::Result<()> {
    let metering = Resource::Memory.metering();
    let mut policy = Resource::Memory.policy();
    policy.mode = MonthlyUsageMode::AllowOverage;
    policy.available_memory_gb_seconds = 0;
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let shutdown = CancellationToken::new();
    let _shutdown_guard = shutdown.clone().drop_guard();
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_millis(100),
        Duration::ZERO,
        metering,
        shutdown,
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
            agent_id!("Networking", "periodic-refresh-seed"),
        )
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .unwrap()
        .primary();
    wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL)
        .await
        .context("seed initialization")?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !seed.stop_if_idle().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("seed idle teardown")?;
    limits.initialize_account(context.account_id).await?;
    // Observe ordinary ticks after seed settlement; do not drive a second batch beside the timer.
    let mut updates = limits
        .subscribe_capacity_updates_for_test(context.account_id)
        .await?;
    for _ in 0..2 {
        updates.borrow_and_update();
        tokio::time::timeout(Duration::from_secs(10), updates.changed())
            .await
            .context("ordinary seed refresh tick")??;
    }
    drop(updates);
    let baseline_updates = registry.applied_updates().len();
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("Networking", "periodic-policy-refresh")
    } else {
        agent_id!("EphemeralNetworking", "periodic-policy-refresh")
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let mut invocation = Some(AbortOnDropHandle::new(tokio::spawn({
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
                    Operation::Read.method(),
                    data_value!(port),
                )
                .await
        }
    })));
    let mut refresh = None;
    let result = async {
        let (mut peer, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept()).await.context("initial TCP connection")??;
        let (initial, connect_start) = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                executor.commit_oplog(&id).await?;
                let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                if let Ok(start) = completed_setup(&entries) {
                    break Ok::<_, anyhow::Error>((entries, start));
                }
                ensure!(!request_is_finished(&invocation));
                tokio::task::yield_now().await;
            }
        }).await??;
        let admitted_revision = registry.current_limits().monthly_policy_revision;
        ensure!(count_agent_invocation_pair_since(&initial, OplogIndex::INITIAL) == (2, 1));
        let stored = worker.all().oplog_service();
        let read_stored = async || {
            let last = stored.get_last_index(&owned, mode).await?;
            stored.read_exact(&owned, mode, OplogIndex::INITIAL, last.as_u64()).await
        };
        let prefix = read_stored().await?;
        let last = *prefix.last_key_value().context("committed invocation prefix")?.0;
        let mut raw = worker.raw_interrupt_for_test();
        let mut updates = limits.subscribe_capacity_updates_for_test(context.account_id).await?;
        updates.borrow_and_update();
        tokio::time::timeout(Duration::from_secs(10), updates.changed()).await.context("ordinary overage refresh with a silent TCP peer")??;
        ensure!(!request_is_finished(&invocation), "overage must keep the silent invocation live");
        {
            let mut current = registry.limits.lock().unwrap();
            current.monthly_policy.mode = MonthlyUsageMode::HardLimit;
            current.monthly_usage_mode_revision += 1;
            current.monthly_policy_revision += 1;
        }
        ensure!(matches!(tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??, InterruptKind::Suspend(_)));
        let mut byte = [0];
        ensure!(tokio::time::timeout(Duration::from_secs(10), peer.read(&mut byte)).await?? == 0);
        tokio::time::timeout(Duration::from_secs(10), async {
            worker.join_accepted_stops_for_test().await?;
            while worker.is_loaded().await { tokio::task::yield_now().await; }
            worker.retained_cleanup_for_test().await
        }).await??;
        ensure!(read_stored().await?.range(..=last).eq(prefix.iter()), "stop changed the full committed prefix");
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(completed_setup(&stopped)? == connect_start);
        let raw_entries = read_stored().await?;
        let errors: Vec<_> = raw_entries.values().filter_map(|entry| match entry { OplogEntry::Error { error, .. } => Some(error), _ => None }).collect();
        let suspends = raw_entries.values().filter(|entry| matches!(entry, OplogEntry::Suspend { .. })).count();
        if durable {
            ensure!(errors.is_empty() && suspends == 1);
            ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
            ensure!(!request_is_finished(&invocation));
        } else {
            ensure!(suspends == 0);
            ensure!(matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)] if reason.reason == "monthly memory exhausted"), "{errors:?}");
            ensure!(join_request(&mut invocation, "ephemeral monthly result").await?.is_err());
        }
        for _ in 0..2 {
            updates.borrow_and_update();
            tokio::time::timeout(Duration::from_secs(10), updates.changed()).await.context("ordinary post-close settlement tick")??;
        }
        let settled = registry.applied_updates();
        check_accounting(&settled[baseline_updates..], Resource::Memory, durable, admitted_revision, policy.period)?;
        let settled_count = settled.len();
        updates.borrow_and_update();
        tokio::time::timeout(Duration::from_secs(10), updates.changed()).await.context("ordinary closed-window zero-usage tick")??;
        for update in &registry.applied_updates()[settled_count..] {
            ensure!(update.memory_gb_seconds_delta == 0 && update.memory_byte_nanoseconds_remainder == 0, "closed window accrued usage: {update:?}");
        }
        policy.mode = MonthlyUsageMode::HardLimit;
        policy.available_memory_gb_seconds = u64::MAX;
        registry.set_policy(policy.clone());
        let grant_revision = registry.current_limits().monthly_policy_revision;
        tokio::time::timeout(Duration::from_secs(10), async {
            while limits.initialized_policy_revision_for_test(context.account_id).await != Some(grant_revision) { updates.changed().await?; }
            Ok::<_, anyhow::Error>(())
        }).await??;
        drop(updates);
        if durable {
            executor.resume(&id, false).await?;
            let (mut peer, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept()).await??;
            peer.write_all(b"x").await?;
            Operation::Read.assert_result(join_request(&mut invocation, "durable continuation").await??)?;
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(completed_setup(&recovered)? == connect_start);
            ensure!(read_stored().await?.range(..=last).eq(prefix.iter()), "recovery changed the full committed prefix");
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            assert_settled_history(&recovered)?;
            Operation::Read.assert_result(executor.invoke_and_await_agent_with_key(&component, &name, &key, Operation::Read.method(), data_value!(port)).await?)?;
        } else {
            ensure!(executor.invoke_and_await_agent_with_key(&component, &name, &key, Operation::Read.method(), data_value!(port)).await.is_err());
            let probe_key = IdempotencyKey::fresh();
            let probe = executor.invoke_and_await_agent_with_key(&component, &name, &probe_key, Operation::Read.method(), data_value!(port));
            let probe_peer = async {
                let (mut peer, _) = listener.accept().await?;
                peer.write_all(b"x").await?;
                ensure!(peer.read(&mut byte).await? == 0);
                Ok::<_, anyhow::Error>(())
            };
            let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async { tokio::try_join!(probe, probe_peer) }).await??;
            Operation::Read.assert_result(result)?;
        }
        ensure!(tokio::time::timeout(Duration::from_millis(100), listener.accept()).await.is_err(), "a terminal/cached original key must not reconnect");
        Ok::<_, anyhow::Error>(())
    }.await;
    drop(listener);
    finish_input_test(
        result,
        &executor,
        &id,
        &worker,
        &mut invocation,
        &mut refresh,
    )
    .await
}
