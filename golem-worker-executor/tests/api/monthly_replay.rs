use super::*;
use anyhow::Context as _;
use golem_common::model::oplog::OplogEntry;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;

#[test]
#[timeout("60s")]
async fn monthly_memory_stop_is_accepted_during_completed_history_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let policy = MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: u64::MAX,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    };
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let shutdown = CancellationToken::new();
    let metering = ResourceUsageMeteringConfig {
        compute: false,
        memory: true,
        filesystem: false,
    };
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown.clone(),
    );
    let context = TestContext::new(last_unique_id);
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let name = agent_id!("Counter", "monthly-completed-replay");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    let key = IdempotencyKey::fresh();
    let result = executor
        .invoke_and_await_agent_with_key(&component, &name, &key, "increment", data_value!())
        .await?
        .into_typed::<u32>()?;
    assert_eq!(result, 1);
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let active = executor.production_active_agent(&owned).await.unwrap();
    let worker = active.primary();
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("initial idle permit release")?;
    assert!(worker.is_loaded().await);
    let fingerprint = worker.get_initial_worker_metadata().fingerprint;
    let initial_generation = worker.resident_generation_for_test();
    let original_tip = worker.oplog().current_oplog_index().await;
    let original = worker
        .oplog()
        .read_exact(OplogIndex::INITIAL, original_tip.as_u64())
        .await;
    let started = |entries: &std::collections::BTreeMap<OplogIndex, OplogEntry>,
                   key: &IdempotencyKey| {
        entries
            .iter()
            .filter_map(|(index, entry)| match entry {
                OplogEntry::AgentInvocationStarted {
                    idempotency_key, ..
                } if idempotency_key == key => Some(*index),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let starts = started(&original, &key);
    assert_eq!(starts.len(), 1);
    let finished_index = *original
        .range(starts[0].next()..)
        .find(|(_, e)| matches!(e, OplogEntry::AgentInvocationFinished { .. }))
        .unwrap()
        .0;
    assert_eq!(
        original
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        2
    );
    assert_eq!(
        original
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        2
    );
    eprintln!("[monthly-replay] original committed oplog: {original:#?}");
    let (replay, release_replay) = worker.pause_completed_replay_for_test(key.clone());
    // The public crash API ignores Idle. Restart the resident Worker through the same stop path.
    worker.set_interrupting(InterruptKind::Restart).await?;
    let replay_indices = tokio::time::timeout(Duration::from_secs(10), replay)
        .await
        .context("completed-history replay boundary after Restart")??;
    assert_eq!(replay_indices, (starts[0], finished_index));
    let generation = worker.resident_generation_for_test();
    assert!(generation > initial_generation);
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(
        worker.get_initial_worker_metadata().fingerprint,
        fingerprint
    );
    let acquisitions = worker.permit_acquisitions_for_test();
    let account = limits.initialize_account(context.account_id).await?;
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    eprintln!(
        "[monthly-replay] committed key={key:?}, indices={replay_indices:?}, result={result}, fingerprint={fingerprint:?}, generation={initial_generation}->{generation}"
    );

    let (_, accepted, release_driver) = worker.pause_next_stop_driver_for_test();
    let mut raw = worker.raw_interrupt_for_test();
    let mut exhausted = policy.clone();
    exhausted.available_memory_gb_seconds = 0;
    registry.set_policy(exhausted);
    let refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), accepted)
        .await
        .context("monthly stop accepted while completed replay is held")??;
    let monthly = worker
        .monthly_stop_for_test()
        .expect("accepted monthly stop");
    assert!(matches!(monthly, InterruptKind::Suspend(_)));
    assert_eq!(worker.pending_stop_for_test().await, Some(monthly));
    assert_eq!(worker.frozen_stop_for_test(), None);
    assert_eq!(worker.owner_stop_for_test().await, None);
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert_eq!(account.monthly_observer_count_for_test(), 1);
    let mut receipt = worker.accepted_stop_receipt_for_test();
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(
        worker
            .oplog()
            .read_exact(OplogIndex::INITIAL, original_tip.as_u64())
            .await,
        original
    );
    eprintln!(
        "[monthly-replay] accepted={monthly:?}, replay gate still held, permit held, receipt pending"
    );
    release_driver.send(false).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
        monthly
    );
    release_replay.send(()).unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        worker.join_accepted_stops_for_test(),
    )
    .await??;
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("replay stop physical unload")?;
    worker.retained_cleanup_for_test().await?;
    refresh.await?;
    tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert_eq!(account.monthly_observer_count_for_test(), 0);
    let stopped = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(
        worker
            .oplog()
            .read_exact(OplogIndex::INITIAL, original_tip.as_u64())
            .await,
        original
    );
    assert_eq!(started(&stopped, &key), starts);
    assert_eq!(
        stopped
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        2
    );
    assert_eq!(
        stopped
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        2
    );
    assert_eq!(
        stopped
            .values()
            .filter(|e| matches!(e, OplogEntry::Suspend { .. }))
            .count(),
        1
    );
    assert!(
        !stopped
            .values()
            .any(|e| matches!(e, OplogEntry::Error { .. } | OplogEntry::Interrupted { .. }))
    );
    eprintln!(
        "[monthly-replay] physical cleanup joined, original history unchanged, one Suspend, no error or duplicate result"
    );

    registry.set_policy(policy);
    limits.run_batch_for_test().await;
    executor.resume(&id, false).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(&component, &name, &key, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        result
    );
    let new_key = IdempotencyKey::fresh();
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(
                &component,
                &name,
                &new_key,
                "increment",
                data_value!()
            )
            .await?
            .into_typed::<u32>()?,
        2
    );
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(
                &component,
                &name,
                &new_key,
                "increment",
                data_value!()
            )
            .await?
            .into_typed::<u32>()?,
        2
    );
    let recovered = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    assert_eq!(
        worker
            .oplog()
            .read_exact(OplogIndex::INITIAL, original_tip.as_u64())
            .await,
        original
    );
    assert_eq!(started(&recovered, &key), starts);
    assert_eq!(started(&recovered, &new_key).len(), 1);
    assert_eq!(
        recovered
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationStarted { .. }))
            .count(),
        3
    );
    assert_eq!(
        recovered
            .values()
            .filter(|e| matches!(e, OplogEntry::AgentInvocationFinished { .. }))
            .count(),
        3
    );
    assert_eq!(
        recovered
            .values()
            .filter(|e| matches!(e, OplogEntry::Suspend { .. }))
            .count(),
        1
    );
    assert!(
        !recovered
            .values()
            .any(|e| matches!(e, OplogEntry::Error { .. } | OplogEntry::Interrupted { .. }))
    );
    assert_eq!(
        worker.get_initial_worker_metadata().fingerprint,
        fingerprint
    );
    assert!(worker.resident_generation_for_test() > generation);
    eprintln!(
        "[monthly-replay] recovered generation={}, new key={new_key:?}, result=2 once, original fingerprint={fingerprint:?}",
        worker.resident_generation_for_test()
    );
    shutdown.cancel();
    Ok(())
}
