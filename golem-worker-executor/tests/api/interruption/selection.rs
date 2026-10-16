use super::*;
use anyhow::Context as _;
use futures::FutureExt;
use golem_common::model::Timestamp;
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::HasOplog;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
#[timeout("2m")]
async fn pending_restart_publishes_only_elected_quota_cause(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for durable in [true, false] {
        let policy = available_quota_policy();
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
                config.agent_sockets.enabled = true;
                config.resource_usage_metering = metering;
            }),
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
        let mut attempts = worker.observe_monthly_acceptance_for_test();
        let (failure_selected, release_failure) =
            executor.gate_next_invocation_outcome_append(&id).await;
        assert!(
            matches!(
                raw.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "interrupt appeared before capacity exhaustion"
        );
        assert!(helper.as_mut().now_or_never().is_none());
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        let refresh = tokio::spawn({
            let limits = limits.clone();
            async move { limits.run_batch_for_test().await }
        });
        let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
            .await?
            .context("monthly proposal for pending TCP")?;
        assert!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        let quota_stop = worker.monthly_stop_for_test().expect("accepted quota stop");
        assert!(matches!(quota_stop, InterruptKind::Suspend(_)));
        // Restart supersession and unpublished rejection are actual-body proofs. Storage
        // backpressure retains this real result and its receipt after typed publication.
        tokio::time::timeout(Duration::from_secs(10), failure_selected).await??;
        let late = worker.await_interrupt_for_test();
        let mut receipt = worker.accepted_stop_receipt_for_test();
        assert_eq!(worker.resident_generation_for_test(), generation);
        assert!(worker.concurrent_agent_permit_is_held().await);
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert_eq!(raw.recv().await?, quota_stop);
        assert_eq!(helper.await, quota_stop);
        assert_eq!(late.await, quota_stop);
        assert_eq!(worker.await_interrupt_for_test().await, quota_stop);
        assert_eq!(worker.owner_stop_for_test().await, Some(quota_stop));
        assert_eq!(worker.frozen_stop_for_test(), Some(quota_stop));
        release_failure.send(false).unwrap();
        receipt.recv().await?;
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
            assert_eq!(result, Err("ErrorCode::ConnectionBroken".to_string()));
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
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed)
        ));
        shutdown.cancel();
    }
    Ok(())
}

#[test]
#[timeout("2m")]
async fn quota_before_guest_and_user_stop_after_selection_keep_one_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for after_selection in [false, true] {
        let policy = available_quota_policy();
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
        let owned = OwnedAgentId::new(context.default_environment_id, &id);
        let mut started = if after_selection {
            None
        } else {
            Some(executor.gate_next_invocation_started(&owned).await?)
        };
        let mut selected_write = if after_selection {
            Some(executor.gate_next_invocation_outcome_append(&id).await)
        } else {
            None
        };
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
        if let Some(started) = &mut started {
            started.entered().await;
        } else {
            (&mut selected_write.as_mut().unwrap().0).await?;
        }
        let mut raw = after_selection.then(|| worker.raw_interrupt_for_test());
        let receipt;
        let expected;
        let mut refresh = None;
        if after_selection {
            expected = InterruptKind::Interrupt(Timestamp::now_utc());
            let mut accepted_receipt = worker
                .set_interrupting(expected)
                .await?
                .context("late user stop has no receipt")?;
            assert_eq!(worker.pending_stop_for_test().await, Some(expected));
            tokio::time::timeout(
                Duration::from_secs(10),
                worker.join_accepted_stops_for_test(),
            )
            .await??;
            assert!(!invocation.is_finished());
            assert!(matches!(
                accepted_receipt.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
            receipt = Some(accepted_receipt);
        } else {
            let runtime = worker.current_monthly_proposal_for_test();
            let mut acceptances = worker.observe_monthly_acceptance_for_test();
            let mut exhausted = policy.clone();
            exhausted.available_memory_gb_seconds = 0;
            registry.set_policy(exhausted);
            refresh = Some(tokio::spawn({
                let limits = limits.clone();
                async move { limits.run_batch_for_test().await }
            }));
            let acceptance = acceptances.recv().await.expect("monthly capacity proposal");
            assert_eq!(acceptance.proposal.fingerprint, runtime.fingerprint);
            assert_eq!(acceptance.proposal.start_attempt, runtime.start_attempt);
            assert_eq!(
                acceptance.proposal.resident_generation,
                runtime.resident_generation
            );
            assert_eq!(acceptance.proposal.window_identity, runtime.window_identity);
            assert_eq!(acceptance.proposal.period, runtime.period);
            assert_eq!(
                acceptance.proposal.exhaustion,
                Some("monthly memory exhausted")
            );
            assert!(acceptance.completed.await?);
            worker.join_accepted_stops_for_test().await?;
            expected = worker
                .monthly_stop_for_test()
                .expect("accepted monthly stop");
            assert!(matches!(expected, InterruptKind::Suspend(_)));
            assert_eq!(Some(expected), worker.monthly_stop_for_test());
            assert_eq!(
                worker.monthly_stop_reason_for_test(),
                Some("monthly memory exhausted")
            );
            assert_eq!(worker.frozen_stop_for_test(), Some(expected));
            assert_eq!(worker.pending_stop_for_test().await, Some(expected));
            assert_eq!(worker.owner_stop_for_test().await, Some(expected));
            // Started precedes Running. This stop exposes driver completion, not a signal receipt.
            receipt = None;
        }
        if let Some(started) = started {
            started.release();
        } else {
            selected_write.take().unwrap().1.send(false).unwrap();
        }
        if let Some(raw) = &mut raw {
            assert_eq!(raw.recv().await?, expected);
        }
        if let Some(mut receipt) = receipt {
            receipt.recv().await?;
            assert!(matches!(
                receipt.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty
                    | tokio::sync::broadcast::error::TryRecvError::Closed)
            ));
        }
        if !after_selection {
            executor
                .wait_for_status(&id, AgentStatus::Suspended, Duration::from_secs(10))
                .await?;
            assert!(!invocation.is_finished());
            worker.await_ready_to_process_commands().await?;
            worker.retained_cleanup_for_test().await?;
            assert!(!worker.is_loaded().await);
            assert!(!worker.concurrent_agent_permit_is_held().await);
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
        executor.resume(&id, false).await?;
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &name, "increment", data_value!())
                .await?
                .into_typed::<u32>()?,
            2,
            "fresh work must continue from the single retained effect"
        );
        assert_eq!(
            count_agent_invocation_pair_since(
                &executor.get_oplog(&id, OplogIndex::INITIAL).await?,
                OplogIndex::INITIAL
            ),
            (3, 3)
        );
        shutdown.cancel();
    }
    Ok(())
}

