use super::monthly_http_peer as peer;

use super::*;
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{
    AgentError, OplogEntry, PublicAgentInvocation, PublicDurableFunctionType,
    PublicOplogEntryWithIndex,
};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::resource_usage_metering::{
    FilesystemUsage, ScriptedFilesystemUsageForTest,
};
use golem_worker_executor::services::{HasOplog, UsesAllDeps};
use golem_worker_executor::worker::{MonthlyClockForTest, Worker};
use test_r::test;

use super::monthly_pending::{FUEL_BUDGET, Resource, check_accounting};
use golem_worker_executor::services::golem_config::{HttpClientConfig, HttpClientEnabledConfig};
use peer::Peer;

macro_rules! pending_case {
    ($name:ident, $resource:ident, $mode:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            pending_headers(
                last_unique_id,
                deps,
                http_tests,
                Resource::$resource,
                AgentMode::$mode,
            )
            .await
        }
    };
}

pending_case!(durable_p3_http_headers_monthly_memory, Memory, Durable);
pending_case!(ephemeral_p3_http_headers_monthly_memory, Memory, Ephemeral);
pending_case!(
    durable_p3_http_headers_monthly_compute_prepaid,
    Compute,
    Durable
);
pending_case!(
    ephemeral_p3_http_headers_monthly_compute_prepaid,
    Compute,
    Ephemeral
);
pending_case!(
    durable_p3_http_headers_monthly_scripted_storage,
    Storage,
    Durable
);
pending_case!(
    ephemeral_p3_http_headers_monthly_scripted_storage,
    Storage,
    Ephemeral
);

const SEND: &str = "http::client::send";

fn assert_invocation(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    finished: usize,
) -> anyhow::Result<()> {
    let mut current = false;
    let mut started = 0;
    let mut terminals = 0;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(start) => {
                current = matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method) if &method.idempotency_key == key);
                started += usize::from(current);
            }
            PublicOplogEntry::AgentInvocationFinished(_) if current => terminals += 1,
            _ => {}
        }
    }
    ensure!(
        started == 1 && terminals == finished,
        "invocation {key}: {started} Started/{terminals} Finished"
    );
    Ok(())
}

fn send_start(
    entries: &[PublicOplogEntryWithIndex],
    terminals: usize,
) -> anyhow::Result<OplogIndex> {
    let starts: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name == SEND => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    ensure!(starts.len() == 1, "one send Start: {entries:#?}");
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(start) => {
                if entry.oplog_index == starts[0] {
                    ensure!(
                        start.parent_start_index.is_none() && start.observational_owner.is_none()
                    );
                    ensure!(matches!(
                        start.durable_function_type,
                        PublicDurableFunctionType::WriteRemote(_)
                    ));
                    ensure!(start.request.is_some());
                }
                ensure!(
                    terminal_count(entries, entry.oplog_index)
                        == if entry.oplog_index == starts[0] {
                            terminals
                        } else {
                            1
                        }
                );
            }
            PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::CompletionDiscarded(_)
            | PublicOplogEntry::Jump(_) => {
                anyhow::bail!("no invented terminal or replay jump: {entry:?}")
            }
            _ => {}
        }
    }
    let delivered = entries.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(delivery) if delivery.start_index == starts[0])).count();
    ensure!(delivered == terminals, "send delivery: {entries:#?}");
    Ok(starts[0])
}

