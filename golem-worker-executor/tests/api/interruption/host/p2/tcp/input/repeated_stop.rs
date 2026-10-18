use super::*;
use crate::monthly_acceptance::Acceptance;
use golem_common::model::Timestamp;
use test_r::test;

macro_rules! repeated_stop_case {
    ($name:ident, $mode:ident, $user_stop:expr) => {
        #[cfg(target_os = "linux")]
        #[test]
        #[ignore = "requires the privileged managed XFS test runner"]
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
    durable_claimed_quota_stop_rejects_tick_and_update,
    Durable,
    false
);
repeated_stop_case!(
    ephemeral_claimed_quota_stop_rejects_tick_and_update,
    Ephemeral,
    false
);
repeated_stop_case!(
    durable_claimed_user_stop_rejects_quota_monitor_tick_and_update,
    Durable,
    true
);
repeated_stop_case!(
    ephemeral_claimed_user_stop_rejects_quota_monitor_tick_and_update,
    Ephemeral,
    true
);

async fn next_attempt(
    attempts: &mut tokio::sync::mpsc::UnboundedReceiver<Acceptance>,
) -> anyhow::Result<Acceptance> {
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
    policy.available_durable_storage_byte_seconds = 1;
    policy.available_ephemeral_storage_byte_seconds = 1;
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
    let filesystem = managed_filesystem_config(metering)?;
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| {
            config.agent_sockets.enabled = true;
            config.resource_usage_metering = metering;
            if let Some(filesystem) = &filesystem {
                config.filesystem_storage.mode = filesystem.clone();
            }
        }),
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
    let mut attempts =
        crate::monthly_acceptance::observe(worker.get_initial_worker_metadata().fingerprint);
    let mut phases = crate::native_waits::input(worker.get_initial_worker_metadata().fingerprint);
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
        let initial = worker.current_monthly_proposal_for_test();
        ensure!(initial == phase.runtime && initial.exhaustion.is_none());
        let acquisitions = worker.permit_acquisitions_for_test();
        limits.initialize_account(context.account_id).await?;
        ensure!(worker.monthly_window_active_for_test());
        let mut raw = worker.raw_interrupt_for_test();
        let helper = worker.await_interrupt_for_test();
        let (selected, release) = executor.gate_next_invocation_outcome_append(&id).await;
        release_outcome = Some(release);
        let mut selected = Some(selected);
        let mut receipt = None;
        let mut signal = None;
        if user_stop {
            let kind = InterruptKind::Interrupt(Timestamp::now_utc());
            receipt = worker.set_interrupting(kind).await?;
            tokio::time::timeout(Duration::from_secs(10), selected.take().unwrap()).await??;
            signal = Some(kind);
        }
        allocate_storage(&owned).await?;
        // Let the real monitor check this allocation. The existing whole-test timeout bounds
        // both ordinary cadence waits; no private clock or ordering retry is involved.
        let first = attempts.recv().await.context("monthly acceptance observer closed")?;
        let mut expected = initial;
        expected.exhaustion = Some(Resource::Storage.reason(durable));
        ensure!(first.proposal == expected, "{:?} != {expected:?}", first.proposal);
        let accepted = first.accepted;
        ensure!(accepted != user_stop, "monthly storage proposal accepted={accepted} after user_stop={user_stop}");
        if !user_stop {
            receipt = Some(published_interrupt_receipt(worker.as_ref()).await?);
            tokio::time::timeout(Duration::from_secs(10), selected.take().unwrap()).await??;
        }
        let observed = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        if let Some(requested) = signal { ensure!(observed == requested); }
        let signal = observed;
        ensure!(user_stop || matches!(signal, InterruptKind::Suspend(_)));
        let mut receipt = receipt.context("missing first stop receipt")?;
        ensure!(tokio::time::timeout(Duration::from_secs(10), helper).await? == signal);
        ensure!(tokio::time::timeout(Duration::from_secs(10), worker.await_interrupt_for_test()).await? == signal);
        ensure!(worker.selected_owner_failure_for_test().await == Some(ToolOwnerFailureMetadata::Lifecycle));
        ensure!(worker.monthly_window_active_for_test());
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(!request_is_finished(&invocation));
        ensure!(matches!(receipt.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)));
        info!(?mode, user_stop, ?signal, ?first.proposal, "Selected outcome holds TerminalClaimed with active monthly window and permit");

        ensure!(attempts.try_recv().is_err());
        let repeated = attempts.recv().await.context("repeated monthly acceptance closed")?;
        ensure!(repeated.proposal == first.proposal);
        let accepted = repeated.accepted;
        info!(?mode, user_stop, accepted, ?signal, "Second tick at claimed terminal");
        ensure!(!accepted, "second tick accepted a monthly stop after TerminalClaimed");
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
        ensure!(!updated.accepted);
        info!(?mode, user_stop, proposal = ?updated.proposal, ?signal, "Changed-resource update rejected after terminal claim");
        ensure!(worker.current_monthly_proposal_for_test() == expected);
        ensure!(worker.selected_owner_failure_for_test().await == Some(ToolOwnerFailureMetadata::Lifecycle));
        ensure!(tokio::time::timeout(Duration::from_secs(10), worker.await_interrupt_for_test()).await? == signal);
        ensure!(matches!(raw.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Closed)));
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
        ensure!(receipt.try_recv().is_err(), "one stop receipt only");
        ensure!(raw.try_recv().is_err(), "one typed publication only");
        ensure!(phase.state() == WaitState::Dropped);
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
        let entries = worker.oplog().read_exact(
            OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64(),
        ).await?;
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
        limits.run_batch_for_test().await;
        assert_storage_deleted(&owned)?;
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
