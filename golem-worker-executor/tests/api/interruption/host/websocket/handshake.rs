use super::*;
use golem_worker_executor::worker::WebSocketHandshakeStateForTest;
use golem_worker_executor::workerctx::default::Context;
use test_r::test;

macro_rules! handshake_case {
    ($name:ident, $resource:ident, $mode:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            pending_handshake(
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

handshake_case!(durable_websocket_handshake_memory_quota, Memory, Durable);
handshake_case!(
    ephemeral_websocket_handshake_memory_quota,
    Memory,
    Ephemeral
);
handshake_case!(
    durable_websocket_handshake_compute_quota_prepaid,
    Compute,
    Durable
);
handshake_case!(
    ephemeral_websocket_handshake_compute_quota_prepaid,
    Compute,
    Ephemeral
);
handshake_case!(
    durable_websocket_handshake_scripted_storage_quota,
    Storage,
    Durable
);
handshake_case!(
    ephemeral_websocket_handshake_scripted_storage_quota,
    Storage,
    Ephemeral
);
async fn pending_handshake(
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
        .start_agent(
            &component.id,
            agent_id!("WebsocketTest", "monthly-ws-handshake-seed"),
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
    // Refund the seed Store before the target borrows the full account budget.
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
        agent_id!("WebsocketTest", "monthly-ws-handshake")
    } else {
        agent_id!("EphemeralWebsocketTest", "monthly-ws-handshake")
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
    let pool = worker.websocket_connection_pool();
    let free = tokio::time::timeout(Duration::from_secs(5), pool.acquire())
        .await
        .context("WebSocket pool initially free")??;
    drop(free);
    let mut peer = Peer::start_with_held_handshake(true).await?;
    let mut native = worker.observe_websocket_handshake_for_test();
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
                    "connect_and_receive_first",
                    data_value!(url),
                )
                .await
        }
    });
    let mut target_joined = false;
    let mut probe_id = None;
    let result = async {
        let held = peer.held().await?;
        ensure!(held.number == 1);
        let pending = tokio::time::timeout(Duration::from_secs(10), native.recv())
            .await?.context("native connect_async never returned Pending")?;
        let connect = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                executor.commit_oplog(&id).await?;
                let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                if !starts_named(&entries, CONNECT).is_empty() {
                    let start = pool::connect_start(&entries, 0, 0)?;
                    assert_invocation(&entries, &key, 0)?;
                    ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
                    return Ok::<_, anyhow::Error>(start);
                }
                tokio::task::yield_now().await;
            }
        }).await.context("committed open connect during native handshake")??;
        tokio::time::timeout(Duration::from_secs(10), polls.recv())
            .await?.context("timer closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(initial.exhaustion.is_none(), "{initial:?}");
        ensure!(pending.invocation_key.as_ref() == Some(&key) && pending.start == connect);
        ensure!(pending.runtime == initial, "native Pending belongs to another runtime");
        ensure!(pending.state() == WebSocketHandshakeStateForTest::Pending);
        peer.assert_effects(1, 0, 0)?;
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        {
            let acquire = pool.acquire();
            tokio::pin!(acquire);
            ensure!(futures::poll!(acquire).is_pending(), "handshake owns the only WebSocket slot");
        }
        ensure!(!invocation.is_finished());
        info!(?resource, ?mode, %key, ?id, %connect, ?initial, "Native connect_async Pending after free pool acquisition; peer withheld Upgrade response");

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
        peer.assert_effects(1, 0, 0)?;
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
        // The Upgrade response remains withheld until the original TCP socket closes.
        peer.closed(1).await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        }).await.context("physical window closure with Upgrade response withheld")?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        if let Some(refresh) = refresh { refresh.await?; }
        ensure!(worker.unload_succeeded_for_test());
        let released_slot = tokio::time::timeout(Duration::from_secs(5), pool.acquire())
            .await.context("interrupted handshake must release WebSocket slot")??;
        drop(released_slot);
        ensure!(pending.state() == WebSocketHandshakeStateForTest::Dropped);
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(clock.active_sleeps() == 0);
        ensure!(account.monthly_observer_count_for_test() == 0);
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_invocation(&stopped, &key, 0)?;
        ensure!(pool::connect_start(&stopped, 0, 0)? == connect);
        peer.assert_effects(1, 0, 0)?;
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
        info!(?resource, ?mode, ?errors, suspends, "Handshake stop retained one unfinished method with mode-specific lifecycle outcome");
        let observations = source.observations();
        limits.run_batch_for_test().await;
        let settled = registry.applied_updates();
        let target_updates = &settled[target_updates_start..];
        check_accounting(
            target_updates,
            resource,
            durable,
            initial.policy_revision,
            initial.period,
        )?;
        info!(?resource, ?mode, ?target_updates, "WebSocket stopped execution window settlement");
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
            ?resource, ?mode, %connect, handshakes = 1, frames = 0,
            "Monthly stop closed TCP before Upgrade response, released Worker permit and monitor, settled usage; connect remains open"
        );

        drop(held.release);
        let mut grant = policy;
        grant.available_durable_storage_byte_seconds = u64::MAX;
        grant.available_ephemeral_storage_byte_seconds = u64::MAX;
        registry.set_policy(grant);
        applied_refresh(&limits, &registry, context.account_id).await?.await?;
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
            ensure!(pool::connect_start(&repairing, 1, 0)? == connect);
            assert_invocation(&repairing, &key, 0)?;
            peer.assert_effects(2, 1, 0)?;
            second.send.send("recovered".to_string()).map_err(|_| anyhow::anyhow!("recovery frame gate closed"))?;
            let resumed = tokio::time::timeout(Duration::from_secs(15), &mut invocation)
                .await.context("retained WebSocket invocation continuation")??;
            target_joined = true;
            ensure!(resumed?.into_typed::<String>()? == "recovered");
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(pool::connect_start(&recovered, 1, 1)? == connect);
            assert_settled_history(&recovered)?;
            assert_invocation(&recovered, &key, 1)?;
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, "connect_and_receive_first", data_value!(peer.url.clone())).await?;
            ensure!(cached.into_typed::<String>()? == "recovered");
            peer.assert_effects(2, 1, 1)?;
            info!(?resource, %key, %connect, ?reconstructed, handshakes = 2, frames = 1,
                "Original connect repaired under same Start; one method Finished; new TCP handshake");

            tokio::time::timeout(Duration::from_secs(10), async {
                while !worker.stop_if_idle().await { tokio::task::yield_now().await; }
            }).await.context("idle eviction before completed WebSocket history")?;
            peer.closed(2).await?;
            tokio::time::timeout(Duration::from_secs(10), async {
                while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
            }).await?;
            tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
            tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;

            // Reconstruct completed history with an invocation that performs no network work.
            let noop_key = IdempotencyKey::fresh();
            let noop = tokio::time::timeout(Duration::from_secs(15), executor.invoke_and_await_agent_with_key(&component, &name, &noop_key, "noop", data_value!())).await??;
            ensure!(noop.into_typed::<String>()? == "ok");
            ensure!(worker.current_monthly_proposal_for_test().resident_generation > reconstructed.resident_generation);
            peer.assert_effects(2, 1, 1)?;
            let replayed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(pool::connect_start(&replayed, 1, 1)? == connect);
            ensure!(count_agent_invocation_pair_since(&replayed, OplogIndex::INITIAL) == (3, 3));
            assert_settled_history(&replayed)?;
            assert_invocation(&replayed, &key, 1)?;
            assert_invocation(&replayed, &noop_key, 1)?;

            peer.assert_effects(2, 1, 1)?;
        } else {
            let retry = tokio::time::timeout(Duration::from_secs(10), executor.invoke_and_await_agent_with_key(&component, &name, &key, "connect_and_receive_first", data_value!(peer.url.clone()))).await?;
            ensure!(retry.is_err(), "failed ephemeral invocation cannot recover after capacity returns");
            peer.assert_effects(1, 0, 0)?;
            // An independent durable Worker proves the failed connection released the only slot.
            let probe_name = agent_id!("WebsocketTest", "monthly-ws-handshake-probe");
            probe_id = Some(AgentId::from_agent_id(component.id, &probe_name).unwrap());
            let probe_key = IdempotencyKey::fresh();
            let probe = executor.invoke_and_await_agent_with_key(&component, &probe_name, &probe_key, "connect_and_receive_first", data_value!(peer.url.clone()));
            let response = async {
                let second = peer.handshake().await?;
                ensure!(second.number == 2);
                second.send.send("probe".to_string()).map_err(|_| anyhow::anyhow!("pool probe frame gate closed"))?;
                Ok::<_, anyhow::Error>(())
            };
            let (probe, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(probe, response) }).await??;
            ensure!(probe.into_typed::<String>()? == "probe");
            let failed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(pool::connect_start(&failed, 0, 0)? == connect);
            assert_invocation(&failed, &key, 0)?;
            ensure!(count_agent_invocation_pair_since(&failed, OplogIndex::INITIAL) == (2, 1));
            ensure!(failed.iter().filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))).count() == 1);
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
            peer.assert_effects(2, 1, 1)?;
            executor.wait_for_status(&id, AgentStatus::Failed, Duration::from_secs(10)).await?;
            info!(?resource, %key, %connect, handshakes = 2, frames = 1, "Ephemeral failure stayed terminal after capacity restoration; independent Worker reused WebSocket slot");
        }
        // Keep accepting to detect unintended reconnects after the asserted work has completed.
        tokio::time::sleep(Duration::from_millis(100)).await;
        peer.assert_effects(2, 1, 1)?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let mut cleanup_errors = Vec::new();
    if let Some(probe_id) = &probe_id {
        let owned = OwnedAgentId::new(context.default_environment_id, probe_id);
        if let Some(probe) = executor.production_active_agent(&owned).await {
            retire_handshake_worker(
                &executor,
                &probe.primary(),
                probe_id,
                false,
                &mut cleanup_errors,
            )
            .await;
        }
    }
    retire_handshake_worker(
        &executor,
        &worker,
        &id,
        !target_joined && !invocation.is_finished(),
        &mut cleanup_errors,
    )
    .await;
    if !target_joined {
        pool::settle_invocation("handshake", &mut invocation, &mut cleanup_errors).await;
    }
    match tokio::time::timeout(Duration::from_secs(5), pool.acquire()).await {
        Ok(Ok(permit)) => drop(permit),
        other => cleanup_errors.push(format!("WebSocket pool permit: {other:?}")),
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

pub(super) async fn retire_handshake_worker(
    executor: &TestWorkerExecutor,
    worker: &Worker<Context>,
    id: &AgentId,
    pending: bool,
    errors: &mut Vec<String>,
) {
    let idle = match tokio::time::timeout(Duration::from_secs(10), worker.stop_if_idle()).await {
        Ok(idle) => idle,
        Err(error) => {
            errors.push(format!("idle stop: {error:#}"));
            false
        }
    };
    if !idle && (worker.is_loaded().await || pending) {
        match tokio::time::timeout(Duration::from_secs(10), executor.interrupt(id)).await {
            Ok(Ok(())) => {}
            other => errors.push(format!("interrupt: {other:?}")),
        }
    }
    match tokio::time::timeout(
        Duration::from_secs(10),
        worker.join_accepted_stops_for_test(),
    )
    .await
    {
        Ok(Ok(())) => {}
        other => errors.push(format!("stop join: {other:?}")),
    }
    match tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await {
        Ok(Ok(())) => {}
        other => errors.push(format!("retained cleanup: {other:?}")),
    }
    if worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
        errors.push(format!("{id} still holds a Store or Worker permit"));
    }
}
