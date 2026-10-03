use super::*;
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::durable_host::io::streams::FilesystemObservationGateForTest;
use golem_worker_executor::services::{HasOplog, UsesAllDeps};
use golem_worker_executor::worker::MonthlyClockForTest;
use golem_worker_executor_test_utils::{
    BlobStoreExistsGate, FailingBlobStoreService, start_with_resource_limits_and_overrides,
};
use test_r::test;

macro_rules! case {
    ($name:ident, $filesystem:expr, $mode:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] component: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            pending_stop(
                last_unique_id,
                deps,
                component,
                $filesystem,
                AgentMode::$mode,
            )
            .await
        }
    };
}
case!(durable_blob_provider_monthly_memory, false, Durable);
case!(ephemeral_blob_provider_monthly_memory, false, Ephemeral);
case!(durable_filesystem_observation_monthly_memory, true, Durable);
case!(
    ephemeral_filesystem_observation_monthly_memory,
    true,
    Ephemeral
);

async fn pending_stop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    filesystem: bool,
    mode: AgentMode,
) -> anyhow::Result<()> {
    let resource = monthly_pending::Resource::Memory;
    let policy = resource.policy();
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let shutdown = CancellationToken::new();
    let _shutdown = shutdown.clone().drop_guard();
    let metering = resource.metering();
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown,
    );
    let context = TestContext::new(last_unique_id);
    let blob = Arc::new(BlobStoreExistsGate::default());
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(move |config| {
            config.resource_usage_metering = metering
        })),
        wrap_blob_store_service: Some(Arc::new({
            let blob = blob.clone();
            move |inner| {
                Arc::new(FailingBlobStoreService::with_exists_gate(
                    inner,
                    blob.clone(),
                ))
            }
        })),
        ..Default::default()
    };
    let executor =
        start_with_resource_limits_and_overrides(deps, &context, limits.clone(), overrides).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(
            &component.id,
            agent_id!("BlobStore", "storage-observation-seed"),
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
    .await?;
    let durable = mode == AgentMode::Durable;
    let name = if !durable {
        agent_id!("EphemeralStorageObservation", "pending-storage")
    } else if filesystem {
        agent_id!("FileSystem", "pending-storage")
    } else {
        agent_id!("BlobStore", "pending-storage")
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
    let worker = Worker::get_or_create_suspended(
        seed.all(),
        &OwnedAgentId::new(context.default_environment_id, &id),
        None,
        vec![],
        None,
        None,
        &InvocationContextStack::fresh(),
        Principal::anonymous(),
    )
    .await?;
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let gate = filesystem.then(|| FilesystemObservationGateForTest::install(key.clone()));
    let method = if filesystem {
        "stat_root"
    } else {
        "container_exists"
    };
    let input = if filesystem {
        data_value!()
    } else {
        data_value!("withheld-container")
    };
    let mut invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let name = name.clone();
        let key = key.clone();
        let input = input.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(&component, &name, &key, method, input)
                .await
        }
    });
    let result = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            if let Some(gate) = &gate { gate.pending.notified().await; } else { blob.pending.notified().await; }
        }).await.context("supported provider/adapter first Poll::Pending")?;
        executor.commit_oplog(&id).await?;
        let before = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let matches_boundary = |name: &str| if filesystem { name.contains("::stat") } else { name.ends_with("::container_exists") };
        let starts: Vec<_> = before.iter().filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if matches_boundary(&start.function_name) => Some(entry.oplog_index), _ => None }).collect();
        ensure!(starts.len() == 1, "expected one open boundary: {before:#?}");
        let start = starts[0];
        ensure!(terminal_count(&before, start) == 0);
        ensure!(!invocation.is_finished() && worker.concurrent_agent_permit_is_held().await);
        tokio::time::timeout(Duration::from_secs(10), polls.recv()).await?.context("monthly timer")?;
        let account = limits.initialize_account(context.account_id).await?;
        let mut raw = worker.raw_interrupt_for_test();
        let mut exhausted = policy.clone(); exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        let refresh = monthly::applied_refresh(&limits, &registry, context.account_id).await?;
        let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv()).await?.context("accepted monthly stop")?;
        ensure!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        let signal = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        ensure!(matches!(signal, InterruptKind::Suspend(_)));
        tokio::time::timeout(Duration::from_secs(3), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
        }).await.context("physical release while provider/adapter gate remains withheld")?;
        worker.join_accepted_stops_for_test().await?;
        worker.retained_cleanup_for_test().await?;
        refresh.await?;
        ensure!(worker.unload_succeeded_for_test());
        ensure!(clock.active_sleeps() == 0 && account.monthly_observer_count_for_test() == 0);
        if let Some(gate) = &gate { ensure!(gate.dropped.load(Ordering::SeqCst) == 1); }
        else { ensure!(blob.dropped.load(Ordering::SeqCst) == 1 && blob.attempts.load(Ordering::SeqCst) == 1); }
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(terminal_count(&stopped, start) == 0);
        ensure!(!stopped.iter().any(|entry| matches!(entry.entry, PublicOplogEntry::Cancelled(_))));
        let raw_entries = worker.oplog().read_exact(OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64()).await;
        let errors: Vec<_> = raw_entries.values().filter_map(|entry| match entry { OplogEntry::Error { error, .. } => Some(error), _ => None }).collect();
        if durable {
            ensure!(errors.is_empty(), "{errors:?}");
            ensure!(!invocation.is_finished());
            if let Some(gate) = &gate { gate.remove(); } else { blob.release.add_permits(1); }
            registry.set_policy(policy.clone());
            monthly::applied_refresh(&limits, &registry, context.account_id).await?.await?;
            executor.resume(&id, false).await?;
            let output = tokio::time::timeout(Duration::from_secs(10), &mut invocation).await.context("same-key continuation")???;
            ensure!(output.into_typed::<bool>()? == filesystem);
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(terminal_count(&recovered, start) == 1);
            let recovered_starts: Vec<_> = recovered.iter().filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(call) if matches_boundary(&call.function_name) => Some(entry.oplog_index),
                _ => None,
            }).collect();
            ensure!(recovered_starts.first() == Some(&start));
            if !filesystem { ensure!(recovered_starts.len() == 1); }
            let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, method, input.clone()).await?;
            ensure!(cached.into_typed::<bool>()? == filesystem);
            if !filesystem { ensure!(blob.attempts.load(Ordering::SeqCst) == 2); }
        } else {
            ensure!(matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)] if reason.reason == "monthly memory exhausted"), "{errors:?}");
            ensure!(tokio::time::timeout(Duration::from_secs(10), &mut invocation).await??.is_err());
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    if let Some(gate) = gate {
        gate.remove();
        gate.release.add_permits(10);
    }
    blob.release.add_permits(10);
    if result.is_err() && !invocation.is_finished() {
        invocation.abort();
        let _ = invocation.await;
    }
    result
}

fn terminal_count(
    entries: &[golem_common::model::oplog::PublicOplogEntryWithIndex],
    start: OplogIndex,
) -> usize {
    entries
        .iter()
        .filter(|entry| match &entry.entry {
            PublicOplogEntry::End(end) => end.start_index == start,
            PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
            _ => false,
        })
        .count()
}
