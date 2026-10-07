mod completion;
mod history;
mod peers;

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
use golem_worker_executor::worker::{P2PollPendingForTest, P2PollStateForTest};
use history::*;
use peers::*;

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
            pending_multi_poll(
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

pending_case!(durable_p2_multi_poll_monthly_memory, Memory, Durable);
pending_case!(ephemeral_p2_multi_poll_monthly_memory, Memory, Ephemeral);
pending_case!(
    durable_p2_multi_poll_monthly_compute_prepaid,
    Compute,
    Durable
);
pending_case!(
    ephemeral_p2_multi_poll_monthly_compute_prepaid,
    Compute,
    Ephemeral
);
pending_case!(
    durable_p2_multi_poll_monthly_scripted_storage,
    Storage,
    Durable
);
pending_case!(
    ephemeral_p2_multi_poll_monthly_scripted_storage,
    Storage,
    Ephemeral
);

async fn pending_multi_poll(
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
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(&component.id, agent_id!("Networking", "p2-multi-poll-seed"))
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
        agent_id!("Networking", "monthly-p2-multi-poll")
    } else {
        agent_id!("EphemeralNetworking", "monthly-p2-multi-poll")
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
    let mut phases = worker.observe_p2_poll_for_test();
    let listeners = listeners().await?;
    let first_port = listeners[0].local_addr()?.port();
    let second_port = listeners[1].local_addr()?.port();
    let mut cleanup = vec![(id.clone(), worker.clone())];
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
                    "tcp_inputs_poll_p2",
                    data_value!(first_port, second_port),
                )
                .await
        }
    });
    let result = async {
        let mut effects = PeerEffects::default();
        let mut peers = accept_pair(&listeners, &mut effects).await?;
        let phase = pending_phase(&mut phases, &key, 2).await?;
        executor.commit_oplog(&id).await?;
        let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let poll_start = input_poll(&entries, &key, 2, None)?;
        ensure!(phase.start == poll_start);
        assert_invocation(&entries, &key, 0)?;
        ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
        tokio::time::timeout(Duration::from_secs(10), polls.recv())
            .await?
            .context("timer closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(initial.exhaustion.is_none(), "{initial:?}");
        ensure!(phase.runtime == initial && phase.state() == P2PollStateForTest::Pending);
        ensure!(effects.accepts == [1, 1] && effects.sent == [0, 0]);
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(!invocation.is_finished());
        info!(
            ?resource, ?mode, %key, ?id, %poll_start, ?phase, ?effects,
            "Two-input native poll Pending with committed count-2 Start and held permit"
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
        ensure!(phase.state() == P2PollStateForTest::Pending);
        let teardown = (resource == Resource::Memory && durable)
            .then(|| worker.pause_next_teardown_fence_for_test());
        let stop_started = tokio::time::Instant::now();
        let allocation_observations = source.observations();
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
        ensure!(attempt.proposal.fingerprint == initial.fingerprint);
        ensure!(attempt.proposal.start_attempt == initial.start_attempt);
        ensure!(attempt.proposal.period == initial.period);
        ensure!(attempt.proposal.window_identity == initial.window_identity);
        ensure!(attempt.proposal.resident_generation == initial.resident_generation);
        ensure!(attempt.proposal.fuel_generation == initial.fuel_generation);
        ensure!(attempt.proposal.exhaustion == Some(resource.reason(durable)));
        ensure!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        let signal = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        ensure!(matches!(signal, InterruptKind::Suspend(_)));
        ensure!(worker.monthly_stop_for_test() == Some(signal));
        ensure!(worker.frozen_stop_for_test() == Some(signal));
        ensure!(worker.owner_stop_for_test().await == Some(signal));
        if let Some((entered, release)) = teardown {
            tokio::time::timeout(Duration::from_secs(10), entered)
                .await
                .context("stop reached teardown fence")??;
            // A delivered signal cannot release the permit while teardown is fenced.
            // Socket closure is observed separately after ordinary unload resumes.
            ensure!(worker.is_loaded().await);
            ensure!(worker.concurrent_agent_permit_is_held().await);
            release.send(()).map_err(|_| anyhow!("teardown fence closed"))?;
        }
        if resource == Resource::Compute {
            ensure!(account.settled_fuel_generation_for_test() > initial.fuel_generation);
        }
        if resource == Resource::Storage {
            ensure!(source.observations() > allocation_observations);
            let other = if durable {
                AgentMode::Ephemeral
            } else {
                AgentMode::Durable
            };
            ensure!(!account.monthly_capacity_is_exhausted_for_test(other));
        }
        // Neither peer sends or closes before the Worker retires both sockets.
        closed_pair(&mut peers, &mut effects).await?;
        ensure!(effects.closed == [1, 1] && effects.sent == [0, 0]);
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
        ensure!(stop_started.elapsed() < Duration::from_secs(8));
        ensure!(phase.state() == P2PollStateForTest::Dropped, "native poll returned: {phase:?}");
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(input_poll(&stopped, &key, 2, None)? == poll_start);
        assert_invocation(&stopped, &key, 0)?;
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
            ?resource, ?mode, %poll_start, ?effects, elapsed = ?stop_started.elapsed(),
            "Monthly stop retired both sockets and permit with no poll terminal"
        );

        let mut grant = policy;
        if resource == Resource::Storage {
            grant.available_durable_storage_byte_seconds = u64::MAX;
            grant.available_ephemeral_storage_byte_seconds = u64::MAX;
        }
        registry.set_policy(grant);
        applied_refresh(&limits, &registry, context.account_id).await?.await?;
        if durable {
            executor.resume(&id, false).await?;
            let mut resumed_peers = accept_pair(&listeners, &mut effects).await?;
            let reconstructed = pending_phase(&mut phases, &key, 2).await?;
            ensure!(reconstructed.start == poll_start);
            ensure!(reconstructed.runtime.resident_generation > initial.resident_generation);
            ensure!(reconstructed.runtime.start_attempt != initial.start_attempt);
            ensure!(reconstructed.runtime.fingerprint == initial.fingerprint);
            ensure!(worker.concurrent_agent_permit_is_held().await);
            executor.commit_oplog(&id).await?;
            ensure!(input_poll(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, &key, 2, None)? == poll_start);
            release_second(&mut resumed_peers, &mut effects).await?;
            let resumed = tokio::time::timeout(Duration::from_secs(10), &mut invocation)
                .await.context("retained multi-poll continuation")???
                .into_typed::<Result<Vec<u32>, String>>()?;
            ensure!(resumed == Ok(vec![1]), "only peer B was made ready: {resumed:?}");
            ensure!(reconstructed.state() == P2PollStateForTest::Returned);
            closed_pair(&mut resumed_peers, &mut effects).await?;
            ensure!(effects.accepts == [2, 2] && effects.closed == [2, 2] && effects.sent == [0, 1]);
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(input_poll(&recovered, &key, 2, Some(&[1]))? == poll_start);
            ensure!(poll_starts(&recovered) == poll_starts(&stopped));
            assert_invocation(&recovered, &key, 1)?;
            settled_history(&recovered)?;
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            let cached = executor.invoke_and_await_agent_with_key(
                &component, &name, &key, "tcp_inputs_poll_p2", data_value!(first_port, second_port),
            ).await?.into_typed::<Result<Vec<u32>, String>>()?;
            ensure!(cached == Ok(vec![1]));
            no_connections(&listeners).await?;
            ensure!(poll_starts(&executor.get_oplog(&id, OplogIndex::INITIAL).await?) == poll_starts(&recovered));
            assert_invocation(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, &key, 1)?;
            let fresh_key = IdempotencyKey::fresh();
            let fresh = executor.invoke_and_await_agent_with_key(
                &component, &name, &fresh_key, "tcp_inputs_poll_p2", data_value!(first_port, second_port),
            );
            let fresh_peers = async {
                let mut peers = accept_pair(&listeners, &mut effects).await?;
                let fresh_phase = pending_phase(&mut phases, &fresh_key, 2).await?;
                ensure!(fresh_phase.start != poll_start);
                release_second(&mut peers, &mut effects).await?;
                closed_pair(&mut peers, &mut effects).await?;
                ensure!(fresh_phase.state() == P2PollStateForTest::Returned);
                Ok::<_, anyhow::Error>(())
            };
            let (fresh, ()) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::try_join!(fresh, fresh_peers)
            }).await??;
            ensure!(fresh.into_typed::<Result<Vec<u32>, String>>()? == Ok(vec![1]));
            ensure!(effects.accepts == [3, 3] && effects.closed == [3, 3] && effects.sent == [0, 2]);
            let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(input_poll(&history, &key, 2, Some(&[1]))? == poll_start);
            input_poll(&history, &fresh_key, 2, Some(&[1]))?;
            assert_invocation(&history, &key, 1)?;
            assert_invocation(&history, &fresh_key, 1)?;
            ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (3, 3));
            settled_history(&history)?;
        } else {
            ensure!(executor.invoke_and_await_agent_with_key(
                &component, &name, &key, "tcp_inputs_poll_p2", data_value!(first_port, second_port),
            ).await.is_err());
            ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            let unchanged = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(input_poll(&unchanged, &key, 2, None)? == poll_start);
            assert_invocation(&unchanged, &key, 0)?;
            // Public tracing attributes are rendered from maps in arbitrary order.
            let raw_after = worker.oplog().read_exact(
                OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64(),
            ).await;
            ensure!(raw_after == entries, "ephemeral retry must not replay or append history");
            ensure!(effects.accepts == [1, 1] && effects.closed == [1, 1] && effects.sent == [0, 0]);
            no_connections(&listeners).await?;
            let fresh_key = IdempotencyKey::fresh();
            let fresh_physical = name.clone().with_ephemeral_invocation_phantom(&fresh_key).unwrap();
            let fresh_id = AgentId::from_agent_id(component.id, &fresh_physical).unwrap();
            ensure!(fresh_id != id);
            let fresh_worker = Worker::get_or_create_suspended(
                seed.all(), &OwnedAgentId::new(context.default_environment_id, &fresh_id),
                None, vec![], None, None, &InvocationContextStack::fresh(), Principal::anonymous(),
            ).await?;
            cleanup.push((fresh_id.clone(), fresh_worker.clone()));
            let mut fresh_phases = fresh_worker.observe_p2_poll_for_test();
            let fresh = executor.invoke_and_await_agent_with_key(
                &component, &name, &fresh_key, "tcp_inputs_poll_p2", data_value!(first_port, second_port),
            );
            let fresh_peers = async {
                let mut peers = accept_pair(&listeners, &mut effects).await?;
                let fresh_phase = pending_phase(&mut fresh_phases, &fresh_key, 2).await?;
                ensure!(fresh_worker.concurrent_agent_permit_is_held().await);
                release_second(&mut peers, &mut effects).await?;
                closed_pair(&mut peers, &mut effects).await?;
                ensure!(fresh_phase.state() == P2PollStateForTest::Returned);
                Ok::<_, anyhow::Error>(())
            };
            let (fresh, ()) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::try_join!(fresh, fresh_peers)
            }).await??;
            ensure!(fresh.into_typed::<Result<Vec<u32>, String>>()? == Ok(vec![1]));
            let history = executor.get_oplog(&fresh_id, OplogIndex::INITIAL).await?;
            input_poll(&history, &fresh_key, 2, Some(&[1]))?;
            assert_invocation(&history, &fresh_key, 1)?;
            settled_history(&history)?;
            ensure!(effects.accepts == [2, 2] && effects.closed == [2, 2] && effects.sent == [0, 1]);
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            let original = worker.oplog().read_exact(
                OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64(),
            ).await;
            ensure!(original == entries, "fresh ephemeral identity cannot execute failed history");
        }
        no_connections(&listeners).await?;
        info!(?resource, ?mode, %key, %poll_start, ?effects,
            "Peer effects counted separately from same-key lookup and fresh invocation");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    // On a failed parked-wait assertion the peer is dropped before stopping the Worker.
    // The deadline is a fixture bound, never evidence that interruption succeeded.
    drop(listeners);
    if result.is_err() {
        for (id, worker) in cleanup {
            let _ = tokio::time::timeout(Duration::from_secs(10), executor.interrupt(&id)).await;
            let _ = tokio::time::timeout(
                Duration::from_secs(10),
                worker.join_accepted_stops_for_test(),
            )
            .await;
            let _ =
                tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test())
                    .await;
        }
        if !invocation.is_finished()
            && tokio::time::timeout(Duration::from_secs(10), &mut invocation)
                .await
                .is_err()
        {
            invocation.abort();
            let _ = tokio::time::timeout(Duration::from_secs(10), &mut invocation).await;
        }
    }
    result
}
