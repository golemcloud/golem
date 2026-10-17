mod completion;
mod cross_socket;
mod repeated_stop;
mod request_tasks;

use super::*;
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry, PublicOplogEntryWithIndex};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::resource_usage_metering::{
    FilesystemUsage, ScriptedFilesystemUsageForTest,
};
use golem_worker_executor::services::{HasOplog, UsesAllDeps};
use golem_worker_executor::worker::Worker;
use request_tasks::{finish_input_test, join_request, request_is_finished, start_refresh};
use test_r::test;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::task::AbortOnDropHandle;

use crate::api::interruption::support::{FUEL_BUDGET, Resource, check_accounting};
use crate::native_waits::{WaitReceipt, WaitState};

macro_rules! pending_case {
    ($name:ident, $operation:ident, $resource:ident, $mode:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            pending_input(
                last_unique_id,
                deps,
                host_api_tests,
                Resource::$resource,
                AgentMode::$mode,
                Operation::$operation,
            )
            .await
        }
    };
}

pending_case!(durable_p2_blocking_read_memory_quota, Read, Memory, Durable);
pending_case!(
    ephemeral_p2_blocking_read_memory_quota,
    Read,
    Memory,
    Ephemeral
);
pending_case!(
    durable_p2_blocking_read_compute_quota_prepaid,
    Read,
    Compute,
    Durable
);
pending_case!(
    ephemeral_p2_blocking_read_compute_quota_prepaid,
    Read,
    Compute,
    Ephemeral
);
pending_case!(
    durable_p2_blocking_read_scripted_storage_quota,
    Read,
    Storage,
    Durable
);
pending_case!(
    ephemeral_p2_blocking_read_scripted_storage_quota,
    Read,
    Storage,
    Ephemeral
);
pending_case!(durable_p2_blocking_skip_memory_quota, Skip, Memory, Durable);
pending_case!(
    ephemeral_p2_blocking_skip_memory_quota,
    Skip,
    Memory,
    Ephemeral
);
pending_case!(
    durable_p2_blocking_skip_compute_quota_prepaid,
    Skip,
    Compute,
    Durable
);
pending_case!(
    ephemeral_p2_blocking_skip_compute_quota_prepaid,
    Skip,
    Compute,
    Ephemeral
);
pending_case!(
    durable_p2_blocking_skip_scripted_storage_quota,
    Skip,
    Storage,
    Durable
);
pending_case!(
    ephemeral_p2_blocking_skip_scripted_storage_quota,
    Skip,
    Storage,
    Ephemeral
);
pending_case!(
    durable_p2_blocking_splice_memory_quota,
    Splice,
    Memory,
    Durable
);
pending_case!(
    ephemeral_p2_blocking_splice_memory_quota,
    Splice,
    Memory,
    Ephemeral
);
pending_case!(
    durable_p2_blocking_splice_compute_quota_prepaid,
    Splice,
    Compute,
    Durable
);
pending_case!(
    ephemeral_p2_blocking_splice_compute_quota_prepaid,
    Splice,
    Compute,
    Ephemeral
);
pending_case!(
    durable_p2_blocking_splice_scripted_storage_quota,
    Splice,
    Storage,
    Durable
);
pending_case!(
    ephemeral_p2_blocking_splice_scripted_storage_quota,
    Splice,
    Storage,
    Ephemeral
);

#[derive(Clone, Copy, Debug)]
enum Operation {
    Read,
    Skip,
    Splice,
}

impl Operation {
    fn method(self) -> &'static str {
        match self {
            Self::Read => "tcp_blocking_read_p2",
            Self::Skip => "tcp_blocking_skip_p2",
            Self::Splice => "tcp_blocking_splice_p2",
        }
    }
    fn export(self) -> &'static str {
        match self {
            Self::Read => "blocking_read",
            Self::Skip => "blocking_skip",
            Self::Splice => "blocking_splice",
        }
    }
    fn assert_result(self, result: golem_test_framework::dsl::AgentResult) -> anyhow::Result<()> {
        match self {
            Self::Read => {
                ensure!(result.into_typed::<Result<Vec<u8>, String>>()? == Ok(vec![b'x']))
            }
            Self::Skip | Self::Splice => {
                ensure!(result.into_typed::<Result<u64, String>>()? == Ok(1))
            }
        }
        Ok(())
    }
}

