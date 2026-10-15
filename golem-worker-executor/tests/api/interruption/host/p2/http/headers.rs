use crate::api::interruption::host::http_peer as peer;

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
use golem_worker_executor::worker::{MonthlyClockForTest, P2PollStateForTest, Worker};
use test_r::test;

use crate::api::interruption::support::{FUEL_BUDGET, Resource, check_accounting};
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

pending_case!(durable_p2_http_headers_memory_quota, Memory, Durable);
pending_case!(ephemeral_p2_http_headers_memory_quota, Memory, Ephemeral);
pending_case!(
    durable_p2_http_headers_compute_quota_prepaid,
    Compute,
    Durable
);
pending_case!(
    ephemeral_p2_http_headers_compute_quota_prepaid,
    Compute,
    Ephemeral
);
pending_case!(
    durable_p2_http_headers_scripted_storage_quota,
    Storage,
    Durable
);
pending_case!(
    ephemeral_p2_http_headers_scripted_storage_quota,
    Storage,
    Ephemeral
);

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
    let mut phases = worker.observe_p2_poll_for_test();
    let mut peer = Peer::start().await?;
    let authority = peer
        .url
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/header-wait")
        .unwrap()
        .to_owned();
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
                    "get_for_p2_header_wait",
                    data_value!(authority),
                )
                .await
        }
    });
    let result = async {
        let first = peer.request().await?;
        ensure!(first.number == 1);
        let phase = tokio::time::timeout(Duration::from_secs(10), phases.recv()).await?.context("P2 native poll observer closed")?;
        ensure!(phase.invocation_key.as_ref() == Some(&key));
        ensure!(phase.pollable_reps.len() == 1 && phase.state() == P2PollStateForTest::Pending, "{phase:?}");
        executor.commit_oplog(&id).await?;
        let started = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let poll_start = phase.start;
        let scope_start = assert_pending_shape(&started, &key, poll_start)?;
        info!(?phase, %scope_start, %poll_start, "P2 response header wait recorded after native Poll::Pending");
        ensure!(count_agent_invocation_pair_since(&started, OplogIndex::INITIAL) == (2, 1));
        tokio::time::timeout(Duration::from_secs(10), polls.recv()).await?.context("monitor timer closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(initial.exhaustion.is_none() && phase.runtime == initial);
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await && !invocation.is_finished());
        peer.assert_counts(1)?;
        ensure!(phase.state() == P2PollStateForTest::Pending);
        if resource == Resource::Compute {
            ensure!(initial.fuel_generation.is_some());
            ensure!(initial.fuel_generation == account.settled_fuel_generation_for_test());
            ensure!(account.monthly_capacity_is_exhausted_for_test(mode), "Store must prepay the remaining fuel");
            clock.advance(Duration::from_secs(30));
            tokio::time::timeout(Duration::from_secs(10), async {
                while polls.recv().await.unwrap().deadline != Duration::from_secs(60) {}
            }).await?;
            ensure!(attempts.try_recv().is_err(), "zero unreserved fuel must not exhaust prepaid fuel");
            ensure!(worker.current_monthly_proposal_for_test() == initial);
            ensure!(!invocation.is_finished() && worker.concurrent_agent_permit_is_held().await);
        }
        let teardown = (resource == Resource::Memory && durable)
            .then(|| worker.pause_next_teardown_fence_for_test());
        let allocation_observations = source.observations();
        let refresh = if resource == Resource::Storage {
            source.set(FilesystemUsage::Authoritative { allocated_bytes: 101, filesystem_objects: 1 });
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
            Some(applied_refresh(&limits, &registry, context.account_id).await?)
        };
        let mut storage_now = Duration::from_secs(30);
        let attempt = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    biased;
                    attempt = attempts.recv() => return attempt.context("monthly acceptance observer closed"),
                    poll = polls.recv(), if resource == Resource::Storage => {
                        let poll = poll.context("storage monitor timer closed")?;
                        if poll.now != storage_now || poll.deadline <= storage_now { continue; }
                        worker.drain_lifecycle_for_test().await?;
                        tokio::select! {
                            biased;
                            attempt = attempts.recv() => return attempt.context("monthly acceptance observer closed"),
                            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                        }
                        storage_now += Duration::from_secs(30);
                        clock.advance(Duration::from_secs(30));
                    }
                }
            }
        }).await.context("monthly proposal")??;
        ensure!(attempt.proposal.period == initial.period);
        ensure!(attempt.proposal.policy_revision == initial.policy_revision + u64::from(resource != Resource::Storage));
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
        ensure!(worker.frozen_stop_for_test() == Some(signal));
        ensure!(worker.owner_stop_for_test().await == Some(signal));
        if let Some((entered, release)) = teardown {
            tokio::time::timeout(Duration::from_secs(10), entered)
                .await
                .context("HTTP stop reached teardown fence")??;
            ensure!(worker.is_loaded().await);
            ensure!(worker.concurrent_agent_permit_is_held().await);
            release.send(()).map_err(|_| anyhow!("HTTP teardown fence closed"))?;
        }
        if resource == Resource::Compute {
            ensure!(account.settled_fuel_generation_for_test() > initial.fuel_generation);
        }
        if resource == Resource::Storage {
            ensure!(source.observations() > allocation_observations);
            let other = if durable { AgentMode::Ephemeral } else { AgentMode::Durable };
            ensure!(!account.monthly_capacity_is_exhausted_for_test(other));
        }
        peer.first_closed().await?;
        drop(first.respond);
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
        }).await.context("physical window closure")?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        if let Some(refresh) = refresh { refresh.await?; }
        ensure!(worker.unload_succeeded_for_test() && worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(clock.active_sleeps() == 0 && account.monthly_observer_count_for_test() == 0);
        ensure!(phase.state() == P2PollStateForTest::Dropped);
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(assert_pending_shape(&stopped, &key, poll_start)? == scope_start);
        ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
        let entries = worker.oplog().read_exact(OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64()).await;
        let errors: Vec<_> = entries.values().filter_map(|entry| match entry {
            OplogEntry::Error { error, .. } => Some(error),
            _ => None,
        }).collect();
        let suspends = entries.values().filter(|entry| matches!(entry, OplogEntry::Suspend { .. })).count();
        if durable {
            ensure!(suspends == 1 && errors.is_empty(), "{errors:?}");
            ensure!(!invocation.is_finished());
        } else {
            ensure!(suspends == 0);
            match resource {
                Resource::Compute => ensure!(matches!(errors.as_slice(), [AgentError::EphemeralFuelExhausted(_)]), "{errors:?}"),
                _ => ensure!(matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)] if reason.reason == resource.reason(false)), "{errors:?}"),
            }
            ensure!(tokio::time::timeout(Duration::from_secs(10), &mut invocation).await??.is_err());
        }
        executor.wait_for_status(&id, if durable { AgentStatus::Suspended } else { AgentStatus::Failed }, Duration::from_secs(10)).await?;
        let observations = source.observations();
        limits.run_batch_for_test().await;
        check_accounting(&registry.applied_updates(), resource, durable, initial.policy_revision, initial.period)?;
        let settled_count = registry.applied_updates().len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        limits.run_batch_for_test().await;
        ensure!(source.observations() == observations, "closed generation sampled allocation");
        for update in &registry.applied_updates()[settled_count..] {
            ensure!(update.fuel_delta == 0 && update.memory_gb_seconds_delta == 0
                && update.memory_byte_nanoseconds_remainder == 0
                && update.durable_storage_byte_seconds_delta == 0
                && update.durable_storage_byte_nanoseconds_remainder == 0
                && update.ephemeral_storage_byte_seconds_delta == 0
                && update.ephemeral_storage_byte_nanoseconds_remainder == 0,
                "closed window billed: {update:?}");
        }
        let mut grant = policy;
        if resource == Resource::Storage {
            grant.available_durable_storage_byte_seconds = u64::MAX;
            grant.available_ephemeral_storage_byte_seconds = u64::MAX;
        }
        registry.set_policy(grant);
        applied_refresh(&limits, &registry, context.account_id).await?.await?;
        if durable {
        executor.resume(&id, false).await?;
        let second = peer.request().await?;
        ensure!(second.number == 2 && second.idempotency_key == first.idempotency_key);
        second.respond.send(()).map_err(|_| anyhow!("second HTTP response gate closed"))?;
        let resumed = tokio::time::timeout(Duration::from_secs(15), &mut invocation).await.context("P2 response reconstruction")???;
        ensure!(resumed.into_typed::<String>()? == "200");
        let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_repaired_shape(&recovered, &key, scope_start, poll_start)?;
        ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
        peer.assert_counts(2)?;
        let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key,
            "get_for_p2_header_wait", data_value!(peer.url.strip_prefix("http://").unwrap().strip_suffix("/header-wait").unwrap().to_owned())).await?;
        ensure!(cached.into_typed::<String>()? == "200");
        peer.assert_counts(2)?;
        ensure!(count_agent_invocation_pair_since(&executor.get_oplog(&id, OplogIndex::INITIAL).await?, OplogIndex::INITIAL) == (2, 2));

        tokio::time::timeout(Duration::from_secs(10), async {
            while !worker.stop_if_idle().await { tokio::task::yield_now().await; }
        }).await.context("idle eviction before completed P2 history")?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
        }).await.context("idle eviction physical closure")?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        let fresh_key = IdempotencyKey::fresh();
        let fresh_authority = peer.url.strip_prefix("http://").unwrap().strip_suffix("/header-wait").unwrap().to_owned();
        let fresh = executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key,
            "get_for_p2_header_wait", data_value!(fresh_authority));
        let response = async {
            let third = peer.request().await?;
            ensure!(third.number == 3 && third.idempotency_key != first.idempotency_key,
                "completed P2 history must not reissue a request");
            third.respond.send(()).map_err(|_| anyhow!("fresh P2 response gate closed"))?;
            Ok::<_, anyhow::Error>(())
        };
        let (fresh, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(fresh, response) }).await??;
        ensure!(fresh.into_typed::<String>()? == "200");
        peer.assert_counts(3)?;
        let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(history.len() > recovered.len());
        assert_repaired_shape(&history[..recovered.len()], &key, scope_start, poll_start)?;
        ensure!(starts(&history, SCOPE).len() == 2
            && starts(&history, SCOPE)[1] > recovered.last().unwrap().oplog_index);
        assert_invocation(&history, &fresh_key, 1)?;
        ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (3, 3));
        assert_settled_history(&history, poll_start)?;
        } else {
            let probe_name = agent_id!("HttpClient4");
            let probe_key = IdempotencyKey::fresh();
            let probe_authority = peer.url.strip_prefix("http://").unwrap().strip_suffix("/header-wait").unwrap().to_owned();
            let probe = executor.invoke_and_await_agent_with_key(&component, &probe_name, &probe_key,
                "get_for_p2_header_wait", data_value!(probe_authority));
            let response = async {
                let second = peer.request().await?;
                ensure!(second.number == 2 && second.idempotency_key != first.idempotency_key);
                second.respond.send(()).map_err(|_| anyhow!("pool probe gate closed"))?;
                Ok::<_, anyhow::Error>(())
            };
            let (probe, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(probe, response) }).await??;
            ensure!(probe.into_typed::<String>()? == "200");
            let failed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(assert_pending_shape(&failed, &key, poll_start)? == scope_start);
            ensure!(count_agent_invocation_pair_since(&failed, OplogIndex::INITIAL) == (2, 1));
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
            peer.assert_counts(2)?;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        peer.assert_counts(if durable { 3 } else { 2 })?;
        Ok::<_, anyhow::Error>(())
    }.await;
    if result.is_err() {
        let _ = tokio::time::timeout(Duration::from_secs(10), executor.interrupt(&id)).await;
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            worker.join_accepted_stops_for_test(),
        )
        .await;
        let _ =
            tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await;
        if !invocation.is_finished()
            && tokio::time::timeout(Duration::from_secs(10), &mut invocation)
                .await
                .is_err()
        {
            invocation.abort();
            let _ = tokio::time::timeout(Duration::from_secs(10), &mut invocation).await;
        }
    }
    let cleanup = peer.finish().await;
    result.and(cleanup)
}

