use super::*;
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry, PublicOplogEntryWithIndex};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::resource_usage_metering::{
    FilesystemUsage, ScriptedFilesystemUsageForTest,
};
use golem_worker_executor::services::{HasOplog, UsesAllDeps};
use golem_worker_executor::worker::{MonthlyClockForTest, Worker};
use test_r::test;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::api::monthly::support::{FUEL_BUDGET, Resource, check_accounting};

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
            pending_block(
                last_unique_id,
                deps,
                host_api_tests,
                Resource::$resource,
                AgentMode::$mode,
                None,
            )
            .await
        }
    };
}

pending_case!(durable_p2_block_monthly_memory, Memory, Durable);
pending_case!(ephemeral_p2_block_monthly_memory, Memory, Ephemeral);
pending_case!(durable_p2_block_monthly_compute_prepaid, Compute, Durable);
pending_case!(
    ephemeral_p2_block_monthly_compute_prepaid,
    Compute,
    Ephemeral
);
pending_case!(durable_p2_block_monthly_scripted_storage, Storage, Durable);
pending_case!(
    ephemeral_p2_block_monthly_scripted_storage,
    Storage,
    Ephemeral
);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplayStopOrder {
    BeforeSubscription,
    DuringWait,
    CompletionFirst,
}

macro_rules! replay_case {
    ($name:ident, $ready:expr, $order:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            pending_block(
                last_unique_id,
                deps,
                host_api_tests,
                Resource::Memory,
                AgentMode::Durable,
                Some(($ready, ReplayStopOrder::$order)),
            )
            .await
        }
    };
}

replay_case!(
    poll_replay_stop_before_subscription,
    false,
    BeforeSubscription
);
replay_case!(poll_replay_stop_during_wait, false, DuringWait);
replay_case!(poll_replay_completion_before_stop, false, CompletionFirst);
replay_case!(
    ready_replay_stop_before_subscription,
    true,
    BeforeSubscription
);
replay_case!(ready_replay_stop_during_wait, true, DuringWait);
replay_case!(ready_replay_completion_before_stop, true, CompletionFirst);

