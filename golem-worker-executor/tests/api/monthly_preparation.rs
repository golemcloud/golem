use super::*;
use anyhow::Context as _;
use golem_common::model::oplog::{AgentError, OplogEntry, OplogErrorKind};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::{HasOplog, UsesAllDeps};
use golem_worker_executor::worker::Worker;
use pretty_assertions::assert_eq;
use test_r::test;

macro_rules! preparation_test {
    ($name:ident, $durable:expr) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            monthly_stop_during_core_initialization(last_unique_id, deps, agent_counters, $durable)
                .await
        }
    };
}

preparation_test!(durable_monthly_stop_during_real_core_initialization, true);
preparation_test!(
    ephemeral_monthly_stop_during_real_core_initialization,
    false
);

async fn monthly_stop_during_core_initialization(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_counters: &PrecompiledComponent,
    durable: bool,
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
    let seed_id = executor
        .start_agent(&component.id, agent_id!("Counter", "preparation-seed"))
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .unwrap()
        .primary();
    wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while seed.eviction_class().await.is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("seed idle")?;
    let name = if durable {
        agent_id!("Counter", "monthly-preparation")
    } else {
        agent_id!("EphemeralCounter", "monthly-preparation")
    };
    let physical_name = if durable {
        name.clone()
    } else {
        name.clone()
            .with_ephemeral_invocation_phantom(&IdempotencyKey::fresh())
            .unwrap()
    };
    let id = AgentId::from_agent_id(component.id, &physical_name).unwrap();
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
    let mut initializer = worker
        .owner_execution()
        .test_gate_next_monotonic_clock_start();
    Worker::start_if_needed(worker.clone()).await?;
    tokio::time::timeout(Duration::from_secs(10), initializer.entered())
        .await
        .context("core initializer gate")?;
    let before = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    assert!(before.iter().any(|e| matches!(&e.entry, PublicOplogEntry::Start(start) if start.function_name == "monotonic_clock::now")));
    assert!(
        !before
            .iter()
            .any(|e| matches!(e.entry, PublicOplogEntry::AgentInvocationStarted(_)))
    );
    let generation = worker.resident_generation_for_test();
    let acquisitions = worker.permit_acquisitions_for_test();
    let (_, entered, release_driver) = worker.pause_next_stop_driver_for_test();
    let mut raw = worker.raw_interrupt_for_test();
    let helper = worker.await_interrupt_for_test();
    let mut exhausted = policy.clone();
    exhausted.available_memory_gb_seconds = 0;
    registry.set_policy(exhausted);
    let refresh = tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    });
    tokio::time::timeout(Duration::from_secs(10), entered)
        .await
        .context("preparation stop driver")??;
    let monthly = worker
        .monthly_stop_for_test()
        .expect("monthly accepted during core initializer");
    assert!(matches!(monthly, InterruptKind::Suspend(_)));
    assert_eq!(worker.frozen_stop_for_test(), None);
    assert!(worker.concurrent_agent_permit_is_held().await);
    let mut receipt = worker.accepted_stop_receipt_for_test();
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    release_driver.send(false).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), raw.recv())
            .await
            .context("preparation raw interrupt")??,
        monthly
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), helper).await?,
        monthly
    );
    initializer.release();
    tokio::time::timeout(
        Duration::from_secs(10),
        worker.join_accepted_stops_for_test(),
    )
    .await
    .context("preparation stop joins")??;
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("preparation physical unload")?;
    worker.retained_cleanup_for_test().await?;
    refresh.await?;
    assert_eq!(worker.resident_generation_for_test(), generation);
    assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
    assert_eq!(
        limits
            .initialize_account(context.account_id)
            .await?
            .monthly_observer_count_for_test(),
        0
    );
    let entries = worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await;
    let errors: Vec<_> = entries
        .values()
        .filter_map(|e| match e {
            OplogEntry::Error { error, kind, .. } => Some((error, kind)),
            _ => None,
        })
        .collect();
    let suspends = entries
        .values()
        .filter(|e| matches!(e, OplogEntry::Suspend { .. }))
        .count();
    let received = receipt.try_recv();
    assert_eq!(
        received,
        Ok(()),
        "accepted preparation stop must acknowledge after physical unload"
    );
    assert!(matches!(
        receipt.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty
            | tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    assert!(
        !entries.values().any(|e| matches!(
            e,
            OplogEntry::AgentInvocationStarted { .. } | OplogEntry::AgentInvocationFinished { .. }
        )),
        "core initialization cannot fabricate a guest invocation"
    );
    if durable {
        assert!(
            errors.is_empty(),
            "monthly preparation wrote recovery failure: {errors:?}"
        );
        assert_eq!(suspends, 1);
        registry.set_policy(policy);
        limits.run_batch_for_test().await;
        executor.resume(&id, false).await?;
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &name, "increment", data_value!())
                .await?
                .into_typed::<u32>()?,
            1
        );
        let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_eq!(
            recovered
                .iter()
                .filter(|e| matches!(e.entry, PublicOplogEntry::AgentInvocationStarted(_)))
                .count(),
            2
        );
        assert_eq!(
            recovered
                .iter()
                .filter(|e| matches!(e.entry, PublicOplogEntry::AgentInvocationFinished(_)))
                .count(),
            2
        );
    } else {
        assert!(
            matches!(
                errors.as_slice(),
                [(
                    AgentError::EphemeralCannotSuspend(_),
                    OplogErrorKind::Recovery
                )]
            ),
            "{errors:?}"
        );
        assert_eq!(suspends, 0);
    }
    shutdown.cancel();
    Ok(())
}
