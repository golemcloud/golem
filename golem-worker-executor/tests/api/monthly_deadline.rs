use super::*;
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
#[timeout("2m")]
async fn durable_monthly_stop_and_invocation_deadline_selection(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    deadline_selection(last_unique_id, deps, host_api_tests, true).await
}

#[test]
#[timeout("2m")]
async fn ephemeral_monthly_stop_and_invocation_deadline_selection(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    deadline_selection(last_unique_id, deps, host_api_tests, false).await
}

async fn deadline_selection(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    durable: bool,
) -> anyhow::Result<()> {
    for (timeout_first, success_first) in [(false, false), (true, false), (false, true)] {
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
            Arc::new(move |config| {
                config.resource_usage_metering = metering;
                config.limits.max_invocation_duration = Some(Duration::from_secs(3600));
                config.retry.max_attempts = 1;
            }),
        )
        .await?;
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let name = if durable {
            agent_id!("Networking", format!("deadline-cutoff-{timeout_first}"))
        } else {
            agent_id!(
                "EphemeralNetworking",
                format!("deadline-cutoff-{timeout_first}")
            )
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
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
        let (mut socket, _) = listener.accept().await?;
        let worker = executor
            .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
            .await
            .unwrap()
            .primary();
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
                    break Ok::<_, anyhow::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        let generation = worker.resident_generation_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let selected_first = timeout_first || success_first;
        let (at_outcome, release_outcome) = worker.pause_next_outcome_for_test(selected_first);
        let (_, driver_entered, release_driver) = worker.pause_next_stop_driver_for_test();
        if timeout_first {
            worker.expire_invocation_deadline_for_test();
        }
        if success_first {
            socket.shutdown().await?;
        }
        let mut at_outcome = Some(at_outcome);
        if selected_first {
            at_outcome.take().unwrap().await?;
        }
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        let refresh = tokio::spawn({
            let limits = limits.clone();
            async move { limits.run_batch_for_test().await }
        });
        driver_entered.await?;
        let monthly = worker
            .monthly_stop_for_test()
            .expect("monthly stop accepted");
        assert!(matches!(monthly, InterruptKind::Suspend(_)));
        assert_eq!(worker.frozen_stop_for_test(), None);
        let mut receipt = worker.accepted_stop_receipt_for_test();
        if !selected_first {
            worker.expire_invocation_deadline_for_test();
            at_outcome.take().unwrap().await?;
        }
        assert!(worker.concurrent_agent_permit_is_held().await);
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        // The real guest call reached the outcome cutoff without a published monthly cause.
        assert_eq!(worker.frozen_stop_for_test(), None);
        let mut raw = worker.raw_interrupt_for_test();
        release_driver.send(false).unwrap();
        if selected_first {
            tokio::time::timeout(Duration::from_secs(10), async {
                while worker.frozen_stop_for_test().is_none() {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            assert!(matches!(
                raw.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
            assert!(matches!(
                receipt.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
        }
        release_outcome.send(false).unwrap();
        tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
        assert_eq!(raw.recv().await?, monthly);
        worker.join_accepted_stops_for_test().await?;
        refresh.await?;
        let mut byte = [0];
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(10), socket.read(&mut byte)).await?,
            Ok(0) | Err(_)
        ));
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        worker.retained_cleanup_for_test().await?;
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
            .filter_map(|entry| match entry {
                OplogEntry::Error { error, .. } => Some(error),
                _ => None,
            })
            .collect();
        let suspends = entries
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
            .count();
        assert_eq!(
            entries
                .values()
                .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
                .count(),
            if success_first { 2 } else { 1 }
        );
        if success_first {
            assert!(
                errors.is_empty(),
                "late monthly stop overwrote success: {errors:?}"
            );
            let result = invocation.await??.into_typed::<Result<String, String>>()?;
            assert_eq!(result, Ok(String::new()));
            let repeated = executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    "tcp_collect_p3",
                    data_value!(port),
                )
                .await?
                .into_typed::<Result<String, String>>()?;
            assert_eq!(repeated, result);
            let after = worker
                .oplog()
                .read_exact(
                    OplogIndex::INITIAL,
                    worker.oplog().current_oplog_index().await.as_u64(),
                )
                .await;
            assert_eq!(
                after
                    .values()
                    .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
                    .count(),
                2
            );
        } else if timeout_first {
            assert!(
                matches!(errors.as_slice(), [AgentError::InternalError(_)]),
                "{errors:?}"
            );
            assert_eq!(suspends, 0);
            assert!(invocation.await?.is_err());
        } else if durable {
            assert!(
                errors.is_empty(),
                "accepted monthly stop lost to deadline: {errors:?}"
            );
            assert_eq!(suspends, 1);
            assert!(!invocation.is_finished());
            registry.set_policy(policy);
            limits.run_batch_for_test().await;
            executor.resume(&id, false).await?;
            let result = invocation.await??.into_typed::<Result<String, String>>()?;
            assert_eq!(result, Err("ErrorCode::InvalidState".to_string()));
        } else {
            assert!(
                matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(_)]),
                "accepted monthly stop lost to deadline: {errors:?}"
            );
            assert_eq!(suspends, 0);
            assert!(invocation.await?.is_err());
        }
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed)
        ));
        shutdown.cancel();
    }
    Ok(())
}
