use super::*;
use test_r::test;

#[test]
#[timeout("2m")]
async fn durable_p3_udp_receive_completion_before_quota_stop(
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
            config.agent_sockets.enabled = true;
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
        while !worker.stop_if_idle().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let reservation = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    let address = reservation.local_addr()?;
    let port = address.port();
    drop(reservation);
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    let ready_port = peer.local_addr()?.port();
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
                    "udp_receive_wait_p3",
                    data_value!(port, ready_port),
                )
                .await
        }
    });
    let result = async {
        ensure!(ready_address(&peer).await? == address);
        failure_address = Some(address);
        let start = admitted_receive(&executor, &id).await?;
        let (selected, release) = executor.gate_next_invocation_outcome_append(&id).await;
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        ensure!(sender.send_to(b"completed", address).await? == 9);
        drop(sender);
        tokio::time::timeout(Duration::from_secs(10), selected).await??;
        executor.commit_oplog(&id).await?;
        ensure!(receive_start(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, 1)? == start);
        let mut raw = worker.raw_interrupt_for_test();
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        let refresh = applied_refresh(&limits, &registry, context.account_id).await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.selected_owner_failure_for_test().await.is_none() {
                tokio::task::yield_now().await;
            }
        }).await?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        ensure!(!invocation.is_finished());
        release.send(false).unwrap();
        ensure!(tokio::time::timeout(Duration::from_secs(10), &mut invocation).await???.into_typed::<Result<Vec<u8>, String>>()? == Ok(b"completed".to_vec()));
        ensure!(matches!(tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??, InterruptKind::Suspend(_)));
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await {
                tokio::task::yield_now().await;
            }
        }).await.context("completion-first physical unload")?;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        refresh.await?;
        ensure!(!worker.is_loaded().await);
        drop(tokio::net::UdpSocket::bind(address).await?);
        let completed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(receive_start(&completed, 1)? == start);
        assert_settled_history(&completed)?;
        assert_invocation(&completed, &key, 1)?;
        ensure!(count_agent_invocation_pair_since(&completed, OplogIndex::INITIAL) == (2, 2));
        let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, "udp_receive_wait_p3", data_value!(port, ready_port)).await?;
        ensure!(cached.into_typed::<Result<Vec<u8>, String>>()? == Ok(b"completed".to_vec()));
        ensure!(matches!(peer.try_recv_from(&mut [0; 32]), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
        info!(%key, %start, datagrams_sent = 1,
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
