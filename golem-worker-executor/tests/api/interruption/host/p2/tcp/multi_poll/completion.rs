use super::*;
use test_r::test;

#[test]
#[timeout("2m")]
async fn durable_p2_multi_poll_duplicate_indices_completion_before_stop_and_history_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    completion_and_history(last_unique_id, deps, host_api_tests, true).await
}

#[test]
#[timeout("2m")]
async fn durable_p2_multi_poll_completion_before_stop_and_history_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    completion_and_history(last_unique_id, deps, host_api_tests, false).await
}

async fn completion_and_history(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    duplicate: bool,
) -> anyhow::Result<()> {
    let method = if duplicate {
        "tcp_inputs_duplicate_poll_p2"
    } else {
        "tcp_inputs_poll_p2"
    };
    let count = if duplicate { 3 } else { 2 };
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
    let name = agent_id!("Networking", "p2-multi-poll-completion-history");
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
    let mut phases = worker.observe_p2_poll_for_test();
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let listeners = listeners().await?;
    let first_port = listeners[0].local_addr()?.port();
    let second_port = listeners[1].local_addr()?.port();
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
                    method,
                    data_value!(first_port, second_port),
                )
                .await
        }
    });
    let result = async {
        let mut effects = PeerEffects::default();
        let mut peers = accept_pair(&listeners, &mut effects).await?;
        let phase = pending_phase(&mut phases, &key, count).await?;
        executor.commit_oplog(&id).await?;
        let start = input_poll(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, &key, count, None)?;
        ensure!(phase.start == start);
        let acquisitions = worker.permit_acquisitions_for_test();
        let (selected, release) = worker.pause_next_outcome_for_test(true);
        release_second(&mut peers, &mut effects).await?;
        tokio::time::timeout(Duration::from_secs(10), selected).await??;
        ensure!(phase.state() == P2PollStateForTest::Returned);
        executor.commit_oplog(&id).await?;
        let completed_poll = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let recorded = poll_result(&completed_poll, start)?;
        assert_indices(&recorded, duplicate)?;
        ensure!(input_poll(&completed_poll, &key, count, Some(&recorded))? == start);
        assert_invocation(&completed_poll, &key, 0)?;
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
        let result = tokio::time::timeout(Duration::from_secs(10), &mut invocation).await???
            .into_typed::<Result<Vec<u32>, String>>()?;
        ensure!(result == Ok(recorded.clone()));
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
        closed_pair(&mut peers, &mut effects).await?;
        ensure!(effects.accepts == [1, 1] && effects.closed == [1, 1] && effects.sent == [0, 1]);
        let completed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(input_poll(&completed, &key, count, Some(&recorded))? == start);
        assert_invocation(&completed, &key, 1)?;
        settled_history(&completed)?;
        ensure!(count_agent_invocation_pair_since(&completed, OplogIndex::INITIAL) == (2, 2));
        let cached = executor.invoke_and_await_agent_with_key(
            &component, &name, &key, method, data_value!(first_port, second_port),
        ).await?.into_typed::<Result<Vec<u32>, String>>()?;
        ensure!(cached == Ok(recorded.clone()));
        no_connections(&listeners).await?;
        ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);

        registry.set_policy(policy);
        applied_refresh(&limits, &registry, context.account_id).await?.await?;
        // Physical reconstruction re-executes both native connects, but must not await
        // historical TCP input readiness. Keep both historical peers silent and open.
        let fresh_key = IdempotencyKey::fresh();
        let fresh = executor.invoke_and_await_agent_with_key(
            &component, &name, &fresh_key, method, data_value!(first_port, second_port),
        );
        let peers = async {
            let mut historical = accept_pair(&listeners, &mut effects).await?;
            closed_pair(&mut historical, &mut effects).await?;
            ensure!(effects.accepts == [2, 2] && effects.closed == [2, 2] && effects.sent == [0, 1]);
            let mut fresh_peers = accept_pair(&listeners, &mut effects).await?;
            let fresh_phase = pending_phase(&mut phases, &fresh_key, count).await?;
            ensure!(fresh_phase.runtime.resident_generation > phase.runtime.resident_generation);
            ensure!(fresh_phase.runtime.fingerprint == phase.runtime.fingerprint);
            ensure!(fresh_phase.start != start);
            release_second(&mut fresh_peers, &mut effects).await?;
            closed_pair(&mut fresh_peers, &mut effects).await?;
            ensure!(fresh_phase.state() == P2PollStateForTest::Returned);
            Ok::<_, anyhow::Error>(())
        };
        let (fresh, ()) = tokio::time::timeout(Duration::from_secs(10), async { tokio::try_join!(fresh, peers) }).await??;
        let fresh_result = fresh.into_typed::<Result<Vec<u32>, String>>()?.map_err(anyhow::Error::msg)?;
        assert_indices(&fresh_result, duplicate)?;
        no_connections(&listeners).await?;
        ensure!(effects.accepts == [3, 3] && effects.closed == [3, 3] && effects.sent == [0, 2]);
        let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(input_poll(&history, &key, count, Some(&recorded))? == start);
        input_poll(&history, &fresh_key, count, Some(&fresh_result))?;
        ensure!(poll_starts(&history).len() == 6, "completed history adds no poll Start");
        assert_invocation(&history, &key, 1)?;
        assert_invocation(&history, &fresh_key, 1)?;
        ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (3, 3));
        settled_history(&history)?;
        info!(%key, %fresh_key, %start, duplicate, ?recorded, ?effects,
            "B readiness indices persisted; completed history reconnected without input readiness; fresh poll completed separately");
        Ok::<_, anyhow::Error>(())
    }.await;
    drop(listeners);
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
            invocation.abort();
            let _ = tokio::time::timeout(Duration::from_secs(10), &mut invocation).await;
        }
    }
    result
}

fn assert_indices(indices: &[u32], duplicate: bool) -> anyhow::Result<()> {
    let mut sorted = indices.to_vec();
    sorted.sort_unstable();
    if duplicate {
        ensure!(
            sorted == [0, 2],
            "B alone ready in [B, A, B] must return both indices: {indices:?}"
        );
    } else {
        ensure!(indices == [1], "only B ready in [A, B]: {indices:?}");
    }
    Ok(())
}