async fn pending_block(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    resource: Resource,
    mode: AgentMode,
    replay_stop: Option<(bool, ReplayStopOrder)>,
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
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(&component.id, agent_id!("Networking", "p2-block-seed"))
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
        agent_id!("Networking", "monthly-p2-block")
    } else {
        agent_id!("EphemeralNetworking", "monthly-p2-block")
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let mut invocation = tokio::spawn({
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
                    "tcp_input_block_p2",
                    data_value!(port),
                )
                .await
        }
    });
    let result = async {
        let (mut peer, _) = tokio::time::timeout(Duration::from_secs(30), listener.accept())
            .await
            .context("first TCP connection")??;
        let mut connections = 1;
        let poll_start = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                executor.commit_oplog(&id).await?;
                let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                let polls = poll_starts(&entries);
                if polls.len() == 2 {
                    ensure!(
                        terminal_count(&entries, polls[0]) == 1,
                        "connect poll must finish"
                    );
                    ensure!(
                        terminal_count(&entries, polls[1]) == 0,
                        "input poll must remain open"
                    );
                    ensure!(
                        count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1)
                    );
                    return Ok::<_, anyhow::Error>(polls[1]);
                }
                ensure!(
                    !invocation.is_finished(),
                    "guest returned before input poll: {entries:#?}"
                );
                tokio::task::yield_now().await;
            }
        })
        .await
        .context("open input IoPollPoll")??;
        tokio::time::timeout(Duration::from_secs(10), polls.recv())
            .await?
            .context("timer closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(initial.exhaustion.is_none(), "{initial:?}");
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(!invocation.is_finished());
        info!(
            ?resource, ?mode, %poll_start, ?initial, connections,
            "P2 input poll parked with live Worker and held permit; peer has sent no data"
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
        ensure!(attempt.proposal.fuel_generation == initial.fuel_generation);
        ensure!(attempt.proposal.exhaustion == Some(resource.reason(durable)));
        ensure!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        let signal = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        ensure!(matches!(signal, InterruptKind::Suspend(_)));
        ensure!(worker.monthly_stop_for_test() == Some(signal));
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
        // Nothing is sent or closed by the peer to make the input poll ready.
        let mut byte = [0];
        let closed = tokio::time::timeout(Duration::from_secs(10), peer.read(&mut byte)).await;
        ensure!(
            closed.is_ok(),
            "socket still open: signal={signal:?}, loaded={}, permit={}, finished={}",
            worker.is_loaded().await,
            worker.concurrent_agent_permit_is_held().await,
            invocation.is_finished()
        );
        ensure!(closed.unwrap()? == 0);
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .context("physical window closure")?;
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
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(clock.active_sleeps() == 0);
        ensure!(account.monthly_observer_count_for_test() == 0);
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(
            terminal_count(&stopped, poll_start) == 0,
            "interruption cannot invent a poll terminal"
        );
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
            ?resource, ?mode, %poll_start, connections,
            "Monthly stop retired P2 input poll before peer release, with no poll terminal"
        );

        if durable {
            let mut grant = policy;
            if resource == Resource::Storage {
                grant.available_durable_storage_byte_seconds = u64::MAX;
            }
            registry.set_policy(grant.clone());
            applied_refresh(&limits, &registry, context.account_id)
                .await?
                .await?;
            let replay_gate = replay_stop.map(|(ready_probe, order)| {
                (worker.pause_p2_connect_replay_for_test(ready_probe), ready_probe, order)
            });
            executor.resume(&id, false).await?;
            if let Some((mut gate, ready_probe, order)) = replay_gate {
                tokio::time::timeout(Duration::from_secs(10), gate.entered).await??;
                let replay_proposal = worker.current_monthly_proposal_for_test();
                ensure!(replay_proposal.resident_generation > initial.resident_generation);
                ensure!(worker.concurrent_agent_permit_is_held().await);
                let replay_acquisitions = worker.permit_acquisitions_for_test();
                let mut replay_signal = worker.raw_interrupt_for_test();
                let connect_start = poll_starts(&stopped)[0];
                let ready_start = stopped.iter().find_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(start)
                        if start.function_name == "io::poll::pollable::ready" => Some(entry.oplog_index),
                    _ => None,
                }).context("recorded-true connect ready Start")?;
                ensure!(terminal_count(&stopped, ready_start) == 1);
                let mut subscribe = Some(gate.subscribe);
                if order != ReplayStopOrder::BeforeSubscription {
                    subscribe.take().unwrap().send(()).unwrap();
                    tokio::time::timeout(Duration::from_secs(10), &mut gate.waiting).await??;
                }
                if order == ReplayStopOrder::CompletionFirst {
                    gate.ready.send(()).unwrap();
                    ensure!(tokio::time::timeout(Duration::from_secs(10), &mut gate.selected).await??);
                }
                // For ready replay, the preceding connect poll has driven the real socket.
                // For poll completion-first, the controlled readiness has done so itself.
                let mut replay_peer = if ready_probe || order == ReplayStopOrder::CompletionFirst {
                    let (peer, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept()).await??;
                    connections += 1;
                    Some(peer)
                } else {
                    None
                };
                let mut exhausted = grant.clone();
                exhausted.available_memory_gb_seconds = 0;
                registry.set_policy(exhausted);
                let refresh = applied_refresh(&limits, &registry, context.account_id).await?;
                let accepted = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
                    .await?.context("replay monthly acceptance")?;
                ensure!(accepted.proposal.window_identity == replay_proposal.window_identity);
                ensure!(accepted.proposal.resident_generation == replay_proposal.resident_generation);
                ensure!(accepted.proposal.policy_revision == replay_proposal.policy_revision + 1);
                ensure!(accepted.proposal.exhaustion == Some("monthly memory exhausted"));
                ensure!(tokio::time::timeout(Duration::from_secs(10), accepted.completed).await??);
                let signal = tokio::time::timeout(Duration::from_secs(10), replay_signal.recv()).await??;
                ensure!(matches!(signal, InterruptKind::Suspend(_)));
                ensure!(worker.monthly_stop_for_test() == Some(signal));
                if let Some(subscribe) = subscribe {
                    subscribe.send(()).unwrap();
                }
                if order != ReplayStopOrder::CompletionFirst {
                    ensure!(!tokio::time::timeout(Duration::from_secs(10), &mut gate.selected).await??);
                }
                // Selection is not cleanup. Keep the Store inside revalidation after the signal
                // has been consumed, and prove its permit and socket have not been released.
                ensure!(worker.concurrent_agent_permit_is_held().await);
                ensure!(worker.permit_acquisitions_for_test() == replay_acquisitions);
                if let Some(peer) = &mut replay_peer {
                    let mut byte = [0];
                    ensure!(matches!(peer.try_read(&mut byte), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
                }
                ensure!(!invocation.is_finished());
                gate.finish.send(()).unwrap();
                if let Some(peer) = &mut replay_peer {
                    let mut byte = [0];
                    ensure!(tokio::time::timeout(Duration::from_secs(10), peer.read(&mut byte)).await?? == 0);
                }
                tokio::time::timeout(Duration::from_secs(10), async {
                    while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                        tokio::task::yield_now().await;
                    }
                }).await.context("revalidation physical unload")?;
                tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
                tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
                refresh.await?;
                ensure!(worker.unload_succeeded_for_test());
                ensure!(worker.permit_acquisitions_for_test() == replay_acquisitions);
                ensure!(account.monthly_observer_count_for_test() == 0);
                ensure!(clock.active_sleeps() == 0);
                let replay_stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                ensure!(poll_starts(&replay_stopped) == poll_starts(&stopped));
                ensure!(terminal_count(&replay_stopped, connect_start) == 1);
                ensure!(terminal_count(&replay_stopped, ready_start) == 1);
                ensure!(terminal_count(&replay_stopped, poll_start) == 0);
                ensure!(count_agent_invocation_pair_since(&replay_stopped, OplogIndex::INITIAL) == (2, 1));
                let raw = worker.oplog().read_exact(
                    OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64(),
                ).await;
                ensure!(raw.values().filter(|entry| matches!(entry, OplogEntry::Suspend { .. })).count() == 2);
                ensure!(raw.values().all(|entry| !matches!(entry, OplogEntry::Error { .. } | OplogEntry::Cancelled { .. })));
                info!(ready_probe, completion_first = order == ReplayStopOrder::CompletionFirst,
                    before_subscription = order == ReplayStopOrder::BeforeSubscription,
                    %poll_start, "Monthly stop retired controlled TCP connect replay; original terminals and retained input Start preserved");
                registry.set_policy(grant);
                applied_refresh(&limits, &registry, context.account_id).await?.await?;
                executor.resume(&id, false).await?;
            }
            let (mut resumed_peer, _) =
                tokio::time::timeout(Duration::from_secs(10), listener.accept())
                    .await
                    .context("P2 reconnect during reconstruction")??;
            connections += 1;
            resumed_peer.write_all(b"x").await?;
            let resumed = tokio::time::timeout(Duration::from_secs(10), &mut invocation)
                .await
                .context("retained invocation continuation")???
                .into_typed::<Result<bool, String>>()?;
            ensure!(
                resumed == Ok(true),
                "reconstructed P2 block returned {resumed:?}"
            );
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(
                poll_starts(&recovered) == poll_starts(&stopped),
                "repair must retain the original Start"
            );
            ensure!(terminal_count(&recovered, poll_start) == 1);
            ensure!(recovered.iter().all(|e| !matches!(
                &e.entry, PublicOplogEntry::Cancelled(c) if c.start_index == poll_start
            )));
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            let cached = executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    "tcp_input_block_p2",
                    data_value!(port),
                )
                .await?
                .into_typed::<Result<bool, String>>()?;
            ensure!(cached == Ok(true));
            ensure!(
                count_agent_invocation_pair_since(
                    &executor.get_oplog(&id, OplogIndex::INITIAL).await?,
                    OplogIndex::INITIAL,
                ) == (2, 2)
            );
            ensure!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err(),
                "same-key lookup cannot reconnect"
            );
            let fresh = executor.invoke_and_await_agent(
                &component,
                &name,
                "tcp_input_block_p2",
                data_value!(port),
            );
            let fresh_peer = async {
                let (mut peer, _) = listener.accept().await?;
                peer.write_all(b"y").await?;
                Ok::<_, anyhow::Error>(())
            };
            let (fresh, _) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::try_join!(fresh, fresh_peer)
            })
            .await??;
            ensure!(fresh.into_typed::<Result<bool, String>>()? == Ok(true));
            connections += 1;
            info!(
                ?resource, %poll_start, connections,
                "Original poll repaired; cached lookup did not connect; fresh invocation succeeded"
            );
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    // On a failed parked-wait assertion the peer is dropped before stopping the Worker.
    // The deadline is a fixture bound, never evidence that interruption succeeded.
    drop(listener);
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
    }
    result
}

fn poll_starts(entries: &[PublicOplogEntryWithIndex]) -> Vec<OplogIndex> {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name == "io::poll::poll" => {
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
