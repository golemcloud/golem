mod completion;

use super::*;
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{
    AgentError, OplogEntry, PublicAgentInvocation, PublicDurableFunctionType,
    PublicOplogEntryWithIndex,
};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::resource_usage_metering::{
    FilesystemUsage, ScriptedFilesystemUsageForTest,
};
use golem_worker_executor::services::{HasOplog, UsesAllDeps};
use golem_worker_executor::worker::{MonthlyClockForTest, Worker};
use test_r::test;

use crate::api::interruption::support::{FUEL_BUDGET, Resource, check_accounting};
use golem_worker_executor::worker::{P3UdpReceivePendingForTest, P3UdpReceiveStateForTest};

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

pending_case!(durable_p3_udp_receive_memory_quota, Memory, Durable);
pending_case!(ephemeral_p3_udp_receive_memory_quota, Memory, Ephemeral);
pending_case!(
    durable_p3_udp_receive_compute_quota_prepaid,
    Compute,
    Durable
);
pending_case!(
    ephemeral_p3_udp_receive_compute_quota_prepaid,
    Compute,
    Ephemeral
);
pending_case!(
    durable_p3_udp_receive_scripted_storage_quota,
    Storage,
    Durable
);
pending_case!(
    ephemeral_p3_udp_receive_scripted_storage_quota,
    Storage,
    Ephemeral
);

async fn pending_phase(
    phases: &mut tokio::sync::mpsc::UnboundedReceiver<P3UdpReceivePendingForTest>,
    key: &IdempotencyKey,
) -> anyhow::Result<P3UdpReceivePendingForTest> {
    let phase = tokio::time::timeout(Duration::from_secs(10), phases.recv())
        .await?
        .context("native UDP receive observer closed")?;
    ensure!(phase.invocation_key.as_ref() == Some(key));
    ensure!(
        phase.state() == P3UdpReceiveStateForTest::Pending,
        "stale Pending: {phase:?}"
    );
    Ok(phase)
}

