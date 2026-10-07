use super::*;
use test_r::test;

#[test]
#[timeout("2m")]
async fn durable_p3_promise_completion_before_quota_stop(
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
            config.suspend.wait_suspend_grace = Duration::from_secs(300);
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let name = agent_id!("GolemHostApi", "p3-promise-completion");
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
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let key = IdempotencyKey::fresh();
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
                    return Ok::<_, anyhow::Error>(record);
                }
                ensure!(!invocation.is_finished());
                tokio::task::yield_now().await;
            }
        }).await??;
        let promises = worker.all().promise_service();
        let pending = promises.poll(original.promise.clone()).await?;
        ensure!(!pending.is_ready().await && pending.get().await.is_none());
        let initial = worker.current_monthly_proposal_for_test();
        ensure!(initial.exhaustion.is_none());
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(count_agent_invocation_pair_since(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, OplogIndex::INITIAL) == (2, 1));
        ensure!(!invocation.is_finished());
        let acquisitions = worker.permit_acquisitions_for_test();
        let (selected, release) = worker.pause_next_outcome_for_test(true);
        ensure!(promises.complete(original.promise.clone(), PAYLOAD.to_vec()).await?);
        tokio::time::timeout(Duration::from_secs(10), selected).await??;
        ensure!(pending.is_ready().await && pending.get().await == Some(PAYLOAD.to_vec()));
        executor.commit_oplog(&id).await?;
        ensure!(history::promise_record(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, &key, &id, true, 0)? == Some(original.clone()));
        ensure!(worker.concurrent_agent_permit_is_held().await);
        let mut raw = worker.raw_interrupt_for_test();
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        let refresh = applied_refresh(&limits, &registry, context.account_id).await?;
        let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv()).await?.context("monthly acceptance")?;
        ensure!(attempt.proposal.window_identity == initial.window_identity);
        ensure!(attempt.proposal.resident_generation == initial.resident_generation);
        ensure!(attempt.proposal.policy_revision == initial.policy_revision + 1);
        ensure!(attempt.proposal.period == initial.period);
        ensure!(attempt.proposal.fingerprint == initial.fingerprint);
        ensure!(attempt.proposal.start_attempt == initial.start_attempt);
        ensure!(attempt.proposal.fuel_generation == initial.fuel_generation);
        ensure!(attempt.proposal.exhaustion == Some("monthly memory exhausted"));
        ensure!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        ensure!(!invocation.is_finished());
        // End and delivery were committed before exhaustion. Release the selected
        // invocation result and check it survives the accepted stop. Receive order
        // below does not establish commit/notification versus signal publication order.
        release.send(false).unwrap();
        ensure!(tokio::time::timeout(Duration::from_secs(10), &mut invocation).await???.into_typed::<Vec<u8>>()? == PAYLOAD);
        let signal = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        ensure!(matches!(signal, InterruptKind::Suspend(_)));
        ensure!(worker.monthly_stop_for_test() == Some(signal));
        ensure!(worker.frozen_stop_for_test() == Some(signal));
        ensure!(worker.owner_stop_for_test().await == Some(signal));
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
        let completed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(history::promise_record(&completed, &key, &id, true, 1)? == Some(original.clone()));
        history::settled(&completed)?;
        history::invocation(&completed, &key, 1)?;
        ensure!(count_agent_invocation_pair_since(&completed, OplogIndex::INITIAL) == (2, 2));
        let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, METHOD, data_value!()).await?;
        ensure!(cached.into_typed::<Vec<u8>>()? == PAYLOAD);
        ensure!(!promises.complete(original.promise.clone(), vec![99]).await?, "duplicate completion must not replace payload");
        ensure!(pending.get().await == Some(PAYLOAD.to_vec()));
        info!(%key, ?original,
            "Committed promise result survived later monthly exhaustion with one invocation result");
        Ok::<_, anyhow::Error>(())
    }.await;
    if result.is_err() {
        let _ = tokio::time::timeout(Duration::from_secs(10), executor.interrupt(&id)).await;
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            worker.join_accepted_stops_for_test(),
        )
        .await;
        let _ =
            tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await;
        if !invocation.is_finished()
            && tokio::time::timeout(Duration::from_secs(10), &mut invocation)
                .await
                .is_err()
        {
            // Only the test's response waiter is aborted, never Worker cleanup.
            invocation.abort();
            let _ = invocation.await;
        }
    }
    result
}
