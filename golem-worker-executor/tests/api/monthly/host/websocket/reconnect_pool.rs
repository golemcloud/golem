use super::*;
use golem_worker_executor::worker::{
    WebSocketReconnectPathForTest, WebSocketReconnectPoolStateForTest,
};
use test_r::test;

const CLOSE: &str = "golem:websocket/client::close";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Path {
    Accessor,
    Direct,
}

macro_rules! reconnect_case {
    ($name:ident, $resource:ident, $path:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            pending_reconnect(
                last_unique_id,
                deps,
                host_api_tests,
                Resource::$resource,
                Path::$path,
                false,
            )
            .await
        }
    };
}
reconnect_case!(
    durable_websocket_reconnect_pool_receive_monthly_memory,
    Memory,
    Accessor
);
reconnect_case!(
    durable_websocket_reconnect_pool_receive_monthly_compute_prepaid,
    Compute,
    Accessor
);
reconnect_case!(
    durable_websocket_reconnect_pool_receive_monthly_scripted_storage,
    Storage,
    Accessor
);
reconnect_case!(
    durable_websocket_reconnect_pool_close_monthly_memory,
    Memory,
    Direct
);
reconnect_case!(
    durable_websocket_reconnect_pool_close_monthly_compute_prepaid,
    Compute,
    Direct
);
reconnect_case!(
    durable_websocket_reconnect_pool_close_monthly_scripted_storage,
    Storage,
    Direct
);

#[test]
#[timeout("2m")]
async fn websocket_reconnect_pool_receive_completion_before_monthly_exhaustion(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    pending_reconnect(
        last_unique_id,
        deps,
        host_api_tests,
        Resource::Memory,
        Path::Accessor,
        true,
    )
    .await
}

#[test]
#[timeout("2m")]
async fn websocket_reconnect_pool_close_completion_before_monthly_exhaustion(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    pending_reconnect(
        last_unique_id,
        deps,
        host_api_tests,
        Resource::Memory,
        Path::Direct,
        true,
    )
    .await
}

pub(super) fn operation_start(
    entries: &[PublicOplogEntryWithIndex],
    path: Path,
    terminals: usize,
) -> anyhow::Result<OplogIndex> {
    let connect = starts_named(entries, CONNECT);
    let first_receive = starts_named(entries, RECEIVE);
    let operations = starts_named(
        entries,
        if path == Path::Accessor {
            RECEIVE
        } else {
            CLOSE
        },
    );
    ensure!(
        connect.len() == 1
            && first_receive
                .first()
                .is_some_and(|first| connect[0] < *first)
    );
    ensure!(
        operations.len() == if path == Path::Accessor { 2 } else { 1 },
        "fresh operation missing/duplicated: {entries:#?}"
    );
    let start = *operations.last().unwrap();
    ensure!(
        terminal_count(entries, connect[0]) == 1,
        "historical Connect must remain completed"
    );
    ensure!(terminal_count(entries, start) == terminals);
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_) => ensure!(
                terminal_count(entries, entry.oplog_index)
                    == if entry.oplog_index == start {
                        terminals
                    } else {
                        1
                    },
                "unexpected Start terminal: {entry:?}"
            ),
            PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::CompletionDiscarded(_)
            | PublicOplogEntry::Jump(_) => anyhow::bail!("invented terminal: {entry:?}"),
            _ => {}
        }
    }
    let delivered = entries.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(marker) if marker.start_index == start)).count();
    ensure!(
        delivered == if path == Path::Accessor { terminals } else { 0 },
        "operation marker count {delivered}"
    );
    if terminals == 1 {
        ensure!(entries.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start)), "operation must end, not cancel");
    }
    Ok(start)
}