const RECEIVE: &str = "sockets::types::udp-socket::receive";

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
    let starts: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name == RECEIVE => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    ensure!(starts.len() == 1, "one receive Start: {entries:#?}");
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(start) => {
                ensure!(start.parent_start_index.is_none() && start.observational_owner.is_none());
                if entry.oplog_index == starts[0] {
                    ensure!(matches!(
                        start.durable_function_type,
                        PublicDurableFunctionType::ReadRemote(_)
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
    let delivered = entries.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(delivery) if delivery.start_index == starts[0])).count();
    ensure!(delivered == terminals, "receive delivery: {entries:#?}");
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
            agent_id!("Networking", "p3-udp-receive-seed"),
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
    if resource == Resource::Compute {
        limits.run_batch_for_test().await;
    }
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("Networking", "monthly-p3-udp-receive")
    } else {
        agent_id!("EphemeralNetworking", "monthly-p3-udp-receive")
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
    let mut phases = worker.observe_p3_udp_receive_for_test();
    let mut failure_address = None;
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
                    "udp_receive_p3",
                    data_value!(),
                )
                .await
        }
    });
    let result = async {
        let phase = pending_phase(&mut phases, &key).await?;
        failure_address = Some(phase.local_address);
        ensure!(phase.local_address.ip().is_loopback() && phase.local_address.port() != 0);
        executor.commit_oplog(&id).await?;
        let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let receive_start = receive_start(&entries, 0)?;
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
        ensure!(phase.runtime == initial);
        ensure!(phase.state() == P3UdpReceiveStateForTest::Pending);
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(!invocation.is_finished());
        info!(
            ?resource, ?mode, %key, ?id, ?phase, %receive_start, ?initial,
            "Native unconnected P3 UDP receive Poll::Pending; open root Start; no datagram sent"
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
        ensure!(phase.state() == P3UdpReceiveStateForTest::Pending);
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
        let attempt = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    attempt = attempts.recv() => return attempt.context("acceptance observer closed"),
                    poll = polls.recv(), if resource == Resource::Storage => {
                        poll.context("storage monitor timer closed")?;
                        // A sampled value is not yet an accepted, accrued allocation. Drive
                        // subsequent monitor ticks until actual capacity exhaustion is proposed.
                        tokio::time::sleep(Duration::from_millis(10)).await;
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
        // No datagram is sent until stop-driven physical unload has finished.
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .with_context(|| format!("physical window closure: phase={phase:?}, invocation_finished={}", invocation.is_finished()))?;
        // UDP has no EOF. Rebinding the exact native address proves socket retirement.
        drop(tokio::net::UdpSocket::bind(phase.local_address).await.context("retired UDP port must be reusable")?);
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
        assert_invocation(&stopped, &key, 0)?;
        ensure!(
            self::receive_start(&stopped, 0)? == receive_start,
            "interruption must leave the original receive Start open"
        );
        ensure!(phase.state() == P3UdpReceiveStateForTest::Dropped, "native future completed instead of interruption: {phase:?}");
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
            ?resource, ?mode, ?phase, %receive_start,
            "Monthly stop dropped native receive and retired Worker without any datagram or terminal"
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
            executor.resume(&id, false).await?;
            let reconstructed = pending_phase(&mut phases, &key).await?;
            failure_address = Some(reconstructed.local_address);
            ensure!(reconstructed.runtime.resident_generation > initial.resident_generation);
            ensure!(reconstructed.runtime.start_attempt != initial.start_attempt);
            ensure!(reconstructed.runtime.fingerprint == initial.fingerprint);
            executor.commit_oplog(&id).await?;
            ensure!(self::receive_start(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, 0)? == receive_start);
            let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
            ensure!(sender.send_to(b"recovered", reconstructed.local_address).await? == 9);
            let resumed = tokio::time::timeout(Duration::from_secs(10), &mut invocation)
                .await.context("retained invocation continuation")???;
            ensure!(resumed.into_typed::<Result<Vec<u8>, String>>()? == Ok(b"recovered".to_vec()));
            ensure!(reconstructed.state() == P3UdpReceiveStateForTest::Returned);
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(self::receive_start(&recovered, 1)? == receive_start);
            assert_settled_history(&recovered)?;
            assert_invocation(&recovered, &key, 1)?;
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, "udp_receive_p3", data_value!()).await?;
            ensure!(cached.into_typed::<Result<Vec<u8>, String>>()? == Ok(b"recovered".to_vec()));
            ensure!(phases.try_recv().is_err(), "cached lookup must not receive natively");
            drop(sender);
            failure_address = None;
            // Force real reconstruction of completed history with the sender gone.
            tokio::time::timeout(Duration::from_secs(10), async {
                while !worker.stop_if_idle().await {
                    tokio::task::yield_now().await;
                }
            }).await.context("idle eviction before completed-history replay")?;
            tokio::time::timeout(Duration::from_secs(10), async {
                while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                    tokio::task::yield_now().await;
                }
            }).await?;
            tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
            tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
            drop(tokio::net::UdpSocket::bind(reconstructed.local_address).await?);
            let before_history = worker.permit_acquisitions_for_test();
            let fresh_key = IdempotencyKey::fresh();
            let fresh = executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key, "udp_receive_p3", data_value!());
            let release_fresh = async {
                // The first native receive must belong to fresh work, not completed history.
                let fresh_phase = pending_phase(&mut phases, &fresh_key).await?;
                ensure!(fresh_phase.runtime.resident_generation > reconstructed.runtime.resident_generation);
                ensure!(worker.permit_acquisitions_for_test() > before_history);
                let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
                ensure!(sender.send_to(b"fresh", fresh_phase.local_address).await? == 5);
                Ok::<_, anyhow::Error>(fresh_phase)
            };
            let (fresh, fresh_phase) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::try_join!(fresh, release_fresh)
            }).await.context("completed-history replay and fresh receive")??;
            ensure!(fresh.into_typed::<Result<Vec<u8>, String>>()? == Ok(b"fresh".to_vec()));
            ensure!(fresh_phase.state() == P3UdpReceiveStateForTest::Returned);
            ensure!(phases.try_recv().is_err(), "no extra native receive");
            let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (3, 3));
            let starts: Vec<_> = history.iter().filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(start) if start.function_name == RECEIVE => Some(entry.oplog_index),
                _ => None,
            }).collect();
            ensure!(starts.len() == 2 && starts[0] == receive_start);
            for start in starts {
                ensure!(terminal_count(&history, start) == 1);
                ensure!(history.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(delivery) if delivery.start_index == start)).count() == 1);
            }
            assert_settled_history(&history)?;
            assert_invocation(&history, &key, 1)?;
            assert_invocation(&history, &fresh_key, 1)?;
            info!(?resource, %key, %receive_start, ?reconstructed, ?fresh_phase, datagrams_sent = 2,
                "Original receive Start repaired once; completed history needed no datagram; fresh receive consumed its own datagram");
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    // Release a stuck native receive only after recording failure, never on the passing stop path.
    if result.is_err() {
        if let Some(address) = failure_address
            && let Ok(sender) = tokio::net::UdpSocket::bind("127.0.0.1:0").await
        {
            let _ = sender.send_to(b"failure-cleanup", address).await;
        }
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
            PublicOplogEntry::Cancelled(_) | PublicOplogEntry::Error(_) => {
                anyhow::bail!("unexpected terminal: {entry:?}")
            }
            _ => {}
        }
    }
    Ok(())
}