#[test]
#[timeout("2m")]
async fn quota_stop_preserves_selected_independent_guest_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    {
        let policy = available_quota_policy();
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
        let name = agent_id!("FailingCounter", "monthly-selected-failure");
        let id = executor.start_agent(&component.id, name.clone()).await?;
        wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
        let worker = executor
            .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
            .await
            .unwrap()
            .primary();
        let (at_outcome, release_outcome) = executor.gate_next_invocation_outcome_append(&id).await;
        let mut attempts = worker.observe_monthly_acceptance_for_test();
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
        let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
            .await?
            .context("monthly proposal behind selected guest-failure writer")?;
        assert_eq!(attempt.proposal.resident_generation, generation);
        assert!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        assert!(worker.monthly_stop_for_test().is_some());
        let mut receipt = worker.accepted_stop_receipt_for_test();
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
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
async fn durable_quota_stop_and_invocation_deadline_selection(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    deadline_selection(last_unique_id, deps, host_api_tests, true).await
}

#[test]
#[timeout("2m")]
async fn ephemeral_quota_stop_and_invocation_deadline_selection(
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
        let policy = available_quota_policy();
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
                config.agent_sockets.enabled = true;
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
        let mut attempts = worker.observe_monthly_acceptance_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let (at_outcome, release_outcome) = executor.gate_next_invocation_outcome_append(&id).await;
        if timeout_first {
            worker.expire_invocation_deadline_for_test();
            let deadline_interrupt =
                tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
            assert!(
                matches!(deadline_interrupt, InterruptKind::Interrupt(_)),
                "explicit deadline expiry must emit one Interrupt: {deadline_interrupt:?}"
            );
            assert!(matches!(
                raw.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
            assert_eq!(worker.monthly_stop_for_test(), None);
            assert_eq!(worker.frozen_stop_for_test(), None);
            assert_eq!(worker.pending_stop_for_test().await, None);
            assert!(!worker.terminal_stop_claimed_for_test().await);
        }
        if success_first {
            socket.shutdown().await?;
        }
        let mut at_outcome = Some(at_outcome);
        if selected_first {
            at_outcome.take().unwrap().await?;
        }
        if timeout_first {
            assert!(matches!(
                &*worker
                    .owner_runtime_resources()
                    .execution_status()
                    .read()
                    .unwrap(),
                golem_worker_executor::model::ExecutionStatus::Running { .. }
            ));
            assert_eq!(worker.monthly_stop_for_test(), None);
            assert_eq!(worker.frozen_stop_for_test(), None);
            assert_eq!(worker.pending_stop_for_test().await, None);
            assert!(!worker.terminal_stop_claimed_for_test().await);
        }
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        let refresh = tokio::spawn({
            let limits = limits.clone();
            async move { limits.run_batch_for_test().await }
        });
        let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
            .await?
            .context("monthly proposal in timer selection integration")?;
        assert!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        let quota_stop = worker
            .monthly_stop_for_test()
            .expect("monthly stop accepted");
        assert!(matches!(quota_stop, InterruptKind::Suspend(_)));
        let mut receipt = worker.accepted_stop_receipt_for_test();
        if !selected_first {
            tokio::time::timeout(Duration::from_secs(10), at_outcome.take().unwrap()).await??;
        }
        assert!(worker.concurrent_agent_permit_is_held().await);
        assert!(matches!(
            receipt.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        // The callback retains its actual append, not a separate writer receipt. Stop signalling
        // may overlap persistence, but invocation result notification still follows commit.
        if selected_first {
            tokio::time::timeout(Duration::from_secs(10), async {
                while worker.frozen_stop_for_test().is_none() {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            assert!(matches!(
                receipt.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
        }
        if !selected_first {
            assert_eq!(worker.frozen_stop_for_test(), Some(quota_stop));
        }
        release_outcome.send(false).unwrap();
        tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
        assert_eq!(raw.recv().await?, quota_stop);
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
            assert_eq!(result, Err("ErrorCode::ConnectionBroken".to_string()));
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
