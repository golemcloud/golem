use super::*;
use golem_worker_executor::worker::WebSocketHandshakeStateForTest;
use test_r::test;

#[test]
#[timeout("2m")]
async fn websocket_handshake_completion_before_monthly_exhaustion(
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
            config.limits.fuel_to_borrow = FUEL_BUDGET;
            config.max_websocket_connections = 1;
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let name = agent_id!("WebsocketTest", "monthly-ws-handshake-completion");
    let key = IdempotencyKey::fresh();
    let id = AgentId::from_agent_id(component.id, &name).unwrap();
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let seed_id = executor
        .start_agent(
            &component.id,
            agent_id!("WebsocketTest", "monthly-ws-handshake-control-seed"),
        )
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .context("seed worker missing")?
        .primary();
    wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
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
    let mut native = worker.observe_websocket_handshake_for_test();
    let pool = worker.websocket_connection_pool();
    let free = tokio::time::timeout(Duration::from_secs(5), pool.acquire()).await??;
    drop(free);
    let mut peer = Peer::start_with_held_handshake(true).await?;
    let mut invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let name = name.clone();
        let key = key.clone();
        let url = peer.url.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    "connect_and_receive_first",
                    data_value!(url),
                )
                .await
        }
    });
    let mut invocation_joined = false;
    let result = async {
        let held = peer.held().await?;
        ensure!(held.number == 1);
        let pending = tokio::time::timeout(Duration::from_secs(10), native.recv())
            .await?
            .context("native connect_async did not return Pending")?;
        executor.commit_oplog(&id).await?;
        let open = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let connect = pool::connect_start(&open, 0, 0)?;
        ensure!(pending.start == connect && pending.invocation_key.as_ref() == Some(&key));
        ensure!(pending.runtime == worker.current_monthly_proposal_for_test());
        ensure!(pending.state() == WebSocketHandshakeStateForTest::Pending);
        ensure!(worker.concurrent_agent_permit_is_held().await && worker.is_loaded().await);
        peer.assert_effects(1, 0, 0)?;
        held.release
            .send(())
            .map_err(|_| anyhow::anyhow!("peer stopped before Upgrade release"))?;
        let handshake = peer.handshake().await?;
        ensure!(handshake.number == 1);
        handshake
            .send
            .send("completed".to_string())
            .map_err(|_| anyhow::anyhow!("frame gate closed"))?;
        let completed = tokio::time::timeout(Duration::from_secs(15), &mut invocation).await?;
        invocation_joined = true;
        let completed = completed??;
        ensure!(completed.into_typed::<String>()? == "completed");
        ensure!(pending.state() == WebSocketHandshakeStateForTest::Returned);
        let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(pool::connect_start(&history, 1, 1)? == connect);
        assert_invocation(&history, &key, 1)?;
        assert_settled_history(&history)?;
        peer.assert_effects(1, 1, 1)?;
        let mut exhausted = policy;
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        applied_refresh(&limits, &registry, context.account_id)
            .await?
            .await?;
        let cached = executor
            .invoke_and_await_agent_with_key(
                &component,
                &name,
                &key,
                "connect_and_receive_first",
                data_value!(peer.url.clone()),
            )
            .await?;
        ensure!(cached.into_typed::<String>()? == "completed");
        peer.assert_effects(1, 1, 1)?;
        let after = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_invocation(&after, &key, 1)?;
        ensure!(pool::connect_start(&after, 1, 1)? == connect);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let mut cleanup_errors = Vec::new();
    if !invocation_joined {
        if !invocation.is_finished() {
            match tokio::time::timeout(Duration::from_secs(10), executor.interrupt(&id)).await {
                Ok(Ok(())) => {}
                other => cleanup_errors.push(format!("interrupt: {other:?}")),
            }
        }
        match tokio::time::timeout(Duration::from_secs(10), &mut invocation).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => cleanup_errors.push(format!("invocation join: {error:#}")),
            Err(error) => {
                cleanup_errors.push(format!("invocation join: {error:#}"));
                invocation.abort();
                match tokio::time::timeout(Duration::from_secs(10), &mut invocation).await {
                    Ok(Err(error)) if error.is_cancelled() => {}
                    other => cleanup_errors.push(format!("aborted invocation join: {other:?}")),
                }
            }
        }
    }
    let idle = match tokio::time::timeout(Duration::from_secs(10), worker.stop_if_idle()).await {
        Ok(idle) => idle,
        Err(error) => {
            cleanup_errors.push(format!("idle retirement: {error:#}"));
            false
        }
    };
    if !idle && (worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await) {
        match tokio::time::timeout(Duration::from_secs(10), executor.interrupt(&id)).await {
            Ok(Ok(())) => {}
            other => cleanup_errors.push(format!("retire: {other:?}")),
        }
    }
    match tokio::time::timeout(
        Duration::from_secs(10),
        worker.join_accepted_stops_for_test(),
    )
    .await
    {
        Ok(Ok(())) => {}
        other => cleanup_errors.push(format!("stop join: {other:?}")),
    }
    match tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await {
        Ok(Ok(())) => {}
        other => cleanup_errors.push(format!("retained cleanup: {other:?}")),
    }
    if worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
        cleanup_errors.push("completion worker still holds a Store or Worker permit".to_string());
    }
    match tokio::time::timeout(Duration::from_secs(5), pool.acquire()).await {
        Ok(Ok(permit)) => drop(permit),
        other => cleanup_errors.push(format!("WebSocket pool permit: {other:?}")),
    }
    if let Err(error) = peer.finish().await {
        cleanup_errors.push(format!("peer shutdown: {error:#}"));
    }
    if cleanup_errors.is_empty() {
        result
    } else {
        let failures = cleanup_errors.join("; ");
        match result {
            Ok(()) => Err(anyhow::anyhow!("cleanup failed: {failures}")),
            Err(error) => Err(error.context(format!("cleanup failed: {failures}"))),
        }
    }
}
