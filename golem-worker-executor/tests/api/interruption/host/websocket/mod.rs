mod completion;
mod handshake;
mod peer;
mod pool;
mod reader_lock;
mod reconnect_handshake;
mod reconnect_pool;
mod timed_receive;

use super::*;
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{
    AgentError, OplogEntry, OplogErrorKind, PublicAgentInvocation, PublicDurableFunctionType,
    PublicOplogEntryWithIndex,
};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::resource_usage_metering::{
    FilesystemUsage, ScriptedFilesystemUsageForTest,
};
use golem_worker_executor::services::{HasOplog, HasWebSocketConnectionPool, UsesAllDeps};
use golem_worker_executor::worker::{MonthlyClockForTest, Worker};
use test_r::test;

use crate::api::interruption::support::{FUEL_BUDGET, Resource, check_accounting};
use peer::Peer;

macro_rules! pending_case {
    ($name:ident, $resource:ident, $mode:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            pending_receive(
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

pending_case!(durable_websocket_receive_memory_quota, Memory, Durable);
pending_case!(ephemeral_websocket_receive_memory_quota, Memory, Ephemeral);
pending_case!(
    durable_websocket_receive_compute_quota_prepaid,
    Compute,
    Durable
);
pending_case!(
    ephemeral_websocket_receive_compute_quota_prepaid,
    Compute,
    Ephemeral
);
pending_case!(
    durable_websocket_receive_scripted_storage_quota,
    Storage,
    Durable
);
pending_case!(
    ephemeral_websocket_receive_scripted_storage_quota,
    Storage,
    Ephemeral
);

const RECEIVE: &str = "golem:websocket/client::receive";
const CONNECT: &str = "golem:websocket/client::connect";

fn assert_invocation(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    finished: usize,
) -> anyhow::Result<()> {
    let mut current = false;
    let mut started = 0;
    let mut terminals = 0;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(start) => {
                current = matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method) if &method.idempotency_key == key);
                started += usize::from(current);
            }
            PublicOplogEntry::AgentInvocationFinished(_) if current => terminals += 1,
            _ => {}
        }
    }
    ensure!(
        started == 1 && terminals == finished,
        "invocation {key}: {started} Started/{terminals} Finished"
    );
    Ok(())
}

fn receive_start(
    entries: &[PublicOplogEntryWithIndex],
    terminals: usize,
) -> anyhow::Result<OplogIndex> {
    let starts = starts_named(entries, RECEIVE);
    let connects = starts_named(entries, CONNECT);
    ensure!(starts.len() == 1, "one receive Start: {entries:#?}");
    ensure!(
        connects.len() == 1 && connects[0] < starts[0],
        "one prior connect Start"
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
                ensure!(
                    terminal_count(entries, entry.oplog_index)
                        == if entry.oplog_index == starts[0] {
                            terminals
                        } else {
                            1
                        }
                );
            }
            PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::CompletionDiscarded(_)
            | PublicOplogEntry::Jump(_) => {
                anyhow::bail!("no invented terminal or replay jump: {entry:?}")
            }
            _ => {}
        }
    }
    assert_receive_terminal(entries, starts[0], terminals)?;
    Ok(starts[0])
}