async fn pending_phase(
    phases: &mut crate::native_waits::WaitSubscription,
    operation: Operation,
    key: &IdempotencyKey,
) -> anyhow::Result<WaitReceipt> {
    let phase = tokio::time::timeout(Duration::from_secs(10), phases.recv())
        .await?
        .context("native input observer closed")?;
    ensure!(phase.operation == operation.export());
    ensure!(phase.invocation_key.as_ref() == Some(key));
    ensure!(
        phase.start_index.is_none(),
        "raw TCP input has no stream Start"
    );
    // See native_splice_interrupts_actual_pending_without_output_effect for writable-output proof.
    ensure!(
        phase.state() == WaitState::Pending,
        "stale Pending: {phase:?}"
    );
    Ok(phase)
}

fn completed_setup(entries: &[PublicOplogEntryWithIndex]) -> anyhow::Result<OplogIndex> {
    let polls = poll_starts(entries);
    ensure!(
        polls.len() == 1,
        "exactly one connect poll, no input poll: {entries:#?}"
    );
    for entry in entries {
        ensure!(
            !matches!(entry.entry, PublicOplogEntry::Cancelled(_)),
            "setup must finish with End"
        );
        if let PublicOplogEntry::Start(start) = &entry.entry {
            ensure!(start.parent_start_index.is_none() && start.observational_owner.is_none());
            ensure!(
                !start.function_name.contains("streams"),
                "raw TCP has NoHostStart: {start:?}"
            );
            ensure!(
                terminal_count(entries, entry.oplog_index) == 1,
                "setup Start must be completed: {entry:?}"
            );
        }
    }
    Ok(polls[0])
}

