use super::*;
use anyhow::{Context as _, ensure};
use golem_worker_executor::worker::{
    MonthlyClockForTest, WebSocketReaderLockStateForTest, WebSocketTimedReceiveStateForTest, Worker,
};
use test_r::test;

const TIMED_RECEIVE: &str = "golem:websocket/client::receive-with-timeout";

#[test]
#[timeout("2m")]
async fn durable_websocket_timed_reader_lock_native_pending(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let policy = Resource::Memory.policy();
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy));
    let shutdown = CancellationToken::new();
    let _shutdown_guard = shutdown.clone().drop_guard();
    let limits = ResourceLimitsGrpc::new(
        registry,
        Duration::from_secs(3600),
        Duration::ZERO,
        Resource::Memory.metering(),
        shutdown,
    );
    let context = TestContext::new(last_unique_id);
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| {
            config.resource_usage_metering = Resource::Memory.metering();
            config.limits.fuel_to_borrow = FUEL_BUDGET;
            config.max_websocket_connections = 1;
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(&component.id, agent_id!("WebsocketTest", "lock-seed"))
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .context("seed Worker missing")?
        .primary();
    wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while seed.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    limits.run_batch_for_test().await;
    let name = agent_id!("WebsocketTest", "lock-contention");
    let key = IdempotencyKey::fresh();
    let id = AgentId::from_agent_id(component.id, &name).unwrap();
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
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut pending = worker.observe_websocket_reader_lock_for_test();
    let mut peer = Peer::start().await?;
    let mut invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let key = key.clone();
        let name = name.clone();
        let url = peer.url.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    "receive_lock_contention",
                    data_value!(url, 300_000u64),
                )
                .await
        }
    });
    let result = async {
        let handshake = peer.handshake().await?;
        ensure!(handshake.number == 1);
        let observed = tokio::time::timeout(Duration::from_secs(10), pending.recv())
            .await.context("timed reader.lock() native first Pending not observed")?
            .context("native reader-lock observer closed")?;
        tokio::time::timeout(Duration::from_secs(10), polls.recv()).await?.context("monthly clock closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        executor.commit_oplog(&id).await?;
        let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let receives = starts_named(&entries, RECEIVE);
        let timed = starts_named(&entries, TIMED_RECEIVE);
        ensure!(receives.len() == 1 && timed.len() == 1 && receives[0] < timed[0], "not two ordered receive Starts: {entries:#?}");
        ensure!(starts_named(&entries, CONNECT).len() == 1);
        ensure!(observed.start == timed[0] && observed.invocation_key.as_ref() == Some(&key));
        ensure!(observed.runtime == initial && observed.path == "timed receive reader.lock");
        ensure!(observed.state() == WebSocketReaderLockStateForTest::Pending);
        for start in [receives[0], timed[0]] {
            assert_receive_terminal(&entries, start, 0)?;
        }
        assert_invocation(&entries, &key, 0)?;
        ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
        ensure!(worker.concurrent_agent_permit_is_held().await && worker.is_loaded().await);
        ensure!(!invocation.is_finished());
        peer.assert_counts(1, 0)?;
        let pool = worker.websocket_connection_pool();
        {
            let acquire = pool.acquire();
            tokio::pin!(acquire);
            ensure!(futures::poll!(acquire).is_pending());
        }
        info!(%key, %id, ?initial, untimed_start = %receives[0], timed_start = %timed[0], "Native timed reader.lock() Pending while silent first read holds guard");
        let first_frame = handshake.send;
        // Only retire the Worker after checking the native lock; a dropped frame gate must not unblock either read.
        tokio::time::timeout(Duration::from_secs(10), executor.interrupt(&id)).await??;
        peer.closed(1).await?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
        ensure!(matches!(observed.state(), WebSocketReaderLockStateForTest::Acquired | WebSocketReaderLockStateForTest::Dropped));
        peer.assert_counts(1, 0)?;
        drop(first_frame);
        Ok::<_, anyhow::Error>(())
    }.await;
    let mut cleanup_errors = Vec::new();
    handshake::retire_handshake_worker(
        &executor,
        &worker,
        &id,
        !invocation.is_finished(),
        &mut cleanup_errors,
    )
    .await;
    pool::settle_invocation("lock reachability", &mut invocation, &mut cleanup_errors).await;
    if let Err(error) = peer.finish().await {
        cleanup_errors.push(format!("peer shutdown: {error:#}"));
    }
    if cleanup_errors.is_empty() {
        result
    } else {
        Err(anyhow::anyhow!(
            "{result:?}; cleanup: {}",
            cleanup_errors.join("; ")
        ))
    }
}