async fn pending_receive(
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
    // Refund the seed Store before the target borrows the full account budget.
    if resource == Resource::Compute {
        limits.run_batch_for_test().await;
    }
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("WebsocketTest", "monthly-ws-receive")
    } else {
        agent_id!("EphemeralWebsocketTest", "monthly-ws-receive")
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
    let result = async {
        let first = peer.handshake().await?;
        ensure!(first.number == 1);
        let receive_start = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                executor.commit_oplog(&id).await?;
                let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                if entries.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == RECEIVE)) {
                    let start = receive_start(&entries, 0)?;
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
            "Peer completed handshake; committed open untimed receive; unfinished method; no peer frame"
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
        drop(first.send);
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
        let released_slot = tokio::time::timeout(Duration::from_secs(5), pool.acquire())
            .await.context("retired socket must release WebSocket slot")??;
        drop(released_slot);
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(clock.active_sleeps() == 0);
        ensure!(account.monthly_observer_count_for_test() == 0);
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_invocation(&stopped, &key, 0)?;
        ensure!(
            self::receive_start(&stopped, 0)? == receive_start,
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
            ensure!(
                tokio::time::timeout(Duration::from_secs(10), &mut invocation)
                    .await??
                    .is_err()
            );
        }
        executor.wait_for_status(&id, if durable { AgentStatus::Suspended } else { AgentStatus::Failed }, Duration::from_secs(10)).await?;
        ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
        info!(?resource, ?mode, ?errors, suspends, "WebSocket stop retained one unfinished method with mode-specific lifecycle outcome");
        let observations = source.observations();
        limits.run_batch_for_test().await;
        let settled = registry.applied_updates();
        check_accounting(
            &settled,
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
            executor.commit_oplog(&id).await?;
            let repairing = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(self::receive_start(&repairing, 0)? == receive_start);
            assert_invocation(&repairing, &key, 0)?;
            peer.assert_counts(2, 0)?;
            second.send.send("recovered".to_string()).map_err(|_| anyhow::anyhow!("recovery frame gate closed"))?;
            let resumed = tokio::time::timeout(Duration::from_secs(15), &mut invocation)
                .await.context("retained WebSocket invocation continuation")???;
            ensure!(resumed.into_typed::<String>()? == "recovered");
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(self::receive_start(&recovered, 1)? == receive_start);
            assert_settled_history(&recovered)?;
            assert_invocation(&recovered, &key, 1)?;
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, "connect_and_receive_first", data_value!(peer.url.clone())).await?;
            ensure!(cached.into_typed::<String>()? == "recovered");
            peer.assert_counts(2, 1)?;
            info!(?resource, %key, %receive_start, ?reconstructed, handshakes = 2, frames = 1,
                "Original receive repaired under same Start; one method Finished; reconstructed socket observed");

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
            peer.assert_counts(2, 1)?;
            let replayed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(self::receive_start(&replayed, 1)? == receive_start);
            ensure!(count_agent_invocation_pair_since(&replayed, OplogIndex::INITIAL) == (3, 3));
            assert_settled_history(&replayed)?;
            assert_invocation(&replayed, &key, 1)?;
            assert_invocation(&replayed, &noop_key, 1)?;

            // Only a fresh receive may reconnect the reconstructed, persisted handle.
            let fresh_key = IdempotencyKey::fresh();
            let fresh = executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key, "receive_next_from_persisted", data_value!());
            let response = async {
                let third = peer.handshake().await?;
                ensure!(third.number == 3);
                third.send.send("fresh".to_string()).map_err(|_| anyhow::anyhow!("fresh frame gate closed"))?;
                Ok::<_, anyhow::Error>(())
            };
            let (fresh, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(fresh, response) }).await??;
            ensure!(fresh.into_typed::<String>()? == "fresh");
            peer.assert_counts(3, 2)?;
            let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (4, 4));
            let receives = starts_named(&history, RECEIVE);
            ensure!(receives.len() == 2 && receives[0] == receive_start);
            ensure!(starts_named(&history, CONNECT) == starts_named(&recovered, CONNECT));
            for start in receives { assert_receive_terminal(&history, start, 1)?; }
            assert_settled_history(&history)?;
            assert_invocation(&history, &key, 1)?;
            assert_invocation(&history, &noop_key, 1)?;
            assert_invocation(&history, &fresh_key, 1)?;
            info!(?resource, %key, %receive_start, handshakes = 3, frames = 2, "Completed WebSocket history stayed offline; distinct fresh receive completed");
        } else {
            let retry = tokio::time::timeout(Duration::from_secs(10), executor.invoke_and_await_agent_with_key(&component, &name, &key, "connect_and_receive_first", data_value!(peer.url.clone()))).await?;
            ensure!(retry.is_err(), "failed ephemeral invocation cannot recover after capacity returns");
            peer.assert_counts(1, 0)?;
            // An independent durable Worker proves the failed connection released the only slot.
            let probe_name = agent_id!("WebsocketTest", "monthly-ws-pool-probe");
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
            ensure!(self::receive_start(&failed, 0)? == receive_start);
            assert_invocation(&failed, &key, 0)?;
            ensure!(count_agent_invocation_pair_since(&failed, OplogIndex::INITIAL) == (2, 1));
            ensure!(failed.iter().filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))).count() == 1);
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
            peer.assert_counts(2, 1)?;
            executor.wait_for_status(&id, AgentStatus::Failed, Duration::from_secs(10)).await?;
            info!(?resource, %key, %receive_start, handshakes = 2, frames = 1, "Ephemeral failure stayed terminal after capacity restoration; independent Worker reused WebSocket slot");
        }
        // Keep accepting to detect unintended reconnects after the asserted work has completed.
        tokio::time::sleep(Duration::from_millis(100)).await;
        peer.assert_counts(if durable { 3 } else { 2 }, if durable { 2 } else { 1 })?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = peer.finish().await;
    if result.is_err() {
        let _ = tokio::time::timeout(Duration::from_secs(10), executor.interrupt(&id)).await;
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            worker.join_accepted_stops_for_test(),
        )
        .await;
        let _ =
            tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await;
        if !invocation.is_finished() {
            let _ = tokio::time::timeout(Duration::from_secs(10), &mut invocation).await;
        }
        invocation.abort();
    }
    result.and(cleanup)
}

fn starts_named(entries: &[PublicOplogEntryWithIndex], name: &str) -> Vec<OplogIndex> {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name == name => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect()
}

fn terminal_count(entries: &[PublicOplogEntryWithIndex], start: OplogIndex) -> usize {
    entries
        .iter()
        .filter(|entry| match &entry.entry {
            PublicOplogEntry::End(end) => end.start_index == start,
            PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
            _ => false,
        })
        .count()
}

fn assert_receive_terminal(
    entries: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
    count: usize,
) -> anyhow::Result<()> {
    ensure!(terminal_count(entries, start) == count);
    let deliveries: Vec<_> = entries.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(delivery) if delivery.start_index == start)).collect();
    ensure!(deliveries.len() == count);
    if count == 1 {
        let end = entries.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start)).context("receive needs End, not Cancelled")?;
        ensure!(start < end.oplog_index && end.oplog_index < deliveries[0].oplog_index);
    }
    Ok(())
}

fn assert_settled_history(entries: &[PublicOplogEntryWithIndex]) -> anyhow::Result<()> {
    let last_finished = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("missing invocation terminal")?
        .oplog_index;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_) => {
                ensure!(
                    terminal_count(entries, entry.oplog_index) == 1,
                    "unsettled/duplicate Start: {entry:?}"
                );
                ensure!(entry.oplog_index < last_finished);
            }
            PublicOplogEntry::End(_) => ensure!(entry.oplog_index < last_finished),
            PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::Error(_)
            | PublicOplogEntry::CompletionDiscarded(_)
            | PublicOplogEntry::Jump(_) => anyhow::bail!("unexpected terminal/jump: {entry:?}"),
            _ => {}
        }
    }
    Ok(())
}