async fn pending_input(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    resource: Resource,
    mode: AgentMode,
    operation: Operation,
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
    let mut refresh = None;
    let context = TestContext::new(last_unique_id);
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| {
            config.agent_sockets.enabled = true;
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
            agent_id!("Networking", "p2-native-input-seed"),
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
    if resource == Resource::Memory && matches!(operation, Operation::Splice) {
        // Flush the seed's fractional memory charge under an earlier revision.
        limits.run_batch_for_test().await;
        registry.set_policy(policy.clone());
        start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
        join_request(&mut refresh, "policy refresh").await?;
        limits.run_batch_for_test().await;
    }
    let baseline_updates = registry.applied_updates().len();
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("Networking", "monthly-p2-native-input")
    } else {
        agent_id!("EphemeralNetworking", "monthly-p2-native-input")
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
    let mut attempts =
        crate::monthly_acceptance::observe(worker.get_initial_worker_metadata().fingerprint);
    let mut phases = crate::native_waits::input(worker.get_initial_worker_metadata().fingerprint);
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
                    operation.method(),
                    data_value!(port),
                )
                .await
        }
    })));
    let result = async {
        let (mut peer, _) = tokio::time::timeout(Duration::from_secs(30), listener.accept())
            .await
            .context("first TCP connection")??;
        let mut connections = 1;
        let phase = pending_phase(&mut phases, operation, &key).await?;
        executor.commit_oplog(&id).await?;
        let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let connect_start = completed_setup(&entries)?;
        ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
        let initial = worker.current_monthly_proposal_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(initial.exhaustion.is_none(), "{initial:?}");
        ensure!(phase.runtime == initial);
        ensure!(phase.state() == WaitState::Pending);
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(!request_is_finished(&invocation));
        if matches!(operation, Operation::Splice) {
            let mut unsent = [0];
            ensure!(
                tokio::time::timeout(Duration::from_millis(25), peer.read(&mut unsent))
                    .await
                    .is_err(),
                "pending splice must not send or close before input arrives"
            );
        }
        info!(
            ?resource, ?mode, ?operation, %key, ?id, ?phase, %connect_start, ?initial, connections,
            "Native P2 input Poll::Pending; NoHostStart; peer has sent no data"
        );

        if resource == Resource::Compute {
            ensure!(initial.fuel_generation.is_some());
            ensure!(initial.fuel_generation == account.settled_fuel_generation_for_test());
            ensure!(
                account.monthly_capacity_is_exhausted_for_test(mode),
                "Store must prepay the remaining fuel"
            );
            ensure!(
                attempts.try_recv().is_err(),
                "zero unreserved fuel must not exhaust prepaid fuel"
            );
            ensure!(worker.current_monthly_proposal_for_test() == initial);
            ensure!(!request_is_finished(&invocation) && worker.concurrent_agent_permit_is_held().await);
        }
        ensure!(phase.state() == WaitState::Pending);
        if resource == Resource::Storage {
            source.set(FilesystemUsage::Authoritative {
                allocated_bytes: 1 << 30,
                filesystem_objects: 1,
            });
        }
        let mut exhausted = policy.clone();
        match resource {
            Resource::Memory => exhausted.available_memory_gb_seconds = 0,
            Resource::Compute => exhausted.available_fuel = 0,
            Resource::Storage if durable => exhausted.available_durable_storage_byte_seconds = 0,
            Resource::Storage => exhausted.available_ephemeral_storage_byte_seconds = 0,
        }
        registry.set_policy(exhausted);
        start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
        let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
            .await.context("monthly proposal")?.context("acceptance observer closed")?;
        ensure!(
            attempt.proposal.policy_revision
                == initial.policy_revision + 1
        );
        ensure!(attempt.proposal.period == initial.period);
        ensure!(attempt.proposal.diagnostic_window_id == initial.diagnostic_window_id);
        ensure!(attempt.proposal.resident_generation == initial.resident_generation);
        ensure!(attempt.proposal.fuel_generation == initial.fuel_generation);
        ensure!(attempt.proposal.exhaustion == Some(resource.reason(durable)));
        ensure!(attempt.accepted);
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
        // Keep the native input pending: the peer neither sends nor closes.
        let mut byte = [0];
        let closed = tokio::time::timeout(Duration::from_secs(10), peer.read(&mut byte)).await;
        ensure!(
            closed.is_ok(),
            "socket still open: signal={signal:?}, loaded={}, permit={}, finished={}",
            worker.is_loaded().await,
            worker.concurrent_agent_permit_is_held().await,
            request_is_finished(&invocation)
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
        if refresh.is_some() { join_request(&mut refresh, "stop refresh").await?; }
        ensure!(worker.unload_succeeded_for_test());
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(account.monthly_observer_count_for_test() == 0);
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(
            completed_setup(&stopped)? == connect_start,
            "interruption must preserve completed setup and NoHostStart"
        );
        ensure!(phase.state() == WaitState::Dropped, "native future completed instead of interruption: {phase:?}");
        ensure!(phase.window_closed(), "the source wait's exact window span must close after physical release");
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
            ensure!(!request_is_finished(&invocation));
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
                join_request(&mut invocation, "invocation result").await?
                    .is_err()
            );
        }
        let observations = source.observations();
        limits.run_batch_for_test().await;
        let settled = registry.applied_updates();
        check_accounting(
            &settled[baseline_updates..],
            resource,
            durable,
            initial.policy_revision + u64::from(resource == Resource::Storage),
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
            ?resource, ?mode, ?operation, ?phase, connections,
            "Monthly stop dropped native pending input and retired Worker before peer release; NoHostStart"
        );

        if durable {
            let mut grant = policy;
            if resource == Resource::Storage {
                grant.available_durable_storage_byte_seconds = u64::MAX;
            }
            registry.set_policy(grant.clone());
            start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
            join_request(&mut refresh, "policy refresh").await?;
            executor.resume(&id, false).await?;
            let (mut resumed_peer, _) =
                tokio::time::timeout(Duration::from_secs(10), listener.accept())
                    .await
                    .context("P2 reconnect during reconstruction")??;
            connections += 1;
            let reconstructed = pending_phase(&mut phases, operation, &key).await?;
            ensure!(reconstructed.runtime.resident_generation > initial.resident_generation);
            ensure!(reconstructed.runtime.start_attempt != initial.start_attempt);
            ensure!(reconstructed.runtime.fingerprint == initial.fingerprint);
            resumed_peer.write_all(b"x").await?;
            if matches!(operation, Operation::Splice) {
                let mut echoed = [0];
                resumed_peer.read_exact(&mut echoed).await?;
                ensure!(echoed == *b"x", "same-connection splice output");
            }
            let resumed = join_request(&mut invocation, "invocation result").await??;
            operation.assert_result(resumed)?;
            ensure!(reconstructed.state() == WaitState::Returned);
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(
                poll_starts(&recovered) == poll_starts(&stopped),
                "reconstruction cannot add a raw TCP Start"
            );
            ensure!(completed_setup(&recovered)? == connect_start);
            assert_settled_history(&recovered)?;
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            let cached = executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    operation.method(),
                    data_value!(port),
                )
                .await?;
            operation.assert_result(cached)?;
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
            let fresh_key = IdempotencyKey::fresh();
            let fresh = executor.invoke_and_await_agent_with_key(
                &component, &name, &fresh_key, operation.method(), data_value!(port),
            );
            let fresh_peer = async {
                let (mut peer, _) = listener.accept().await?;
                let fresh_phase = pending_phase(&mut phases, operation, &fresh_key).await?;
                peer.write_all(b"x").await?;
                let mut byte = [0];
                if matches!(operation, Operation::Splice) {
                    peer.read_exact(&mut byte).await?;
                    ensure!(byte == *b"x", "fresh splice must echo on the same connection");
                }
                ensure!(peer.read(&mut byte).await? == 0);
                ensure!(fresh_phase.state() == WaitState::Returned);
                Ok::<_, anyhow::Error>(())
            };
            let (fresh, _) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::try_join!(fresh, fresh_peer)
            }).await??;
            operation.assert_result(fresh)?;
            ensure!(tokio::time::timeout(Duration::from_millis(100), listener.accept()).await.is_err(), "exactly three native connections expected");
            let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (3, 3));
            assert_settled_history(&history)?;
            connections += 1;
            info!(
                ?resource, ?operation, %connect_start, connections,
                "Native input re-executed; cached lookup did not connect; fresh invocation succeeded"
            );
        } else if matches!(operation, Operation::Splice) {
            let mut grant = policy;
            if resource == Resource::Storage {
                grant.available_ephemeral_storage_byte_seconds = u64::MAX;
            }
            registry.set_policy(grant);
            start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
            join_request(&mut refresh, "policy refresh").await?;
            let retry = tokio::time::timeout(
                Duration::from_secs(10),
                executor.invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    operation.method(),
                    data_value!(port),
                ),
            )
            .await?;
            ensure!(retry.is_err(), "terminal ephemeral splice cannot resume");
            ensure!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err(),
                "terminal ephemeral splice must not reconnect"
            );
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            let probe_name = agent_id!("EphemeralNetworking", "monthly-p2-splice-probe");
            let probe_key = IdempotencyKey::fresh();
            let probe = executor.invoke_and_await_agent_with_key(
                &component,
                &probe_name,
                &probe_key,
                operation.method(),
                data_value!(port),
            );
            let probe_peer = async {
                let (mut peer, _) = listener.accept().await?;
                peer.write_all(b"x").await?;
                let mut echoed = [0];
                peer.read_exact(&mut echoed).await?;
                ensure!(echoed == *b"x", "fresh ephemeral splice output");
                ensure!(peer.read(&mut echoed).await? == 0);
                Ok::<_, anyhow::Error>(())
            };
            let (probe_result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::try_join!(probe, probe_peer)
            })
            .await??;
            operation.assert_result(probe_result)?;
            ensure!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err(),
                "terminal ephemeral key must not reconnect"
            );
            connections += 1;
            info!(?resource, ?operation, connections, "Fresh ephemeral splice probe succeeded after terminal failure");
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    // On a failed parked-wait assertion the peer is dropped before stopping the Worker.
    // The deadline is a fixture bound, never evidence that interruption succeeded.
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

fn assert_settled_history(entries: &[PublicOplogEntryWithIndex]) -> anyhow::Result<()> {
    let last_finished = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("missing invocation terminal")?
        .oplog_index;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(start) => {
                ensure!(
                    !start.function_name.contains("streams"),
                    "raw TCP has NoHostStart"
                );
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
