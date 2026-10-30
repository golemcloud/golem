mod history;
// monthly_limits::tests proves dual prepaid stop registration before either refund.
// Caller-first cases hold the target's real GrowMemory append through caller cleanup.
// A target-first scenario checks capacity restoration after refund and RPC recovery.
mod refund_ordering;

use super::*;
use crate::api::interruption::support::{FUEL_BUDGET, Resource, check_accounting};
use anyhow::{Context as _, ensure};
use golem_common::model::Timestamp;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::{HasActiveAgents, HasOplog, HasPromiseService, UsesAllDeps};
use golem_worker_executor::worker::Worker;
use golem_worker_executor_test_utils::{RecordingRpc, start_with_resource_limits_and_overrides};
use test_r::test;

const METHOD: &str = "await_counter";
const STOP_BOUND: Duration = Duration::from_secs(8);

macro_rules! pending_case {
    ($name:ident, Storage, $mode:ident, $asynchronous:expr) => {
        pending_case!(#[cfg(target_os = "linux")] #[ignore = "requires the privileged managed XFS test runner"] $name, Storage, $mode, $asynchronous);
    };
    ($(#[$attr:meta])* $name:ident, $resource:ident, $mode:ident, $asynchronous:expr) => {
        $(#[$attr])*
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

pending_case!(durable_sync_rpc_memory_quota, Memory, Durable, false);
pending_case!(ephemeral_sync_rpc_memory_quota, Memory, Ephemeral, false);
pending_case!(
    durable_sync_rpc_compute_quota_prepaid,
    Compute,
    Durable,
    false
);
pending_case!(
    ephemeral_sync_rpc_compute_quota_prepaid,
    Compute,
    Ephemeral,
    false
);
pending_case!(
    durable_sync_rpc_managed_xfs_storage_quota,
    Storage,
    Durable,
    false
);
pending_case!(
    ephemeral_sync_rpc_managed_xfs_storage_quota,
    Storage,
    Ephemeral,
    false
);

pending_case!(durable_async_rpc_memory_quota, Memory, Durable, true);
pending_case!(ephemeral_async_rpc_memory_quota, Memory, Ephemeral, true);
pending_case!(
    ephemeral_async_rpc_compute_quota_prepaid,
    Compute,
    Ephemeral,
    true
);
pending_case!(
    durable_async_rpc_managed_xfs_storage_quota,
    Storage,
    Durable,
    true
);
pending_case!(
    ephemeral_async_rpc_managed_xfs_storage_quota,
    Storage,
    Ephemeral,
    true
);

#[test]
#[timeout("2m")]
async fn durable_sync_rpc_result_before_quota_stop_and_completed_history(
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
    pending_rpc_scenario(
        last_unique_id,
        deps,
        fixture,
        resource,
        mode,
        asynchronous,
        refund_ordering::Scenario::Quota { completion_first },
    )
    .await
}

async fn pending_rpc_scenario(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    resource: Resource,
    mode: AgentMode,
    asynchronous: bool,
    scenario: refund_ordering::Scenario,
) -> anyhow::Result<()> {
    let completion_first = match scenario {
        refund_ordering::Scenario::Quota { completion_first } => completion_first,
        _ => false,
    };
    let held_growth = !matches!(scenario, refund_ordering::Scenario::Quota { .. });
    let caller_user_stop = scenario == refund_ordering::Scenario::RefundBeforeCaller;
    let hold_target_acceptance = resource == Resource::Compute && !held_growth && !completion_first;
    if held_growth {
        ensure!(resource == Resource::Compute && mode == AgentMode::Durable);
        ensure!(asynchronous && !completion_first);
    }
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
    let filesystem = managed_filesystem_config(metering)?;
    let executor = start_with_resource_limits_and_overrides(
        deps,
        &context,
        limits.clone(),
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.resource_usage_metering = metering;
                if let Some(filesystem) = &filesystem {
                    config.filesystem_storage.mode = filesystem.clone();
                }
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
    let expected_target_id = AgentId::from_agent_id(component.id, &target_name_id).unwrap();
    let mut target_growth = if held_growth || hold_target_acceptance {
        Some(
            executor
                .gate_next_memory_growth_append(&expected_target_id)
                .await,
        )
    } else {
        None
    };
    let target_id = executor
        .start_agent(&component.id, target_name_id.clone())
        .await?;
    ensure!(target_id == expected_target_id);
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
    executor
        .wait_for_status(&target_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    let (_target_usage, mut target_flushes) = refund_ordering::observe_usage(&target);
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
    let mut caller_growth = if held_growth {
        Some(executor.gate_next_memory_growth_append(&id).await)
    } else {
        None
    };
    if held_growth {
        let started = executor
            .start_agent(&component.id, physical.clone())
            .await?;
        ensure!(started == id);
        wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
        executor
            .wait_for_status(&id, AgentStatus::Idle, Duration::from_secs(10))
            .await?;
    }
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
    let (_caller_usage, mut caller_flushes) = refund_ordering::observe_usage(&worker);
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
    let mut caller_waiter_joined = false;
    let mut refreshes = Vec::new();
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
        let initial = worker.owner_runtime_resources().with_monthly_capacity(mode, |capacity| capacity);
        if caller_user_stop {
            refund_ordering::usage_flushes(&mut caller_flushes, initial.policy_revision, 1).await?;
        }
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(initial.exhaustion.is_none());
        ensure!(worker.is_loaded().await && !invocation.is_finished());
        let promises = worker.all().promise_service();
        let pending = promises.poll(promise.clone()).await?;
        ensure!(!pending.is_ready().await && pending.get().await.is_none());
        {
            let dispatches = dispatches.lock().unwrap();
            ensure!(dispatches.as_slice() == [Some(original.target_key.clone())], "{dispatches:?}");
        }
        let mut growth_entries = None;
        if held_growth {
            let (caller_entry, target_entry) = tokio::time::timeout(Duration::from_secs(15), async {
                caller_growth.as_mut().unwrap().entered().await;
                target_growth.as_mut().unwrap().entered().await;
                Ok::<_, anyhow::Error>((
                    caller_growth.as_ref().unwrap().entry(),
                    target_growth.as_ref().unwrap().entry(),
                ))
            }).await.context("existing RPC Wasm did not produce both real GrowMemory predecessors")??;
            for entry in [&caller_entry, &target_entry] {
                ensure!(matches!(entry, OplogEntry::GrowMemory { delta, .. } if *delta > 0),
                    "not a real positive memory-growth append: {entry:?}");
            }
            refund_ordering::entry_count(&worker, &caller_entry, 0).await?;
            refund_ordering::entry_count(&target, &target_entry, 0).await?;
            growth_entries = Some((caller_entry.clone(), target_entry.clone()));
            // The recorded RPC Starts coexist with both intercepted appends.
            ensure!(worker.is_loaded().await && target.is_loaded().await);
            let target_initial = target.owner_runtime_resources().with_monthly_capacity(AgentMode::Durable, |capacity| capacity);
            if caller_user_stop {
                refund_ordering::usage_flushes(&mut target_flushes, target_initial.policy_revision, 1).await?;
            }
            ensure!(target_initial.exhaustion.is_none());
            ensure!(target_initial.fuel_generation == initial.fuel_generation);
            info!(?caller_entry, ?target_entry, ?initial, ?target_initial,
                "Both GrowMemory appends remain withheld while the original RPC is unfinished");
        }
        if scenario == refund_ordering::Scenario::CheckpointOnly {
            drop(caller_growth.take());
            drop(target_growth.take());
            let (caller_entry, target_entry) = growth_entries.as_ref().unwrap();
            refund_ordering::entry_count(&worker, caller_entry, 1).await?;
            refund_ordering::entry_count(&target, target_entry, 1).await?;
            ensure!(!pending.is_ready().await && pending.get().await.is_none());
            ensure!(!invocation.is_finished());
            return Ok::<_, anyhow::Error>(());
        }
        let release_result = if completion_first {
            let (selected, release) = executor.gate_next_invocation_outcome_append(&id).await;
            ensure!(promises.complete(promise.clone(), vec![]).await?);
            tokio::time::timeout(Duration::from_secs(10), selected).await??;
            executor.commit_oplog(&id).await?;
            executor.commit_oplog(&target_id).await?;
            ensure!(history::caller(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, &key, &target_id, asynchronous, true, 0)? == Some(original.clone()));
            ensure!(history::target(&executor.get_oplog(&target_id, OplogIndex::INITIAL).await?, &original.target_key, true)? == Some(target_wait));
            Some(release)
        } else { None };
        let mut raw = worker.raw_interrupt_for_test();
        if hold_target_acceptance {
            // Invocation exit refunds unused fuel before its result append. Hold the target's
            // existing real GrowMemory append instead, so its lifecycle actor cannot accept a
            // quota stop and refund that reservation before caller acceptance.
            let growth = target_growth.as_mut().unwrap();
            tokio::time::timeout(Duration::from_secs(15), growth.entered()).await?;
            ensure!(matches!(growth.entry(), OplogEntry::GrowMemory { delta, .. } if delta > 0));
            refund_ordering::entry_count(&target, &growth.entry(), 0).await?;
        }
        if resource == Resource::Memory {
            // Settle live byte-time under the healthy policy before publishing exhaustion.
            limits.run_batch_for_test().await;
        }
        let stop_started = Instant::now();
        tokio::time::timeout(STOP_BOUND, async {
            if resource == Resource::Compute {
                ensure!(initial.fuel_generation.is_some() && !account.prepaid_fuel_is_exhausted(mode, initial.fuel_generation));
                ensure!(account.monthly_capacity_is_exhausted_for_test(mode), "two real Stores must prepay the remaining account fuel");
                ensure!(!invocation.is_finished());
            }
            if resource == Resource::Storage {
                allocate_storage_with_billing_refresh(&owned, &limits, &registry, &policy, durable).await?;
            }
            let mut exhausted = policy.clone();
            match resource {
                Resource::Memory => exhausted.available_memory_gb_seconds = 0,
                Resource::Compute => exhausted.available_fuel = 0,
                Resource::Storage if durable => exhausted.available_durable_storage_byte_seconds = 0,
                Resource::Storage => exhausted.available_ephemeral_storage_byte_seconds = 0,
            }
            registry.set_policy(exhausted);
            let refresh = Some(if resource == Resource::Compute && !caller_user_stop {
                let refresh = tokio::spawn({
                    let limits = limits.clone();
                    async move { limits.run_batch_for_test().await }
                });
                while !refresh.is_finished() { tokio::task::yield_now().await; }
                refresh
            } else {
                applied_refresh(&limits, &registry, context.account_id).await?
            });
            if let Some(refresh) = refresh { refreshes.push(refresh); }
            if caller_user_stop {
                let expected_revision = initial.policy_revision + 1;
                // The real usage dependency observes the applied revision, not monitor ordering.
                refund_ordering::usage_flushes(&mut caller_flushes, expected_revision, 1).await?;
                refund_ordering::usage_flushes(&mut target_flushes, expected_revision, 1).await?;
                let mut target_raw = target.raw_interrupt_for_test();
                drop(target_growth.take());
                let signal = tokio::time::timeout(Duration::from_secs(5), target_raw.recv()).await??;
                ensure!(matches!(signal, InterruptKind::Suspend(_)));
                ensure!(target.selected_owner_failure_for_test().await == Some(ToolOwnerFailureMetadata::Lifecycle));
                refund_ordering::physical_cleanup(&target).await?;
                refund_ordering::compute_suspension(&target).await?;
                let target_stopped = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
                ensure!(count_agent_invocation_pair_since(&target_stopped, OplogIndex::INITIAL) == (3, 2));
                ensure!(history::target(&target_stopped, &original.target_key, false)? == Some(target_wait));
                let (caller_entry, target_entry) = growth_entries.as_ref().unwrap();
                refund_ordering::entry_count(&target, target_entry, 1).await?;
                refund_ordering::entry_count(&worker, caller_entry, 0).await?;
                limits.release_registry_reply().await;
                ensure!(!account.prepaid_fuel_is_exhausted(mode, initial.fuel_generation), "target refund must restore caller fuel capacity");
                ensure!(worker.is_loaded().await && !invocation.is_finished());
                ensure!(!pending.is_ready().await && pending.get().await.is_none());
                ensure!(worker.selected_owner_failure_for_test().await.is_none());
                ensure!(matches!(raw.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)));
                drop(caller_growth.take());
                refund_ordering::entry_count(&worker, caller_entry, 1).await?;
                ensure!(worker.selected_owner_failure_for_test().await.is_none());
                ensure!(matches!(raw.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)));
                ensure!(worker.is_loaded().await && !invocation.is_finished());
                let kind = InterruptKind::Interrupt(Timestamp::now_utc());
                let mut receipt = worker.set_interrupting(kind).await?.context("explicit caller Interrupt was not accepted")?;
                // The separate request has one fixed cause. It is not a compute stop.
                let observed = raw.recv().await?;
                ensure!(observed == kind);
                ensure!(worker.selected_owner_failure_for_test().await == Some(ToolOwnerFailureMetadata::Lifecycle));
                receipt.recv().await?;
            } else {
            if let Some(release) = release_result {
                // The RPC End and target Finished preceded exhaustion. This checks result
                // survival, not the relative publication order of signal and notification.
                while worker.selected_owner_failure_for_test().await.is_none() { tokio::task::yield_now().await; }
                release.send(false).unwrap();
            }
            let signal = raw.recv().await?;
            ensure!(matches!(signal, InterruptKind::Suspend(_)));
            ensure!(worker.selected_owner_failure_for_test().await == Some(ToolOwnerFailureMetadata::Lifecycle));
            };
            while worker.is_loaded().await { tokio::task::yield_now().await; }
            worker.join_accepted_stops_for_test().await?;
            worker.retained_cleanup_for_test().await?;
            if hold_target_acceptance {
                // Caller refund restores capacity before this target can validate a queued
                // proposal. Keep its still-pending invocation live, rather than require both
                // monitors to stop an account that is no longer exhausted.
                ensure!(!account.prepaid_fuel_is_exhausted(AgentMode::Durable, initial.fuel_generation));
                let target_entry = target_growth.as_ref().unwrap().entry();
                drop(target_growth.take());
                refund_ordering::entry_count(&target, &target_entry, 1).await?;
                ensure!(target.is_loaded().await);
            }
            // Full refresh also waits for the live target's filesystem-limit reply. Join it
            // after target completion, not as part of the caller's physical stop bound.
            if !completion_first { ensure!(!pending.is_ready().await && pending.get().await.is_none()); }
            if resource == Resource::Storage {
                ensure!(!account.monthly_capacity_is_exhausted_for_test(if durable { AgentMode::Ephemeral } else { AgentMode::Durable }));
            }
            info!(?resource, ?mode, asynchronous, completion_first, caller_user_stop, stop_elapsed = ?stop_started.elapsed(), "RPC stop retired caller");
            Ok::<_, anyhow::Error>(())
        }).await.context("RPC monthly stop exceeded 8s physical cleanup bound")??;
        if completion_first {
            ensure!(tokio::time::timeout(Duration::from_secs(5), &mut invocation).await???.into_typed::<u64>()? == 7);
        }
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(history::caller(&stopped, &key, &target_id, asynchronous, completion_first, usize::from(completion_first))? == Some(original.clone()));
        let entries = worker.oplog().read_exact(OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64()).await?;
        let mut errors = Vec::new();
        let mut suspends = 0;
        let mut interrupts = 0;
        for entry in entries.values() {
            match entry {
                OplogEntry::Error { error, kind, entity_parent_start_index, .. } => {
                    ensure!(*kind == OplogErrorKind::Invocation && entity_parent_start_index.is_none());
                    errors.push(error);
                }
                OplogEntry::Suspend { .. } => suspends += 1,
                OplogEntry::Interrupted { .. } if caller_user_stop => interrupts += 1,
                OplogEntry::Interrupted { .. } | OplogEntry::Exited { .. } => bail!("wrong lifecycle terminal: {entry:?}"),
                _ => {}
            }
        }
        if durable {
            ensure!(errors.is_empty());
            if caller_user_stop {
                ensure!(suspends == 0 && interrupts == 1);
                let interrupted = tokio::time::timeout(Duration::from_secs(5), &mut invocation).await??;
                caller_waiter_joined = true;
                ensure!(interrupted.is_err(),
                    "the explicitly interrupted caller waiter must report its Interrupt");
            } else if !completion_first { ensure!(suspends == 1 && !invocation.is_finished()); }
        } else {
            ensure!(suspends == 0);
            match resource {
                Resource::Compute => ensure!(matches!(errors.as_slice(), [AgentError::EphemeralFuelExhausted(_)]), "{errors:?}"),
                _ => ensure!(matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)] if reason.reason == resource.reason(false)), "{errors:?}"),
            }
            ensure!(tokio::time::timeout(Duration::from_secs(5), &mut invocation).await??.is_err());
        }
        let user_interrupt_index = if caller_user_stop {
            let interrupted: Vec<_> = stopped.iter().filter(|entry| matches!(entry.entry, PublicOplogEntry::Interrupted(_))).collect();
            ensure!(interrupted.len() == 1);
            Some(interrupted[0].oplog_index)
        } else {
            None
        };
        ensure!(dispatches.lock().unwrap().as_slice() == [Some(original.target_key.clone())]);
        if !completion_first {
            executor.commit_oplog(&target_id).await?;
            ensure!(history::target(&executor.get_oplog(&target_id, OplogIndex::INITIAL).await?, &original.target_key, false)? == Some(target_wait));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        ensure!(!worker.is_loaded().await);
        if resource == Resource::Storage { assert_storage_deleted(&owned)?; }
        if resource == Resource::Compute && !hold_target_acceptance {
            tokio::time::timeout(Duration::from_secs(5), async {
                while target.is_loaded().await { tokio::task::yield_now().await; }
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
        refreshes.push(applied_refresh(&limits, &registry, context.account_id).await?);
        info!(updates = ?registry.applied_updates(), "RPC account settlement");
        let updates = registry.applied_updates();
        if resource == Resource::Compute && !hold_target_acceptance {
            let fuel: i128 = updates.iter().map(|update| i128::from(update.fuel_delta)).sum();
            ensure!(fuel > 0 && fuel < i128::from(2 * FUEL_BUDGET), "two-Store prepaid account must return unused fuel: {updates:?}");
            ensure!(updates.iter().any(|update| update.fuel_delta < 0));
            ensure!(updates.iter().all(|update| update.memory_gb_seconds_delta == 0 && update.memory_byte_nanoseconds_remainder == 0
                && update.durable_storage_byte_seconds_delta == 0 && update.durable_storage_byte_nanoseconds_remainder == 0
                && update.ephemeral_storage_byte_seconds_delta == 0 && update.ephemeral_storage_byte_nanoseconds_remainder == 0));
        } else if resource != Resource::Compute {
            check_accounting(&updates, resource, durable, initial.policy_revision + u64::from(resource == Resource::Storage), initial.period)?;
        }
        if !completion_first { ensure!(promises.complete(promise.clone(), vec![]).await?); }
        // A target continuation and a caller continuation are independent. Completion of
        // the target promise wakes the target, while caller demand repairs its RPC Start.
        let target_value = tokio::time::timeout(Duration::from_secs(15), executor.invoke_and_await_agent(&component, &target_name_id, "get_value", data_value!())).await??.into_typed::<u64>()?;
        ensure!(target_value == 7, "exactly one target logical increment, independent of caller return");
        if hold_target_acceptance {
            // Preserve two-Store physical settlement and exact refund assertions. This target
            // completed normally after caller refund, so use its ordinary idle eviction.
            tokio::time::timeout(Duration::from_secs(5), async {
                // `stop_if_idle` also refuses for pending invocations, active entity bodies and a
                // stopping instance, so an eviction class does not prove the first call succeeds.
                while target.is_loaded().await && !target.stop_if_idle().await { tokio::task::yield_now().await; }
                while target.is_loaded().await { tokio::task::yield_now().await; }
                target.join_accepted_stops_for_test().await?;
                target.retained_cleanup_for_test().await?;
                Ok::<_, anyhow::Error>(())
            }).await.context("target compute window closure after normal completion")??;
            limits.run_batch_for_test().await;
            let updates = registry.applied_updates();
            let fuel: i128 = updates.iter().map(|update| i128::from(update.fuel_delta)).sum();
            ensure!(fuel > 0 && fuel < i128::from(2 * FUEL_BUDGET), "two-Store prepaid account must return unused fuel: {updates:?}");
            ensure!(updates.iter().any(|update| update.fuel_delta < 0));
            ensure!(updates.iter().all(|update| update.memory_gb_seconds_delta == 0 && update.memory_byte_nanoseconds_remainder == 0
                && update.durable_storage_byte_seconds_delta == 0 && update.durable_storage_byte_nanoseconds_remainder == 0
                && update.ephemeral_storage_byte_seconds_delta == 0 && update.ephemeral_storage_byte_nanoseconds_remainder == 0));
        }
        ensure!(!refreshes.is_empty(), "applied grant refresh missing");
        for refresh in refreshes.drain(..) {
            tokio::time::timeout(Duration::from_secs(5), refresh).await.context("join applied policy refresh after target completion")??;
        }
        let target_entries = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
        ensure!(history::target(&target_entries, &original.target_key, true)? == Some(target_wait));
        history::settled(&target_entries)?;
        if durable {
            if !completion_first {
                if caller_user_stop { executor.resume(&id, false).await?; }
                let resumed = tokio::time::timeout(Duration::from_secs(15), executor.invoke_and_await_agent_with_key(&component, &name, &key, METHOD, data_value!(target_name, promise.clone(), asynchronous))).await??;
                ensure!(resumed.into_typed::<u64>()? == 7);
                if !caller_user_stop { ensure!(tokio::time::timeout(Duration::from_secs(5), &mut invocation).await???.into_typed::<u64>()? == 7); }
            }
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(history::caller(&recovered, &key, &target_id, asynchronous, true, 1)? == Some(original.clone()));
            if let Some(index) = user_interrupt_index {
                history::settled_after_user_interrupt(&recovered, index)?;
            } else { history::settled(&recovered)?; }
            let attempts_before = dispatches.lock().unwrap().clone();
            ensure!(attempts_before.iter().all(|key| key.as_ref() == Some(&original.target_key)));
            ensure!(attempts_before.len() == if completion_first { 1 } else { 2 }, "{attempts_before:?}");
            tokio::time::timeout(Duration::from_secs(5), async {
                while worker.is_loaded().await && !worker.stop_if_idle().await { tokio::task::yield_now().await; }
                while worker.is_loaded().await { tokio::task::yield_now().await; }
                worker.join_accepted_stops_for_test().await?;
                worker.retained_cleanup_for_test().await
            }).await.context("idle unload before completed RPC replay")??;
            let fresh_key = IdempotencyKey::fresh();
            let fresh = tokio::time::timeout(Duration::from_secs(15), executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key, METHOD, data_value!(target_name, promise.clone(), asynchronous))).await??;
            ensure!(fresh.into_typed::<u64>()? == 14);
            let after = dispatches.lock().unwrap().clone();
            ensure!(after.len() == attempts_before.len() + 1 && after[..attempts_before.len()] == attempts_before);
            let fresh_target_key = after.last().unwrap().as_ref().context("fresh RPC key")?;
            ensure!(fresh_target_key != &original.target_key);
            let replayed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            history::invocation(&replayed, &key, METHOD, 1)?;
            history::invocation(&replayed, &fresh_key, METHOD, 1)?;
            history::terminal(&replayed, original.start, true, usize::from(asynchronous))?;
            if let Some(index) = user_interrupt_index {
                history::settled_after_user_interrupt(&replayed, index)?;
            } else { history::settled(&replayed)?; }
            let value = executor.invoke_and_await_agent(&component, &target_name_id, "get_value", data_value!()).await?.into_typed::<u64>()?;
            ensure!(value == 14, "completed history must not repeat its target increment");
            let target_entries = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
            history::invocation(&target_entries, &original.target_key, "inc_after_promise", 1)?;
            history::invocation(&target_entries, fresh_target_key, "inc_after_promise", 1)?;
            history::settled(&target_entries)?;
            info!(?original, ?attempts_before, ?after, value, "Caller repaired one RPC Start; completed reconstruction dispatched only the fresh logical increment");
        } else {
            tokio::time::timeout(Duration::from_secs(5), async {
                while worker.active_agents().try_get_active_agent(&owned).await.is_some() {
                    tokio::task::yield_now().await;
                }
            }).await.context("terminal ephemeral caller must leave the active-agent cache")?;
            ensure!(worker.selected_owner_failure_for_test().await == Some(ToolOwnerFailureMetadata::Lifecycle));
            ensure!(tokio::time::timeout(Duration::from_secs(5), executor.invoke_and_await_agent_with_key(&component, &name, &key, METHOD, data_value!(target_name, promise.clone(), asynchronous))).await?.is_err());
            let failed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(history::caller(&failed, &key, &target_id, asynchronous, false, 0)? == Some(original.clone()));
            ensure!(failed.iter().filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))).count() == 1);
            ensure!(!worker.is_loaded().await);
            ensure!(worker.selected_owner_failure_for_test().await == Some(ToolOwnerFailureMetadata::Lifecycle));
            ensure!(dispatches.lock().unwrap().as_slice() == [Some(original.target_key)]);
            executor.invoke_and_await_agent(&component, &target_name_id, "inc_by", data_value!(1u64)).await?;
            ensure!(executor.invoke_and_await_agent(&component, &target_name_id, "get_value", data_value!()).await?.into_typed::<u64>()? == 8);
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    drop(caller_growth.take());
    drop(target_growth.take());
    if scenario == refund_ordering::Scenario::CheckpointOnly
        || (caller_user_stop && result.is_err())
    {
        let mut cleanup_errors = Vec::new();
        if let Err(error) =
            refund_ordering::cleanup(&executor, [&worker, &target], [&id, &target_id]).await
        {
            cleanup_errors.push(format!("{error:#}"));
        }
        for mut refresh in refreshes {
            match tokio::time::timeout(Duration::from_secs(5), &mut refresh).await {
                Ok(Ok(())) => {}
                other => cleanup_errors.push(format!("applied policy refresh join: {other:?}")),
            }
        }
        if !caller_waiter_joined {
            match tokio::time::timeout(Duration::from_secs(5), &mut invocation).await {
                Ok(Ok(Err(_))) => {} // Ordinary cleanup interrupted this still-pending response waiter.
                Ok(Ok(Ok(_))) => cleanup_errors
                    .push("the pending RPC completed during stop-only cleanup".to_owned()),
                Ok(Err(error)) => {
                    cleanup_errors.push(format!("RPC response waiter panicked: {error}"))
                }
                Err(error) => {
                    // Only the response waiter, after both real Worker cleanup attempts above.
                    invocation.abort();
                    match tokio::time::timeout(Duration::from_secs(5), &mut invocation).await {
                        Ok(Err(join)) if join.is_cancelled() => {}
                        other => cleanup_errors
                            .push(format!("RPC response waiter abort join: {other:?}")),
                    }
                    cleanup_errors.push(format!("RPC response waiter join: {error}"));
                }
            }
        }
        return match (result, cleanup_errors.is_empty()) {
            (Ok(()), true) => Ok(()),
            (Err(error), true) => Err(error),
            (Ok(()), false) => Err(anyhow!("cleanup: {}", cleanup_errors.join("; "))),
            (Err(error), false) => {
                Err(anyhow!("{error:#}; cleanup: {}", cleanup_errors.join("; ")))
            }
        };
    }
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
