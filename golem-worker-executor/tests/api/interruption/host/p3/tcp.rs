use super::*;
use pretty_assertions::assert_eq;
use test_r::test;

async fn memory_quota_exhaustion_interrupts_silent_tcp(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    agent_type: &str,
    durable: bool,
    tick: bool,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::PublicOplogEntry;
    use tokio::io::AsyncReadExt;

    let available_policy = MonthlyResourcePolicy {
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
    let mut initial_policy = available_policy.clone();
    if tick {
        initial_policy.available_memory_gb_seconds = 0;
        initial_policy.available_memory_byte_nanoseconds_remainder = 50_000_000_000_000_000;
    }
    let registry = Arc::new(MutableResourceLimitsRegistry::new(initial_policy));
    let metering = ResourceUsageMeteringConfig {
        compute: false,
        memory: true,
        filesystem: false,
    };
    let shutdown = CancellationToken::new();
    let resource_limits_grpc = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown.clone(),
    );
    let resource_limits: Arc<dyn ResourceLimits> = resource_limits_grpc.clone();
    let context = TestContext::new(last_unique_id);
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        resource_limits,
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (connected_tx, connected_rx) = tokio::sync::oneshot::channel();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let accepted_connections = Arc::new(AtomicUsize::new(0));
    let server_accepted_connections = accepted_connections.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        server_accepted_connections.fetch_add(1, Ordering::SeqCst);
        let _ = connected_tx.send(());
        let mut byte = [0u8; 1];
        let _ = closed_tx.send(matches!(stream.read(&mut byte).await, Ok(0)));
    });

    let agent_id = match agent_type {
        "Networking" => agent_id!("Networking", format!("monthly-memory-silent-tcp-{durable}")),
        "EphemeralNetworking" => agent_id!(
            "EphemeralNetworking",
            format!("monthly-memory-silent-tcp-{durable}")
        ),
        _ => unreachable!("unexpected networking agent type"),
    };
    let invocation_key = IdempotencyKey::fresh();
    let (worker_id, before_invocation) = if durable {
        let worker_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        wait_for_invocation_pair(&executor, &worker_id, OplogIndex::INITIAL).await?;
        let before_invocation = executor.oplog_max_index(&worker_id).await?;
        (worker_id, Some(before_invocation))
    } else {
        let finalized_agent_id = agent_id
            .clone()
            .with_ephemeral_invocation_phantom(&invocation_key)
            .map_err(|error| anyhow!("invalid ephemeral invocation identity: {error}"))?;
        (
            AgentId::from_agent_id(component.id, &finalized_agent_id)
                .map_err(|error| anyhow!("invalid ephemeral agent id: {error}"))?,
            None,
        )
    };
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        let invocation_key = invocation_key.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent_id,
                    &invocation_key,
                    "tcp_collect_p3",
                    data_value!(port),
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(30), connected_rx).await??;

    let invocation_boundary = before_invocation.unwrap_or(OplogIndex::INITIAL);
    let (receive_start, chunk_start) = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if executor.commit_oplog(&worker_id).await.is_err() {
                tokio::task::yield_now().await;
                continue;
            }
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            let terminal_starts = oplog
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::End(end) => Some(end.start_index),
                    PublicOplogEntry::Cancelled(cancelled) => Some(cancelled.start_index),
                    _ => None,
                })
                .collect::<HashSet<_>>();
            let starts = |function_name: &str| {
                oplog
                    .iter()
                    .filter_map(|entry| match &entry.entry {
                        PublicOplogEntry::Start(start)
                            if entry.oplog_index > invocation_boundary
                                && start.function_name == function_name =>
                        {
                            Some(entry.oplog_index)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };
            let receive_starts = starts("sockets::types::tcp-socket::receive");
            let chunk_starts = starts("sockets::types::tcp-socket::receive-chunk");
            let invocation_pairs = count_agent_invocation_pair_since(&oplog, invocation_boundary);
            if receive_starts.len() == 1 && chunk_starts.len() == 1 {
                assert!(
                    !terminal_starts.contains(&receive_starts[0]),
                    "the silent receive parent must remain open before quota interruption"
                );
                assert!(
                    !terminal_starts.contains(&chunk_starts[0]),
                    "the silent receive chunk must remain open before quota interruption"
                );
                let expected_invocation_pairs = if durable { (1, 0) } else { (2, 1) };
                assert_eq!(
                    invocation_pairs,
                    expected_invocation_pairs,
                    "the silent TCP invocation must be admitted but unfinished before quota interruption"
                );
                break Ok::<_, anyhow::Error>((receive_starts[0], chunk_starts[0]));
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;

    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);
    assert!(
        executor
            .concurrent_agent_permit_is_held(&owned_agent_id)
            .await,
        "the silent receive must retain the concurrent-agent permit"
    );
    let admitted_revision = resource_limits_grpc
        .initialized_policy_revision_for_test(context.account_id)
        .await
        .expect("the admitted account must have initialized limits");
    let admitted_period = available_policy.period;

    if !tick {
        let exhausted_revision = registry.current_limits().monthly_policy_revision + 1;
        let mut exhausted_policy = available_policy.clone();
        exhausted_policy.available_memory_gb_seconds = 0;
        let (refresh_started, refresh_release) = registry.delay_next_policy(exhausted_policy);
        let exhausted_refresh = {
            let resource_limits_grpc = resource_limits_grpc.clone();
            tokio::spawn(async move { resource_limits_grpc.run_batch_for_test().await })
        };
        tokio::time::timeout(Duration::from_secs(30), refresh_started.acquire())
            .await??
            .forget();
        refresh_release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(30), exhausted_refresh).await??;
        assert_eq!(
            resource_limits_grpc
                .initialized_policy_revision_for_test(context.account_id)
                .await,
            Some(exhausted_revision),
            "the target account entry must apply the exhausted Registry revision"
        );
    }

    assert!(
        tokio::time::timeout(Duration::from_secs(45), closed_rx).await??,
        "monthly memory exhaustion must close the silent TCP socket"
    );
    server.await?;
    assert_eq!(
        accepted_connections.load(Ordering::SeqCst),
        1,
        "completed connect replay must not open a second external connection"
    );

    if durable {
        executor
            .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(30))
            .await?;
        assert!(
            !executor
                .concurrent_agent_permit_is_held(&owned_agent_id)
                .await,
            "permit release must follow the interrupted execution window"
        );

        let before_invocation =
            before_invocation.expect("durable invocation has an oplog boundary");
        let before_resume = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_eq!(
            count_agent_invocation_pair_since(&before_resume, before_invocation),
            (1, 0),
            "the retained invocation must remain unfinished while suspended"
        );
        for interrupted_start in [receive_start, chunk_start] {
            assert!(before_resume.iter().all(|entry| !matches!(
                &entry.entry,
                PublicOplogEntry::End(end) if end.start_index == interrupted_start
            )));
            assert!(before_resume.iter().all(|entry| !matches!(
                &entry.entry,
                PublicOplogEntry::Cancelled(cancelled)
                    if cancelled.start_index == interrupted_start
            )));
        }

        if tick {
            // A policy boundary flushes the sub-unit remainder retained in the account accumulator.
            registry.set_policy(available_policy.clone());
            resource_limits_grpc.run_batch_for_test().await;
        }
        resource_limits_grpc.run_batch_for_test().await;
        let settled_usage = registry.applied_memory_byte_nanoseconds();
        assert!(
            settled_usage > 0,
            "the interrupted window must settle memory usage"
        );
        assert!(
            registry.applied_memory_byte_nanoseconds_at_revision(admitted_revision) > 0,
            "memory settlement must retain admitted policy revision {admitted_revision}"
        );
        assert!(registry.applied_updates().iter().any(|update| {
            update.monthly_policy_revision == admitted_revision
                && update.period == admitted_period
                && (update.memory_gb_seconds_delta > 0
                    || update.memory_byte_nanoseconds_remainder > 0)
        }));
        resource_limits_grpc.run_batch_for_test().await;
        assert_eq!(
            registry.applied_memory_byte_nanoseconds(),
            settled_usage,
            "a second batch must not accrue memory after permit release closed the window"
        );

        let recovered_revision = registry.current_limits().monthly_policy_revision + 1;
        let (recovery_started, recovery_release) = registry.delay_next_policy(available_policy);
        let recovery_refresh = {
            let resource_limits_grpc = resource_limits_grpc.clone();
            tokio::spawn(async move { resource_limits_grpc.run_batch_for_test().await })
        };
        tokio::time::timeout(Duration::from_secs(30), recovery_started.acquire())
            .await??
            .forget();
        recovery_release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(30), recovery_refresh).await??;
        assert_eq!(
            resource_limits_grpc
                .initialized_policy_revision_for_test(context.account_id)
                .await,
            Some(recovered_revision),
            "the target account entry must apply the recovered Registry revision"
        );
        executor.resume(&worker_id, false).await?;
        let recovered = tokio::time::timeout(Duration::from_secs(30), invocation).await???;
        let recovered = recovered.into_typed::<Result<String, String>>()?;
        assert_eq!(recovered, Err("ErrorCode::InvalidState".to_string()));
        executor
            .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(30))
            .await?;

        let after_resume = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_eq!(
            count_agent_invocation_pair_since(&after_resume, before_invocation),
            (1, 1),
            "recovery must commit one retained invocation terminal"
        );
        let ended_starts = after_resume
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::End(end) => Some(end.start_index),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let cancelled_starts = after_resume
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Cancelled(cancelled) => Some(cancelled.start_index),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let jump_regions = after_resume
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Jump(jump) => Some(jump.jump.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (function_name, interrupted_start) in [
            ("sockets::types::tcp-socket::receive", receive_start),
            ("sockets::types::tcp-socket::receive-chunk", chunk_start),
        ] {
            let starts = after_resume
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(start)
                        if entry.oplog_index > before_invocation
                            && start.function_name == function_name =>
                    {
                        Some(entry.oplog_index)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(starts.len(), 2, "{function_name} must replay exactly once");
            assert!(!ended_starts.contains(&interrupted_start));
            assert!(!cancelled_starts.contains(&interrupted_start));
            assert!(
                jump_regions
                    .iter()
                    .any(|region| region.contains(interrupted_start)),
                "interrupted {function_name} must be covered by a recovery jump"
            );
            let replay_start = starts
                .into_iter()
                .find(|start| *start != interrupted_start)
                .expect("one replay start must differ from the interrupted start");
            assert!(ended_starts.contains(&replay_start));
            assert!(!cancelled_starts.contains(&replay_start));
            assert!(
                jump_regions
                    .iter()
                    .all(|region| !region.contains(replay_start)),
                "settled replay {function_name} must be outside recovery jumps"
            );
        }
        assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);
    } else {
        let error = tokio::time::timeout(Duration::from_secs(30), invocation)
            .await??
            .expect_err("ephemeral monthly memory exhaustion must fail the invocation");
        assert!(error.to_string().contains("monthly memory exhausted"));
        tokio::time::timeout(Duration::from_secs(30), async {
            while executor
                .concurrent_agent_permit_is_held(&owned_agent_id)
                .await
            {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert!(
            !executor
                .concurrent_agent_permit_is_held(&owned_agent_id)
                .await,
            "ephemeral terminal failure must release the concurrent-agent permit"
        );
    }

    shutdown.cancel();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
#[test_r::tag(group5)]
async fn durable_memory_quota_exhaustion_interrupts_silent_tcp_and_recovers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    memory_quota_exhaustion_interrupts_silent_tcp(
        last_unique_id,
        deps,
        host_api_tests,
        "Networking",
        true,
        false,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
#[test_r::tag(group5)]
async fn ephemeral_memory_quota_exhaustion_interrupts_silent_tcp_with_terminal_error(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    memory_quota_exhaustion_interrupts_silent_tcp(
        last_unique_id,
        deps,
        host_api_tests,
        "EphemeralNetworking",
        false,
        false,
    )
    .await
}

mod tick {
    use super::*;
    use test_r::test;

    #[test]
    #[timeout("2m")]
    async fn durable_memory_quota_monitor_tick_interrupts_silent_tcp_and_recovers(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
        _tracing: &Tracing,
    ) -> anyhow::Result<()> {
        memory_quota_exhaustion_interrupts_silent_tcp(
            last_unique_id,
            deps,
            host_api_tests,
            "Networking",
            true,
            true,
        )
        .await
    }

    #[test]
    #[timeout("2m")]
    async fn ephemeral_memory_quota_monitor_tick_interrupts_silent_tcp_with_terminal_error(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
        _tracing: &Tracing,
    ) -> anyhow::Result<()> {
        memory_quota_exhaustion_interrupts_silent_tcp(
            last_unique_id,
            deps,
            host_api_tests,
            "EphemeralNetworking",
            false,
            true,
        )
        .await
    }
}

test_r::tag_suite!(tick, group5);
