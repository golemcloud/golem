mod history;

use super::*;
use crate::api::monthly::support::{FUEL_BUDGET, Resource, check_accounting};
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::resource_usage_metering::{
    FilesystemUsage, ScriptedFilesystemUsageForTest,
};
use golem_worker_executor::services::{HasOplog, HasPromiseService, UsesAllDeps};
use golem_worker_executor::worker::{MonthlyClockForTest, RpcResultStateForTest, Worker};
use golem_worker_executor_test_utils::{RecordingRpc, start_with_resource_limits_and_overrides};
use test_r::test;

const METHOD: &str = "await_counter";
const STOP_BOUND: Duration = Duration::from_secs(8);

macro_rules! pending_case {
    ($name:ident, $resource:ident, $mode:ident, $asynchronous:expr) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            pending_rpc(
                last_unique_id,
                deps,
                fixture,
                Resource::$resource,
                AgentMode::$mode,
                $asynchronous,
                false,
            )
            .await
        }
    };
}

pending_case!(durable_sync_rpc_monthly_memory, Memory, Durable, false);
pending_case!(ephemeral_sync_rpc_monthly_memory, Memory, Ephemeral, false);
pending_case!(
    durable_sync_rpc_monthly_compute_prepaid,
    Compute,
    Durable,
    false
);
pending_case!(
    ephemeral_sync_rpc_monthly_compute_prepaid,
    Compute,
    Ephemeral,
    false
);
pending_case!(
    durable_sync_rpc_monthly_scripted_storage,
    Storage,
    Durable,
    false
);
pending_case!(
    ephemeral_sync_rpc_monthly_scripted_storage,
    Storage,
    Ephemeral,
    false
);

pending_case!(durable_async_rpc_monthly_memory, Memory, Durable, true);
pending_case!(ephemeral_async_rpc_monthly_memory, Memory, Ephemeral, true);
pending_case!(
    durable_async_rpc_monthly_compute_prepaid,
    Compute,
    Durable,
    true
);
pending_case!(
    ephemeral_async_rpc_monthly_compute_prepaid,
    Compute,
    Ephemeral,
    true
);
pending_case!(
    durable_async_rpc_monthly_scripted_storage,
    Storage,
    Durable,
    true
);
pending_case!(
    ephemeral_async_rpc_monthly_scripted_storage,
    Storage,
    Ephemeral,
    true
);

#[test]
#[timeout("2m")]
async fn durable_sync_rpc_result_before_monthly_stop_and_completed_history(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    pending_rpc(
        last_unique_id,
        deps,
        fixture,
        Resource::Memory,
        AgentMode::Durable,
        false,
        true,
    )
    .await
}