async fn pending_headers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    http_tests: &PrecompiledComponent,
    resource: Resource,
    mode: AgentMode,
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
    let context = TestContext::new(last_unique_id);
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| {
            config.resource_usage_metering = metering;
            config.limits.fuel_to_borrow = FUEL_BUDGET;
            config.http_client = HttpClientConfig::Enabled(HttpClientEnabledConfig {
                max_idle_per_host: 1,
                max_connections_per_host: 1,
                max_total_connections: 1,
                ..Default::default()
            });
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(&component.id, agent_id!("HttpClient2"))
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
    // Refund the seed Store before the target borrows the full account budget.
    if resource == Resource::Compute {
        limits.run_batch_for_test().await;
    }
    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("HttpClient4")
    } else {
        agent_id!("EphemeralHttpClient4")
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
    // Script only authoritative allocation, not filesystem deletion, metering or permit release.
    let source = ScriptedFilesystemUsageForTest::new(FilesystemUsage::Authoritative {
        allocated_bytes: 0,
        filesystem_objects: 0,
    });
    if resource == Resource::Storage {
        worker
            .owner_runtime_resources()
            .set_scripted_filesystem_usage_for_test(source.clone());
    }
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let mut peer = Peer::start().await?;
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
                    "get_for_header_wait",
                    data_value!(url),
                )
                .await
        }
    });
    let result = async {
        let first = peer.request().await?;
        ensure!(first.number == 1);
        let send_start = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                executor.commit_oplog(&id).await?;
                let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                if entries.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == SEND)) {
                    let start = send_start(&entries, 0)?;
                    assert_invocation(&entries, &key, 0)?;
                    ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
                    return Ok::<_, anyhow::Error>(start);
                }
                tokio::task::yield_now().await;
            }
        }).await.context("committed open send under unfinished method")??;
        tokio::time::timeout(Duration::from_secs(10), polls.recv())
            .await?
            .context("timer closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(initial.exhaustion.is_none(), "{initial:?}");
        peer.assert_counts(1)?;
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(!invocation.is_finished());
        info!(
            ?resource, ?mode, %key, ?id, %send_start, ?initial, http_key = %first.idempotency_key, requests = 1,
            "Peer accepted request; committed open P3 send; unfinished method; no response headers"
        );

        if resource == Resource::Compute {
            ensure!(initial.fuel_generation.is_some());
            ensure!(initial.fuel_generation == account.settled_fuel_generation_for_test());
            ensure!(
                account.monthly_capacity_is_exhausted_for_test(mode),
                "Store must prepay the remaining fuel"
            );
            clock.advance(Duration::from_secs(30));
            tokio::time::timeout(Duration::from_secs(10), async {
                while polls.recv().await.unwrap().deadline != Duration::from_secs(60) {}
            })
            .await?;
            ensure!(
                attempts.try_recv().is_err(),
                "zero unreserved fuel must not exhaust prepaid fuel"
            );
            ensure!(worker.current_monthly_proposal_for_test() == initial);
            ensure!(!invocation.is_finished() && worker.concurrent_agent_permit_is_held().await);
        }
        ensure!(!invocation.is_finished());
        peer.assert_counts(1)?;
        let refresh = if resource == Resource::Storage {
            source.set(FilesystemUsage::Authoritative {
                allocated_bytes: 101,
                filesystem_objects: 1,
            });
            clock.advance(Duration::from_secs(30));
            None
        } else {
            let mut exhausted = policy.clone();
            if resource == Resource::Memory {
                exhausted.available_memory_gb_seconds = 0;
            } else {
                exhausted.available_fuel = 0;
            }
            registry.set_policy(exhausted);
            Some(monthly::applied_refresh(&limits, &registry, context.account_id).await?)
        };
        let mut storage_now = Duration::from_secs(30);
        let attempt = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    biased;
                    attempt = attempts.recv() => return attempt.context("acceptance observer closed"),
                    poll = polls.recv(), if resource == Resource::Storage => {
                        let poll = poll.context("storage monitor timer closed")?;
                        if poll.now != storage_now || poll.deadline <= storage_now {
                            continue;
                        }
                        // Rearming follows settlement and proposal submission. Drain that tick's
                        // lifecycle work and observe its first proposal before advancing again.
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
        })
        .await
        .context("monthly proposal")??;
        ensure!(
            attempt.proposal.policy_revision
                == initial.policy_revision + u64::from(resource != Resource::Storage)
        );
        ensure!(attempt.proposal.period == initial.period);
        ensure!(attempt.proposal.window_identity == initial.window_identity);
        ensure!(attempt.proposal.resident_generation == initial.resident_generation);
        ensure!(attempt.proposal.start_attempt == initial.start_attempt);
        ensure!(attempt.proposal.fingerprint == initial.fingerprint);
        ensure!(attempt.proposal.fuel_generation == initial.fuel_generation);
        ensure!(attempt.proposal.exhaustion == Some(resource.reason(durable)));
        ensure!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        let signal = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        ensure!(matches!(signal, InterruptKind::Suspend(_)));
        ensure!(worker.monthly_stop_for_test() == Some(signal));
        info!(?resource, ?mode, proposal = ?attempt.proposal, ?signal, "Accepted matching monthly HTTP stop and published typed signal");
        if resource == Resource::Compute {
            ensure!(account.settled_fuel_generation_for_test() > initial.fuel_generation);
        }
        if resource == Resource::Storage {
            let other = if durable {
                AgentMode::Ephemeral
            } else {
                AgentMode::Durable
            };
            ensure!(!account.monthly_capacity_is_exhausted_for_test(other));
        }
        // Keep the response gate held until monthly stop closes the physical socket.
        peer.first_closed().await?;
        drop(first.respond);
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .with_context(|| format!("physical window closure: invocation_finished={}", invocation.is_finished()))?;
        tokio::time::timeout(
            Duration::from_secs(10),
            worker.join_accepted_stops_for_test(),
        )
        .await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        if let Some(refresh) = refresh {
            refresh.await?;
        }
        ensure!(worker.unload_succeeded_for_test());
        ensure!(worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(clock.active_sleeps() == 0);
        ensure!(account.monthly_observer_count_for_test() == 0);
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_invocation(&stopped, &key, 0)?;
        ensure!(
            self::send_start(&stopped, 0)? == send_start,
            "interruption must leave the original send Start open"
        );
        peer.assert_counts(1)?;
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
        if durable {
            ensure!(errors.is_empty(), "{errors:?}");
            ensure!(suspends == 1);
            ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
            ensure!(!invocation.is_finished());
        } else {
            ensure!(suspends == 0);
            match resource {
                Resource::Compute => {
                    ensure!(
                        matches!(errors.as_slice(), [AgentError::EphemeralFuelExhausted(_)]),
                        "{errors:?}"
                    );
                }
                _ => {
                    ensure!(
                        matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)]
                            if reason.reason == resource.reason(false)),
                        "{errors:?}"
                    );
                }
            }
            ensure!(
                tokio::time::timeout(Duration::from_secs(10), &mut invocation)
                    .await??
                    .is_err()
            );
        }
        executor.wait_for_status(&id, if durable { AgentStatus::Suspended } else { AgentStatus::Failed }, Duration::from_secs(10)).await?;
        ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
        info!(?resource, ?mode, ?errors, suspends, "HTTP stop retained one unfinished method with mode-specific lifecycle outcome");
        let observations = source.observations();
        limits.run_batch_for_test().await;
        let settled = registry.applied_updates();
        check_accounting(
            &settled,
            resource,
            durable,
            initial.policy_revision,
            initial.period,
        )?;
        info!(?resource, ?mode, ?settled, "HTTP stopped execution window settlement");
        let settled_count = settled.len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        limits.run_batch_for_test().await;
        ensure!(
            source.observations() == observations,
            "closed generation must stop sampling"
        );
        // Idle policy refreshes may still emit updates, but cannot charge a closed window.
        for update in &registry.applied_updates()[settled_count..] {
            ensure!(
                update.fuel_delta == 0
                    && update.memory_gb_seconds_delta == 0
                    && update.memory_byte_nanoseconds_remainder == 0
                    && update.durable_storage_byte_seconds_delta == 0
                    && update.durable_storage_byte_nanoseconds_remainder == 0
                    && update.ephemeral_storage_byte_seconds_delta == 0
                    && update.ephemeral_storage_byte_nanoseconds_remainder == 0,
                "closed window must stop accruing: {update:?}"
            );
        }
        info!(
            ?resource, ?mode, %send_start, requests = 1,
            "Monthly stop closed silent HTTP socket, released Worker permit and monitor, settled usage; send remains open"
        );

        if durable {
            let mut grant = policy;
            if resource == Resource::Storage {
                grant.available_durable_storage_byte_seconds = u64::MAX;
            }
            registry.set_policy(grant);
            monthly::applied_refresh(&limits, &registry, context.account_id).await?.await?;
            executor.resume(&id, false).await?;
            let second = peer.request().await?;
            ensure!(second.number == 2);
            ensure!(second.idempotency_key == first.idempotency_key);
            let reconstructed = worker.current_monthly_proposal_for_test();
            ensure!(reconstructed.resident_generation > initial.resident_generation);
            ensure!(reconstructed.start_attempt != initial.start_attempt);
            ensure!(reconstructed.fingerprint == initial.fingerprint);
            executor.commit_oplog(&id).await?;
            ensure!(self::send_start(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, 0)? == send_start);
            second.respond.send(()).map_err(|_| anyhow::anyhow!("second HTTP response gate closed"))?;
            let resumed = tokio::time::timeout(Duration::from_secs(15), &mut invocation)
                .await.context("retained HTTP invocation continuation and tail finalization")???;
            ensure!(resumed.into_typed::<String>()? == "200 hi");
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            info!(?recovered, "HTTP recovery oplog after body and request finalization");
            ensure!(self::send_start(&recovered, 1)? == send_start);
            assert_settled_history(&recovered)?;
            assert_invocation(&recovered, &key, 1)?;
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, "get_for_header_wait", data_value!(peer.url.clone())).await?;
            ensure!(cached.into_typed::<String>()? == "200 hi");
            peer.assert_counts(2)?;
            info!(?resource, %key, %send_start, ?reconstructed, requests = 2,
                "Original HTTP send repaired under same Start and HTTP key; one method Finished; second external request observed");

            // Idle eviction forces completed history through a new Store before fresh work.
            tokio::time::timeout(Duration::from_secs(10), async {
                while !worker.stop_if_idle().await { tokio::task::yield_now().await; }
            }).await.context("idle eviction before completed HTTP history")?;
            tokio::time::timeout(Duration::from_secs(10), async {
                while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
            }).await?;
            tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
            tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
            let fresh_key = IdempotencyKey::fresh();
            let fresh = executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key, "get_for_header_wait", data_value!(peer.url.clone()));
            let response = async {
                let third = peer.request().await?;
                ensure!(third.number == 3);
                ensure!(third.idempotency_key != first.idempotency_key, "completed HTTP history must not reissue a request");
                third.respond.send(()).map_err(|_| anyhow::anyhow!("fresh HTTP response gate closed"))?;
                Ok::<_, anyhow::Error>(())
            };
            let (fresh, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(fresh, response) }).await??;
            ensure!(fresh.into_typed::<String>()? == "200 hi");
            peer.assert_counts(3)?;
            let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (3, 3));
            let starts: Vec<_> = history.iter().filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(start) if start.function_name == SEND => Some(entry.oplog_index),
                _ => None,
            }).collect();
            ensure!(starts.len() == 2 && starts[0] == send_start);
            for start in starts { ensure!(terminal_count(&history, start) == 1); }
            assert_settled_history(&history)?;
            assert_invocation(&history, &key, 1)?;
            assert_invocation(&history, &fresh_key, 1)?;
            info!(?resource, %key, %send_start, requests = 3, "Completed HTTP history made no external request; fresh invocation completed");
        } else {
            // A different durable Worker must be able to acquire the single HTTP pool slot.
            // The failed ephemeral invocation is never resumed or re-executed.
            let mut grant = policy;
            grant.available_durable_storage_byte_seconds = u64::MAX;
            grant.available_ephemeral_storage_byte_seconds = u64::MAX;
            registry.set_policy(grant);
            monthly::applied_refresh(&limits, &registry, context.account_id).await?.await?;
            let probe_name = agent_id!("HttpClient4");
            let probe_key = IdempotencyKey::fresh();
            let probe = executor.invoke_and_await_agent_with_key(&component, &probe_name, &probe_key, "get_for_header_wait", data_value!(peer.url.clone()));
            let response = async {
                let second = peer.request().await?;
                ensure!(second.number == 2);
                ensure!(second.idempotency_key != first.idempotency_key);
                second.respond.send(()).map_err(|_| anyhow::anyhow!("pool probe response gate closed"))?;
                Ok::<_, anyhow::Error>(())
            };
            let (probe, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(probe, response) }).await??;
            ensure!(probe.into_typed::<String>()? == "200 hi");
            let failed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(self::send_start(&failed, 0)? == send_start);
            assert_invocation(&failed, &key, 0)?;
            ensure!(count_agent_invocation_pair_since(&failed, OplogIndex::INITIAL) == (2, 1));
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
            peer.assert_counts(2)?;
            info!(?resource, %key, %send_start, requests = 2, "Ephemeral HTTP failure stayed terminal; independent durable Worker reused single pool slot");
        }
        // The accept loop remains active to reject extra network attempts after completion.
        tokio::time::sleep(Duration::from_millis(100)).await;
        peer.assert_counts(if durable { 3 } else { 2 })?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    // Stop and join the peer independently, including failure cleanup of every socket handler.
    let cleanup = peer.finish().await;
    if result.is_err() {
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
    result.and(cleanup)
}

fn terminal_count(entries: &[PublicOplogEntryWithIndex], start: OplogIndex) -> usize {
    entries
        .iter()
        .filter(|entry| match &entry.entry {
            PublicOplogEntry::End(end) => end.start_index == start,
            PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
            _ => false,
        })
        .count()
}

fn assert_settled_history(entries: &[PublicOplogEntryWithIndex]) -> anyhow::Result<()> {
    let last_finished = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("missing invocation terminal")?
        .oplog_index;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_) => {
                ensure!(
                    terminal_count(entries, entry.oplog_index) == 1,
                    "unsettled/duplicate Start: {entry:?}"
                );
                ensure!(entry.oplog_index < last_finished);
            }
            PublicOplogEntry::End(_) => ensure!(entry.oplog_index < last_finished),
            PublicOplogEntry::Cancelled(_) | PublicOplogEntry::Error(_) => {
                anyhow::bail!("unexpected terminal: {entry:?}")
            }
            _ => {}
        }
    }
    Ok(())
}