const SCOPE: &str = "<scope:batched-write>";
const RESPONSE_GET: &str = "http::types::future_incoming_response::get";
const POLL: &str = "io::poll::poll";

fn assert_invocation(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    expected_finished: usize,
) -> anyhow::Result<()> {
    let mut current = false;
    let mut started = 0;
    let mut finished = 0;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(start) => {
                current = matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method) if &method.idempotency_key == key);
                started += usize::from(current);
            }
            PublicOplogEntry::AgentInvocationFinished(_) if current => finished += 1,
            _ => {}
        }
    }
    ensure!(
        started == 1 && finished == expected_finished,
        "invocation {key}: {started} Started/{finished} Finished"
    );
    Ok(())
}

fn starts(entries: &[PublicOplogEntryWithIndex], name: &str) -> Vec<OplogIndex> {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name == name => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect()
}

fn assert_pending_shape(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    poll_start: OplogIndex,
) -> anyhow::Result<OplogIndex> {
    let scopes = starts(entries, SCOPE);
    let children = starts(entries, RESPONSE_GET);
    let polls = starts(entries, POLL);
    ensure!(
        scopes.len() == 1 && children.len() == 1 && polls == [poll_start],
        "P2 scope, completed pending response-get child and open poll: {entries:#?}"
    );
    let scope = scopes[0];
    let child = children[0];
    let child_end = entries
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == child => Some(entry.oplog_index),
            _ => None,
        })
        .context("pending response get End missing")?;
    ensure!(scope < child && child < child_end && child_end < poll_start);
    for (index, expected_name, parent, batched_index, expected_terminals) in [
        (scope, SCOPE, None, None, 0),
        (child, RESPONSE_GET, Some(scope), Some(scope), 1),
        (poll_start, POLL, None, None, 0),
    ] {
        let start = entries
            .iter()
            .find(|entry| entry.oplog_index == index)
            .with_context(|| format!("P2 {expected_name} Start missing"))?;
        let PublicOplogEntry::Start(start) = &start.entry else {
            anyhow::bail!("P2 {expected_name} is not a Start: {start:?}");
        };
        ensure!(
            start.function_name == expected_name
                && start.parent_start_index == parent
                && start.observational_owner.is_none()
        );
        if expected_name == POLL {
            ensure!(matches!(
                start.durable_function_type,
                PublicDurableFunctionType::ReadLocal(_)
            ));
        } else {
            ensure!(matches!(&start.durable_function_type,
                PublicDurableFunctionType::WriteRemoteBatched(params) if params.index == batched_index));
        }
        ensure!(start.request.is_some() == (expected_name != SCOPE));
        ensure!(terminal_count(entries, index) == expected_terminals);
    }
    for entry in entries {
        if matches!(entry.entry, PublicOplogEntry::Start(_))
            && entry.oplog_index != scope
            && entry.oplog_index != poll_start
        {
            ensure!(
                terminal_count(entries, entry.oplog_index) == 1,
                "only the P2 scope and native poll may remain open: {entry:?}"
            );
        }
    }
    ensure!(entries.iter().all(|entry| !matches!(&entry.entry,
        PublicOplogEntry::Start(start) if start.function_name == "http::client::send"
            || start.function_name == "io::poll::pollable::ready")));
    ensure!(entries.iter().all(|entry| !matches!(
        entry.entry,
        PublicOplogEntry::Jump(_)
            | PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::CompletionDelivered(_)
            | PublicOplogEntry::CompletionDiscarded(_)
    )));
    assert_invocation(entries, key, 0)?;
    Ok(scope)
}