macro_rules! lock_case {
    ($name:ident, $resource:ident, $mode:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            pending_reader_lock(
                last_unique_id,
                deps,
                host_api_tests,
                Resource::$resource,
                AgentMode::$mode,
            )
            .await
        }
    };
}
lock_case!(
    durable_websocket_reader_lock_stop_only_monthly_memory,
    Memory,
    Durable
);
lock_case!(
    ephemeral_websocket_reader_lock_stop_only_monthly_memory,
    Memory,
    Ephemeral
);
lock_case!(
    durable_websocket_reader_lock_stop_only_monthly_compute_prepaid,
    Compute,
    Durable
);
lock_case!(
    ephemeral_websocket_reader_lock_stop_only_monthly_compute_prepaid,
    Compute,
    Ephemeral
);
lock_case!(
    durable_websocket_reader_lock_stop_only_monthly_scripted_storage,
    Storage,
    Durable
);
lock_case!(
    ephemeral_websocket_reader_lock_stop_only_monthly_scripted_storage,
    Storage,
    Ephemeral
);

fn lock_receive_starts(
    entries: &[PublicOplogEntryWithIndex],
    terminals: usize,
) -> anyhow::Result<(OplogIndex, OplogIndex)> {
    let untimed = starts_named(entries, RECEIVE);
    let timed = starts_named(entries, TIMED_RECEIVE);
    let connect = starts_named(entries, CONNECT);
    ensure!(
        connect.len() == 1
            && untimed.len() == 1
            && timed.len() == 1
            && connect[0] < untimed[0]
            && untimed[0] < timed[0],
        "one ordered Connect/untimed/timed Start: {entries:#?}"
    );
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(start) => {
                ensure!(start.parent_start_index.is_none() && start.observational_owner.is_none());
                if entry.oplog_index == timed[0] {
                    ensure!(matches!(
                        start.durable_function_type,
                        PublicDurableFunctionType::WriteRemote(_)
                    ));
                    ensure!(
                        format!("{:?}", start.request).contains("300000"),
                        "wrong timeout request: {start:?}"
                    );
                }
                ensure!(
                    terminal_count(entries, entry.oplog_index)
                        == if entry.oplog_index == untimed[0] || entry.oplog_index == timed[0] {
                            terminals
                        } else {
                            1
                        }
                );
            }
            PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::CompletionDiscarded(_)
            | PublicOplogEntry::Jump(_) => anyhow::bail!("unexpected terminal/jump: {entry:?}"),
            _ => {}
        }
    }
    for start in [untimed[0], timed[0]] {
        assert_receive_terminal(entries, start, terminals)?;
    }
    Ok((untimed[0], timed[0]))
}

