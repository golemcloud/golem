use super::*;
use test_r::test;

#[test]
#[timeout("2m")]
async fn durable_p2_cross_socket_splice_completion_before_stop_and_history_reconstruction(
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
    let name = agent_id!("Networking", "p2-cross-splice-completion-history");
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
    let source = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let destination = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let source_port = source.local_addr()?.port();
    let destination_port = destination.local_addr()?.port();
    ensure!(source_port != destination_port);
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
                    METHOD,
                    data_value!(source_port, destination_port),
                )
                .await
        }
    })));
    let result = async {
        let (mut input, mut output) = peers(&source, &destination).await?;
        let phase = pending_phase(&mut phases, Operation::Splice, &key).await?;
        executor.commit_oplog(&id).await?;
        let starts = cross_setup(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, 2)?;
        let acquisitions = worker.permit_acquisitions_for_test();
        let (selected, release) = worker.pause_next_outcome_for_test(true);
        echo(&mut input, &mut output).await?;
        tokio::time::timeout(Duration::from_secs(10), selected).await??;
        ensure!(phase.state() == P2NativeInputStateForTest::Returned);
        ensure!(worker.concurrent_agent_permit_is_held().await);
        let mut raw = worker.raw_interrupt_for_test();
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
        let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
            .await?
            .context("monthly acceptance")?;
        ensure!(attempt.proposal.window_identity == phase.runtime.window_identity);
        ensure!(attempt.proposal.resident_generation == phase.runtime.resident_generation);
        ensure!(attempt.proposal.policy_revision == phase.runtime.policy_revision + 1);
        ensure!(attempt.proposal.exhaustion == Some("monthly memory exhausted"));
        ensure!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.frozen_stop_for_test().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        ensure!(matches!(
            raw.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        ensure!(!request_is_finished(&invocation));
        release.send(false).unwrap();
        ensure!(
            join_request(&mut invocation, "invocation result")
                .await??
                .into_typed::<Result<u64, String>>()?
                == Ok(1)
        );
        ensure!(matches!(
            tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
            InterruptKind::Suspend(_)
        ));
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
        .context("completion-first physical unload")?;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        join_request(&mut refresh, "stop refresh").await?;
        ensure!(
            worker.unload_succeeded_for_test()
                && worker.permit_acquisitions_for_test() == acquisitions
        );
        ensure!(clock.active_sleeps() == 0);
        ensure!(
            limits
                .initialize_account(context.account_id)
                .await?
                .monthly_observer_count_for_test()
                == 0
        );
        let completed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(cross_setup(&completed, 2)? == starts);
        assert_settled_history(&completed)?;
        ensure!(count_agent_invocation_pair_since(&completed, OplogIndex::INITIAL) == (2, 2));
        ensure!(
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    METHOD,
                    data_value!(source_port, destination_port)
                )
                .await?
                .into_typed::<Result<u64, String>>()?
                == Ok(1)
        );
        ensure!(
            tokio::time::timeout(Duration::from_millis(100), source.accept())
                .await
                .is_err()
        );
        ensure!(
            tokio::time::timeout(Duration::from_millis(100), destination.accept())
                .await
                .is_err()
        );
        registry.set_policy(policy);
        start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
        join_request(&mut refresh, "policy refresh").await?;
        let fresh_key = IdempotencyKey::fresh();
        let fresh = executor.invoke_and_await_agent_with_key(
            &component,
            &name,
            &fresh_key,
            METHOD,
            data_value!(source_port, destination_port),
        );
        let peer_work = async {
            // The completed historical raw splice re-executes against fresh sockets.
            let (mut old_input, mut old_output) = peers(&source, &destination).await?;
            let historical = pending_phase(&mut phases, Operation::Splice, &key).await?;
            ensure!(historical.runtime.resident_generation > phase.runtime.resident_generation);
            echo(&mut old_input, &mut old_output).await?;
            ensure!(historical.state() == P2NativeInputStateForTest::Returned);
            let (mut new_input, mut new_output) = peers(&source, &destination).await?;
            let fresh_phase = pending_phase(&mut phases, Operation::Splice, &fresh_key).await?;
            echo(&mut new_input, &mut new_output).await?;
            ensure!(fresh_phase.state() == P2NativeInputStateForTest::Returned);
            Ok::<_, anyhow::Error>(())
        };
        let (fresh, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::try_join!(fresh, peer_work)
        })
        .await??;
        ensure!(fresh.into_typed::<Result<u64, String>>()? == Ok(1));
        ensure!(
            tokio::time::timeout(Duration::from_millis(100), source.accept())
                .await
                .is_err()
        );
        ensure!(
            tokio::time::timeout(Duration::from_millis(100), destination.accept())
                .await
                .is_err()
        );
        let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (3, 3));
        ensure!(cross_setup(&history, 4)?.len() == 4);
        assert_settled_history(&history)?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    drop(source);
    drop(destination);
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
