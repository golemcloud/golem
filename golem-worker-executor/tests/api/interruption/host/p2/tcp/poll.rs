use super::*;
use crate::native_waits::WaitState;
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry, PublicOplogEntryWithIndex};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::resource_limits::ResourceUsageFlusher;
use golem_worker_executor::services::{HasOplog, HasOplogService, UsesAllDeps};
use golem_worker_executor::worker::Worker;
use test_r::test;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::api::interruption::support::{FUEL_BUDGET, Resource, check_accounting};

macro_rules! pending_case {
    ($name:ident, Storage, $mode:ident) => {
        pending_case!(#[cfg(target_os = "linux")] #[ignore = "requires the privileged managed XFS test runner"] $name, Storage, $mode);
    };
    ($(#[$attr:meta])* $name:ident, $resource:ident, $mode:ident) => {
        $(#[$attr])*
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
                false,
            )
            .await
        }
    };
}

pending_case!(durable_p2_block_memory_quota, Memory, Durable);
pending_case!(ephemeral_p2_block_memory_quota, Memory, Ephemeral);
pending_case!(durable_p2_block_compute_quota_prepaid, Compute, Durable);
pending_case!(ephemeral_p2_block_compute_quota_prepaid, Compute, Ephemeral);
pending_case!(durable_p2_block_managed_xfs_storage_quota, Storage, Durable);
pending_case!(
    ephemeral_p2_block_managed_xfs_storage_quota,
    Storage,
    Ephemeral
);

#[test]
#[timeout("2m")]
async fn repeated_monthly_stop_preserves_reconstructed_connect_and_input_poll(
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
        true,
    )
    .await
}

