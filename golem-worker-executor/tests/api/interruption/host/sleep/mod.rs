mod history;

use super::*;
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry, OplogErrorKind};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::{HasOplog, UsesAllDeps};
use golem_worker_executor::worker::Worker;

use crate::api::interruption::support::{FUEL_BUDGET, Resource, check_accounting};

const GUEST_SLEEP: Duration = Duration::from_secs(20);
const STOP_BOUND: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Abi {
    P2,
    P3,
}

impl Abi {
    fn method(self) -> &'static str {
        match self {
            Self::P2 => "sleep",
            Self::P3 => "sleep_p3",
        }
    }

    fn check_result(self, result: golem_test_framework::dsl::AgentResult) -> anyhow::Result<()> {
        match self {
            Self::P2 => ensure!(result.into_typed::<Result<(), String>>()? == Ok(())),
            Self::P3 => ensure!(result.into_typed::<bool>()?),
        }
        Ok(())
    }
}

pub(super) async fn pending_sleep(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    resource: Resource,
    mode: AgentMode,
    abi: Abi,
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
    let filesystem = managed_filesystem_config(metering)?;
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| {
            config.resource_usage_metering = metering;
            if let Some(filesystem) = &filesystem {
                config.filesystem_storage.mode = filesystem.clone();
            }
            config.limits.fuel_to_borrow = FUEL_BUDGET;
            // Isolate quota interruption from voluntary scheduling, without changing the guest clock.
            config.suspend.wait_suspend_grace = Duration::from_secs(300);
            assert!(config.suspend.ephemeral_max_sleep > GUEST_SLEEP);
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(&component.id, agent_id!("Clock", "sleep-seed"))
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
        agent_id!("Clock", "monthly-sleep")
    } else {
        agent_id!("EphemeralClock", "monthly-sleep")
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
    let mut attempts =
        crate::monthly_acceptance::observe(worker.get_initial_worker_metadata().fingerprint);
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
                    abi.method(),
                    data_value!(GUEST_SLEEP.as_secs()),
                )
                .await
        }
    });
    let result = async {
        let original = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                executor.commit_oplog(&id).await?;
                let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                if let Some(record) = history::sleep_record(&entries, &key, abi, false)? {
                    ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
                    info!(?abi, ?entries, ?record, "Committed clock input and open sleep wait under one unfinished method");
                    return Ok::<_, anyhow::Error>(record);
                }
                ensure!(!invocation.is_finished(), "guest returned before open clock wait: {entries:?}");
                tokio::task::yield_now().await;
            }
        }).await.context("committed open sleep Start")??;
        let initial = worker.current_monthly_proposal_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        let mut resource_events = crate::resource_limit_observations::observe(&account);
        ensure!(initial.exhaustion.is_none(), "{initial:?}");
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(!invocation.is_finished());
        ensure!(original.deadline > history::unix_nanos() + STOP_BOUND.as_nanos() as u64,
            "setup left too little time to prove stop before guest deadline: {original:?}");
        let stop_started = std::time::Instant::now();
        // Apply exhaustion while the original sleep remains pending. This bounds a hang,
        // not the product's quota-detection latency.
        tokio::time::timeout(STOP_BOUND, async {
            if resource == Resource::Compute {
                ensure!(initial.fuel_generation.is_some());
                ensure!(!account.prepaid_fuel_is_exhausted(mode, initial.fuel_generation));
                ensure!(account.monthly_capacity_is_exhausted_for_test(mode), "Store must prepay the remaining fuel");
                ensure!(attempts.try_recv().is_err(), "zero unreserved fuel cannot exhaust prepaid fuel");
                ensure!(worker.current_monthly_proposal_for_test() == initial);
                ensure!(!invocation.is_finished() && worker.concurrent_agent_permit_is_held().await);
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
            let refresh = Some(applied_refresh(&limits, &registry, context.account_id).await?);
            let attempt = attempts.recv().await.context("acceptance observer closed")?;
            ensure!(attempt.proposal.policy_revision == initial.policy_revision + 1 + u64::from(resource == Resource::Storage));
            ensure!(attempt.proposal.period == initial.period);
            ensure!(attempt.proposal.diagnostic_window_id == initial.diagnostic_window_id);
            ensure!(attempt.proposal.resident_generation == initial.resident_generation);
            ensure!(attempt.proposal.start_attempt == initial.start_attempt);
            ensure!(attempt.proposal.fingerprint == initial.fingerprint);
            ensure!(attempt.proposal.fuel_generation == initial.fuel_generation);
            ensure!(attempt.proposal.exhaustion == Some(resource.reason(durable)));
            ensure!(attempt.accepted);
            let signal = raw.recv().await?;
            ensure!(matches!(signal, InterruptKind::Suspend(_)));
            ensure!(worker.selected_owner_failure_for_test().await == Some(ToolOwnerFailureMetadata::Lifecycle));
            if resource == Resource::Compute {
                resource_events.assert_policy_advanced_after(registry.current_limits().monthly_policy_revision, initial.fuel_generation)?;
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
            ensure!(history::unix_nanos() < original.deadline, "quota stop must precede guest readiness");
            info!(?abi, ?resource, ?mode, ?signal, proposal = ?attempt.proposal,
                stop_elapsed = ?stop_started.elapsed(), remaining_nanos = original.deadline - history::unix_nanos(),
                "Typed quota stop physically unloaded sleep Worker before real guest deadline");
            Ok::<_, anyhow::Error>(())
        }).await.context("quota stop and physical settlement exceeded 8s bound")??;
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(history::sleep_record(&stopped, &key, abi, false)? == Some(original));
        ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
        let entries = worker.oplog().read_exact(OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64()).await?;
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
        flush_stopped_usage(&limits, &registry, resource).await?;
        let settled = registry.applied_updates();
        check_accounting(&settled, resource, durable, initial.policy_revision + u64::from(resource == Resource::Storage), initial.period)?;
        let settled_count = settled.len();
        info!(?abi, ?resource, ?mode, ?settled, "Stopped sleep window accounting settled");
        // Wait out the real recorded deadline with no Store. Restarting a fresh duration on
        // reconstruction would miss the five-second completion bound below by a wide margin.
        if durable {
            let remaining = original.deadline.saturating_sub(history::unix_nanos());
            tokio::time::sleep(Duration::from_nanos(remaining) + Duration::from_millis(200)).await;
            ensure!(history::unix_nanos() >= original.deadline);
            ensure!(!invocation.is_finished());
        } else {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        if resource == Resource::Storage { assert_storage_deleted(&owned)?; }
        limits.run_batch_for_test().await;
        for update in &registry.applied_updates()[settled_count..] {
            ensure!(update.fuel_delta == 0 && update.memory_gb_seconds_delta == 0 && update.memory_byte_nanoseconds_remainder == 0
                && update.durable_storage_byte_seconds_delta == 0 && update.durable_storage_byte_nanoseconds_remainder == 0
                && update.ephemeral_storage_byte_seconds_delta == 0 && update.ephemeral_storage_byte_nanoseconds_remainder == 0,
                "closed sleep window accrued: {update:?}");
        }
        let mut grant = policy;
        if resource == Resource::Storage {
            grant.available_durable_storage_byte_seconds = u64::MAX;
            grant.available_ephemeral_storage_byte_seconds = u64::MAX;
        }
        registry.set_policy(grant);
        applied_refresh(&limits, &registry, context.account_id).await?.await?;
        if durable {
            let resume_started = std::time::Instant::now();
            executor.resume(&id, false).await?;
            let resumed = tokio::time::timeout(Duration::from_secs(5), &mut invocation).await.context("sleep must retain its expired original deadline, not start another duration")???;
            abi.check_result(resumed)?;
            ensure!(worker.observed_creation_count() > initial.resident_generation);
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(history::sleep_record(&recovered, &key, abi, true)? == Some(original), "replay changed recorded now, duration or original wait identity");
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            history::settled(&recovered)?;
            abi.check_result(executor.invoke_and_await_agent_with_key(&component, &name, &key, abi.method(), data_value!(GUEST_SLEEP.as_secs())).await?)?;
            info!(?abi, ?resource, ?original, resume_elapsed = ?resume_started.elapsed(), ?recovered,
                "Original clock input and deadline replayed; retained wait ended and invocation finished once");
            tokio::time::timeout(Duration::from_secs(5), async {
                while !worker.stop_if_idle().await { tokio::task::yield_now().await; }
                while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
                worker.join_accepted_stops_for_test().await?;
                worker.retained_cleanup_for_test().await
            }).await.context("idle eviction before completed clock replay")??;
            let generation = worker.observed_creation_count();
            let fresh_key = IdempotencyKey::fresh();
            let healthy = tokio::time::timeout(Duration::from_secs(5), executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key, "healthcheck", data_value!())).await??;
            ensure!(healthy.into_typed::<bool>()?);
            ensure!(worker.observed_creation_count() > generation);
            let replayed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(history::sleep_record(&replayed, &key, abi, true)? == Some(original));
            history::invocation(&replayed, &fresh_key, 1)?;
            ensure!(count_agent_invocation_pair_since(&replayed, OplogIndex::INITIAL) == (3, 3));
            history::settled(&replayed)?;
        } else {
            ensure!(tokio::time::timeout(Duration::from_secs(5), executor.invoke_and_await_agent_with_key(&component, &name, &key, abi.method(), data_value!(GUEST_SLEEP.as_secs()))).await?.is_err());
            let failed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(history::sleep_record(&failed, &key, abi, false)? == Some(original));
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

mod p2 {
    use super::*;
    use golem_common::model::agent::AgentMode;
    use test_r::test;

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
                super::pending_sleep(
                    last_unique_id,
                    deps,
                    host_api_tests,
                    support::Resource::$resource,
                    AgentMode::$mode,
                    super::Abi::P2,
                )
                .await
            }
        };
    }

    pending_case!(durable_p2_sleep_memory_quota, Memory, Durable);
    pending_case!(ephemeral_p2_sleep_memory_quota, Memory, Ephemeral);
    pending_case!(durable_p2_sleep_compute_quota_prepaid, Compute, Durable);
    pending_case!(ephemeral_p2_sleep_compute_quota_prepaid, Compute, Ephemeral);
    pending_case!(durable_p2_sleep_managed_xfs_storage_quota, Storage, Durable);
    pending_case!(
        ephemeral_p2_sleep_managed_xfs_storage_quota,
        Storage,
        Ephemeral
    );
}

mod p3 {
    use super::*;
    use golem_common::model::agent::AgentMode;
    use test_r::test;

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
                super::pending_sleep(
                    last_unique_id,
                    deps,
                    host_api_tests,
                    support::Resource::$resource,
                    AgentMode::$mode,
                    super::Abi::P3,
                )
                .await
            }
        };
    }

    pending_case!(durable_p3_sleep_memory_quota, Memory, Durable);
    pending_case!(ephemeral_p3_sleep_memory_quota, Memory, Ephemeral);
    pending_case!(durable_p3_sleep_compute_quota_prepaid, Compute, Durable);
    pending_case!(ephemeral_p3_sleep_compute_quota_prepaid, Compute, Ephemeral);
    pending_case!(durable_p3_sleep_managed_xfs_storage_quota, Storage, Durable);
    pending_case!(
        ephemeral_p3_sleep_managed_xfs_storage_quota,
        Storage,
        Ephemeral
    );
}
