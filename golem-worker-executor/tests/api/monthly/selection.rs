use super::*;
use futures::FutureExt;
use golem_common::model::Timestamp;
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn available_monthly_policy() -> MonthlyResourcePolicy {
    MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: u64::MAX,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    }
}

#[test]
#[timeout("2m")]
async fn pending_restart_publishes_only_elected_monthly_cause(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for (durable, restart_first) in [(true, true), (false, true), (true, false), (false, false)] {
        let policy = available_monthly_policy();
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
                .await?
                .expect("accepted Restart");
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
                .await?
                .is_none()
        );
        assert!(
            worker
                .set_interrupting(InterruptKind::Interrupt(
                    golem_common::model::Timestamp::now_utc()
                ))
                .await?
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

#[test]
#[timeout("2m")]
async fn monthly_before_selection_and_user_stop_after_selection_keep_one_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for after_selection in [false, true] {
        let policy = available_monthly_policy();
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
            .component_dep(&context.default_environment_id, agent_counters)
            .store()
            .await?;
        let name = agent_id!("Counter", format!("outcome-cutoff-{after_selection}"));
        let id = executor.start_agent(&component.id, name.clone()).await?;
        wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
        let worker = executor
            .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
            .await
            .unwrap()
            .primary();
        let (selected, release) = worker.pause_next_outcome_for_test(after_selection);
        let key = IdempotencyKey::fresh();
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
                        "increment",
                        data_value!(),
                    )
                    .await
            }
        });
        selected.await?;
        let mut raw = worker.raw_interrupt_for_test();
        let helper = worker.await_interrupt_for_test();
        let receipt;
        let expected;
        let mut refresh = None;
        if after_selection {
            expected = InterruptKind::Interrupt(Timestamp::now_utc());
            receipt = worker.set_interrupting(expected).await?;
            tokio::time::timeout(Duration::from_secs(5), async {
                while worker.frozen_stop_for_test().is_none() {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            assert!(
                matches!(
                    raw.try_recv(),
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                ),
                "late stop must wait for the selected result writer"
            );
        } else {
            let mut exhausted = policy.clone();
            exhausted.available_memory_gb_seconds = 0;
            registry.set_policy(exhausted);
            refresh = Some(tokio::spawn({
                let limits = limits.clone();
                async move { limits.run_batch_for_test().await }
            }));
            expected = tokio::time::timeout(Duration::from_secs(5), helper).await?;
            assert_eq!(Some(expected), worker.monthly_stop_for_test());
            receipt = None;
        }
        release.send(false).unwrap();
        assert_eq!(raw.recv().await?, expected);
        if let Some(mut receipt) = receipt {
            receipt.recv().await?;
        }
        if !after_selection {
            executor
                .wait_for_status(&id, AgentStatus::Suspended, Duration::from_secs(10))
                .await?;
            assert!(!invocation.is_finished());
            refresh.take().unwrap().await?;
            let oplog = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            assert_eq!(
                count_agent_invocation_pair_since(&oplog, OplogIndex::INITIAL),
                (2, 1)
            );
            registry.set_policy(policy);
            limits.run_batch_for_test().await;
            executor.resume(&id, false).await?;
        }
        let result = invocation.await??;
        assert_eq!(
            result.into_return_value(),
            Some(golem_common::schema::SchemaValue::U32(1))
        );
        worker.join_accepted_stops_for_test().await?;
        let oplog = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_eq!(
            count_agent_invocation_pair_since(&oplog, OplogIndex::INITIAL),
            (2, 2)
        );
        let same_result = executor
            .invoke_and_await_agent_with_key(&component, &name, &key, "increment", data_value!())
            .await?;
        assert_eq!(
            same_result.into_return_value(),
            Some(golem_common::schema::SchemaValue::U32(1))
        );
        let again = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_eq!(
            count_agent_invocation_pair_since(&again, OplogIndex::INITIAL),
            (2, 2)
        );
        shutdown.cancel();
    }
    Ok(())
}

#[test]
#[timeout("2m")]
async fn monthly_stop_preserves_independent_guest_failure_on_both_sides_of_selection(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for after_selection in [false, true] {
        let policy = available_monthly_policy();
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
            .component_dep(&context.default_environment_id, agent_counters)
            .store()
            .await?;
        let name = agent_id!(
            "FailingCounter",
            format!("monthly-failure-{after_selection}")
        );
        let id = executor.start_agent(&component.id, name.clone()).await?;
        wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
        let worker = executor
            .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
            .await
            .unwrap()
            .primary();
        let (at_outcome, release_outcome) = worker.pause_next_outcome_for_test(after_selection);
        let (_, driver_entered, release_driver) = worker.pause_next_stop_driver_for_test();
        let key = IdempotencyKey::fresh();
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
                        "add",
                        data_value!(11u64),
                    )
                    .await
            }
        });
        at_outcome.await?;
        let acquisitions = worker.permit_acquisitions_for_test();
        let generation = worker.resident_generation_for_test();
        let mut exhausted = policy;
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        let refresh = tokio::spawn({
            let limits = limits.clone();
            async move { limits.run_batch_for_test().await }
        });
        driver_entered.await?;
        assert!(worker.monthly_stop_for_test().is_some());
        let mut receipt = worker.accepted_stop_receipt_for_test();
        release_driver.send(false).unwrap();
        release_outcome.send(false).unwrap();
        let error = invocation
            .await?
            .expect_err("genuine guest trap must survive quota stop");
        assert!(!error.to_string().contains("monthly"), "{error}");
        tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
        worker.join_accepted_stops_for_test().await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        worker.retained_cleanup_for_test().await?;
        assert_eq!(worker.permit_acquisitions_for_test(), acquisitions);
        assert_eq!(worker.resident_generation_for_test(), generation);
        refresh.await?;
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
        assert!(
            matches!(errors.as_slice(), [AgentError::DeterministicTrap(_)]),
            "{errors:?}"
        );
        assert_eq!(
            entries
                .values()
                .filter(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
                .count(),
            1
        );
        assert_eq!(
            entries
                .values()
                .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
                .count(),
            0
        );
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed)
        ));
        assert_eq!(
            limits
                .initialize_account(context.account_id)
                .await?
                .monthly_observer_count_for_test(),
            0
        );
        shutdown.cancel();
    }
    Ok(())
}

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
        let policy = available_monthly_policy();
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
