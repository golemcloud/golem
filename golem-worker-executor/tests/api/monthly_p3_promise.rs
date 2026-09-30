mod completion;
mod history;

use super::*;
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry, OplogErrorKind};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::resource_usage_metering::{
    FilesystemUsage, ScriptedFilesystemUsageForTest,
};
use golem_worker_executor::services::{HasOplog, HasPromiseService, UsesAllDeps};
use golem_worker_executor::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use golem_worker_executor::worker::{MonthlyClockForTest, Worker};
use golem_worker_executor_test_utils::start_with_resource_limits_and_overrides;

use super::monthly_pending::{FUEL_BUDGET, Resource, check_accounting};

use test_r::test;

const STOP_BOUND: Duration = Duration::from_secs(8);
const PAYLOAD: &[u8] = &[42, 0, 255, 7];
const METHOD: &str = "create_and_await_promise";

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
            pending_promise(
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

pending_case!(durable_p3_promise_monthly_memory, Memory, Durable);
pending_case!(ephemeral_p3_promise_monthly_memory, Memory, Ephemeral);
pending_case!(durable_p3_promise_monthly_compute_prepaid, Compute, Durable);
pending_case!(
    ephemeral_p3_promise_monthly_compute_prepaid,
    Compute,
    Ephemeral
);
pending_case!(
    durable_p3_promise_monthly_scripted_storage,
    Storage,
    Durable
);
pending_case!(
    ephemeral_p3_promise_monthly_scripted_storage,
    Storage,
    Ephemeral
);

pub(super) async fn pending_promise(
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
    let storage_calls = KeyValueStorageFaults::default();
    let executor = start_with_resource_limits_and_overrides(
        deps,
        &context,
        limits.clone(),
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.resource_usage_metering = metering;
                config.limits.fuel_to_borrow = FUEL_BUDGET;
                // Keep voluntary suspension outside the bounded quota-stop phase.
                config.suspend.wait_suspend_grace = Duration::from_secs(300);
            })),
            wrap_key_value_storage: Some(Arc::new({
                let storage_calls = storage_calls.clone();
                move |inner| {
                    Arc::new(FaultInjectingKeyValueStorage::new(
                        inner,
                        storage_calls.clone(),
                    ))
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(&component.id, agent_id!("GolemHostApi", "promise-seed"))
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
    if resource == Resource::Compute {
        limits.run_batch_for_test().await;
    }
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("GolemHostApi", "monthly-promise")
    } else {
        agent_id!("EphemeralPromise", "monthly-promise")
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
    let mut invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let name = name.clone();
        let key = key.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(&component, &name, &key, METHOD, data_value!())
                .await
        }
    });
    let result = async {
        let original = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                executor.commit_oplog(&id).await?;
                let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                if let Some(record) = history::promise_record(&entries, &key, &id, false, 0)? {
                    ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
                    info!(?entries, ?record, "Committed guest promise creation and open get-result Start under one unfinished method");
                    return Ok::<_, anyhow::Error>(record);
                }
                ensure!(!invocation.is_finished(), "guest returned before open promise wait: {entries:?}");
                tokio::task::yield_now().await;
            }
        }).await.context("committed open promise Start")??;
        tokio::time::timeout(Duration::from_secs(5), polls.recv()).await?.context("monthly timer closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(initial.exhaustion.is_none(), "{initial:?}");
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(!invocation.is_finished());
        let promises = worker.all().promise_service();
        let pending = promises.poll(original.promise.clone()).await?;
        ensure!(!pending.is_ready().await && pending.get().await.is_none());
        if !durable {
            ensure!(physical != name && original.promise.agent_id == id);
        }
        let stop_started = std::time::Instant::now();
        // No promise completion is supplied until the accepted quota stop physically retires.
        tokio::time::timeout(STOP_BOUND, async {
            if resource == Resource::Compute {
                ensure!(initial.fuel_generation.is_some());
                ensure!(initial.fuel_generation == account.settled_fuel_generation_for_test());
                ensure!(account.monthly_capacity_is_exhausted_for_test(mode), "Store must prepay the remaining fuel");
                clock.advance(Duration::from_secs(30));
                while polls.recv().await.context("negative tick timer closed")?.deadline != Duration::from_secs(60) {}
                worker.drain_lifecycle_for_test().await?;
                ensure!(attempts.try_recv().is_err(), "zero unreserved fuel cannot exhaust prepaid fuel");
                ensure!(worker.current_monthly_proposal_for_test() == initial);
                ensure!(!invocation.is_finished() && worker.concurrent_agent_permit_is_held().await);
            }
            let refresh = if resource == Resource::Storage {
                // Script authoritative allocation only, not deletion or window settlement.
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
            let attempt = loop {
                tokio::select! {
                    biased;
                    attempt = attempts.recv() => break attempt.context("acceptance observer closed")?,
                    poll = polls.recv(), if resource == Resource::Storage => {
                        let poll = poll.context("storage timer closed")?;
                        if poll.now != storage_now || poll.deadline <= storage_now { continue; }
                        worker.drain_lifecycle_for_test().await?;
                        tokio::select! {
                            biased;
                            attempt = attempts.recv() => break attempt.context("acceptance observer closed")?,
                            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                        }
                        storage_now += Duration::from_secs(30);
                        clock.advance(Duration::from_secs(30));
                    }
                }
            };
            ensure!(attempt.proposal.policy_revision == initial.policy_revision + u64::from(resource != Resource::Storage));
            ensure!(attempt.proposal.period == initial.period);
            ensure!(attempt.proposal.window_identity == initial.window_identity);
            ensure!(attempt.proposal.resident_generation == initial.resident_generation);
            ensure!(attempt.proposal.start_attempt == initial.start_attempt);
            ensure!(attempt.proposal.fingerprint == initial.fingerprint);
            ensure!(attempt.proposal.fuel_generation == initial.fuel_generation);
            ensure!(attempt.proposal.exhaustion == Some(resource.reason(durable)));
            ensure!(attempt.completed.await?);
            let signal = raw.recv().await?;
            ensure!(matches!(signal, InterruptKind::Suspend(_)));
            ensure!(worker.monthly_stop_for_test() == Some(signal));
            ensure!(worker.frozen_stop_for_test() == Some(signal));
            ensure!(worker.owner_stop_for_test().await == Some(signal));
            if resource == Resource::Compute {
                ensure!(account.settled_fuel_generation_for_test() > initial.fuel_generation);
            }
            if resource == Resource::Storage {
                let other = if durable { AgentMode::Ephemeral } else { AgentMode::Durable };
                ensure!(!account.monthly_capacity_is_exhausted_for_test(other));
            }
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
            worker.join_accepted_stops_for_test().await?;
            worker.retained_cleanup_for_test().await?;
            if let Some(refresh) = refresh { refresh.await?; }
            ensure!(worker.unload_succeeded_for_test());
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            ensure!(clock.active_sleeps() == 0);
            ensure!(account.monthly_observer_count_for_test() == 0);
            ensure!(!pending.is_ready().await && pending.get().await.is_none());
            info!(?resource, ?mode, ?signal, proposal = ?attempt.proposal,
                stop_elapsed = ?stop_started.elapsed(),
                "Typed quota stop physically unloaded promise Worker while service result stayed pending");
            Ok::<_, anyhow::Error>(())
        }).await.context("quota stop and physical settlement exceeded 8s bound")??;
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(history::promise_record(&stopped, &key, &id, false, 0)? == Some(original.clone()));
        ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
        let entries = worker.oplog().read_exact(OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64()).await;
        for entry in entries.values() {
            match entry {
                OplogEntry::Error { kind, entity_parent_start_index, .. } => ensure!(*kind == OplogErrorKind::Invocation && entity_parent_start_index.is_none()),
                OplogEntry::Interrupted { .. } | OplogEntry::Exited { .. } => anyhow::bail!("unexpected lifecycle terminal: {entry:?}"),
                _ => {}
            }
        }
        let errors: Vec<_> = entries.values().filter_map(|entry| match entry { OplogEntry::Error { error, .. } => Some(error), _ => None }).collect();
        let suspends = entries.values().filter(|entry| matches!(entry, OplogEntry::Suspend { .. })).count();
        if durable {
            ensure!(errors.is_empty() && suspends == 1, "{errors:?}, {suspends} suspends");
            ensure!(!invocation.is_finished());
        } else {
            ensure!(suspends == 0);
            match resource {
                Resource::Compute => ensure!(matches!(errors.as_slice(), [AgentError::EphemeralFuelExhausted(_)]), "{errors:?}"),
                _ => ensure!(matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)] if reason.reason == resource.reason(false)), "{errors:?}"),
            }
            ensure!(tokio::time::timeout(Duration::from_secs(5), &mut invocation).await??.is_err());
        }
        executor.wait_for_status(&id, if durable { AgentStatus::Suspended } else { AgentStatus::Failed }, Duration::from_secs(5)).await?;
        let observations = source.observations();
        limits.run_batch_for_test().await;
        let settled = registry.applied_updates();
        check_accounting(&settled, resource, durable, initial.policy_revision, initial.period)?;
        let settled_count = settled.len();
        info!(?resource, ?mode, ?settled, "Stopped promise window accounting settled");
        tokio::time::sleep(Duration::from_millis(150)).await;
        ensure!(!pending.is_ready().await && pending.get().await.is_none());
        if durable { ensure!(!invocation.is_finished()); }
        ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
        ensure!(clock.active_sleeps() == 0 && account.monthly_observer_count_for_test() == 0);
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(source.observations() == observations, "closed generation cannot keep sampling");
        limits.run_batch_for_test().await;
        for update in &registry.applied_updates()[settled_count..] {
            ensure!(update.fuel_delta == 0 && update.memory_gb_seconds_delta == 0 && update.memory_byte_nanoseconds_remainder == 0
                && update.durable_storage_byte_seconds_delta == 0 && update.durable_storage_byte_nanoseconds_remainder == 0
                && update.ephemeral_storage_byte_seconds_delta == 0 && update.ephemeral_storage_byte_nanoseconds_remainder == 0,
                "closed promise window accrued: {update:?}");
        }
        let retired_handle = pending.downgrade();
        drop(pending);
        ensure!(retired_handle.upgrade().is_none(), "unloaded guest must release its resident promise handle");
        let pending_reads = storage_calls.calls("get-completed");
        let pending = promises.poll(original.promise.clone()).await?;
        ensure!(!pending.is_ready().await && pending.get().await.is_none(), "re-hydrated service state must still be pending");
        ensure!(storage_calls.calls("get-completed") == pending_reads + 1, "cold promise poll must reach the observed dependency");
        let mut grant = policy;
        if resource == Resource::Storage {
            grant.available_durable_storage_byte_seconds = u64::MAX;
            grant.available_ephemeral_storage_byte_seconds = u64::MAX;
        }
        registry.set_policy(grant);
        monthly::applied_refresh(&limits, &registry, context.account_id).await?.await?;
        if durable {
            ensure!(!pending.is_ready().await);
            ensure!(promises.complete(original.promise.clone(), PAYLOAD.to_vec()).await?);
            ensure!(pending.is_ready().await && pending.get().await == Some(PAYLOAD.to_vec()));
            let resumed = tokio::time::timeout(Duration::from_secs(10), &mut invocation).await.context("promise completion must resume the original invocation after the grant")???;
            ensure!(resumed.into_typed::<Vec<u8>>()? == PAYLOAD);
            ensure!(worker.resident_generation_for_test() > initial.resident_generation);
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(history::promise_record(&recovered, &key, &id, true, 1)? == Some(original.clone()));
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            ensure!(recovered.iter().filter(|entry| matches!(entry.entry, PublicOplogEntry::Suspend(_))).count() == 1);
            history::settled(&recovered)?;
            ensure!(executor.invoke_and_await_agent_with_key(&component, &name, &key, METHOD, data_value!()).await?.into_typed::<Vec<u8>>()? == PAYLOAD);
            info!(?resource, ?original, ?recovered, "Original promise wait repaired with one End, delivery and invocation Finished");
            tokio::time::timeout(Duration::from_secs(5), async {
                while !worker.stop_if_idle().await { tokio::task::yield_now().await; }
                while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
                worker.join_accepted_stops_for_test().await?;
                worker.retained_cleanup_for_test().await
            }).await.context("idle eviction before completed promise replay")??;
            let completed_handle = pending.downgrade();
            drop(pending);
            ensure!(completed_handle.upgrade().is_none(), "completed replay must not hide service reads behind a retained hydrated handle");
            // The service registry holds only weak handles. Any repeated create, poll or
            // completion now reaches storage, even if it returns the same ID or payload.
            let labels = ["create", "get-completed", "complete"];
            let calls_before_replay = labels.map(|label| storage_calls.calls(label));
            ensure!(calls_before_replay.iter().all(|count| *count > 0), "promise dependency counters must observe live operations: {calls_before_replay:?}");
            let generation = worker.resident_generation_for_test();
            let fresh_key = IdempotencyKey::fresh();
            let healthy = tokio::time::timeout(Duration::from_secs(5), executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key, "get_retry_policy_count", data_value!())).await??;
            let _ = healthy.into_typed::<u64>()?;
            ensure!(worker.resident_generation_for_test() > generation);
            let replayed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(history::promise_record(&replayed, &key, &id, true, 1)? == Some(original.clone()));
            history::invocation(&replayed, &fresh_key, 1)?;
            ensure!(count_agent_invocation_pair_since(&replayed, OplogIndex::INITIAL) == (3, 3));
            history::settled(&replayed)?;
            let calls_after_replay = labels.map(|label| storage_calls.calls(label));
            ensure!(calls_after_replay == calls_before_replay, "completed promise replay touched storage: {labels:?}: {calls_before_replay:?} -> {calls_after_replay:?}");
            info!(?resource, ?labels, ?calls_before_replay, ?calls_after_replay, "Completed promise history reconstructed without create, result-read or completion storage calls");
        } else {
            ensure!(promises.complete(original.promise.clone(), PAYLOAD.to_vec()).await?);
            ensure!(pending.is_ready().await && pending.get().await == Some(PAYLOAD.to_vec()));
            ensure!(tokio::time::timeout(Duration::from_secs(5), executor.invoke_and_await_agent_with_key(&component, &name, &key, METHOD, data_value!())).await?.is_err());
            let failed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(history::promise_record(&failed, &key, &id, false, 0)? == Some(original.clone()));
            ensure!(count_agent_invocation_pair_since(&failed, OplogIndex::INITIAL) == (2, 1));
            ensure!(failed.iter().filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))).count() == 1);
            ensure!(!failed.iter().any(|entry| matches!(entry.entry, PublicOplogEntry::Suspend(_))));
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    if result.is_err() {
        let _ = tokio::time::timeout(Duration::from_secs(5), executor.interrupt(&id)).await;
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            worker.join_accepted_stops_for_test(),
        )
        .await;
        let _ =
            tokio::time::timeout(Duration::from_secs(5), worker.retained_cleanup_for_test()).await;
        if !invocation.is_finished()
            && tokio::time::timeout(Duration::from_secs(5), &mut invocation)
                .await
                .is_err()
        {
            // This is only the test's response waiter, not Worker cleanup or replay.
            invocation.abort();
            let _ = invocation.await;
        }
    }
    result
}