async fn pending_reader_lock(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    resource: Resource,
    mode: AgentMode,
) -> anyhow::Result<()> {
    let policy = resource.policy();
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let metering = resource.metering();
    let shutdown = CancellationToken::new();
    let _shutdown_guard = shutdown.clone().drop_guard();
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_secs(3600),
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
            config.resource_usage_metering = metering;
            config.limits.fuel_to_borrow = FUEL_BUDGET;
            config.max_websocket_connections = 1;
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(&component.id, agent_id!("WebsocketTest", "monthly-ws-seed"))
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
    // Settle the seed before the target's accounting interval begins.
    limits.run_batch_for_test().await;
    if resource == Resource::Memory {
        registry.set_policy(policy.clone());
        applied_refresh(&limits, &registry, context.account_id)
            .await?
            .await?;
        limits.run_batch_for_test().await;
    }
    let target_updates_start = registry.applied_updates().len();
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("WebsocketTest", "monthly-ws-timed-receive")
    } else {
        agent_id!("EphemeralWebsocketTest", "monthly-ws-timed-receive")
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
    // Script only authoritative allocation, not filesystem deletion, metering or permit release.
    let source = ScriptedFilesystemUsageForTest::new(FilesystemUsage::Authoritative {
        allocated_bytes: 0,
        filesystem_objects: 0,
    });
    if resource == Resource::Storage {
        worker
            .owner_runtime_resources()
            .set_scripted_filesystem_usage_for_test(source.clone());
    }
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let mut peer = Peer::start().await?;
    let mut native = worker.observe_websocket_reader_lock_for_test();
    let mut invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let name = name.clone();
        let key = key.clone();
        let url = peer.url.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    "receive_lock_contention",
                    data_value!(url, 300_000u64),
                )
                .await
        }
    });
    let mut target_joined = false;
    let mut probe_id = None;
    let mut first_gate = None;
    let mut second_gate = None;
    let result = async {
        let first = peer.handshake().await?;
        first_gate = Some(first.send);
        second_gate = Some(first.send_second);
        ensure!(first.number == 1);
        let pending = tokio::time::timeout(Duration::from_secs(10), native.recv())
            .await?.context("native timed reader.lock() never returned Pending")?;
        let receive_start = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                executor.commit_oplog(&id).await?;
                let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                if entries.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == TIMED_RECEIVE)) {
                    let start = lock_receive_starts(&entries, 0)?.1;
                    assert_invocation(&entries, &key, 0)?;
                    ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
                    return Ok::<_, anyhow::Error>(start);
                }
                tokio::task::yield_now().await;
            }
        }).await.context("committed open receive under unfinished method")??;
        tokio::time::timeout(Duration::from_secs(10), polls.recv())
            .await?
            .context("timer closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(initial.exhaustion.is_none(), "{initial:?}");
        ensure!(pending.invocation_key.as_ref() == Some(&key) && pending.start == receive_start && pending.path == "timed receive reader.lock");
        ensure!(pending.runtime == initial, "native Pending belongs to another runtime");
        ensure!(pending.state() == WebSocketReaderLockStateForTest::Pending);
        peer.assert_counts(1, 0)?;
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        let pool = worker.websocket_connection_pool();
        {
            let acquire = pool.acquire();
            tokio::pin!(acquire);
            ensure!(futures::poll!(acquire).is_pending(), "live socket must hold the only WebSocket slot");
        }
        ensure!(!invocation.is_finished());
        info!(
            ?resource, ?mode, %key, ?id, %receive_start, ?initial, handshakes = 1, frames = 0,
            "Native timed reader.lock() Pending behind untimed frame read; two committed open Starts; no peer frame"
        );

        if resource == Resource::Compute {
            ensure!(initial.fuel_generation.is_some());
            ensure!(initial.fuel_generation == account.settled_fuel_generation_for_test());
            ensure!(
                account.monthly_capacity_is_exhausted_for_test(mode),
                "Store must prepay the remaining fuel"
            );
            clock.advance(Duration::from_secs(30));
            tokio::time::timeout(Duration::from_secs(10), async {
                while polls.recv().await.unwrap().deadline != Duration::from_secs(60) {}
            })
            .await?;
            ensure!(
                attempts.try_recv().is_err(),
                "zero unreserved fuel must not exhaust prepaid fuel"
            );
            ensure!(worker.current_monthly_proposal_for_test() == initial);
            ensure!(!invocation.is_finished() && worker.concurrent_agent_permit_is_held().await);
        }
        ensure!(!invocation.is_finished());
        peer.assert_counts(1, 0)?;
        let refresh = if resource == Resource::Storage {
            source.set(FilesystemUsage::Authoritative {
                allocated_bytes: 101,
                filesystem_objects: 1,
            });
            clock.advance(Duration::from_secs(30));
            None
        } else {
            let mut exhausted = policy.clone();
            if resource == Resource::Memory {
                exhausted.available_memory_gb_seconds = 0;
            } else {
                exhausted.available_fuel = 0;
            }
            registry.set_policy(exhausted);
            Some(applied_refresh(&limits, &registry, context.account_id).await?)
        };
        let mut storage_now = Duration::from_secs(30);
        let attempt = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    biased;
                    attempt = attempts.recv() => return attempt.context("acceptance observer closed"),
                    poll = polls.recv(), if resource == Resource::Storage => {
                        let poll = poll.context("storage monitor timer closed")?;
                        if poll.now != storage_now || poll.deadline <= storage_now {
                            continue;
                        }
                        // Rearming follows settlement and proposal submission. Drain that tick's
                        // lifecycle work and observe its first proposal before advancing again.
                        worker.drain_lifecycle_for_test().await?;
                        tokio::select! {
                            biased;
                            attempt = attempts.recv() => return attempt.context("acceptance observer closed"),
                            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                        }
                        storage_now += Duration::from_secs(30);
                        clock.advance(Duration::from_secs(30));
                    }
                }
            }
        })
        .await
        .context("monthly proposal")??;
        ensure!(
            attempt.proposal.policy_revision
                == initial.policy_revision + u64::from(resource != Resource::Storage)
        );
        ensure!(attempt.proposal.period == initial.period);
        ensure!(attempt.proposal.window_identity == initial.window_identity);
        ensure!(attempt.proposal.resident_generation == initial.resident_generation);
        ensure!(attempt.proposal.start_attempt == initial.start_attempt);
        ensure!(attempt.proposal.fingerprint == initial.fingerprint);
        ensure!(attempt.proposal.fuel_generation == initial.fuel_generation);
        ensure!(attempt.proposal.exhaustion == Some(resource.reason(durable)));
        ensure!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        let signal = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        ensure!(matches!(signal, InterruptKind::Suspend(_)));
        ensure!(worker.monthly_stop_for_test() == Some(signal));
        info!(?resource, ?mode, proposal = ?attempt.proposal, ?signal, "Accepted matching monthly WebSocket stop and published typed signal");
        if resource == Resource::Compute {
            ensure!(account.settled_fuel_generation_for_test() > initial.fuel_generation);
        }
        if resource == Resource::Storage {
            let other = if durable {
                AgentMode::Ephemeral
            } else {
                AgentMode::Durable
            };
            ensure!(!account.monthly_capacity_is_exhausted_for_test(other));
        }
        // Keep the frame gate held until monthly stop closes the physical socket.
        peer.closed(1).await?;
        ensure!(matches!(pending.state(), WebSocketReaderLockStateForTest::Acquired | WebSocketReaderLockStateForTest::Dropped));
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .with_context(|| format!("physical window closure: invocation_finished={}", invocation.is_finished()))?;
        tokio::time::timeout(
            Duration::from_secs(10),
            worker.join_accepted_stops_for_test(),
        )
        .await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        if let Some(refresh) = refresh {
            refresh.await?;
        }
        ensure!(worker.unload_succeeded_for_test());
        drop(first_gate.take());
        drop(second_gate.take());
        ensure!(matches!(pending.state(), WebSocketReaderLockStateForTest::Acquired | WebSocketReaderLockStateForTest::Dropped));
        let released_slot = tokio::time::timeout(Duration::from_secs(5), pool.acquire())
            .await.context("retired socket must release WebSocket slot")??;
        drop(released_slot);
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(clock.active_sleeps() == 0);
        ensure!(account.monthly_observer_count_for_test() == 0);
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_invocation(&stopped, &key, 0)?;
        ensure!(
            lock_receive_starts(&stopped, 0)?.1 == receive_start,
            "interruption must leave the original receive Start open"
        );
        peer.assert_counts(1, 0)?;
        let entries = worker
            .oplog()
            .read_exact(
                OplogIndex::INITIAL,
                worker.oplog().current_oplog_index().await.as_u64(),
            )
            .await;
        let errors: Vec<_> = entries
            .values()
            .filter_map(|entry| match entry {
                OplogEntry::Error { error, .. } => Some(error),
                _ => None,
            })
            .collect();
        for entry in entries.values() {
            match entry {
                OplogEntry::Error { kind, entity_parent_start_index, .. } => {
                    ensure!(*kind == OplogErrorKind::Invocation && entity_parent_start_index.is_none(), "primary invocation error, not recovery: {entry:?}");
                }
                OplogEntry::Interrupted { .. } | OplogEntry::Exited { .. } => {
                    anyhow::bail!("unexpected lifecycle terminal: {entry:?}");
                }
                _ => {}
            }
        }
        let suspends = entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
            .count();
        if durable {
            ensure!(errors.is_empty(), "{errors:?}");
            ensure!(suspends == 1);
            ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
            ensure!(!invocation.is_finished());
        } else {
            ensure!(suspends == 0);
            match resource {
                Resource::Compute => {
                    ensure!(
                        matches!(errors.as_slice(), [AgentError::EphemeralFuelExhausted(_)]),
                        "{errors:?}"
                    );
                }
                _ => {
                    ensure!(
                        matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)]
                            if reason.reason == resource.reason(false)),
                        "{errors:?}"
                    );
                }
            }
            let failed = tokio::time::timeout(Duration::from_secs(10), &mut invocation).await?;
            target_joined = true;
            ensure!(failed?.is_err());
        }
        executor.wait_for_status(&id, if durable { AgentStatus::Suspended } else { AgentStatus::Failed }, Duration::from_secs(10)).await?;
        ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
        info!(?resource, ?mode, ?errors, suspends, "Reader-lock stop retained one unfinished method with mode-specific lifecycle outcome");
        let observations = source.observations();
        limits.run_batch_for_test().await;
        let settled = registry.applied_updates();
        check_accounting(
            &settled[target_updates_start..],
            resource,
            durable,
            initial.policy_revision,
            initial.period,
        )?;
        info!(?resource, ?mode, ?settled, "WebSocket stopped execution window settlement");
        let settled_count = settled.len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        limits.run_batch_for_test().await;
        ensure!(
            source.observations() == observations,
            "closed generation must stop sampling"
        );
        // Idle policy refreshes may still emit updates, but cannot charge a closed window.
        for update in &registry.applied_updates()[settled_count..] {
            ensure!(
                update.fuel_delta == 0
                    && update.memory_gb_seconds_delta == 0
                    && update.memory_byte_nanoseconds_remainder == 0
                    && update.durable_storage_byte_seconds_delta == 0
                    && update.durable_storage_byte_nanoseconds_remainder == 0
                    && update.ephemeral_storage_byte_seconds_delta == 0
                    && update.ephemeral_storage_byte_nanoseconds_remainder == 0,
                "closed window must stop accruing: {update:?}"
            );
        }
        info!(
            ?resource, ?mode, %receive_start, handshakes = 1, frames = 0,
            "Monthly stop closed silent post-handshake WebSocket socket, released Worker permit and monitor, settled usage; receive remains open"
        );

        if !durable {
            let mut grant = policy;
            grant.available_durable_storage_byte_seconds = u64::MAX;
            grant.available_ephemeral_storage_byte_seconds = u64::MAX;
            registry.set_policy(grant);
            applied_refresh(&limits, &registry, context.account_id).await?.await?;
            let retry = tokio::time::timeout(Duration::from_secs(10), executor.invoke_and_await_agent_with_key(&component, &name, &key, "receive_lock_contention", data_value!(peer.url.clone(), 300_000u64))).await?;
            ensure!(retry.is_err(), "failed ephemeral invocation cannot recover after capacity returns");
            peer.assert_counts(1, 0)?;
            // An independent durable Worker proves the failed connection released the only slot.
            let probe_name = agent_id!("WebsocketTest", "monthly-ws-pool-probe");
            probe_id = Some(AgentId::from_agent_id(component.id, &probe_name).unwrap());
            let probe_key = IdempotencyKey::fresh();
            let probe = executor.invoke_and_await_agent_with_key(&component, &probe_name, &probe_key, "receive_lock_contention", data_value!(peer.url.clone(), 300_000u64));
            let response = async {
                let second = peer.handshake().await?;
                ensure!(second.number == 2);
                second.send.send("probe-1".to_string()).map_err(|_| anyhow::anyhow!("first probe frame gate closed"))?;
                second.send_second.send("probe-2".to_string()).map_err(|_| anyhow::anyhow!("second probe frame gate closed"))?;
                Ok::<_, anyhow::Error>(())
            };
            let (probe, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(probe, response) }).await??;
            ensure!(probe.into_typed::<Result<(String, Option<String>), String>>()? == Ok(("probe-1".to_string(), Some("probe-2".to_string()))));
            let failed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(lock_receive_starts(&failed, 0)?.1 == receive_start);
            assert_invocation(&failed, &key, 0)?;
            ensure!(count_agent_invocation_pair_since(&failed, OplogIndex::INITIAL) == (2, 1));
            ensure!(failed.iter().filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))).count() == 1);
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
            peer.assert_counts(2, 2)?;
            executor.wait_for_status(&id, AgentStatus::Failed, Duration::from_secs(10)).await?;
            info!(?resource, %key, %receive_start, handshakes = 2, frames = 2, "Ephemeral failure stayed terminal after capacity restoration; independent Worker reused WebSocket slot");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        peer.assert_counts(if durable { 1 } else { 2 }, if durable { 0 } else { 2 })?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let mut cleanup_errors = Vec::new();
    if let Some(probe_id) = &probe_id {
        let owned = OwnedAgentId::new(context.default_environment_id, probe_id);
        if let Some(probe) = executor.production_active_agent(&owned).await {
            handshake::retire_handshake_worker(
                &executor,
                &probe.primary(),
                probe_id,
                false,
                &mut cleanup_errors,
            )
            .await;
        }
    }
    handshake::retire_handshake_worker(
        &executor,
        &worker,
        &id,
        result.is_err() && !target_joined && !invocation.is_finished(),
        &mut cleanup_errors,
    )
    .await;
    if !target_joined {
        if result.is_ok() && durable {
            // An accepted suspension leaves the original invocation pending.
            if invocation.is_finished() {
                cleanup_errors.push("suspended invocation unexpectedly settled".to_string());
            }
            invocation.abort();
            match tokio::time::timeout(Duration::from_secs(10), &mut invocation).await {
                Ok(Err(error)) if error.is_cancelled() => {}
                other => cleanup_errors.push(format!("suspended invocation abort: {other:?}")),
            }
        } else {
            pool::settle_invocation("reader-lock stop", &mut invocation, &mut cleanup_errors).await;
        }
    }
    match tokio::time::timeout(
        Duration::from_secs(5),
        worker.websocket_connection_pool().acquire(),
    )
    .await
    {
        Ok(Ok(permit)) => drop(permit),
        other => cleanup_errors.push(format!("WebSocket pool permit: {other:?}")),
    }
    if let Err(error) = peer.finish().await {
        cleanup_errors.push(format!("peer shutdown: {error:#}"));
    }
    drop(first_gate);
    drop(second_gate);
    if cleanup_errors.is_empty() {
        result
    } else {
        let failures = cleanup_errors.join("; ");
        match result {
            Ok(()) => Err(anyhow::anyhow!("cleanup failed: {failures}")),
            Err(error) => Err(error.context(format!("cleanup failed: {failures}"))),
        }
    }
}

