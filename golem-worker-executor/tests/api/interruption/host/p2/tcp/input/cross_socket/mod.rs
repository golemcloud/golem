mod completion;

use super::*;
use test_r::test;

macro_rules! cross_case {
    ($name:ident, $resource:ident, $mode:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            cross_socket_pending(
                last_unique_id,
                deps,
                host_api_tests,
                Resource::$resource,
                AgentMode::$mode,
                false,
            )
            .await
        }
    };
}

cross_case!(durable_p2_cross_socket_splice_memory_quota, Memory, Durable);
cross_case!(
    ephemeral_p2_cross_socket_splice_memory_quota,
    Memory,
    Ephemeral
);
cross_case!(
    durable_p2_cross_socket_splice_compute_quota_prepaid,
    Compute,
    Durable
);
cross_case!(
    ephemeral_p2_cross_socket_splice_compute_quota_prepaid,
    Compute,
    Ephemeral
);
cross_case!(
    durable_p2_cross_socket_splice_scripted_storage_quota,
    Storage,
    Durable
);
cross_case!(
    ephemeral_p2_cross_socket_splice_scripted_storage_quota,
    Storage,
    Ephemeral
);

#[test]
#[timeout("2m")]
async fn durable_p2_cross_socket_splice_stop_before_subscription(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    cross_socket_pending(
        last_unique_id,
        deps,
        host_api_tests,
        Resource::Memory,
        AgentMode::Durable,
        true,
    )
    .await
}

const METHOD: &str = "tcp_cross_socket_blocking_splice_p2";

async fn peers(
    source: &tokio::net::TcpListener,
    destination: &tokio::net::TcpListener,
) -> anyhow::Result<(tokio::net::TcpStream, tokio::net::TcpStream)> {
    let (input, _) = tokio::time::timeout(Duration::from_secs(10), source.accept()).await??;
    let (output, _) = tokio::time::timeout(Duration::from_secs(10), destination.accept()).await??;
    Ok((input, output))
}

async fn echo(
    input: &mut tokio::net::TcpStream,
    output: &mut tokio::net::TcpStream,
) -> anyhow::Result<()> {
    input.write_all(b"x").await?;
    let mut byte = [0];
    tokio::time::timeout(Duration::from_secs(10), output.read_exact(&mut byte)).await??;
    ensure!(byte == *b"x", "source A's byte must reach destination B");
    ensure!(
        tokio::time::timeout(Duration::from_secs(10), input.read(&mut byte)).await?? == 0,
        "A must not receive B's output"
    );
    ensure!(
        tokio::time::timeout(Duration::from_secs(10), output.read(&mut byte)).await?? == 0,
        "B must close after completion"
    );
    Ok(())
}