async fn pending_reconnect(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    resource: Resource,
    path: Path,
    completion_first: bool,
) -> anyhow::Result<()> {
    let mut policy = resource.policy();
    if resource == Resource::Compute {
        policy.available_fuel = 2 * FUEL_BUDGET;
    }
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
    let name = agent_id!("WebsocketTest", "monthly-ws-reconnect-target");
    let id = AgentId::from_agent_id(component.id, &name).unwrap();
    let seed_id = executor
        .start_agent(
            &component.id,
            agent_id!("WebsocketTest", "monthly-ws-reconnect-seed"),
        )
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .context("seed missing")?
        .primary();
    wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while seed.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    if resource == Resource::Compute {
        limits.run_batch_for_test().await;
    }
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
    let source = ScriptedFilesystemUsageForTest::new(FilesystemUsage::Authoritative {
        allocated_bytes: 0,
        filesystem_objects: 0,
    });
    if resource == Resource::Storage {
        worker
            .owner_runtime_resources()
            .set_scripted_filesystem_usage_for_test(source.clone());
    }
    let pool = worker.websocket_connection_pool();
    let mut peer = if path == Path::Direct {
        Peer::start_allowing_close().await?
    } else {
        Peer::start().await?
    };
    let holder_id = executor
        .start_agent(
            &component.id,
            agent_id!("WebsocketTest", "monthly-ws-reconnect-holder"),
        )
        .await?;
    let holder = executor
        .production_active_agent(&OwnedAgentId::new(
            context.default_environment_id,
            &holder_id,
        ))
        .await
        .context("holder missing")?
        .primary();
    let mut invocation: Option<tokio::task::JoinHandle<anyhow::Result<_>>> = None;
    let mut holder_invocation: Option<tokio::task::JoinHandle<anyhow::Result<_>>> = None;
    let mut target_joined = false;
    let mut holder_joined = false;
    let result = async {
        // Persist a genuine completed Connect and the saved guest handle, then discard its Store.
        let first_key = IdempotencyKey::fresh();
        let first_call = executor.invoke_and_await_agent_with_key(&component, &name, &first_key, "connect_and_receive_first", data_value!(peer.url.clone()));
        let first_frame = async {
            let first = peer.handshake().await?;
            ensure!(first.number == 1);
            first.send.send("original".to_owned()).map_err(|_| anyhow::anyhow!("first frame gate closed"))?;
            Ok::<_, anyhow::Error>(())
        };
        let (first_result, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(first_call, first_frame) }).await??;
        ensure!(first_result.into_typed::<String>()? == "original");
        let original = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        pool::connect_start(&original, 1, 1)?;
        assert_receive_terminal(&original, starts_named(&original, RECEIVE)[0], 1)?;
        assert_invocation(&original, &first_key, 1)?;
        ensure!(count_agent_invocation_pair_since(&original, OplogIndex::INITIAL) == (2, 2));
        tokio::time::timeout(Duration::from_secs(10), async { while worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; } }).await.context("target initial idle release")?;
        tokio::time::timeout(Duration::from_secs(10), async { while !worker.stop_if_idle().await { tokio::task::yield_now().await; } }).await.context("target idle eviction")?;
        peer.closed(1).await?;
        tokio::time::timeout(Duration::from_secs(10), async { while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; } }).await?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        let slot = tokio::time::timeout(Duration::from_secs(5), pool.acquire()).await??;
        drop(slot);
        limits.run_batch_for_test().await;
        if resource == Resource::Memory {
            registry.set_policy(policy.clone());
            applied_refresh(&limits, &registry, context.account_id).await?.await?;
            limits.run_batch_for_test().await;
        }
        let target_updates_start = registry.applied_updates().len();
        let (clock, mut polls) = MonthlyClockForTest::new();
        worker.set_monthly_clock_for_test(clock.clone());
        let mut attempts = worker.observe_monthly_acceptance_for_test();
        let mut native = worker.observe_websocket_reconnect_pool_for_test();
        let holder_key = IdempotencyKey::fresh();
        holder_invocation = Some(tokio::spawn({
            let executor = executor.clone(); let component = component.clone(); let key = holder_key.clone(); let url = peer.url.clone();
            async move { executor.invoke_and_await_agent_with_key(&component, &agent_id!("WebsocketTest", "monthly-ws-reconnect-holder"), &key, "connect_and_receive_first", data_value!(url)).await }
        }));
        let second = peer.handshake().await?;
        ensure!(second.number == 2);
        second.send.send("holder".to_owned()).map_err(|_| anyhow::anyhow!("holder frame gate closed"))?;
        let held = tokio::time::timeout(Duration::from_secs(15), holder_invocation.as_mut().unwrap()).await??;
        holder_joined = true;
        ensure!(held?.into_typed::<String>()? == "holder");
        tokio::time::timeout(Duration::from_secs(10), async { while holder.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; } }).await?;
        ensure!(holder.is_loaded().await, "holder must stay loaded idle");
        if resource == Resource::Compute {
            limits.run_batch_for_test().await;
            let account = limits.initialize_account(context.account_id).await?;
            let unreserved = account.effective_fuel_for_test();
            ensure!(unreserved > FUEL_BUDGET && unreserved <= policy.available_fuel);
            policy.available_fuel -= unreserved - FUEL_BUDGET;
            registry.set_policy(policy.clone());
            applied_refresh(&limits, &registry, context.account_id).await?.await?;
            ensure!(account.effective_fuel_for_test() == FUEL_BUDGET, "one full prepayment remains before target startup");
        }
        {
            let acquire = pool.acquire(); tokio::pin!(acquire);
            ensure!(futures::poll!(acquire).is_pending(), "holder must own the real socket pool slot");
        }
        let key = IdempotencyKey::fresh();
        let method = if path == Path::Accessor { "receive_next_from_persisted" } else { "close_persisted_result" };
        invocation = Some(tokio::spawn({
            let executor = executor.clone(); let component = component.clone(); let key = key.clone(); let name = name.clone();
            async move { executor.invoke_and_await_agent_with_key(&component, &name, &key, method, data_value!()).await }
        }));
        let pending = tokio::time::timeout(Duration::from_secs(15), native.recv()).await?.context("target native reconnect pool acquire did not poll Pending")?;
        executor.commit_oplog(&id).await?;
        let open = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let start = operation_start(&open, path, 0)?;
        assert_invocation(&open, &key, 0)?;
        assert_invocation(&open, &first_key, 1)?;
        ensure!(count_agent_invocation_pair_since(&open, OplogIndex::INITIAL) == (3, 2));
        ensure!(pending.start == start && pending.invocation_key.as_ref() == Some(&key));
        ensure!(pending.path == if path == Path::Accessor { WebSocketReconnectPathForTest::Accessor } else { WebSocketReconnectPathForTest::Direct });
        ensure!(pending.state() == WebSocketReconnectPoolStateForTest::Pending);
        let pending_runtime = worker.current_monthly_proposal_for_test();
        ensure!(pending.runtime == pending_runtime && pending_runtime.exhaustion.is_none(), "Pending must belong to fresh target runtime");
        let initial = worker.current_monthly_proposal_for_test();
        ensure!(initial.exhaustion.is_none() && initial.resident_generation == pending.runtime.resident_generation);
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await && holder.is_loaded().await);
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        peer.assert_counts(2, 2)?;
        info!(?path, ?resource, %start, ?key, "real target reconnect pool acquire Pending; historical Connect already completed; holder owns sole socket");
        if completion_first {
            evict_holder(&holder, &pool, &mut peer, false).await?;
            let third = peer.handshake().await?;
            ensure!(third.number == 3);
            if path == Path::Accessor { third.send.send("completed".into()).map_err(|_| anyhow::anyhow!("frame gate closed"))?; }
            let done = tokio::time::timeout(Duration::from_secs(15), invocation.as_mut().unwrap()).await??;
            target_joined = true;
            if path == Path::Accessor { ensure!(done?.into_typed::<String>()? == "completed"); }
            else { ensure!(done?.into_typed::<Result<(), String>>()?.is_ok()); }
            if path == Path::Direct { peer.closed(3).await?; }
            ensure!(pending.state() == WebSocketReconnectPoolStateForTest::Returned);
            let completed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(operation_start(&completed, path, 1)? == start);
            if path == Path::Accessor {
                assert_receive_terminal(&completed, start, 1)?;
            }
            assert_invocation(&completed, &key, 1)?;
            peer.assert_effects(3, 3, if path == Path::Accessor { 3 } else { 2 })?;
            let mut exhausted = policy;
            exhausted.available_memory_gb_seconds = 0;
            registry.set_policy(exhausted);
            applied_refresh(&limits, &registry, context.account_id).await?.await?;
            let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, method, data_value!()).await?;
            if path == Path::Accessor { ensure!(cached.into_typed::<String>()? == "completed"); }
            else { ensure!(cached.into_typed::<Result<(), String>>()?.is_ok()); }
            peer.assert_effects(3, 3, if path == Path::Accessor { 3 } else { 2 })?;
            return Ok::<_, anyhow::Error>(());
        }
        tokio::time::timeout(Duration::from_secs(10), polls.recv()).await?.context("target monitor timer closed")?;
        let mut raw = worker.raw_interrupt_for_test();
        if resource == Resource::Compute {
            ensure!(initial.fuel_generation.is_some() && initial.fuel_generation == account.settled_fuel_generation_for_test());
            ensure!(account.monthly_capacity_is_exhausted_for_test(AgentMode::Durable), "target Store prepaid fuel");
            clock.advance(Duration::from_secs(30));
            tokio::time::timeout(Duration::from_secs(10), async { while polls.recv().await.unwrap().deadline != Duration::from_secs(60) {} }).await?;
            ensure!(attempts.try_recv().is_err(), "zero unreserved fuel cannot stop prepaid Store");
            ensure!(worker.current_monthly_proposal_for_test() == initial);
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
            Some(applied_refresh(&limits, &registry, context.account_id).await?)
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
        ensure!(attempt.proposal.period == initial.period && attempt.proposal.window_identity == initial.window_identity);
        ensure!(attempt.proposal.resident_generation == initial.resident_generation && attempt.proposal.start_attempt == initial.start_attempt);
        ensure!(attempt.proposal.fingerprint == initial.fingerprint && attempt.proposal.fuel_generation == initial.fuel_generation);
        ensure!(attempt.proposal.exhaustion == Some(resource.reason(true)));
        ensure!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        let signal = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        ensure!(matches!(signal, InterruptKind::Suspend(_)) && worker.monthly_stop_for_test() == Some(signal));
        ensure!(holder.is_loaded().await && !holder.concurrent_agent_permit_is_held().await);
        peer.assert_counts(2, 2)?;
        info!(?path, ?resource, %start, ?signal, "typed monthly stop accepted while target native acquire Pending and holder still loaded");
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
        }).await.context("target physical unload before holder eviction")?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        if let Some(refresh) = refresh { refresh.await?; }
        ensure!(worker.unload_succeeded_for_test() && worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(pending.state() == WebSocketReconnectPoolStateForTest::Dropped);
        ensure!(clock.active_sleeps() == 0 && account.monthly_observer_count_for_test() == 0);
        ensure!(holder.is_loaded().await && !holder.concurrent_agent_permit_is_held().await);
        {
            let acquire = pool.acquire(); tokio::pin!(acquire);
            ensure!(futures::poll!(acquire).is_pending(), "holder socket released before target teardown");
        }
        peer.assert_counts(2, 2)?;
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(operation_start(&stopped, path, 0)? == start);
        assert_invocation(&stopped, &key, 0)?;
        let entries = worker.oplog().read_exact(OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64()).await;
        ensure!(entries.values().filter(|entry| matches!(entry, OplogEntry::Suspend { .. })).count() == 1);
        ensure!(!entries.values().any(|entry| matches!(entry, OplogEntry::Error { .. } | OplogEntry::Interrupted { .. } | OplogEntry::Exited { .. })));
        ensure!(!invocation.as_ref().unwrap().is_finished());
        executor.wait_for_status(&id, AgentStatus::Suspended, Duration::from_secs(10)).await?;
        let observations = source.observations();
        limits.run_batch_for_test().await;
        let updates = registry.applied_updates();
        check_accounting(&updates[target_updates_start..], resource, true, initial.policy_revision, initial.period)?;
        let count = updates.len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        limits.run_batch_for_test().await;
        ensure!(source.observations() == observations, "closed window sampled filesystem");
        for update in &registry.applied_updates()[count..] {
            ensure!(update.fuel_delta == 0 && update.memory_gb_seconds_delta == 0 && update.memory_byte_nanoseconds_remainder == 0 && update.durable_storage_byte_seconds_delta == 0 && update.durable_storage_byte_nanoseconds_remainder == 0 && update.ephemeral_storage_byte_seconds_delta == 0 && update.ephemeral_storage_byte_nanoseconds_remainder == 0, "closed window accrued: {update:?}");
        }
        if resource == Resource::Storage { ensure!(!account.monthly_capacity_is_exhausted_for_test(AgentMode::Ephemeral)); }
        if resource == Resource::Compute { ensure!(account.settled_fuel_generation_for_test() > initial.fuel_generation); }

        evict_holder(&holder, &pool, &mut peer, true).await?;
        let mut grant = policy;
        grant.available_durable_storage_byte_seconds = u64::MAX;
        registry.set_policy(grant);
        applied_refresh(&limits, &registry, context.account_id).await?.await?;
        executor.resume(&id, false).await?;
        let third = peer.handshake().await?;
        ensure!(third.number == 3);
        let reconstructing = worker.current_monthly_proposal_for_test();
        ensure!(reconstructing.resident_generation > initial.resident_generation && reconstructing.start_attempt != initial.start_attempt && reconstructing.fingerprint == initial.fingerprint);
        executor.commit_oplog(&id).await?;
        let repairing = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let intermediate_terminals = if path == Path::Accessor { 0 } else { terminal_count(&repairing, start) };
        ensure!(intermediate_terminals <= 1 && operation_start(&repairing, path, intermediate_terminals)? == start);
        if path == Path::Accessor { assert_invocation(&repairing, &key, 0)?; }
        peer.assert_effects(3, 3, 2)?;
        if path == Path::Accessor { third.send.send("repaired".into()).map_err(|_| anyhow::anyhow!("repair frame gate closed"))?; }
        let done = tokio::time::timeout(Duration::from_secs(15), invocation.as_mut().unwrap()).await.context("retained target invocation")??;
        target_joined = true;
        if path == Path::Accessor { ensure!(done?.into_typed::<String>()? == "repaired"); }
        else { ensure!(done?.into_typed::<Result<(), String>>()?.is_ok()); }
        if path == Path::Direct { peer.closed(3).await?; }
        let repaired = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(operation_start(&repaired, path, 1)? == start);
        if path == Path::Accessor {
            assert_receive_terminal(&repaired, start, 1)?;
        }
        assert_settled_history(&repaired)?;
        assert_invocation(&repaired, &first_key, 1)?;
        assert_invocation(&repaired, &key, 1)?;
        ensure!(count_agent_invocation_pair_since(&repaired, OplogIndex::INITIAL) == (3, 3));
        let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, method, data_value!()).await?;
        if path == Path::Accessor { ensure!(cached.into_typed::<String>()? == "repaired"); }
        else { ensure!(cached.into_typed::<Result<(), String>>()?.is_ok()); }
        peer.assert_effects(3, 3, if path == Path::Accessor { 3 } else { 2 })?;
        tokio::time::timeout(Duration::from_secs(10), async { while !worker.stop_if_idle().await { tokio::task::yield_now().await; } }).await?;
        if path == Path::Accessor { peer.closed(3).await?; }
        tokio::time::timeout(Duration::from_secs(10), async { while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; } }).await?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        let noop_key = IdempotencyKey::fresh();
        let noop = tokio::time::timeout(Duration::from_secs(15), executor.invoke_and_await_agent_with_key(&component, &name, &noop_key, "noop", data_value!())).await??;
        ensure!(noop.into_typed::<String>()? == "ok");
        peer.assert_effects(3, 3, if path == Path::Accessor { 3 } else { 2 })?;
        let replayed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(operation_start(&replayed, path, 1)? == start);
        assert_invocation(&replayed, &key, 1)?;
        assert_invocation(&replayed, &noop_key, 1)?;
        assert_settled_history(&replayed)?;
        ensure!(count_agent_invocation_pair_since(&replayed, OplogIndex::INITIAL) == (4, 4));
        Ok::<_, anyhow::Error>(())
    }.await;
    let mut cleanup_errors = Vec::new();
    // Retire the target first; never free the holder merely to mask a stuck target acquire.
    handshake::retire_handshake_worker(
        &executor,
        &worker,
        &id,
        invocation
            .as_ref()
            .is_some_and(|handle| !target_joined && !handle.is_finished()),
        &mut cleanup_errors,
    )
    .await;
    if let Some(handle) = invocation.as_mut().filter(|_| !target_joined) {
        pool::settle_invocation("target", handle, &mut cleanup_errors).await;
    }
    let target_cleanup_error_count = cleanup_errors.len();
    if target_cleanup_error_count == 0
        && !worker.is_loaded().await
        && !worker.concurrent_agent_permit_is_held().await
    {
        handshake::retire_handshake_worker(
            &executor,
            &holder,
            &holder_id,
            holder_invocation
                .as_ref()
                .is_some_and(|handle| !holder_joined && !handle.is_finished()),
            &mut cleanup_errors,
        )
        .await;
        if let Some(handle) = holder_invocation.as_mut().filter(|_| !holder_joined) {
            pool::settle_invocation("holder", handle, &mut cleanup_errors).await;
        }
        match tokio::time::timeout(Duration::from_secs(5), pool.acquire()).await {
            Ok(Ok(slot)) => drop(slot),
            other => cleanup_errors.push(format!("pool permit: {other:?}")),
        }
    } else {
        cleanup_errors.push(format!(
            "holder retained: target cleanup had {target_cleanup_error_count} error(s) or physical Store/permit release could not be verified"
        ));
        if let Some(handle) = holder_invocation.as_mut().filter(|_| !holder_joined) {
            pool::settle_invocation("holder", handle, &mut cleanup_errors).await;
        }
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

pub(super) async fn evict_holder(
    holder: &Worker<golem_worker_executor::workerctx::default::Context>,
    pool: &golem_worker_executor::durable_host::websocket::WebSocketConnectionPool,
    peer: &mut Peer,
    probe_pool: bool,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !holder.stop_if_idle().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("holder idle eviction")?;
    peer.closed(2).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while holder.is_loaded().await || holder.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    tokio::time::timeout(
        Duration::from_secs(10),
        holder.join_accepted_stops_for_test(),
    )
    .await??;
    tokio::time::timeout(Duration::from_secs(10), holder.retained_cleanup_for_test()).await??;
    if probe_pool {
        let released = tokio::time::timeout(Duration::from_secs(5), pool.acquire())
            .await
            .context("holder pool release")??;
        drop(released);
    }
    Ok(())
}
