use super::*;
use test_r::test;

macro_rules! pool_case {
    ($name:ident, $resource:ident, $mode:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            pending_pool_acquire(
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

pool_case!(
    durable_websocket_initial_pool_monthly_memory,
    Memory,
    Durable
);
pool_case!(
    ephemeral_websocket_initial_pool_monthly_memory,
    Memory,
    Ephemeral
);
pool_case!(
    durable_websocket_initial_pool_monthly_compute_prepaid,
    Compute,
    Durable
);
pool_case!(
    ephemeral_websocket_initial_pool_monthly_compute_prepaid,
    Compute,
    Ephemeral
);
pool_case!(
    durable_websocket_initial_pool_monthly_scripted_storage,
    Storage,
    Durable
);
pool_case!(
    ephemeral_websocket_initial_pool_monthly_scripted_storage,
    Storage,
    Ephemeral
);

pub(super) fn connect_start(
    entries: &[PublicOplogEntryWithIndex],
    terminals: usize,
    receive_terminals: usize,
) -> anyhow::Result<OplogIndex> {
    let starts = starts_named(entries, CONNECT);
    ensure!(starts.len() == 1, "one initial connect Start: {entries:#?}");
    ensure!(
        starts_named(entries, RECEIVE).len() == terminals,
        "unexpected receive Start count for connect terminal count {terminals}"
    );
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(start) => {
                if entry.oplog_index == starts[0] {
                    ensure!(
                        start.parent_start_index.is_none() && start.observational_owner.is_none()
                    );
                    ensure!(matches!(
                        start.durable_function_type,
                        PublicDurableFunctionType::WriteRemote(_)
                    ));
                    ensure!(start.request.is_some());
                }
                let expected = if entry.oplog_index == starts[0] {
                    terminals
                } else if start.function_name == RECEIVE {
                    receive_terminals
                } else {
                    1
                };
                ensure!(
                    terminal_count(entries, entry.oplog_index) == expected,
                    "unexpected call terminal: {entry:?}"
                );
            }
            PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::CompletionDiscarded(_)
            | PublicOplogEntry::Jump(_) => {
                anyhow::bail!("no fabricated terminal or replay jump: {entry:?}")
            }
            _ => {}
        }
    }
    ensure!(terminal_count(entries, starts[0]) == terminals);
    ensure!(!entries.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(delivery) if delivery.start_index == starts[0])), "direct connect has no accessor delivery marker");
    Ok(starts[0])
}

