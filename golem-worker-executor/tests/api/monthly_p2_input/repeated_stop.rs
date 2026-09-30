use super::*;
use golem_common::model::Timestamp;
use golem_worker_executor::worker::{MonthlyAcceptanceForTest, MonthlyTimerPollForTest};
use test_r::test;

macro_rules! repeated_stop_case {
    ($name:ident, $mode:ident, $user_stop:expr) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            repeated_stop(
                last_unique_id,
                deps,
                host_api_tests,
                AgentMode::$mode,
                $user_stop,
            )
            .await
        }
    };
}

repeated_stop_case!(
    durable_claimed_monthly_stop_rejects_tick_and_update,
    Durable,
    false
);
repeated_stop_case!(
    ephemeral_claimed_monthly_stop_rejects_tick_and_update,
    Ephemeral,
    false
);
repeated_stop_case!(
    durable_claimed_user_stop_rejects_monthly_tick_and_update,
    Durable,
    true
);
repeated_stop_case!(
    ephemeral_claimed_user_stop_rejects_monthly_tick_and_update,
    Ephemeral,
    true
);

async fn armed_timer(
    polls: &mut tokio::sync::mpsc::UnboundedReceiver<MonthlyTimerPollForTest>,
    now: Duration,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let poll = polls.recv().await.context("monthly timer closed")?;
            if poll.now == now && poll.deadline > now {
                return Ok(());
            }
        }
    })
    .await?
}

async fn next_attempt(
    attempts: &mut tokio::sync::mpsc::UnboundedReceiver<MonthlyAcceptanceForTest>,
) -> anyhow::Result<MonthlyAcceptanceForTest> {
    tokio::time::timeout(Duration::from_secs(10), attempts.recv())
        .await?
        .context("monthly acceptance observer closed")
}