async fn cross_socket_pending(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    resource: Resource,
    mode: AgentMode,
    pre_subscription: bool,
) -> anyhow::Result<()> {
    let policy = resource.policy();
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let metering = resource.metering();
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
            config.limits.fuel_to_borrow = FUEL_BUDGET;
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(
            &component.id,
            agent_id!("Networking", "p2-cross-splice-seed"),
        )
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
    .await
    .context("seed permit release")?;
    if resource == Resource::Compute {
        limits.run_batch_for_test().await;
    }
    if resource == Resource::Memory {
        // Separate the seed's sub-unit remainder from the target's policy revision.
        limits.run_batch_for_test().await;
        registry.set_policy(policy.clone());
        start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
        join_request(&mut refresh, "policy refresh").await?;
        limits.run_batch_for_test().await;
    }
    let baseline_updates = registry.applied_updates().len();
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("Networking", "p2-cross-socket-splice")
    } else {
        agent_id!("EphemeralNetworking", "p2-cross-socket-splice")
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
    let source_usage = ScriptedFilesystemUsageForTest::new(FilesystemUsage::Authoritative {
        allocated_bytes: 0,
        filesystem_objects: 0,
    });
    if resource == Resource::Storage {
        worker
            .owner_runtime_resources()
            .set_scripted_filesystem_usage_for_test(source_usage.clone());
    }
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let mut phases = worker.observe_p2_native_input_for_test();
    let mut pre_subscription_control = pre_subscription
        .then(|| worker.pause_next_p2_splice_pre_subscription_for_test(key.clone()));
    let source = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let destination = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let source_port = source.local_addr()?.port();
    let destination_port = destination.local_addr()?.port();
    ensure!(source_port != destination_port);
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
        let (phase, runtime) = if let Some(control) = pre_subscription_control.as_mut() {
            let entered = tokio::time::timeout(Duration::from_secs(10), &mut control.entered)
                .await.context("raw splice did not reach pre-subscription gate")??;
            ensure!(entered.invocation_key == key && entered.operation == Operation::Splice.export(),
                "wrong pre-subscription splice: {entered:?}");
            ensure!(entered.input_stream_rep != entered.output_stream_rep);
            ensure!(entered.output_capacity.is_some_and(|bytes| bytes >= 1),
                "B not writable before splice subscription: {entered:?}");
            ensure!(phases.try_recv().is_err(), "native splice polled before subscription gate");
            (None, entered.runtime)
        } else {
            let phase = pending_phase(&mut phases, Operation::Splice, &key).await?;
            let runtime = phase.runtime;
            (Some(phase), runtime)
        };
        let mut byte = [0];
        ensure!(tokio::time::timeout(Duration::from_millis(25), output.read(&mut byte)).await.is_err(),
            "destination must not receive data before A supplies input");
        executor.commit_oplog(&id).await?;
        let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let starts = cross_setup(&entries, 2)?;
        ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
        tokio::time::timeout(Duration::from_secs(10), polls.recv()).await?.context("monitor timer closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(initial.exhaustion.is_none() && initial == runtime);
        ensure!(phase.as_ref().is_none_or(|phase| phase.state() == P2NativeInputStateForTest::Pending));
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await && !request_is_finished(&invocation));
        if resource == Resource::Compute {
            ensure!(initial.fuel_generation.is_some() && initial.fuel_generation == account.settled_fuel_generation_for_test());
            ensure!(account.monthly_capacity_is_exhausted_for_test(mode));
            clock.advance(Duration::from_secs(30));
            tokio::time::timeout(Duration::from_secs(10), async {
                while polls.recv().await.unwrap().deadline != Duration::from_secs(60) {}
            }).await?;
            ensure!(attempts.try_recv().is_err() && worker.current_monthly_proposal_for_test() == initial);
        }
        if resource == Resource::Storage {
            source_usage.set(FilesystemUsage::Authoritative { allocated_bytes: 101, filesystem_objects: 1 });
            clock.advance(Duration::from_secs(30));
        } else {
            let mut exhausted = policy.clone();
            if resource == Resource::Memory { exhausted.available_memory_gb_seconds = 0; }
            else { exhausted.available_fuel = 0; }
            registry.set_policy(exhausted);
            start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
        }
        let mut storage_now = Duration::from_secs(30);
        let attempt = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    biased;
                    attempt = attempts.recv() => return attempt.context("acceptance observer closed"),
                    poll = polls.recv(), if resource == Resource::Storage => {
                        let poll = poll.context("storage monitor timer closed")?;
                        if poll.now != storage_now || poll.deadline <= storage_now { continue; }
                        worker.drain_lifecycle_for_test().await?;
                        tokio::select! {
                            biased;
                            attempt = attempts.recv() => return attempt.context("acceptance observer closed"),
                            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                        }
                        storage_now += Duration::from_secs(30);
                        clock.advance(Duration::from_secs(30));
                    }
                }
            }
        }).await.context("monthly proposal")??;
        ensure!(attempt.proposal.policy_revision == initial.policy_revision + u64::from(resource != Resource::Storage));
        ensure!(attempt.proposal.period == initial.period && attempt.proposal.window_identity == initial.window_identity);
        ensure!(attempt.proposal.resident_generation == initial.resident_generation);
        ensure!(attempt.proposal.fuel_generation == initial.fuel_generation);
        ensure!(attempt.proposal.exhaustion == Some(resource.reason(durable)));
        ensure!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        let signal = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        ensure!(matches!(signal, InterruptKind::Suspend(_)) && worker.monthly_stop_for_test() == Some(signal));
        ensure!(worker.frozen_stop_for_test() == Some(signal));
        ensure!(worker.owner_stop_for_test().await == Some(signal));
        if let Some(control) = pre_subscription_control.as_mut() {
            ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
            ensure!(worker.monthly_window_active_for_test() && !request_is_finished(&invocation));
            ensure!(phases.try_recv().is_err(), "native splice ran before stop publication");
            ensure!(tokio::time::timeout(Duration::from_millis(25), input.read(&mut byte)).await.is_err(),
                "A closed before the pre-subscription gate was released");
            ensure!(tokio::time::timeout(Duration::from_millis(25), output.read(&mut byte)).await.is_err(),
                "B received data or EOF before the pre-subscription gate was released");
            control.release();
        }
        if resource == Resource::Compute { ensure!(account.settled_fuel_generation_for_test() > initial.fuel_generation); }
        if resource == Resource::Storage {
            let other = if durable { AgentMode::Ephemeral } else { AgentMode::Durable };
            ensure!(!account.monthly_capacity_is_exhausted_for_test(other));
        }
        // Both TCP connections must close while A has still sent nothing.
        let closed = tokio::time::timeout(Duration::from_secs(10), input.read(&mut byte)).await;
        ensure!(closed.is_ok(), "A still open: loaded={}, permit={}, finished={}",
            worker.is_loaded().await, worker.concurrent_agent_permit_is_held().await, request_is_finished(&invocation));
        ensure!(closed.unwrap()? == 0);
        ensure!(tokio::time::timeout(Duration::from_secs(10), output.read(&mut byte)).await?? == 0);
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
        }).await.context("physical window closure")?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        if refresh.is_some() { join_request(&mut refresh, "stop refresh").await?; }
        ensure!(worker.unload_succeeded_for_test() && worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(clock.active_sleeps() == 0 && account.monthly_observer_count_for_test() == 0);
        if let Some(phase) = phase {
            ensure!(phase.state() == P2NativeInputStateForTest::Dropped);
        } else if let Ok(phase) = phases.try_recv() {
            // The unbiased select may poll native input once before the latched stop.
            ensure!(phase.invocation_key.as_ref() == Some(&key)
                && phase.operation == Operation::Splice.export()
                && phase.start_index.is_none() && phase.runtime == initial
                && phase.output_capacity.is_some_and(|bytes| bytes >= 1)
                && phase.state() == P2NativeInputStateForTest::Dropped,
                "wrong post-subscription native observation: {phase:?}");
        }
        ensure!(phases.try_recv().is_err(), "unexpected extra native splice observation");
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(cross_setup(&stopped, 2)? == starts);
        let entries = worker.oplog().read_exact(OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64()).await;
        let errors: Vec<_> = entries.values().filter_map(|entry| match entry {
            OplogEntry::Error { error, .. } => Some(error), _ => None,
        }).collect();
        let suspends = entries.values().filter(|entry| matches!(entry, OplogEntry::Suspend { .. })).count();
        if durable {
            ensure!(errors.is_empty() && suspends == 1 && !request_is_finished(&invocation));
            ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
        } else {
            ensure!(suspends == 0);
            match resource {
                Resource::Compute => ensure!(matches!(errors.as_slice(), [AgentError::EphemeralFuelExhausted(_)])),
                _ => ensure!(matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)]
                    if reason.reason == resource.reason(false))),
            }
            ensure!(join_request(&mut invocation, "invocation result").await?.is_err());
        }
        let observations = source_usage.observations();
        limits.run_batch_for_test().await;
        let updates = registry.applied_updates();
        check_accounting(&updates[baseline_updates..], resource, durable, initial.policy_revision, initial.period)?;
        let settled_count = updates.len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        limits.run_batch_for_test().await;
        ensure!(source_usage.observations() == observations);
        for update in &registry.applied_updates()[settled_count..] {
            ensure!(update.fuel_delta == 0 && update.memory_gb_seconds_delta == 0
                && update.memory_byte_nanoseconds_remainder == 0
                && update.durable_storage_byte_seconds_delta == 0
                && update.durable_storage_byte_nanoseconds_remainder == 0
                && update.ephemeral_storage_byte_seconds_delta == 0
                && update.ephemeral_storage_byte_nanoseconds_remainder == 0,
                "closed window accrued: {update:?}");
        }
        if durable {
            let mut grant = policy;
            if resource == Resource::Storage { grant.available_durable_storage_byte_seconds = u64::MAX; }
            registry.set_policy(grant);
            start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
            join_request(&mut refresh, "policy refresh").await?;
            executor.resume(&id, false).await?;
            let (mut input, mut output) = peers(&source, &destination).await?;
            let reconstructed = pending_phase(&mut phases, Operation::Splice, &key).await?;
            ensure!(reconstructed.runtime.resident_generation > initial.resident_generation);
            ensure!(reconstructed.runtime.start_attempt != initial.start_attempt);
            ensure!(reconstructed.runtime.fingerprint == initial.fingerprint);
            echo(&mut input, &mut output).await?;
            ensure!(join_request(&mut invocation, "invocation result").await??
                .into_typed::<Result<u64, String>>()? == Ok(1));
            ensure!(reconstructed.state() == P2NativeInputStateForTest::Returned);
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(cross_setup(&recovered, 2)? == starts);
            assert_settled_history(&recovered)?;
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            ensure!(executor.invoke_and_await_agent_with_key(&component, &name, &key, METHOD,
                data_value!(source_port, destination_port)).await?.into_typed::<Result<u64, String>>()? == Ok(1));
            ensure!(tokio::time::timeout(Duration::from_millis(100), source.accept()).await.is_err());
            ensure!(tokio::time::timeout(Duration::from_millis(100), destination.accept()).await.is_err());
            let fresh_key = IdempotencyKey::fresh();
            let fresh = executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key, METHOD,
                data_value!(source_port, destination_port));
            let fresh_peers = async {
                let (mut input, mut output) = peers(&source, &destination).await?;
                let phase = pending_phase(&mut phases, Operation::Splice, &fresh_key).await?;
                echo(&mut input, &mut output).await?;
                ensure!(phase.state() == P2NativeInputStateForTest::Returned);
                Ok::<_, anyhow::Error>(())
            };
            let (fresh, ()) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::try_join!(fresh, fresh_peers)
            }).await??;
            ensure!(fresh.into_typed::<Result<u64, String>>()? == Ok(1));
            ensure!(tokio::time::timeout(Duration::from_millis(100), source.accept()).await.is_err());
            ensure!(tokio::time::timeout(Duration::from_millis(100), destination.accept()).await.is_err());
            let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (3, 3));
            ensure!(cross_setup(&history, 4)?.len() == 4);
            assert_settled_history(&history)?;
        } else {
            let mut grant = policy;
            if resource == Resource::Storage { grant.available_ephemeral_storage_byte_seconds = u64::MAX; }
            registry.set_policy(grant);
            start_refresh(&limits, &registry, context.account_id, &mut refresh).await?;
            join_request(&mut refresh, "policy refresh").await?;
            ensure!(executor.invoke_and_await_agent_with_key(&component, &name, &key, METHOD,
                data_value!(source_port, destination_port)).await.is_err());
            ensure!(tokio::time::timeout(Duration::from_millis(100), source.accept()).await.is_err());
            ensure!(tokio::time::timeout(Duration::from_millis(100), destination.accept()).await.is_err());
            let probe_name = agent_id!("EphemeralNetworking", "p2-cross-socket-splice-probe");
            let probe_key = IdempotencyKey::fresh();
            let probe = executor.invoke_and_await_agent_with_key(&component,
                &probe_name, &probe_key, METHOD, data_value!(source_port, destination_port));
            let probe_peers = async {
                let (mut input, mut output) = peers(&source, &destination).await?;
                echo(&mut input, &mut output).await
            };
            let (probe, ()) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::try_join!(probe, probe_peers)
            }).await??;
            ensure!(probe.into_typed::<Result<u64, String>>()? == Ok(1));
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    if let Some(control) = pre_subscription_control.as_mut() {
        control.release();
    }
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

fn cross_setup(
    entries: &[PublicOplogEntryWithIndex],
    expected: usize,
) -> anyhow::Result<Vec<OplogIndex>> {
    let polls = poll_starts(entries);
    ensure!(
        polls.len() == expected,
        "expected {expected} completed connect polls, got {polls:?}"
    );
    for entry in entries {
        ensure!(!matches!(entry.entry, PublicOplogEntry::Cancelled(_)));
        if let PublicOplogEntry::Start(start) = &entry.entry {
            ensure!(start.parent_start_index.is_none() && start.observational_owner.is_none());
            ensure!(
                !start.function_name.contains("streams"),
                "raw splice has no stream Start"
            );
            ensure!(terminal_count(entries, entry.oplog_index) == 1);
        }
    }
    Ok(polls)
}