async fn pending_pool_acquire(
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
    let holder_id = executor
        .start_agent(
            &component.id,
            agent_id!("WebsocketTest", "monthly-ws-pool-holder"),
        )
        .await?;
    let holder = executor
        .production_active_agent(&OwnedAgentId::new(
            context.default_environment_id,
            &holder_id,
        ))
        .await
        .unwrap()
        .primary();
    wait_for_invocation_pair(&executor, &holder_id, OplogIndex::INITIAL).await?;
    let holder_key = IdempotencyKey::fresh();
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("WebsocketTest", "monthly-ws-pool-target")
    } else {
        agent_id!("EphemeralWebsocketTest", "monthly-ws-pool-target")
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
        holder.all(),
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
    if resource == Resource::Storage {
        worker
            .owner_runtime_resources()
            .set_scripted_filesystem_usage_for_test(source.clone());
    }
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let mut peer = Peer::start().await?;
    let holder_name = agent_id!("WebsocketTest", "monthly-ws-pool-holder");
    // Start the holder only after fallible target setup; everything after this spawn
    // runs inside the cleanup scope below.
    let mut holder_invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let url = peer.url.clone();
        let key = holder_key.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &holder_name,
                    &key,
                    "connect_and_receive_first",
                    data_value!(url),
                )
                .await
        }
    });
    let pool = holder.websocket_connection_pool();
    let mut invocation: Option<tokio::task::JoinHandle<_>> = None;
    let mut probe_id = None;
    let mut holder_joined = false;
    let mut target_joined = false;
    let result = async {
        let first = peer.handshake().await?;
        ensure!(first.number == 1);
        first.send.send("holder".to_owned()).map_err(|_| anyhow::anyhow!("holder frame gate closed"))?;
        let held = tokio::time::timeout(Duration::from_secs(15), &mut holder_invocation).await.context("holder method completion")?;
        holder_joined = true;
        let held = held??;
        ensure!(held.into_typed::<String>()? == "holder");
        let holder_history = executor.get_oplog(&holder_id, OplogIndex::INITIAL).await?;
        assert_invocation(&holder_history, &holder_key, 1)?;
        ensure!(count_agent_invocation_pair_since(&holder_history, OplogIndex::INITIAL) == (2, 2));
        receive_start(&holder_history, 1)?;
        peer.assert_counts(1, 1)?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while holder.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
        }).await.context("holder idle permit release")?;
        ensure!(holder.is_loaded().await, "STOP: holder must remain loaded idle");
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(account.monthly_observer_count_for_test() == 0, "idle holder retained a monthly monitor");
        {
            let acquire = pool.acquire();
            tokio::pin!(acquire);
            ensure!(futures::poll!(acquire).is_pending(), "STOP: idle holder lost the real pool slot");
        }
        // Flush the holder's completed window before starting the target. Every
        // subsequent positive usage update occurs while the holder is idle.
        limits.run_batch_for_test().await;
        let target_updates_start = registry.applied_updates().len();
        info!(?resource, ?mode, %holder_id, %holder_key, "Completed holder method; live socket owns pool slot while its Worker is loaded idle without monthly permit");

        invocation = Some(tokio::spawn({
            let executor = executor.clone();
            let component = component.clone();
            let name = name.clone();
            let key = key.clone();
            let url = peer.url.clone();
            async move { executor.invoke_and_await_agent_with_key(&component, &name, &key, "connect_and_receive_first", data_value!(url)).await }
        }));
        let connect = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                executor.commit_oplog(&id).await?;
                let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                if !starts_named(&entries, CONNECT).is_empty() {
                    let start = connect_start(&entries, 0, 0)?;
                    assert_invocation(&entries, &key, 0)?;
                    ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
                    return Ok::<_, anyhow::Error>(start);
                }
                tokio::task::yield_now().await;
            }
        }).await.context("target committed open connect under pool contention")??;
        tokio::time::timeout(Duration::from_secs(10), polls.recv()).await?.context("timer closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        ensure!(initial.exhaustion.is_none(), "{initial:?}");
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(!invocation.as_ref().unwrap().is_finished());
        peer.assert_counts(1, 1)?;
        {
            let acquire = pool.acquire();
            tokio::pin!(acquire);
            ensure!(futures::poll!(acquire).is_pending(), "STOP: holder lost slot before target stop");
        }
        if resource == Resource::Compute {
            ensure!(initial.fuel_generation.is_some());
            ensure!(initial.fuel_generation == account.settled_fuel_generation_for_test());
            ensure!(account.monthly_capacity_is_exhausted_for_test(mode), "target Store must prepay fuel");
            clock.advance(Duration::from_secs(30));
            tokio::time::timeout(Duration::from_secs(10), async {
                while polls.recv().await.unwrap().deadline != Duration::from_secs(60) {}
            }).await?;
            ensure!(attempts.try_recv().is_err(), "prepaid fuel cannot stop at zero unreserved fuel");
            ensure!(worker.current_monthly_proposal_for_test() == initial);
            ensure!(!invocation.as_ref().unwrap().is_finished() && worker.concurrent_agent_permit_is_held().await);
        }
        let refresh = if resource == Resource::Storage {
            source.set(FilesystemUsage::Authoritative { allocated_bytes: 101, filesystem_objects: 1 });
            clock.advance(Duration::from_secs(30));
            None
        } else {
            let mut exhausted = policy.clone();
            if resource == Resource::Memory { exhausted.available_memory_gb_seconds = 0; }
            else { exhausted.available_fuel = 0; }
            registry.set_policy(exhausted);
            Some(monthly::applied_refresh(&limits, &registry, context.account_id).await?)
        };
        let mut storage_now = Duration::from_secs(30);
        let attempt = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    biased;
                    attempt = attempts.recv() => return attempt.context("acceptance observer closed"),
                    poll = polls.recv(), if resource == Resource::Storage => {
                        let poll = poll.context("storage timer closed")?;
                        if poll.now != storage_now || poll.deadline <= storage_now { continue; }
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
        }).await.context("monthly proposal")??;
        ensure!(attempt.proposal.policy_revision == initial.policy_revision + u64::from(resource != Resource::Storage));
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
        if resource == Resource::Compute { ensure!(account.settled_fuel_generation_for_test() > initial.fuel_generation); }
        if resource == Resource::Storage {
            let other = if durable { AgentMode::Ephemeral } else { AgentMode::Durable };
            ensure!(!account.monthly_capacity_is_exhausted_for_test(other));
        }
        ensure!(holder.is_loaded().await && !holder.concurrent_agent_permit_is_held().await, "STOP: account cap also stopped idle holder");
        {
            let acquire = pool.acquire();
            tokio::pin!(acquire);
            ensure!(futures::poll!(acquire).is_pending(), "STOP: holder pool slot vanished at quota publication");
        }
        peer.assert_counts(1, 1)?;
        info!(?resource, ?mode, %connect, ?signal, %id, %key, "Typed monthly stop accepted while target still waits for holder's pool permit; no target peer connection");

        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
        }).await.context("target physical unload before holder release")?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        if let Some(refresh) = refresh { refresh.await?; }
        ensure!(worker.unload_succeeded_for_test());
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(clock.active_sleeps() == 0);
        ensure!(account.monthly_observer_count_for_test() == 0);
        ensure!(holder.is_loaded().await && !holder.concurrent_agent_permit_is_held().await, "STOP: holder unloaded under target's cap");
        peer.assert_counts(1, 1)?;
        {
            let acquire = pool.acquire();
            tokio::pin!(acquire);
            ensure!(futures::poll!(acquire).is_pending(), "STOP: holder lost its slot before explicit eviction");
        }
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(connect_start(&stopped, 0, 0)? == connect);
        assert_invocation(&stopped, &key, 0)?;
        let entries = worker.oplog().read_exact(OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64()).await;
        let errors: Vec<_> = entries.values().filter_map(|entry| match entry { OplogEntry::Error { error, .. } => Some(error), _ => None }).collect();
        for entry in entries.values() {
            match entry {
                OplogEntry::Error { kind, entity_parent_start_index, .. } => ensure!(*kind == OplogErrorKind::Invocation && entity_parent_start_index.is_none(), "{entry:?}"),
                OplogEntry::Interrupted { .. } | OplogEntry::Exited { .. } => anyhow::bail!("unexpected lifecycle terminal: {entry:?}"),
                _ => {}
            }
        }
        let suspends = entries.values().filter(|entry| matches!(entry, OplogEntry::Suspend { .. })).count();
        if durable {
            ensure!(errors.is_empty(), "{errors:?}");
            ensure!(suspends == 1);
            ensure!(!invocation.as_ref().unwrap().is_finished());
        } else {
            ensure!(suspends == 0);
            match resource {
                Resource::Compute => ensure!(matches!(errors.as_slice(), [AgentError::EphemeralFuelExhausted(_)]), "{errors:?}"),
                _ => ensure!(matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)] if reason.reason == resource.reason(false)), "{errors:?}"),
            }
            let failed = tokio::time::timeout(Duration::from_secs(10), invocation.as_mut().unwrap()).await?;
            target_joined = true;
            ensure!(failed?.is_err());
        }
        executor.wait_for_status(&id, if durable { AgentStatus::Suspended } else { AgentStatus::Failed }, Duration::from_secs(10)).await?;
        let observations = source.observations();
        limits.run_batch_for_test().await;
        let settled = registry.applied_updates();
        check_accounting(&settled[target_updates_start..], resource, durable, initial.policy_revision, initial.period)?;
        let settled_count = settled.len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        limits.run_batch_for_test().await;
        ensure!(source.observations() == observations, "closed generation sampled filesystem again");
        for update in &registry.applied_updates()[settled_count..] {
            ensure!(update.fuel_delta == 0 && update.memory_gb_seconds_delta == 0 && update.memory_byte_nanoseconds_remainder == 0 && update.durable_storage_byte_seconds_delta == 0 && update.durable_storage_byte_nanoseconds_remainder == 0 && update.ephemeral_storage_byte_seconds_delta == 0 && update.ephemeral_storage_byte_nanoseconds_remainder == 0, "closed window accrued: {update:?}");
        }
        info!(?resource, ?mode, %connect, ?errors, suspends, target_updates_start, target_updates = settled.len() - target_updates_start, handshakes = 1, frames = 1, "Stopped target with incomplete connect; physical target permit and monthly window released before holder pool slot");

        tokio::time::timeout(Duration::from_secs(10), async {
            while !holder.stop_if_idle().await {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("holder must become idle and unload after target stop settles")?;
        peer.closed(1).await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while holder.is_loaded().await || holder.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
        }).await?;
        tokio::time::timeout(Duration::from_secs(10), holder.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), holder.retained_cleanup_for_test()).await??;
        let released = tokio::time::timeout(Duration::from_secs(5), pool.acquire()).await.context("unloading holder must free pool slot")??;
        drop(released);
        let mut grant = policy;
        grant.available_durable_storage_byte_seconds = u64::MAX;
        grant.available_ephemeral_storage_byte_seconds = u64::MAX;
        registry.set_policy(grant);
        monthly::applied_refresh(&limits, &registry, context.account_id).await?.await?;
        if durable {
            executor.resume(&id, false).await?;
            let second = peer.handshake().await?;
            ensure!(second.number == 2);
            let reconstructed = worker.current_monthly_proposal_for_test();
            ensure!(reconstructed.resident_generation > initial.resident_generation);
            ensure!(reconstructed.start_attempt != initial.start_attempt);
            ensure!(reconstructed.fingerprint == initial.fingerprint);
            let repairing = tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    executor.commit_oplog(&id).await?;
                    let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                    if !starts_named(&entries, RECEIVE).is_empty() {
                        return Ok::<_, anyhow::Error>(entries);
                    }
                    tokio::task::yield_now().await;
                }
            }).await.context("repaired connect and pending receive")??;
            ensure!(connect_start(&repairing, 1, 0)? == connect);
            assert_invocation(&repairing, &key, 0)?;
            peer.assert_counts(2, 1)?;
            second.send.send("recovered".to_owned()).map_err(|_| anyhow::anyhow!("recovery frame gate closed"))?;
            let resumed = tokio::time::timeout(Duration::from_secs(15), invocation.as_mut().unwrap()).await.context("retained target invocation")?;
            target_joined = true;
            let resumed = resumed??;
            ensure!(resumed.into_typed::<String>()? == "recovered");
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(connect_start(&recovered, 1, 1)? == connect);
            assert_settled_history(&recovered)?;
            assert_invocation(&recovered, &key, 1)?;
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, "connect_and_receive_first", data_value!(peer.url.clone())).await?;
            ensure!(cached.into_typed::<String>()? == "recovered");
            peer.assert_counts(2, 2)?;
            info!(?resource, %key, %connect, handshakes = 2, frames = 2, "Original connect Start repaired with one method result and one peer frame after holder release");
            tokio::time::timeout(Duration::from_secs(10), async {
                while !worker.stop_if_idle().await { tokio::task::yield_now().await; }
            }).await.context("idle target eviction before offline completed replay")?;
            peer.closed(2).await?;
            tokio::time::timeout(Duration::from_secs(10), async {
                while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
            }).await?;
            tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
            tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
            let noop_key = IdempotencyKey::fresh();
            let noop = tokio::time::timeout(Duration::from_secs(15), executor.invoke_and_await_agent_with_key(&component, &name, &noop_key, "noop", data_value!())).await??;
            ensure!(noop.into_typed::<String>()? == "ok");
            ensure!(worker.current_monthly_proposal_for_test().resident_generation > reconstructed.resident_generation);
            peer.assert_counts(2, 2)?;
            let replayed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(connect_start(&replayed, 1, 1)? == connect);
            ensure!(count_agent_invocation_pair_since(&replayed, OplogIndex::INITIAL) == (3, 3));
            assert_settled_history(&replayed)?;
            assert_invocation(&replayed, &key, 1)?;
            assert_invocation(&replayed, &noop_key, 1)?;
            let fresh_key = IdempotencyKey::fresh();
            let fresh = executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key, "receive_next_from_persisted", data_value!());
            let response = async {
                let third = peer.handshake().await?;
                ensure!(third.number == 3);
                third.send.send("fresh".to_owned()).map_err(|_| anyhow::anyhow!("fresh frame gate closed"))?;
                Ok::<_, anyhow::Error>(())
            };
            let (fresh, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(fresh, response) }).await??;
            ensure!(fresh.into_typed::<String>()? == "fresh");
            peer.assert_counts(3, 3)?;
            let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (4, 4));
            ensure!(starts_named(&history, CONNECT) == starts_named(&recovered, CONNECT));
            ensure!(starts_named(&history, RECEIVE).len() == 2);
            assert_settled_history(&history)?;
            assert_invocation(&history, &key, 1)?;
            assert_invocation(&history, &noop_key, 1)?;
            assert_invocation(&history, &fresh_key, 1)?;
            info!(?resource, %key, %connect, handshakes = 3, frames = 3, "Completed history replayed offline; independent fresh receive connected once");
        } else {
            let retry = tokio::time::timeout(Duration::from_secs(10), executor.invoke_and_await_agent_with_key(&component, &name, &key, "connect_and_receive_first", data_value!(peer.url.clone()))).await?;
            ensure!(retry.is_err(), "failed ephemeral target must not recover");
            peer.assert_counts(1, 1)?;
            let probe_name = agent_id!("WebsocketTest", "monthly-ws-pool-probe");
            probe_id = Some(AgentId::from_agent_id(component.id, &probe_name).unwrap());
            let probe_key = IdempotencyKey::fresh();
            let probe = executor.invoke_and_await_agent_with_key(&component, &probe_name, &probe_key, "connect_and_receive_first", data_value!(peer.url.clone()));
            let response = async {
                let second = peer.handshake().await?;
                ensure!(second.number == 2);
                second.send.send("probe".to_owned()).map_err(|_| anyhow::anyhow!("probe frame gate closed"))?;
                Ok::<_, anyhow::Error>(())
            };
            let (probe, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(probe, response) }).await??;
            ensure!(probe.into_typed::<String>()? == "probe");
            peer.assert_counts(2, 2)?;
            let failed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(connect_start(&failed, 0, 0)? == connect);
            assert_invocation(&failed, &key, 0)?;
            ensure!(count_agent_invocation_pair_since(&failed, OplogIndex::INITIAL) == (2, 1));
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
            executor.wait_for_status(&id, AgentStatus::Failed, Duration::from_secs(10)).await?;
            info!(?resource, %key, %connect, handshakes = 2, frames = 2, "Ephemeral failure remained terminal; independent Worker used freed slot");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        peer.assert_counts(if durable { 3 } else { 2 }, if durable { 3 } else { 2 })?;
        Ok::<_, anyhow::Error>(())
    }.await;
    let mut cleanup_errors = Vec::new();
    macro_rules! check_cleanup {
        ($label:expr, $operation:expr) => {
            match tokio::time::timeout(Duration::from_secs(10), $operation).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => cleanup_errors.push(format!("{}: {error:#}", $label)),
                Err(error) => cleanup_errors.push(format!("{}: {error:#}", $label)),
            }
        };
    }
    macro_rules! retire {
        ($label:literal, $owner:expr, $agent_id:expr, $pending:expr) => {{
            let idle =
                match tokio::time::timeout(Duration::from_secs(10), $owner.stop_if_idle()).await {
                    Ok(idle) => idle,
                    Err(error) => {
                        cleanup_errors.push(format!("{} idle stop: {error:#}", $label));
                        false
                    }
                };
            // Stopping is not "loaded"; always join its stop and retained cleanup.
            if !idle && ($owner.is_loaded().await || $pending) {
                check_cleanup!(concat!($label, " interrupt"), executor.interrupt($agent_id));
            }
            check_cleanup!(
                concat!($label, " stop join"),
                $owner.join_accepted_stops_for_test()
            );
            check_cleanup!(
                concat!($label, " retained cleanup"),
                $owner.retained_cleanup_for_test()
            );
            if $owner.is_loaded().await || $owner.concurrent_agent_permit_is_held().await {
                cleanup_errors.push(format!(
                    "{} still has a resident instance or execution permit",
                    $label
                ));
            }
        }};
    }
    // The holder may be in Running or Stopping; release its slot before the target.
    retire!(
        "holder",
        holder,
        &holder_id,
        !holder_joined && !holder_invocation.is_finished()
    );
    retire!(
        "target",
        worker,
        &id,
        invocation
            .as_ref()
            .is_some_and(|handle| !target_joined && !handle.is_finished())
    );
    if let Some(probe_id) = &probe_id {
        let probe_owned = OwnedAgentId::new(context.default_environment_id, probe_id);
        if let Some(probe) = executor.production_active_agent(&probe_owned).await {
            retire!("probe", probe.primary(), probe_id, false);
        }
    }
    if !holder_joined {
        settle_invocation("holder", &mut holder_invocation, &mut cleanup_errors).await;
    }
    if let Some(invocation) = invocation.as_mut().filter(|_| !target_joined) {
        settle_invocation("target", invocation, &mut cleanup_errors).await;
    }
    // Polling an acquire only proves that a permit is currently unavailable.
    // Acquire it after retiring every socket owner, including on an early failure.
    match tokio::time::timeout(Duration::from_secs(5), pool.acquire()).await {
        Ok(Ok(permit)) => drop(permit),
        Ok(Err(error)) => cleanup_errors.push(format!("pool permit: {error:#}")),
        Err(error) => cleanup_errors.push(format!("pool permit not released: {error:#}")),
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

pub(super) async fn settle_invocation<T>(
    name: &str,
    invocation: &mut tokio::task::JoinHandle<T>,
    errors: &mut Vec<String>,
) {
    match tokio::time::timeout(Duration::from_secs(10), &mut *invocation).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => errors.push(format!("{name} invocation task: {error:#}")),
        Err(error) => {
            errors.push(format!(
                "{name} invocation did not settle after retirement: {error:#}"
            ));
            invocation.abort();
            if let Err(error) = (&mut *invocation).await
                && !error.is_cancelled()
            {
                errors.push(format!("{name} aborted invocation task: {error:#}"));
            }
        }
    }
}
