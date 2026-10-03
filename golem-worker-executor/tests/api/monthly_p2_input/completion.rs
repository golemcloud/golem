use super::*;
use test_r::test;

macro_rules! completion_case {
    ($name:ident, $operation:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            completion_and_history(last_unique_id, deps, host_api_tests, Operation::$operation)
                .await
        }
    };
}

completion_case!(
    durable_p2_blocking_read_completion_before_stop_and_history_reconstruction,
    Read
);
completion_case!(
    durable_p2_blocking_skip_completion_before_stop_and_history_reconstruction,
    Skip
);
completion_case!(
    durable_p2_blocking_splice_completion_before_stop_and_history_reconstruction,
    Splice
);

async fn completion_and_history(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    operation: Operation,
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
    let mut refresh = None;
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
    let name = agent_id!("Networking", "p2-input-completion-history");
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
    let mut phases = worker.observe_p2_native_input_for_test();
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let key = IdempotencyKey::fresh();
    let mut invocation = Some(AbortOnDropHandle::new(tokio::spawn({
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
                    operation.method(),
                    data_value!(port),
                )
                .await
        }
    })));
    let result = async {
        let (mut peer, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept()).await??;
        let phase = pending_phase(&mut phases, operation, &key).await?;
        executor.commit_oplog(&id).await?;
        let connect_start = completed_setup(&executor.get_oplog(&id, OplogIndex::INITIAL).await?)?;
        let acquisitions = worker.permit_acquisitions_for_test();
        let (selected, release) = worker.pause_next_outcome_for_test(true);
        peer.write_all(b"x").await?;
        if matches!(operation, Operation::Splice) {
            let mut echoed = [0];
            peer.read_exact(&mut echoed).await?;
            ensure!(echoed == *b"x");
        }
        tokio::time::timeout(Duration::from_secs(10), selected).await??;
        ensure!(phase.state() == P2NativeInputStateForTest::Returned);
        ensure!(worker.concurrent_agent_permit_is_held().await);
        let mut raw = worker.raw_interrupt_for_test();
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
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
        ensure!(!request_is_finished(&invocation));
        release.send(false).unwrap();
        operation.assert_result(join_request(&mut invocation, "invocation result").await?? )?;
        ensure!(matches!(tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??, InterruptKind::Suspend(_)));
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        }).await.context("completion-first physical unload")?;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        join_request(&mut refresh, "stop refresh").await?;
        ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
        ensure!(worker.unload_succeeded_for_test());
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(clock.active_sleeps() == 0);
        ensure!(limits.initialize_account(context.account_id).await?.monthly_observer_count_for_test() == 0);
        let mut byte = [0];
        ensure!(tokio::time::timeout(Duration::from_secs(10), peer.read(&mut byte)).await?? == 0);
        let completed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(completed_setup(&completed)? == connect_start);
        assert_settled_history(&completed)?;
        ensure!(count_agent_invocation_pair_since(&completed, OplogIndex::INITIAL) == (2, 2));
        operation.assert_result(executor.invoke_and_await_agent_with_key(&component, &name, &key, operation.method(), data_value!(port)).await?)?;
        ensure!(tokio::time::timeout(Duration::from_millis(100), listener.accept()).await.is_err(), "cached lookup must not execute history");

        registry.set_policy(policy);
        start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
        join_request(&mut refresh, "policy refresh").await?;
        // A fresh invocation on the physically unloaded Worker reconstructs completed history.
        // Raw TCP reads/skips are not recorded: the old call must connect and consume again.
        let fresh_key = IdempotencyKey::fresh();
        let fresh = executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key, operation.method(), data_value!(port));
        let peers = async {
            let (mut historical_peer, _) = listener.accept().await?;
            let historical = pending_phase(&mut phases, operation, &key).await?;
            ensure!(historical.runtime.resident_generation > phase.runtime.resident_generation);
            ensure!(historical.runtime.fingerprint == phase.runtime.fingerprint);
            historical_peer.write_all(b"x").await?;
            if matches!(operation, Operation::Splice) {
                historical_peer.read_exact(&mut byte).await?;
                ensure!(byte == *b"x");
            }
            ensure!(historical_peer.read(&mut byte).await? == 0);
            ensure!(historical.state() == P2NativeInputStateForTest::Returned);
            let (mut fresh_peer, _) = listener.accept().await?;
            let fresh_phase = pending_phase(&mut phases, operation, &fresh_key).await?;
            ensure!(fresh_phase.runtime.resident_generation == historical.runtime.resident_generation);
            fresh_peer.write_all(b"x").await?;
            if matches!(operation, Operation::Splice) {
                fresh_peer.read_exact(&mut byte).await?;
                ensure!(byte == *b"x");
            }
            ensure!(fresh_peer.read(&mut byte).await? == 0);
            ensure!(fresh_phase.state() == P2NativeInputStateForTest::Returned);
            Ok::<_, anyhow::Error>(())
        };
        let (fresh, ()) = tokio::time::timeout(Duration::from_secs(10), async { tokio::try_join!(fresh, peers) }).await??;
        operation.assert_result(fresh)?;
        ensure!(tokio::time::timeout(Duration::from_millis(100), listener.accept()).await.is_err(), "exactly three native connections expected");
        let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (3, 3));
        ensure!(poll_starts(&history).len() == 2, "completed historical connect replays, fresh invocation adds one poll");
        assert_settled_history(&history)?;
        info!(?operation, %key, %fresh_key, ?phase, connections = 3, peer_bytes = 3,
            "Native input completed before monthly stop; selected result survived; completed raw TCP history consumed again before fresh work");
        Ok::<_, anyhow::Error>(())
    }.await;
    drop(listener);
    finish_input_test(
        result,
        &executor,
        &id,
        &worker,
        &mut invocation,
        &mut refresh,
    )
    .await
}
