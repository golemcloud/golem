use super::*;
use test_r::test;

#[test]
#[timeout("2m")]
async fn durable_p3_udp_receive_completion_before_monthly_stop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let policy = Resource::Memory.policy();
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let metering = Resource::Memory.metering();
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
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let name = agent_id!("Networking", "p3-udp-completion");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
    let worker = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
        .await
        .unwrap()
        .primary();
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let (clock, _polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut phases = worker.observe_p3_udp_receive_for_test();
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let mut failure_address = None;
    let key = IdempotencyKey::fresh();
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
        executor.commit_oplog(&id).await?;
        let start = receive_start(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, 0)?;
        let acquisitions = worker.permit_acquisitions_for_test();
        let (selected, release) = worker.pause_next_outcome_for_test(true);
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        ensure!(sender.send_to(b"completed", phase.local_address).await? == 9);
        drop(sender);
        tokio::time::timeout(Duration::from_secs(10), selected).await??;
        ensure!(phase.state() == P3UdpReceiveStateForTest::Returned);
        executor.commit_oplog(&id).await?;
        ensure!(receive_start(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, 1)? == start);
        ensure!(worker.concurrent_agent_permit_is_held().await);
        let mut raw = worker.raw_interrupt_for_test();
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        let refresh = applied_refresh(&limits, &registry, context.account_id).await?;
        let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv()).await?.context("monthly acceptance")?;
        ensure!(attempt.proposal.window_identity == phase.runtime.window_identity);
        ensure!(attempt.proposal.resident_generation == phase.runtime.resident_generation);
        ensure!(attempt.proposal.policy_revision == phase.runtime.policy_revision + 1);
        ensure!(attempt.proposal.exhaustion == Some("monthly memory exhausted"));
        ensure!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.frozen_stop_for_test().is_none() { tokio::task::yield_now().await; }
        }).await?;
        ensure!(matches!(raw.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)), "stop cannot race the selected result's commit");
        ensure!(!invocation.is_finished());
        release.send(false).unwrap();
        ensure!(tokio::time::timeout(Duration::from_secs(10), &mut invocation).await???.into_typed::<Result<Vec<u8>, String>>()? == Ok(b"completed".to_vec()));
        ensure!(matches!(tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??, InterruptKind::Suspend(_)));
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        }).await.context("completion-first physical unload")?;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        refresh.await?;
        ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
        ensure!(worker.unload_succeeded_for_test());
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(clock.active_sleeps() == 0);
        ensure!(limits.initialize_account(context.account_id).await?.monthly_observer_count_for_test() == 0);
        drop(tokio::net::UdpSocket::bind(phase.local_address).await?);
        let completed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(receive_start(&completed, 1)? == start);
        assert_settled_history(&completed)?;
        assert_invocation(&completed, &key, 1)?;
        ensure!(count_agent_invocation_pair_since(&completed, OplogIndex::INITIAL) == (2, 2));
        let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, "udp_receive_p3", data_value!()).await?;
        ensure!(cached.into_typed::<Result<Vec<u8>, String>>()? == Ok(b"completed".to_vec()));
        ensure!(phases.try_recv().is_err());
        info!(%key, %start, ?phase, datagrams_sent = 1,
            "Native UDP receive End and delivery preceded monthly stop; selected invocation result committed once");
        Ok::<_, anyhow::Error>(())
    }.await;
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
