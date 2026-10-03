use super::*;
use futures::FutureExt;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::AsyncReadExt;

#[test]
#[timeout("2m")]
async fn pending_restart_publishes_only_elected_monthly_cause(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for (durable, restart_first) in [(true, true), (false, true), (true, false), (false, false)] {
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
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let name = if durable {
            agent_id!("Networking", "cause-freeze")
        } else {
            agent_id!("EphemeralNetworking", "cause-freeze")
        };
        let key = IdempotencyKey::fresh();
        let id = if durable {
            executor.start_agent(&component.id, name.clone()).await?
        } else {
            AgentId::from_agent_id(
                component.id,
                &name
                    .clone()
                    .with_ephemeral_invocation_phantom(&key)
                    .unwrap(),
            )
            .unwrap()
        };
        let owned = OwnedAgentId::new(context.default_environment_id, &id);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let (connected, connection) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            connected.send(()).unwrap();
            let mut byte = [0];
            assert!(matches!(socket.read(&mut byte).await, Ok(0) | Err(_)));
        });
        let invocation = tokio::spawn({
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
                        "tcp_collect_p3",
                        data_value!(port),
                    )
                    .await
            }
        });
        connection.await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                executor.commit_oplog(&id).await?;
                let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                if let Some(start) = entries.iter().find_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(start)
                        if start.function_name == "sockets::types::tcp-socket::receive-chunk" =>
                    {
                        Some(entry.oplog_index)
                    }
                    _ => None,
                }) {
                    assert!(!entries.iter().any(|entry| match &entry.entry {
                        PublicOplogEntry::End(end) => end.start_index == start,
                        PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
                        _ => false,
                    }));
                    assert_eq!(
                        count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL),
                        (2, 1)
                    );
                    break Ok::<_, anyhow::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        let active = executor.production_active_agent(&owned).await.unwrap();
        let worker = active.primary();
        let generation = worker.resident_generation_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let mut helper = worker.await_interrupt_for_test();
        let (_, entered, release) = worker.pause_next_stop_driver_for_test();
        let (failure_selected, release_failure) = worker.pause_next_outcome_for_test(false);
        let mut entered = Some(entered);
        let mut receipt = if restart_first {
            let receipt = worker
                .set_interrupting(InterruptKind::Restart)
                .await
                .unwrap();
            entered.take().unwrap().await?;
            Some(receipt)
        } else {
            None
        };
        assert!(
            matches!(
                raw.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "provisional Restart escaped before freeze"
        );
        assert!(helper.as_mut().now_or_never().is_none());
        let late = worker.await_interrupt_for_test();
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        let refresh = tokio::spawn({
            let limits = limits.clone();
            async move { limits.run_batch_for_test().await }
        });
        let monthly = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(kind) = worker.monthly_stop_for_test() {
                    break kind;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        if let Some(entered) = entered {
            entered.await?;
        }
        assert!(matches!(monthly, InterruptKind::Suspend(_)));
        assert!(
            worker
                .set_interrupting(InterruptKind::Restart)
                .await
                .is_none()
        );
        assert!(
            worker
                .set_interrupting(InterruptKind::Interrupt(
                    golem_common::model::Timestamp::now_utc()
                ))
                .await
                .is_none()
        );
        assert_eq!(worker.resident_generation_for_test(), generation);
        assert!(worker.concurrent_agent_permit_is_held().await);
        assert!(matches!(
            raw.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert!(helper.as_mut().now_or_never().is_none());
        if let Some(receipt) = &mut receipt {
            assert!(matches!(
                receipt.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
        }
        release.send(false).unwrap();
        assert_eq!(raw.recv().await?, monthly);
        assert_eq!(helper.await, monthly);
        assert_eq!(late.await, monthly);
        failure_selected.await?;
        assert_eq!(worker.await_interrupt_for_test().await, monthly);
        assert_eq!(worker.owner_stop_for_test().await, Some(monthly));
        assert_eq!(worker.frozen_stop_for_test(), Some(monthly));
        release_failure.send(false).unwrap();
        if let Some(receipt) = &mut receipt {
            receipt.recv().await?;
        }
        worker.join_accepted_stops_for_test().await?;
        server.await?;
        refresh.await?;
        if durable {
            executor
                .wait_for_status(&id, AgentStatus::Suspended, Duration::from_secs(10))
                .await?;
            assert!(!invocation.is_finished());
            registry.set_policy(policy);
            limits.run_batch_for_test().await;
            executor.resume(&id, false).await?;
            let result = invocation.await??.into_typed::<Result<String, String>>()?;
            assert_eq!(result, Err("ErrorCode::InvalidState".to_string()));
            let oplog = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            assert_eq!(
                count_agent_invocation_pair_since(&oplog, OplogIndex::INITIAL),
                (2, 2)
            );
        } else {
            let error = invocation
                .await?
                .expect_err("monthly exhaustion must fail ephemeral invocation");
            assert!(
                error.to_string().contains("monthly memory exhausted"),
                "{error}"
            );
            let entries = worker
                .oplog()
                .read_exact(
                    OplogIndex::INITIAL,
                    worker.oplog().current_oplog_index().await.as_u64(),
                )
                .await;
            assert_eq!(
                entries
                    .values()
                    .filter(|entry| matches!(
                        entry,
                        golem_common::model::oplog::OplogEntry::Error {
                            error: golem_common::model::oplog::AgentError::EphemeralCannotSuspend(
                                _
                            ),
                            ..
                        }
                    ))
                    .count(),
                1
            );
            assert_eq!(
                entries
                    .values()
                    .filter(|entry| matches!(
                        entry,
                        golem_common::model::oplog::OplogEntry::AgentInvocationFinished { .. }
                    ))
                    .count(),
                1,
                "only initialization may finish after monthly failure"
            );
        }
        if let Some(receipt) = &mut receipt {
            assert!(matches!(
                receipt.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty
                    | tokio::sync::broadcast::error::TryRecvError::Closed)
            ));
        }
        shutdown.cancel();
    }
    Ok(())
}