fn assert_repaired_shape(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    scope: OplogIndex,
    original_poll: OplogIndex,
) -> anyhow::Result<()> {
    let jump = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Jump(jump) => Some((entry.oplog_index, jump)),
            _ => None,
        })
        .collect::<Vec<_>>();
    ensure!(jump.len() == 1, "one P2 scope-repair Jump: {jump:?}");
    let (jump_index, jump) = jump[0];
    ensure!(
        jump.jump.start == scope.next() && jump.jump.end.next() == jump_index,
        "replace the complete attempt suffix through the pre-Jump horizon: {jump:?}"
    );
    assert_repair_suffix(entries, scope, original_poll, jump_index)?;
    let polls = starts(entries, POLL);
    let children = starts(entries, RESPONSE_GET);
    ensure!(polls.len() == 2 && polls[0] == original_poll && children.len() == 3);
    ensure!(scope < children[0] && children[0] < original_poll && original_poll < jump_index);
    ensure!(
        jump_index < children[1] && children[1] < polls[1] && polls[1] < children[2],
        "pending P2 get, repaired poll, then response headers get"
    );
    ensure!(terminal_count(entries, original_poll) == 0 && terminal_count(entries, polls[1]) == 1);
    let terminal = |start| {
        entries.iter().find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == start => Some(entry.oplog_index),
            _ => None,
        })
    };
    let old_get_end = terminal(children[0]).context("original pending get End missing")?;
    let repaired_get_end = terminal(children[1]).context("repaired pending get End missing")?;
    let repaired_poll_end = terminal(polls[1]).context("repaired poll End missing")?;
    ensure!(old_get_end < original_poll);
    ensure!(repaired_get_end < polls[1] && repaired_poll_end < children[2]);
    for child in children.iter().copied() {
        let start = entries
            .iter()
            .find(|entry| entry.oplog_index == child)
            .unwrap();
        ensure!(matches!(&start.entry, PublicOplogEntry::Start(start)
            if start.parent_start_index == Some(scope)
                && matches!(&start.durable_function_type, PublicDurableFunctionType::WriteRemoteBatched(params) if params.index == Some(scope))));
        ensure!(terminal_count(entries, child) == 1);
    }
    ensure!(starts(entries, SCOPE) == [scope]);
    let scope_end = entries
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == scope => Some(entry.oplog_index),
            _ => None,
        })
        .context("P2 scope End missing")?;
    let headers_end = entries
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == children[2] => Some(entry.oplog_index),
            _ => None,
        })
        .context("response headers get End missing")?;
    let method_finished = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("retained P2 method Finished missing")?
        .oplog_index;
    ensure!(terminal_count(entries, scope) == 1);
    ensure!(children[2] < headers_end && headers_end < scope_end && scope_end < method_finished);
    assert_invocation(entries, key, 1)?;
    Ok(())
}