#[test]
#[timeout("2m")]
async fn durable_websocket_reader_lock_timeout_starts_after_acquire(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let policy = Resource::Memory.policy();
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy));
    let shutdown = CancellationToken::new();
    let _shutdown_guard = shutdown.clone().drop_guard();
    let limits = ResourceLimitsGrpc::new(
        registry,
        Duration::from_secs(3600),
        Duration::ZERO,
        Resource::Memory.metering(),
        shutdown,
    );
    let context = TestContext::new(last_unique_id);
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| {
            config.resource_usage_metering = Resource::Memory.metering();
            config.limits.fuel_to_borrow = FUEL_BUDGET;
            config.max_websocket_connections = 1;
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
            agent_id!("WebsocketTest", "timeout-lock-seed"),
        )
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .context("seed Worker missing")?
        .primary();
    wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while seed.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    limits.run_batch_for_test().await;
    let name = agent_id!("WebsocketTest", "timeout-lock-contention");
    let key = IdempotencyKey::fresh();
    let id = AgentId::from_agent_id(component.id, &name).unwrap();
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
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut pending = worker.observe_websocket_reader_lock_for_test();
    let mut native_frame = worker.observe_websocket_timed_receive_for_test();
    let mut accepted = worker.observe_monthly_acceptance_for_test();
    let mut peer = Peer::start().await?;
    let mut invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let key = key.clone();
        let name = name.clone();
        let url = peer.url.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    "receive_lock_contention",
                    data_value!(url, 250u64),
                )
                .await
        }
    });
    let result = async {
        let handshake = peer.handshake().await?;
        ensure!(handshake.number == 1);
        let lock = tokio::time::timeout(Duration::from_secs(10), pending.recv())
            .await.context("timed reader lock never polled Pending")?
            .context("reader lock observer closed")?;
        tokio::time::timeout(Duration::from_secs(10), polls.recv()).await?.context("monthly timer closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        executor.commit_oplog(&id).await?;
        let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let untimed = starts_named(&entries, RECEIVE);
        let timed = starts_named(&entries, TIMED_RECEIVE);
        ensure!(starts_named(&entries, CONNECT).len() == 1 && untimed.len() == 1 && timed.len() == 1 && untimed[0] < timed[0]);
        ensure!(lock.invocation_key.as_ref() == Some(&key) && lock.start == timed[0] && lock.runtime == initial);
        ensure!(lock.state() == WebSocketReaderLockStateForTest::Pending);
        for start in [untimed[0], timed[0]] { assert_receive_terminal(&entries, start, 0)?; }
        assert_invocation(&entries, &key, 0)?;
        ensure!(worker.concurrent_agent_permit_is_held().await);
        peer.assert_counts(1, 0)?;
        tokio::time::sleep(Duration::from_millis(800)).await;
        ensure!(!invocation.is_finished() && lock.state() == WebSocketReaderLockStateForTest::Pending);
        executor.commit_oplog(&id).await?;
        let before_release = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        for start in [untimed[0], timed[0]] { assert_receive_terminal(&before_release, start, 0)?; }
        ensure!(accepted.try_recv().is_err() && raw.try_recv().is_err());
        peer.assert_counts(1, 0)?;
        handshake.send.send("first".to_string()).map_err(|_| anyhow::anyhow!("first frame gate closed"))?;
        let frame = tokio::time::timeout(Duration::from_secs(5), native_frame.recv()).await?.context("timed reader frame never polled Pending after lock")?;
        ensure!(frame.start == timed[0] && frame.invocation_key.as_ref() == Some(&key) && frame.runtime == initial);
        ensure!(lock.state() == WebSocketReaderLockStateForTest::Acquired);
        let result = tokio::time::timeout(Duration::from_secs(10), &mut invocation).await.context("timed frame did not expire after lock acquisition")??;
        ensure!(result?.into_typed::<Result<(String, Option<String>), String>>()? == Ok(("first".to_string(), None)));
        ensure!(frame.state() == WebSocketTimedReceiveStateForTest::Dropped);
        ensure!(accepted.try_recv().is_err() && raw.try_recv().is_err());
        let complete = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        for start in [untimed[0], timed[0]] { assert_receive_terminal(&complete, start, 1)?; }
        assert_invocation(&complete, &key, 1)?;
        assert_settled_history(&complete)?;
        ensure!(count_agent_invocation_pair_since(&complete, OplogIndex::INITIAL) == (2, 2));
        peer.assert_counts(1, 1)?;
        // A withheld second frame cannot account for the timed None result.
        tokio::time::timeout(Duration::from_secs(10), async {
            while !worker.stop_if_idle().await {
                tokio::task::yield_now().await;
            }
        })
        .await.context("idle retirement after guest timeout")?;
        peer.closed(1).await?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(account.monthly_observer_count_for_test() == 0);
        ensure!(clock.active_sleeps() == 0);
        let slot = tokio::time::timeout(Duration::from_secs(5), worker.websocket_connection_pool().acquire()).await??;
        drop(slot);
        drop(handshake.send_second);
        info!(%key, untimed = %untimed[0], timed = %timed[0], "Timed read remained locked beyond requested timeout, then timed out on the frame after untimed delivery");
        Ok::<_, anyhow::Error>(())
    }.await;
    let mut cleanup_errors = Vec::new();
    handshake::retire_handshake_worker(
        &executor,
        &worker,
        &id,
        result.is_err() && !invocation.is_finished(),
        &mut cleanup_errors,
    )
    .await;
    if !invocation.is_finished() {
        pool::settle_invocation("timeout-after-lock", &mut invocation, &mut cleanup_errors).await;
    }
    if let Err(error) = peer.finish().await {
        cleanup_errors.push(format!("peer shutdown: {error:#}"));
    }
    if cleanup_errors.is_empty() {
        result
    } else {
        let failures = cleanup_errors.join("; ");
        match result {
            Ok(()) => Err(anyhow::anyhow!("cleanup failed: {failures}")),
            Err(error) => Err(error.context(format!("cleanup failed: {failures}"))),
        }
    }
}