async fn repeated_stop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    mode: AgentMode,
    user_stop: bool,
) -> anyhow::Result<()> {
    let mut policy = Resource::Storage.policy();
    policy.available_memory_gb_seconds = u64::MAX;
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let metering = ResourceUsageMeteringConfig {
        compute: false,
        memory: true,
        filesystem: true,
    };
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
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(&component.id, agent_id!("Networking", "repeated-stop-seed"))
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .unwrap()
        .primary();
    wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while seed.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("Networking", "repeated-stop")
    } else {
        agent_id!("EphemeralNetworking", "repeated-stop")
    };
    let key = IdempotencyKey::fresh();
    let physical = if durable {
        name.clone()
    } else {
        name.clone()
            .with_ephemeral_invocation_phantom(&key)
            .unwrap()
    };
    let id = AgentId::from_agent_id(component.id, &physical).unwrap();
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
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
    let source = ScriptedFilesystemUsageForTest::new(FilesystemUsage::Authoritative {
        allocated_bytes: 0,
        filesystem_objects: 0,
    });
    worker
        .owner_runtime_resources()
        .set_scripted_filesystem_usage_for_test(source.clone());
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let mut phases = worker.observe_p2_native_input_for_test();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
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
                    Operation::Read.method(),
                    data_value!(port),
                )
                .await
        }
    })));
    let mut release_outcome = None;
    let result = async {
        let (mut peer, _) = tokio::time::timeout(Duration::from_secs(30), listener.accept()).await??;
        let phase = pending_phase(&mut phases, Operation::Read, &key).await?;
        armed_timer(&mut polls, Duration::ZERO).await?;
        let initial = worker.current_monthly_proposal_for_test();
        ensure!(initial == phase.runtime && initial.exhaustion.is_none());
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        let mut raw = worker.raw_interrupt_for_test();
        let helper = worker.await_interrupt_for_test();
        let (selected, release) = worker.pause_next_outcome_for_test(true);
        release_outcome = Some(release);
        let (_, driver_entered, release_driver) = worker.pause_next_stop_driver_for_test();
        let mut selected = Some(selected);
        let mut driver_entered = Some(driver_entered);
        let mut release_driver = Some(release_driver);
        let mut receipt = None;
        let mut signal = None;
        if user_stop {
            let kind = InterruptKind::Interrupt(Timestamp::now_utc());
            receipt = worker.set_interrupting(kind).await;
            tokio::time::timeout(Duration::from_secs(10), driver_entered.take().unwrap()).await??;
            release_driver.take().unwrap().send(false).unwrap();
            tokio::time::timeout(Duration::from_secs(10), selected.take().unwrap()).await??;
            ensure!(worker.terminal_stop_claimed_for_test().await);
            ensure!(worker.monthly_stop_for_test().is_none());
            signal = Some(kind);
        }
        source.set(FilesystemUsage::Authoritative { allocated_bytes: 101, filesystem_objects: 1 });
        let mut now = Duration::from_secs(30);
        let mut armed = false;
        clock.advance(now);
        let first = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    biased;
                    attempt = attempts.recv() => return attempt.context("monthly acceptance observer closed"),
                    poll = polls.recv() => {
                        let poll = poll.context("monthly timer closed")?;
                        if poll.now != now || poll.deadline <= now {
                            continue;
                        }
                        armed = true;
                        worker.drain_lifecycle_for_test().await?;
                        tokio::select! {
                            biased;
                            attempt = attempts.recv() => return attempt.context("monthly acceptance observer closed"),
                            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                        }
                        now += Duration::from_secs(30);
                        armed = false;
                        clock.advance(Duration::from_secs(30));
                    }
                }
            }
        }).await??;
        let mut expected = initial;
        expected.exhaustion = Some(Resource::Storage.reason(durable));
        ensure!(first.proposal == expected, "{:?} != {expected:?}", first.proposal);
        let accepted = tokio::time::timeout(Duration::from_secs(10), first.completed).await??;
        ensure!(accepted != user_stop, "monthly storage proposal accepted={accepted} after user_stop={user_stop}");
        if !user_stop {
            tokio::time::timeout(Duration::from_secs(10), driver_entered.take().unwrap()).await??;
            receipt = Some(worker.accepted_stop_receipt_for_test());
            signal = worker.monthly_stop_for_test();
            release_driver.take().unwrap().send(false).unwrap();
            tokio::time::timeout(Duration::from_secs(10), selected.take().unwrap()).await??;
        }
        let signal = signal.context("missing first stop")?;
        ensure!(user_stop || matches!(signal, InterruptKind::Suspend(_)));
        let mut receipt = receipt.context("missing first stop receipt")?;
        ensure!(tokio::time::timeout(Duration::from_secs(10), raw.recv()).await?? == signal);
        ensure!(tokio::time::timeout(Duration::from_secs(10), helper).await? == signal);
        ensure!(tokio::time::timeout(Duration::from_secs(10), worker.await_interrupt_for_test()).await? == signal);
        let expected_monthly = (!user_stop).then_some(signal);
        let expected_reason = (!user_stop).then_some(Resource::Storage.reason(durable));
        ensure!(worker.terminal_stop_claimed_for_test().await);
        ensure!(worker.pending_stop_for_test().await.is_none());
        ensure!(worker.frozen_stop_for_test() == Some(signal));
        ensure!(worker.owner_stop_for_test().await == Some(signal));
        ensure!(worker.monthly_stop_for_test() == expected_monthly);
        ensure!(worker.monthly_stop_reason_for_test() == expected_reason);
        ensure!(worker.monthly_window_active_for_test());
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(!request_is_finished(&invocation));
        ensure!(matches!(receipt.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)));
        info!(?mode, user_stop, ?signal, ?first.proposal, "Selected outcome holds TerminalClaimed with active monthly window and permit");

        // Monitor rearming follows submission. No pending earlier tick may stand in for this tick.
        if !armed {
            armed_timer(&mut polls, now).await?;
        }
        worker.drain_lifecycle_for_test().await?;
        ensure!(attempts.try_recv().is_err());
        now += Duration::from_secs(30);
        clock.advance(Duration::from_secs(30));
        let repeated = next_attempt(&mut attempts).await?;
        ensure!(repeated.proposal == first.proposal);
        let accepted = tokio::time::timeout(Duration::from_secs(10), repeated.completed).await??;
        info!(?mode, user_stop, accepted, retained = ?worker.monthly_stop_for_test(), ?signal, "Second tick at claimed terminal");
        ensure!(!accepted, "second tick accepted a monthly stop after TerminalClaimed");
        armed_timer(&mut polls, now).await?;
        worker.drain_lifecycle_for_test().await?;
        ensure!(attempts.try_recv().is_err());

        // A different exhausted resource must not replace the first retained reason either.
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
        let updated = next_attempt(&mut attempts).await?;
        let mut expected = first.proposal;
        expected.policy_revision += 1;
        expected.exhaustion = Some(Resource::Memory.reason(durable));
        ensure!(updated.proposal == expected, "{:?} != {expected:?}", updated.proposal);
        ensure!(!tokio::time::timeout(Duration::from_secs(10), updated.completed).await??);
        info!(?mode, user_stop, proposal = ?updated.proposal, ?signal, "Changed-resource update rejected after terminal claim");
        worker.drain_lifecycle_for_test().await?;
        ensure!(worker.terminal_stop_claimed_for_test().await);
        ensure!(worker.pending_stop_for_test().await.is_none());
        ensure!(worker.current_monthly_proposal_for_test() == expected);
        ensure!(worker.monthly_stop_for_test() == expected_monthly);
        ensure!(worker.monthly_stop_reason_for_test() == expected_reason);
        ensure!(worker.frozen_stop_for_test() == Some(signal));
        ensure!(worker.owner_stop_for_test().await == Some(signal));
        ensure!(tokio::time::timeout(Duration::from_secs(10), worker.await_interrupt_for_test()).await? == signal);
        ensure!(matches!(raw.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)));
        ensure!(matches!(receipt.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)));
        ensure!(worker.monthly_window_active_for_test());
        ensure!(worker.concurrent_agent_permit_is_held().await);
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        let mut byte = [0];
        ensure!(tokio::time::timeout(Duration::from_millis(20), peer.read(&mut byte)).await.is_err(), "outcome gate must retain the native socket");
        release_outcome.take().unwrap().send(false).unwrap();
        tokio::time::timeout(Duration::from_secs(10), receipt.recv()).await??;
        ensure!(tokio::time::timeout(Duration::from_secs(10), peer.read(&mut byte)).await?? == 0);
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        }).await?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        join_request(&mut refresh, "stop refresh").await?;
        ensure!(worker.unload_succeeded_for_test());
        ensure!(!worker.monthly_window_active_for_test());
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(clock.active_sleeps() == 0 && account.monthly_observer_count_for_test() == 0);
        ensure!(receipt.try_recv().is_err(), "one stop receipt only");
        ensure!(raw.try_recv().is_err(), "one typed publication only");
        ensure!(phase.state() == P2NativeInputStateForTest::Dropped);
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
        let entries = worker.oplog().read_exact(
            OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64(),
        ).await;
        let errors: Vec<_> = entries.values().filter_map(|entry| match entry {
            OplogEntry::Error { error, .. } => Some(error), _ => None,
        }).collect();
        let suspends = entries.values().filter(|entry| matches!(entry, OplogEntry::Suspend { .. })).count();
        let interrupted = entries.values().filter(|entry| matches!(entry, OplogEntry::Interrupted { .. })).count();
        if user_stop {
            ensure!(errors.is_empty() && suspends == 0 && interrupted == 1, "{entries:?}");
            ensure!(join_request(&mut invocation, "invocation result").await?.is_err());
        } else if durable {
            ensure!(errors.is_empty() && suspends == 1 && interrupted == 0, "{entries:?}");
            ensure!(!request_is_finished(&invocation));
        } else {
            ensure!(suspends == 0 && interrupted == 0);
            ensure!(matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)]
                if reason.reason == Resource::Storage.reason(false)), "{errors:?}");
            ensure!(join_request(&mut invocation, "invocation result").await?.is_err());
            // Terminal ephemeral failure must take the archival branch, not a late TryStop.
            tokio::time::timeout(Duration::from_secs(10), async {
                while executor.production_active_agent(&owned).await.is_some() {
                    tokio::task::yield_now().await;
                }
            }).await.context("ephemeral archival removes the stopped owner")?;
        }
        let samples = source.observations();
        clock.advance(Duration::from_secs(30));
        limits.run_batch_for_test().await;
        ensure!(source.observations() == samples);
        ensure!(attempts.try_recv().is_err());
        if durable && !user_stop {
            let mut grant = policy;
            grant.available_durable_storage_byte_seconds = u64::MAX;
            registry.set_policy(grant);
            start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
            join_request(&mut refresh, "policy refresh").await?;
            executor.resume(&id, false).await?;
            let (mut peer, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept()).await??;
            let recovered = pending_phase(&mut phases, Operation::Read, &key).await?;
            ensure!(recovered.runtime.resident_generation > initial.resident_generation);
            ensure!(recovered.runtime.start_attempt != initial.start_attempt);
            ensure!(recovered.runtime.fingerprint == initial.fingerprint);
            peer.write_all(b"x").await?;
            Operation::Read.assert_result(join_request(&mut invocation, "invocation result").await??)?;
            let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (2, 2));
            assert_settled_history(&history)?;
            Operation::Read.assert_result(executor.invoke_and_await_agent_with_key(
                &component, &name, &key, Operation::Read.method(), data_value!(port),
            ).await?)?;
            ensure!(tokio::time::timeout(Duration::from_millis(100), listener.accept()).await.is_err());
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    if let Some(release) = release_outcome {
        let _ = release.send(false);
    }
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