async fn pending_rpc(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    resource: Resource,
    mode: AgentMode,
    asynchronous: bool,
    completion_first: bool,
) -> anyhow::Result<()> {
    let mut policy = resource.policy();
    // Both the caller and its durable target need a real prepaid reservation.
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
    let dispatches = Arc::new(Mutex::new(Vec::new()));
    let executor = start_with_resource_limits_and_overrides(
        deps,
        &context,
        limits.clone(),
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.resource_usage_metering = metering;
                config.limits.fuel_to_borrow = FUEL_BUDGET;
                // Neither caller RPC nor target promise may voluntarily suspend during the test.
                config.suspend.rpc_suspend_after = Duration::from_secs(300);
                config.suspend.wait_suspend_grace = Duration::from_secs(300);
            })),
            wrap_rpc: Some(Arc::new({
                let dispatches = dispatches.clone();
                move |rpc| {
                    Arc::new(RecordingRpc::new(
                        rpc,
                        "inc_after_promise",
                        dispatches.clone(),
                    ))
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let target_name = "monthly-rpc-target";
    let target_name_id = agent_id!("RpcBlockingCounter", target_name);
    let target_id = executor
        .start_agent(&component.id, target_name_id.clone())
        .await?;
    let target = executor
        .production_active_agent(&OwnedAgentId::new(
            context.default_environment_id,
            &target_id,
        ))
        .await
        .unwrap()
        .primary();
    let promise = executor
        .invoke_and_await_agent(&component, &target_name_id, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while target.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("target setup permit release")?;
    let zero_usage = ScriptedFilesystemUsageForTest::new(FilesystemUsage::Authoritative {
        allocated_bytes: 0,
        filesystem_objects: 0,
    });
    if resource == Resource::Storage {
        target
            .owner_runtime_resources()
            .set_scripted_filesystem_usage_for_test(zero_usage);
    }
    // Keep the target's independent local tick still; applied account updates remain real.
    let (target_clock, _target_polls) = MonthlyClockForTest::new();
    target.set_monthly_clock_for_test(target_clock);
    if resource == Resource::Compute {
        limits.run_batch_for_test().await;
    }
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("CancelTester", "monthly-rpc-caller")
    } else {
        agent_id!("EphemeralRpcCaller", "monthly-rpc-caller")
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
        target.all(),
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
    let mut pending_results = worker.observe_rpc_result_for_test();
    let mut invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let name = name.clone();
        let key = key.clone();
        let promise = promise.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    METHOD,
                    data_value!(target_name, promise, asynchronous),
                )
                .await
        }
    });
    let result = async {
        let (original, target_wait) = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                executor.commit_oplog(&id).await?;
                executor.commit_oplog(&target_id).await?;
                let caller_entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                let target_entries = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
                if let Some(call) = history::caller(&caller_entries, &key, &target_id, asynchronous, false, 0)?
                    && let Some(wait) = history::target(&target_entries, &call.target_key, false)? {
                    ensure!(count_agent_invocation_pair_since(&caller_entries, OplogIndex::INITIAL) == (2, 1));
                    ensure!(count_agent_invocation_pair_since(&target_entries, OplogIndex::INITIAL) == (3, 2));
                    info!(?call, ?wait, ?caller_entries, ?target_entries, "Committed caller RPC Start and target accepted invocation with open promise result");
                    return Ok::<_, anyhow::Error>((call, wait));
                }
                ensure!(!invocation.is_finished(), "caller returned before target accepted pending result: {caller_entries:?} {target_entries:?}");
                tokio::task::yield_now().await;
            }
        }).await.context("committed accepted pending RPC")??;
        tokio::time::timeout(Duration::from_secs(5), polls.recv()).await?.context("monthly timer closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        let pending_result = if asynchronous {
            let pending = tokio::time::timeout(Duration::from_secs(5), pending_results.recv()).await?.context("future-invoke-result.get Pending")?;
            ensure!(pending.start == original.start && pending.runtime == initial);
            ensure!(pending.state() == RpcResultStateForTest::Pending);
            Some(pending)
        } else { None };
        let account = limits.initialize_account(context.account_id).await?;
        let acquisitions = worker.permit_acquisitions_for_test();
        ensure!(initial.exhaustion.is_none());
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await && !invocation.is_finished());
        ensure!(acquisitions > 0);
        let promises = worker.all().promise_service();
        let pending = promises.poll(promise.clone()).await?;
        ensure!(!pending.is_ready().await && pending.get().await.is_none());
        {
            let dispatches = dispatches.lock().unwrap();
            ensure!(dispatches.as_slice() == [Some(original.target_key.clone())], "{dispatches:?}");
        }
        let release_result = if completion_first {
            let (selected, release) = worker.pause_next_outcome_for_test(true);
            ensure!(promises.complete(promise.clone(), vec![]).await?);
            tokio::time::timeout(Duration::from_secs(10), selected).await??;
            executor.commit_oplog(&id).await?;
            executor.commit_oplog(&target_id).await?;
            ensure!(history::caller(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, &key, &target_id, asynchronous, true, 0)? == Some(original.clone()));
            ensure!(history::target(&executor.get_oplog(&target_id, OplogIndex::INITIAL).await?, &original.target_key, true)? == Some(target_wait));
            Some(release)
        } else { None };
        let mut raw = worker.raw_interrupt_for_test();
        let stop_started = Instant::now();
        tokio::time::timeout(STOP_BOUND, async {
            if resource == Resource::Compute {
                ensure!(initial.fuel_generation.is_some() && initial.fuel_generation == account.settled_fuel_generation_for_test());
                ensure!(account.monthly_capacity_is_exhausted_for_test(mode), "two real Stores must prepay the remaining account fuel");
                clock.advance(Duration::from_secs(30));
                while polls.recv().await.context("negative tick")?.deadline != Duration::from_secs(60) {}
                worker.drain_lifecycle_for_test().await?;
                ensure!(attempts.try_recv().is_err());
                ensure!(worker.current_monthly_proposal_for_test() == initial);
                ensure!(worker.concurrent_agent_permit_is_held().await && !invocation.is_finished());
            }
            let refresh = if resource == Resource::Storage {
                source.set(FilesystemUsage::Authoritative { allocated_bytes: 101, filesystem_objects: 1 });
                clock.advance(Duration::from_secs(30));
                None
            } else {
                let mut exhausted = policy.clone();
                if resource == Resource::Memory { exhausted.available_memory_gb_seconds = 0; } else { exhausted.available_fuel = 0; }
                registry.set_policy(exhausted);
                Some(applied_refresh(&limits, &registry, context.account_id).await?)
            };
            let mut storage_now = Duration::from_secs(30);
            let attempt = loop {
                tokio::select! {
                    biased;
                    attempt = attempts.recv() => break attempt.context("monthly acceptance closed")?,
                    poll = polls.recv(), if resource == Resource::Storage => {
                        let poll = poll.context("storage timer closed")?;
                        if poll.now != storage_now || poll.deadline <= storage_now { continue; }
                        worker.drain_lifecycle_for_test().await?;
                        tokio::select! {
                            biased;
                            attempt = attempts.recv() => break attempt.context("monthly acceptance closed")?,
                            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                        }
                        storage_now += Duration::from_secs(30);
                        clock.advance(Duration::from_secs(30));
                    }
                }
            };
            ensure!(attempt.proposal.policy_revision == initial.policy_revision + u64::from(resource != Resource::Storage));
            ensure!(attempt.proposal.period == initial.period && attempt.proposal.fingerprint == initial.fingerprint);
            ensure!(attempt.proposal.window_identity == initial.window_identity && attempt.proposal.resident_generation == initial.resident_generation);
            ensure!(attempt.proposal.start_attempt == initial.start_attempt && attempt.proposal.fuel_generation == initial.fuel_generation);
            ensure!(attempt.proposal.exhaustion == Some(resource.reason(durable)));
            ensure!(attempt.completed.await?);
            if let Some(release) = release_result {
                // The RPC End and target Finished preceded exhaustion. This checks result
                // survival, not the relative publication order of signal and notification.
                release.send(false).unwrap();
            }
            let signal = raw.recv().await?;
            ensure!(matches!(signal, InterruptKind::Suspend(_)));
            ensure!(worker.monthly_stop_for_test() == Some(signal) && worker.frozen_stop_for_test() == Some(signal));
            ensure!(worker.owner_stop_for_test().await == Some(signal));
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
            worker.join_accepted_stops_for_test().await?;
            worker.retained_cleanup_for_test().await?;
            if let Some(refresh) = refresh { refresh.await?; }
            ensure!(worker.unload_succeeded_for_test());
            ensure!(worker.permit_acquisitions_for_test() == acquisitions && clock.active_sleeps() == 0);
            if !completion_first { ensure!(!pending.is_ready().await && pending.get().await.is_none()); }
            if resource == Resource::Compute { ensure!(account.settled_fuel_generation_for_test() > initial.fuel_generation); }
            if resource == Resource::Storage {
                ensure!(!account.monthly_capacity_is_exhausted_for_test(if durable { AgentMode::Ephemeral } else { AgentMode::Durable }));
            }
            info!(?resource, ?mode, asynchronous, completion_first, ?signal, proposal = ?attempt.proposal, stop_elapsed = ?stop_started.elapsed(), "RPC monthly stop physically retired caller and released its permit");
            Ok::<_, anyhow::Error>(())
        }).await.context("RPC monthly stop exceeded 8s physical cleanup bound")??;
        if completion_first {
            ensure!(tokio::time::timeout(Duration::from_secs(5), &mut invocation).await???.into_typed::<u64>()? == 7);
        }
        if let Some(pending) = &pending_result {
            ensure!(pending.state() == RpcResultStateForTest::Dropped, "monthly interruption must drop the pending get, not invent a result");
        }
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(history::caller(&stopped, &key, &target_id, asynchronous, completion_first, usize::from(completion_first))? == Some(original.clone()));
        let entries = worker.oplog().read_exact(OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64()).await;
        let mut errors = Vec::new();
        let mut suspends = 0;
        for entry in entries.values() {
            match entry {
                OplogEntry::Error { error, kind, entity_parent_start_index, .. } => {
                    ensure!(*kind == OplogErrorKind::Invocation && entity_parent_start_index.is_none());
                    errors.push(error);
                }
                OplogEntry::Suspend { .. } => suspends += 1,
                OplogEntry::Interrupted { .. } | OplogEntry::Exited { .. } => bail!("wrong lifecycle terminal: {entry:?}"),
                _ => {}
            }
        }
        if durable {
            ensure!(errors.is_empty());
            if !completion_first { ensure!(suspends == 1 && !invocation.is_finished()); }
        } else {
            ensure!(suspends == 0);
            match resource {
                Resource::Compute => ensure!(matches!(errors.as_slice(), [AgentError::EphemeralFuelExhausted(_)]), "{errors:?}"),
                _ => ensure!(matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)] if reason.reason == resource.reason(false)), "{errors:?}"),
            }
            ensure!(tokio::time::timeout(Duration::from_secs(5), &mut invocation).await??.is_err());
        }
        ensure!(dispatches.lock().unwrap().as_slice() == [Some(original.target_key.clone())]);
        if !completion_first {
            executor.commit_oplog(&target_id).await?;
            ensure!(history::target(&executor.get_oplog(&target_id, OplogIndex::INITIAL).await?, &original.target_key, false)? == Some(target_wait));
        }
        let observations = source.observations();
        tokio::time::sleep(Duration::from_millis(100)).await;
        ensure!(worker.permit_acquisitions_for_test() == acquisitions && clock.active_sleeps() == 0);
        ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
        ensure!(source.observations() == observations);
        if resource == Resource::Compute {
            tokio::time::timeout(Duration::from_secs(5), async {
                while target.is_loaded().await || target.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
                target.join_accepted_stops_for_test().await?;
                target.retained_cleanup_for_test().await
            }).await.context("target compute window closure before account-wide refund assertion")??;
        }
        let mut grant = policy;
        if resource == Resource::Storage {
            grant.available_durable_storage_byte_seconds = u64::MAX;
            grant.available_ephemeral_storage_byte_seconds = u64::MAX;
        }
        registry.set_policy(grant);
        // Applied policy is observable before filesystem-limit callbacks finish. A live
        // target can retain that callback until its pending promise returns.
        let grant_refresh = applied_refresh(&limits, &registry, context.account_id).await?;
        info!(updates = ?registry.applied_updates(), "RPC account settlement");
        let updates = registry.applied_updates();
        if resource == Resource::Compute {
            let fuel: i128 = updates.iter().map(|update| i128::from(update.fuel_delta)).sum();
            ensure!(fuel > 0 && fuel < i128::from(2 * FUEL_BUDGET), "two-Store prepaid account must return unused fuel: {updates:?}");
            ensure!(updates.iter().any(|update| update.fuel_delta < 0));
            ensure!(updates.iter().all(|update| update.memory_gb_seconds_delta == 0 && update.memory_byte_nanoseconds_remainder == 0
                && update.durable_storage_byte_seconds_delta == 0 && update.durable_storage_byte_nanoseconds_remainder == 0
                && update.ephemeral_storage_byte_seconds_delta == 0 && update.ephemeral_storage_byte_nanoseconds_remainder == 0));
        } else {
            check_accounting(&updates, resource, durable, initial.policy_revision, initial.period)?;
        }
        if !completion_first { ensure!(promises.complete(promise.clone(), vec![]).await?); }
        // A target continuation and a caller continuation are independent. Completion of
        // the target promise wakes the target, while caller demand repairs its RPC Start.
        let target_value = tokio::time::timeout(Duration::from_secs(15), executor.invoke_and_await_agent(&component, &target_name_id, "get_value", data_value!())).await??.into_typed::<u64>()?;
        ensure!(target_value == 7, "exactly one target logical increment, independent of caller return");
        tokio::time::timeout(Duration::from_secs(5), grant_refresh).await.context("join applied grant filesystem callbacks after target completion")??;
        let target_entries = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
        ensure!(history::target(&target_entries, &original.target_key, true)? == Some(target_wait));
        history::settled(&target_entries)?;
        if durable {
            if !completion_first {
                let resumed = tokio::time::timeout(Duration::from_secs(15), executor.invoke_and_await_agent_with_key(&component, &name, &key, METHOD, data_value!(target_name, promise.clone(), asynchronous))).await??;
                ensure!(resumed.into_typed::<u64>()? == 7);
                ensure!(tokio::time::timeout(Duration::from_secs(5), &mut invocation).await???.into_typed::<u64>()? == 7);
                ensure!(worker.resident_generation_for_test() > initial.resident_generation);
            }
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(history::caller(&recovered, &key, &target_id, asynchronous, true, 1)? == Some(original.clone()));
            history::settled(&recovered)?;
            let attempts_before = dispatches.lock().unwrap().clone();
            ensure!(attempts_before.iter().all(|key| key.as_ref() == Some(&original.target_key)));
            ensure!(attempts_before.len() == if completion_first { 1 } else { 2 }, "{attempts_before:?}");
            tokio::time::timeout(Duration::from_secs(5), async {
                while worker.is_loaded().await && !worker.stop_if_idle().await { tokio::task::yield_now().await; }
                while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
                worker.join_accepted_stops_for_test().await?;
                worker.retained_cleanup_for_test().await
            }).await.context("idle unload before completed RPC replay")??;
            let generation = worker.resident_generation_for_test();
            let fresh_key = IdempotencyKey::fresh();
            let fresh = tokio::time::timeout(Duration::from_secs(15), executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key, METHOD, data_value!(target_name, promise.clone(), asynchronous))).await??;
            ensure!(fresh.into_typed::<u64>()? == 14);
            ensure!(worker.resident_generation_for_test() > generation);
            let after = dispatches.lock().unwrap().clone();
            ensure!(after.len() == attempts_before.len() + 1 && after[..attempts_before.len()] == attempts_before);
            let fresh_target_key = after.last().unwrap().as_ref().context("fresh RPC key")?;
            ensure!(fresh_target_key != &original.target_key);
            let replayed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            history::invocation(&replayed, &key, METHOD, 1)?;
            history::invocation(&replayed, &fresh_key, METHOD, 1)?;
            history::terminal(&replayed, original.start, true, usize::from(asynchronous))?;
            history::settled(&replayed)?;
            let value = executor.invoke_and_await_agent(&component, &target_name_id, "get_value", data_value!()).await?.into_typed::<u64>()?;
            ensure!(value == 14, "completed history must not repeat its target increment");
            let target_entries = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
            history::invocation(&target_entries, &original.target_key, "inc_after_promise", 1)?;
            history::invocation(&target_entries, fresh_target_key, "inc_after_promise", 1)?;
            history::settled(&target_entries)?;
            info!(?original, ?attempts_before, ?after, value, "Caller repaired one RPC Start; completed reconstruction dispatched only the fresh logical increment");
        } else {
            ensure!(tokio::time::timeout(Duration::from_secs(5), executor.invoke_and_await_agent_with_key(&component, &name, &key, METHOD, data_value!(target_name, promise.clone(), asynchronous))).await?.is_err());
            let failed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(history::caller(&failed, &key, &target_id, asynchronous, false, 0)? == Some(original.clone()));
            ensure!(failed.iter().filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))).count() == 1);
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
            ensure!(dispatches.lock().unwrap().as_slice() == [Some(original.target_key)]);
            executor.invoke_and_await_agent(&component, &target_name_id, "inc_by", data_value!(1u64)).await?;
            ensure!(executor.invoke_and_await_agent(&component, &target_name_id, "get_value", data_value!()).await?.into_typed::<u64>()? == 8);
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    if let Err(error) = &result {
        eprintln!(
            "RPC case failed before test cleanup: resource={resource:?}, mode={mode:?}, asynchronous={asynchronous}, completion_first={completion_first}: {error:#}"
        );
        for (id, worker) in [(&id, &worker), (&target_id, &target)] {
            let _ = tokio::time::timeout(Duration::from_secs(5), executor.interrupt(id)).await;
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                worker.join_accepted_stops_for_test(),
            )
            .await;
            let _ =
                tokio::time::timeout(Duration::from_secs(5), worker.retained_cleanup_for_test())
                    .await;
        }
        if !invocation.is_finished()
            && tokio::time::timeout(Duration::from_secs(5), &mut invocation)
                .await
                .is_err()
        {
            // Only the test response waiter, never the Worker or its cleanup.
            invocation.abort();
            let _ = invocation.await;
        }
    }
    result
}