fn assert_settled_history(
    entries: &[PublicOplogEntryWithIndex],
    deleted_poll: OplogIndex,
) -> anyhow::Result<()> {
    assert_surviving_references(entries)?;
    let last_finished = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("method Finished missing")?
        .oplog_index;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_) => {
                ensure!(
                    terminal_count(entries, entry.oplog_index)
                        == usize::from(entry.oplog_index != deleted_poll),
                    "unsettled or duplicate Start: {entry:?}"
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

fn assert_repair_suffix(
    entries: &[PublicOplogEntryWithIndex],
    scope: OplogIndex,
    abandoned: OplogIndex,
    jump_index: OplogIndex,
) -> anyhow::Result<()> {
    let suffix = entries
        .iter()
        .filter(|entry| entry.oplog_index > scope && entry.oplog_index <= jump_index)
        .collect::<Vec<_>>();
    ensure!(
        suffix
            .first()
            .is_some_and(|entry| entry.oplog_index == scope.next())
    );
    ensure!(
        suffix
            .last()
            .is_some_and(|entry| entry.oplog_index == jump_index)
    );
    ensure!(
        suffix
            .windows(2)
            .all(|pair| pair[0].oplog_index.next() == pair[1].oplog_index)
    );
    let PublicOplogEntry::Jump(jump) = &suffix.last().unwrap().entry else {
        anyhow::bail!("repair horizon not followed by Jump");
    };
    ensure!(jump.jump.contains(abandoned) && !jump.jump.contains(scope));
    ensure!(
        suffix[..suffix.len() - 1]
            .iter()
            .all(|entry| jump.jump.contains(entry.oplog_index)),
        "the complete physical attempt suffix must be deleted"
    );
    ensure!(
        entries.iter().all(|entry| !matches!(
            entry.entry,
            PublicOplogEntry::BeginAtomicRegion(_) | PublicOplogEntry::EndAtomicRegion(_)
        )),
        "isolated HTTP fixture must have no crossing atomic interval"
    );
    assert_surviving_references(entries)
}

fn assert_surviving_references(entries: &[PublicOplogEntryWithIndex]) -> anyhow::Result<()> {
    let deleted = |index| {
        entries.iter().any(|entry| {
            matches!(&entry.entry,
        PublicOplogEntry::Jump(jump) if jump.jump.contains(index))
        })
    };
    let survives = |index| {
        !deleted(index)
            && entries.iter().any(|entry| {
                entry.oplog_index == index && matches!(entry.entry, PublicOplogEntry::Start(_))
            })
    };
    for entry in entries.iter().filter(|entry| !deleted(entry.oplog_index)) {
        match &entry.entry {
            PublicOplogEntry::Start(start) => {
                ensure!(
                    start.parent_start_index.is_none_or(survives),
                    "orphaned parent: {entry:?}"
                );
                ensure!(
                    start.observational_owner.is_none_or(survives),
                    "orphaned owner: {entry:?}"
                );
                if let PublicDurableFunctionType::WriteRemoteBatched(params) =
                    &start.durable_function_type
                {
                    ensure!(
                        params.index.is_none_or(survives),
                        "orphaned HTTP scope: {entry:?}"
                    );
                }
            }
            PublicOplogEntry::End(end) => {
                ensure!(survives(end.start_index), "orphaned End: {entry:?}")
            }
            PublicOplogEntry::Cancelled(cancelled) => {
                ensure!(
                    survives(cancelled.start_index),
                    "orphaned Cancelled: {entry:?}"
                );
                anyhow::bail!("direct P2 HTTP must not invent cancellation: {entry:?}");
            }
            PublicOplogEntry::CompletionDelivered(marker) => {
                ensure!(survives(marker.start_index), "orphaned delivery: {entry:?}");
                anyhow::bail!("direct P2 HTTP must not invent delivery markers: {entry:?}");
            }
            PublicOplogEntry::CompletionDiscarded(marker) => {
                ensure!(survives(marker.start_index), "orphaned discard: {entry:?}");
                anyhow::bail!("direct P2 HTTP must not invent discard markers: {entry:?}");
            }
            _ => {}
        }
    }
    Ok(())
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