async fn pending_block(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    resource: Resource,
    mode: AgentMode,
    repeat_stop: bool,
) -> anyhow::Result<()> {
    let mut policy = resource.policy();
    policy.available_durable_storage_byte_seconds = 1;
    policy.available_ephemeral_storage_byte_seconds = 1;
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
    let filesystem = managed_filesystem_config(metering)?;
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| {
            config.agent_sockets.enabled = true;
            config.resource_usage_metering = metering;
            if let Some(filesystem) = &filesystem {
                config.filesystem_storage.mode = filesystem.clone();
            }
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
    let (flushed, mut flushes) = tokio::sync::mpsc::unbounded_channel();
    let _usage_source = (resource != Resource::Memory).then(|| {
        let source: Arc<dyn ResourceUsageFlusher> = Arc::new(ObservedUsageFlusher {
            owner: Arc::downgrade(&worker),
            send: flushed,
        });
        worker
            .owner_runtime_resources()
            .register_resource_usage_flusher(Arc::downgrade(&source));
        source
    });
    let mut capacity_updates = if resource != Resource::Memory {
        let mut updates = limits
            .subscribe_capacity_updates_for_test(context.account_id)
            .await?;
        updates.borrow_and_update();
        Some(updates)
    } else {
        None
    };
    let oplog = worker.all().oplog_service();
    let read_stored = async || {
        let last = oplog.get_last_index(&owned, mode).await?;
        oplog
            .read_exact(&owned, mode, OplogIndex::INITIAL, last.as_u64())
            .await
    };
    let mut attempts =
        crate::monthly_acceptance::observe(worker.get_initial_worker_metadata().fingerprint);
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
        let initial = worker.current_monthly_proposal_for_test();
        let mut next_flush = async || {
            loop {
                let (window, revision) = flushes.recv().await.context("owner usage flusher closed")?;
                if window == initial.diagnostic_window_id && revision == initial.policy_revision {
                    return Ok::<_, anyhow::Error>(());
                }
            }
        };
        if resource != Resource::Memory {
            tokio::time::timeout(Duration::from_secs(10), next_flush())
                .await.context("initial monitor pre-wait flush")??;
        }
        let prefix = read_stored().await?;
        let prefix_last = *prefix.last_key_value().context("committed input-poll prefix is empty")?.0;
        let mut raw = worker.raw_interrupt_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        let mut resource_events = crate::resource_limit_observations::observe(&account);
        ensure!(initial.exhaustion.is_none(), "{initial:?}");
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(!invocation.is_finished());
        info!(
            ?resource, ?mode, %poll_start, ?initial, connections,
            "P2 input poll parked with live Worker and held permit; peer has sent no data"
        );

        if resource == Resource::Compute {
            ensure!(initial.fuel_generation.is_some());
            ensure!(!account.prepaid_fuel_is_exhausted(mode, initial.fuel_generation));
            ensure!(
                account.monthly_capacity_is_exhausted_for_test(mode),
                "Store must prepay the remaining fuel"
            );
            next_flush().await?;
            tokio::time::timeout(Duration::from_secs(10), next_flush())
                .await.context("compute check did not reach the next pre-wait flush")??;
            ensure!(!capacity_updates.as_ref().unwrap().has_changed()?);
            drop(capacity_updates.take());
            ensure!(worker.monthly_checks().contains(&(initial.diagnostic_window_id, initial.policy_revision)));
            ensure!(
                attempts.try_recv().is_err(),
                "zero unreserved fuel must not exhaust prepaid fuel"
            );
            ensure!(worker.current_monthly_proposal_for_test() == initial);
            ensure!(!invocation.is_finished() && worker.concurrent_agent_permit_is_held().await);
        }
        let refresh = if resource == Resource::Storage {
            allocate_storage(&owned).await?;
            // Keep the original revision: actual timer delivery, not a policy update,
            // triggers this storage check. The existing whole-test bound includes cadence.
            next_flush().await?;
            ensure!(!capacity_updates.as_ref().unwrap().has_changed()?);
            drop(capacity_updates.take());
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
        let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
        .await
        .context("monthly proposal")?
        .context("acceptance observer closed")?;
        ensure!(
            attempt.proposal.policy_revision
                == initial.policy_revision + u64::from(resource != Resource::Storage)
        );
        ensure!(attempt.proposal.period == initial.period);
        ensure!(attempt.proposal.diagnostic_window_id == initial.diagnostic_window_id);
        ensure!(attempt.proposal.resident_generation == initial.resident_generation);
        ensure!(attempt.proposal.fuel_generation == initial.fuel_generation);
        ensure!(attempt.proposal.exhaustion == Some(resource.reason(durable)));
        ensure!(attempt.accepted);
        let signal = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        ensure!(matches!(signal, InterruptKind::Suspend(_)));
        if resource == Resource::Compute {
            resource_events.assert_policy_advanced_after(registry.current_limits().monthly_policy_revision, initial.fuel_generation)?;
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
        ensure!(!worker.monthly_window_active_for_test());
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let mut retained_raw = read_stored().await?;
        ensure!(retained_raw.range(..=prefix_last).eq(prefix.iter()), "the complete committed input-poll prefix must survive stop");
        ensure!(retained_raw.values().all(|entry| !matches!(entry, OplogEntry::Jump { .. })));
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
            .await?;
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
        limits.run_batch_for_test().await;
        if resource == Resource::Storage { assert_storage_deleted(&owned)?; }
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
            let mut native_polls = repeat_stop.then(|| crate::native_waits::poll(worker.get_initial_worker_metadata().fingerprint));
            executor.resume(&id, false).await?;
            if let Some(native_polls) = &mut native_polls {
                let (mut replay_peer, _) =
                    tokio::time::timeout(Duration::from_secs(10), listener.accept()).await??;
                connections += 1;
                let pending = tokio::time::timeout(Duration::from_secs(10), native_polls.recv())
                    .await?
                    .context("reconstructed native input poll Pending")?;
                ensure!(pending.invocation_key.as_ref() == Some(&key));
                ensure!(pending.start == poll_start);
                ensure!(pending.state() == WaitState::Pending);
                ensure!(pending.pollable_reps.len() == 1);
                let replay_proposal = worker.current_monthly_proposal_for_test();
                ensure!(pending.runtime == replay_proposal);
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
                let reconstructed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                ensure!(terminal_count(&reconstructed, connect_start) == 1);
                ensure!(terminal_count(&reconstructed, ready_start) == 1);
                ensure!(terminal_count(&reconstructed, poll_start) == 0);
                ensure!(!invocation.is_finished());
                let mut exhausted = grant.clone();
                exhausted.available_memory_gb_seconds = 0;
                registry.set_policy(exhausted);
                let refresh = applied_refresh(&limits, &registry, context.account_id).await?;
                let accepted = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
                    .await?.context("replay monthly acceptance")?;
                ensure!(accepted.proposal.diagnostic_window_id == replay_proposal.diagnostic_window_id);
                ensure!(accepted.proposal.resident_generation == replay_proposal.resident_generation);
                ensure!(accepted.proposal.policy_revision == replay_proposal.policy_revision + 1);
                ensure!(accepted.proposal.exhaustion == Some("monthly memory exhausted"));
                ensure!(accepted.accepted);
                let signal = tokio::time::timeout(Duration::from_secs(10), replay_signal.recv()).await??;
                ensure!(matches!(signal, InterruptKind::Suspend(_)));
                let mut byte = [0];
                ensure!(tokio::time::timeout(Duration::from_secs(10), replay_peer.read(&mut byte)).await?? == 0);
                tokio::time::timeout(Duration::from_secs(10), async {
                    while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                        tokio::task::yield_now().await;
                    }
                }).await.context("reconstructed input physical unload")?;
                tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
                tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
                refresh.await?;
                ensure!(worker.unload_succeeded_for_test());
                ensure!(worker.permit_acquisitions_for_test() == replay_acquisitions);
                ensure!(!worker.monthly_window_active_for_test());
                ensure!(pending.state() == WaitState::Dropped);
                ensure!(pending.window_closed(), "the reconstructed poll's own window must close");
                ensure!(!invocation.is_finished());
                let replay_stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                let replay_raw = read_stored().await?;
                let retained_last = *retained_raw.last_key_value().unwrap().0;
                ensure!(replay_raw.range(..=retained_last).eq(retained_raw.iter()), "the full first stopped prefix must survive the repeated stop");
                retained_raw = replay_raw;
                ensure!(poll_starts(&replay_stopped) == poll_starts(&stopped));
                ensure!(terminal_count(&replay_stopped, connect_start) == 1);
                ensure!(terminal_count(&replay_stopped, ready_start) == 1);
                ensure!(terminal_count(&replay_stopped, poll_start) == 0);
                ensure!(count_agent_invocation_pair_since(&replay_stopped, OplogIndex::INITIAL) == (2, 1));
                let raw = worker.oplog().read_exact(
                    OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64(),
                ).await?;
                ensure!(raw.values().filter(|entry| matches!(entry, OplogEntry::Suspend { .. })).count() == 2);
                ensure!(raw.values().all(|entry| !matches!(entry, OplogEntry::Error { .. } | OplogEntry::Cancelled { .. })));
                limits.run_batch_for_test().await;
                check_accounting(
                    &registry.applied_updates(), Resource::Memory, true,
                    replay_proposal.policy_revision, replay_proposal.period,
                )?;
                info!(%poll_start, connections,
                    "Repeated monthly stop retired native input after connect reconstruction; original terminals and retained input Start preserved");
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
            let recovered_raw = read_stored().await?;
            let retained_last = *retained_raw.last_key_value().unwrap().0;
            ensure!(recovered_raw.range(..=retained_last).eq(retained_raw.iter()), "the full stopped prefix must survive ordinary P2 reconstruction");
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
