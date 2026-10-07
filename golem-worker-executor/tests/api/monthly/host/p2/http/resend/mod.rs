mod peer;

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
use golem_worker_executor::worker::{
    MonthlyClockForTest, P2HttpPreSubscriptionOperationForTest,
    P2HttpPreSubscriptionSelectionForTest, P2NativeInputStateForTest, Worker,
};
use test_r::test;

use crate::api::monthly::support::{FUEL_BUDGET, Resource, check_accounting};
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
            pending_resend(
                last_unique_id,
                deps,
                http_tests,
                Resource::$resource,
                AgentMode::$mode,
                Scenario::MonthlyStop,
            )
            .await
        }
    };
}

#[derive(Clone, Copy, PartialEq)]
enum Scenario {
    MonthlyStop,
    CompletionBeforeStop,
    FailedRecoveryAssertion,
}

const RECOVERY_ASSERTION_FAILURE: &str = "intentional recovered-history assertion failure";

macro_rules! cleanup_case {
    ($name:ident, $resource:ident, $pre_subscription:expr) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            let error = run_resend(
                last_unique_id,
                deps,
                http_tests,
                Resource::$resource,
                AgentMode::Durable,
                Scenario::FailedRecoveryAssertion,
                $pre_subscription,
            )
            .await
            .expect_err("the recovered-history assertion must fail");
            ensure!(error.to_string() == RECOVERY_ASSERTION_FAILURE, "{error:#}");
            Ok(())
        }
    };
}

cleanup_case!(cleanup_after_recovery_memory, Memory, false);
cleanup_case!(cleanup_after_recovery_compute_prepaid, Compute, false);
cleanup_case!(cleanup_after_recovery_scripted_storage, Storage, false);
cleanup_case!(cleanup_after_recovery_pre_subscription, Memory, true);

pending_case!(durable_p2_http_resend_monthly_memory, Memory, Durable);

#[test]
#[timeout("2m")]
async fn durable_p2_http_resend_stop_before_subscription(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_resend(
        last_unique_id,
        deps,
        http_tests,
        Resource::Memory,
        AgentMode::Durable,
        Scenario::MonthlyStop,
        true,
    )
    .await
}
pending_case!(ephemeral_p2_http_resend_monthly_memory, Memory, Ephemeral);
pending_case!(
    durable_p2_http_resend_monthly_compute_prepaid,
    Compute,
    Durable
);
pending_case!(
    ephemeral_p2_http_resend_monthly_compute_prepaid,
    Compute,
    Ephemeral
);
pending_case!(
    durable_p2_http_resend_monthly_scripted_storage,
    Storage,
    Durable
);
pending_case!(
    ephemeral_p2_http_resend_monthly_scripted_storage,
    Storage,
    Ephemeral
);

#[test]
#[timeout("2m")]
async fn durable_p2_http_resend_completion_before_stop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    pending_resend(
        last_unique_id,
        deps,
        http_tests,
        Resource::Memory,
        AgentMode::Durable,
        Scenario::CompletionBeforeStop,
    )
    .await
}

async fn pending_resend(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    http_tests: &PrecompiledComponent,
    resource: Resource,
    mode: AgentMode,
    scenario: Scenario,
) -> anyhow::Result<()> {
    run_resend(
        last_unique_id,
        deps,
        http_tests,
        resource,
        mode,
        scenario,
        false,
    )
    .await
}

async fn run_resend(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    http_tests: &PrecompiledComponent,
    resource: Resource,
    mode: AgentMode,
    scenario: Scenario,
    pre_subscription: bool,
) -> anyhow::Result<()> {
    let completion_before_stop = scenario == Scenario::CompletionBeforeStop;
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
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
            config.http_client = HttpClientConfig::Enabled(HttpClientEnabledConfig {
                max_idle_per_host: 1,
                max_connections_per_host: 2,
                max_total_connections: 2,
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
    limits.run_batch_for_test().await;
    let baseline_updates = registry.applied_updates().len();

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
    let mut phases = worker.observe_p2_native_input_for_test();
    let mut pre_subscription_control = pre_subscription.then(|| {
        worker.pause_next_p2_http_pre_subscription_for_test(
            key.clone(),
            P2HttpPreSubscriptionOperationForTest::ResendResponseReady,
        )
    });
    let mut peer = Peer::start().await?;
    let authority = peer
        .url
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/body-wait")
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
                    "get_for_p2_body_wait",
                    data_value!(authority),
                )
                .await
        }
    });
    let mut invocation_joined = false;
    let result = async {
        let first = peer.request().await?;
        ensure!(first.number == 1 && first.socket == 1);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                executor.commit_oplog(&id).await?;
                let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                let reads = starts(&history, BODY_READ);
                if !reads.is_empty() && terminal_count(&history, reads[0]) == 1 { break Ok::<_, anyhow::Error>(()); }
                tokio::task::yield_now().await;
            }
        }).await.context("original byte h was never delivered")??;
        peer.release(1)?;
        peer.wait_closed(1).await?;
        let phase = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = phases.recv().await.context("P2 native HTTP body observer closed")?;
                ensure!(event.operation == "http_body_blocking_read" && event.invocation_key.as_ref() == Some(&key));
                let start = event.start_index.context("HTTP body observer omitted child Start")?;
                executor.commit_oplog(&id).await?;
                let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                if starts(&history, BODY_READ).len() == 2 && starts(&history, BODY_READ)[1] == start {
                    break Ok::<_, anyhow::Error>(event);
                }
                ensure!(event.state() != P2NativeInputStateForTest::Dropped, "first HTTP body read was dropped");
            }
        }).await.context("second native body read never parked")??;
        let second = peer.request().await?;
        ensure!(second.number == 2 && second.socket == 2
            && second.idempotency_key == first.idempotency_key);
        ensure!(phase.state() == P2NativeInputStateForTest::Returned,
            "original native body read must fail before resend");
        let resend = if let Some(control) = pre_subscription_control.as_mut() {
            let entered = tokio::time::timeout(Duration::from_secs(10), &mut control.entered)
                .await.context("resend did not reach pre-subscription gate")??;
            ensure!(entered.operation == P2HttpPreSubscriptionOperationForTest::ResendResponseReady
                && entered.invocation_key == key
                && Some(entered.start_index) == phase.start_index
                && entered.stream_rep == phase.stream_rep
                && entered.runtime == phase.runtime,
                "wrong pre-subscription wait: {entered:?}");
            ensure!(phases.try_recv().is_err(), "native resend future was polled before subscription gate");
            None
        } else {
            let resend = tokio::time::timeout(Duration::from_secs(10), phases.recv())
                .await.context("resend response ready never polled Pending")?
                .context("resend observer closed")?;
            ensure!(resend.operation == "http_body_resend_response_ready"
                && resend.invocation_key.as_ref() == Some(&key)
                && resend.start_index == phase.start_index
                && resend.stream_rep == phase.stream_rep
                && resend.runtime == phase.runtime
                && resend.state() == P2NativeInputStateForTest::Pending,
                "wrong native wait observed: {resend:?}");
            Some(resend)
        };
        executor.commit_oplog(&id).await?;
        let started = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let body_start = phase.start_index.unwrap();
        let scope_start = assert_pending_shape(&started, &key, body_start)?;
        info!(?resend, %scope_start, %body_start, "P2 response-body inline resend reached native wait");
        ensure!(count_agent_invocation_pair_since(&started, OplogIndex::INITIAL) == (2, 1));
        if completion_before_stop {
            ensure!(resend.as_ref().is_some_and(|event| event.state() == P2NativeInputStateForTest::Pending));
            ensure!(worker.concurrent_agent_permit_is_held().await && !invocation.is_finished());
            peer.assert_counts(2, 2)?;
            peer.release(2)?;
            let completed = tokio::time::timeout(Duration::from_secs(15), &mut invocation)
                .await.context("resend response completion")?;
            invocation_joined = true;
            ensure!(completed??.into_typed::<String>()? == "200 hi");
            ensure!(resend.as_ref().is_some_and(|event| event.state() == P2NativeInputStateForTest::Returned),
                "native resend-ready future did not complete normally");
            executor.commit_oplog(&id).await?;
            let settled = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            assert_completed_resend(&settled, &key, scope_start, body_start)?;
            ensure!(count_agent_invocation_pair_since(&settled, OplogIndex::INITIAL) == (2, 2));
            ensure!(worker.monthly_stop_for_test().is_none()
                && worker.frozen_stop_for_test().is_none()
                && attempts.try_recv().is_err(), "monthly stop won before completed resend");
            peer.assert_counts(2, 2)?;
            return Ok::<_, anyhow::Error>(());
        }
        tokio::time::timeout(Duration::from_secs(10), polls.recv()).await?.context("monitor timer closed")?;
        let initial = worker.current_monthly_proposal_for_test();
        let mut raw = worker.raw_interrupt_for_test();
        let acquisitions = worker.permit_acquisitions_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(initial.exhaustion.is_none() && phase.runtime == initial);
        ensure!(worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await && !invocation.is_finished());
        peer.assert_counts(2, 2)?;
        ensure!(resend.as_ref().is_none_or(|event| event.state() == P2NativeInputStateForTest::Pending));
        if pre_subscription {
            ensure!(phases.try_recv().is_err(), "resend native future was polled while gated");
        }
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
        if let Some(control) = pre_subscription_control.as_mut() {
            ensure!(worker.concurrent_agent_permit_is_held().await && !invocation.is_finished());
            peer.assert_counts(2, 2)?;
            ensure!(phases.try_recv().is_err(), "native resend ran before stop publication");
            control.release();
            let selected = tokio::time::timeout(Duration::from_secs(10), &mut control.selected)
                .await.context("resend did not select after gate release")??;
            ensure!(selected == P2HttpPreSubscriptionSelectionForTest::Interrupt(signal),
                "native-first resend selected wrong branch: {selected:?}");
        }
        if let Some((entered, release)) = teardown {
            ensure!(worker.concurrent_agent_permit_is_held().await);
            info!(%body_start, "typed HTTP body stop accepted while native read and permit remain pending");
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
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
        }).await.context("physical window closure")?;
        tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
        tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
        if let Some(refresh) = refresh { refresh.await?; }
        ensure!(worker.unload_succeeded_for_test() && worker.permit_acquisitions_for_test() == acquisitions);
        ensure!(clock.active_sleeps() == 0 && account.monthly_observer_count_for_test() == 0);
        peer.wait_closed(2).await?;
        if let Some(resend) = &resend {
            ensure!(resend.state() == P2NativeInputStateForTest::Dropped);
        } else if let Ok(event) = phases.try_recv() {
            ensure!(event.operation == P2HttpPreSubscriptionOperationForTest::ResendResponseReady.label()
                && event.invocation_key.as_ref() == Some(&key)
                && event.start_index == Some(body_start)
                && event.stream_rep == phase.stream_rep
                && event.runtime == initial
                && event.state() == P2NativeInputStateForTest::Dropped,
                "gated resend polled a different or completed native wait: {event:?}");
            ensure!(phases.try_recv().is_err(), "unexpected extra native wait after gated resend");
        }
        peer.assert_counts(2, 2)?;
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(assert_pending_shape(&stopped, &key, body_start)? == scope_start);
        ensure!(count_agent_invocation_pair_since(&stopped, OplogIndex::INITIAL) == (2, 1));
        let entries = worker.oplog().read_exact(OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64()).await;
        let retry_hints = entries.values().filter(|entry| matches!(entry,
            OplogEntry::Error { error: AgentError::TransientError(message), .. }
                if message == "in-function retry")).count();
        ensure!(retry_hints == 1, "native LastOperationFailed must enter one inline retry: {retry_hints}");
        let errors: Vec<_> = entries.values().filter_map(|entry| match entry {
            OplogEntry::Error { error: AgentError::TransientError(message), .. }
                if message == "in-function retry" => None,
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
            let failed = tokio::time::timeout(Duration::from_secs(10), &mut invocation).await?;
            invocation_joined = true;
            ensure!(failed?.is_err());
        }
        executor.wait_for_status(&id, if durable { AgentStatus::Suspended } else { AgentStatus::Failed }, Duration::from_secs(10)).await?;
        let observations = source.observations();
        limits.run_batch_for_test().await;
        check_accounting(&registry.applied_updates()[baseline_updates..], resource, durable, initial.policy_revision, initial.period)?;
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
        let third = peer.request().await?;
        ensure!(third.number == 3 && third.socket == 3
            && third.idempotency_key == first.idempotency_key);
        let resumed = tokio::time::timeout(Duration::from_secs(15), &mut invocation).await.context("P2 body reconstruction")?;
        invocation_joined = true;
        ensure!(resumed??.into_typed::<String>()? == "200 hi");
        let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_repaired_shape(&recovered, &key, scope_start, body_start)?;
        if scenario == Scenario::FailedRecoveryAssertion {
            let status = tokio::time::timeout(Duration::from_secs(10), worker.get_last_known_status())
                .await.context("recovered invocation status")?;
            ensure!(status.status == AgentStatus::Idle && status.pending_invocations.is_empty(),
                "reconstructed invocation did not become Idle: {status:?}");
            ensure!(worker.is_loaded().await, "reconstructed worker already unloaded");
            ensure!(false, "{RECOVERY_ASSERTION_FAILURE}");
        }
        ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
        peer.assert_counts(3, 3)?;
        let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key,
            "get_for_p2_body_wait", data_value!(peer.url.strip_prefix("http://").unwrap().strip_suffix("/body-wait").unwrap().to_owned())).await?;
        ensure!(cached.into_typed::<String>()? == "200 hi");
        peer.assert_counts(3, 3)?;
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
        let fresh_authority = peer.url.strip_prefix("http://").unwrap().strip_suffix("/body-wait").unwrap().to_owned();
        let fresh = executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key,
            "get_for_p2_body_wait", data_value!(fresh_authority));
        let response = async {
            let fourth = peer.request().await?;
            ensure!(fourth.number == 4 && fourth.socket == 4
                && fourth.idempotency_key != first.idempotency_key,
                "completed P2 history must not reissue a request");
            Ok::<_, anyhow::Error>(())
        };
        let (fresh, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(fresh, response) }).await??;
        ensure!(fresh.into_typed::<String>()? == "200 hi");
        peer.assert_counts(4, 4)?;
        let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(history.len() > recovered.len());
        assert_repaired_shape(&history[..recovered.len()], &key, scope_start, body_start)?;
        ensure!(starts(&history, SCOPE).len() == 2
            && starts(&history, SCOPE)[1] > recovered.last().unwrap().oplog_index);
        assert_invocation(&history, &fresh_key, 1)?;
        ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (3, 3));
        assert_settled_history(&history, body_start)?;
        } else {
            let probe_name = agent_id!("HttpClient4");
            let probe_key = IdempotencyKey::fresh();
            let probe_authority = peer.url.strip_prefix("http://").unwrap().strip_suffix("/body-wait").unwrap().to_owned();
            let probe = executor.invoke_and_await_agent_with_key(&component, &probe_name, &probe_key,
                "get_for_p2_body_wait", data_value!(probe_authority));
            let response = async {
                let third = peer.request().await?;
                ensure!(third.number == 3 && third.socket == 3
                    && third.idempotency_key != first.idempotency_key);
                Ok::<_, anyhow::Error>(())
            };
            let (probe, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(probe, response) }).await??;
            ensure!(probe.into_typed::<String>()? == "200 hi");
            let failed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(assert_pending_shape(&failed, &key, body_start)? == scope_start);
            ensure!(count_agent_invocation_pair_since(&failed, OplogIndex::INITIAL) == (2, 1));
            ensure!(worker.permit_acquisitions_for_test() == acquisitions);
            ensure!(!worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await);
            peer.assert_counts(3, 3)?;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        peer.assert_counts(if durable { 4 } else { 3 }, if durable { 4 } else { 3 })?;
        Ok::<_, anyhow::Error>(())
    }.await;
    let mut cleanup_errors = Vec::new();
    if result.is_err() {
        if let Some(control) = pre_subscription_control.as_mut() {
            control.release();
        }
        peer.release_all();
        match tokio::time::timeout(Duration::from_secs(10), executor.interrupt(&id)).await {
            Ok(Ok(())) => {}
            other => cleanup_errors.push(anyhow!("cleanup interrupt: {other:?}")),
        }
        match tokio::time::timeout(
            Duration::from_secs(10),
            worker.join_accepted_stops_for_test(),
        )
        .await
        {
            Ok(Ok(())) => {}
            other => cleanup_errors.push(anyhow!("cleanup stop join: {other:?}")),
        }
        // Public interrupt leaves Idle workers loaded; join their physical unload explicitly.
        if let Err(error) = tokio::time::timeout(Duration::from_secs(10), worker.test_stop()).await
        {
            cleanup_errors.push(anyhow!("cleanup physical stop: {error:#}"));
        }
        match tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test())
            .await
        {
            Ok(Ok(())) => {}
            other => cleanup_errors.push(anyhow!("cleanup retained work: {other:?}")),
        }
    }
    if !invocation_joined {
        match tokio::time::timeout(Duration::from_secs(10), &mut invocation).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => cleanup_errors.push(anyhow!("invocation join: {error:#}")),
            Err(error) => {
                invocation.abort();
                if let Err(join_error) =
                    tokio::time::timeout(Duration::from_secs(10), &mut invocation).await
                {
                    cleanup_errors.push(anyhow!("invocation abort join: {join_error:#}"));
                }
                cleanup_errors.push(anyhow!("invocation join timed out: {error:#}"));
            }
        }
    }
    if let Err(error) = peer.finish().await {
        cleanup_errors.push(anyhow!("peer join: {error:#}"));
    }
    if scenario == Scenario::FailedRecoveryAssertion && cleanup_errors.is_empty() {
        ensure!(
            !worker.is_loaded().await && !worker.concurrent_agent_permit_is_held().await,
            "assertion-failure cleanup retained a loaded runtime or permit"
        );
        ensure!(
            worker.unload_succeeded_for_test(),
            "assertion-failure unload did not succeed"
        );
        let account = limits.initialize_account(context.account_id).await?;
        ensure!(
            clock.active_sleeps() == 0 && account.monthly_observer_count_for_test() == 0,
            "assertion-failure cleanup retained a monitor or observer"
        );
    }
    match (result, cleanup_errors.is_empty()) {
        (Ok(()), true) => Ok(()),
        (Err(primary), true) => Err(primary),
        (Ok(()), false) => Err(anyhow!("cleanup failed: {cleanup_errors:#?}")),
        (Err(primary), false) => Err(anyhow!("{primary:#}; cleanup failed: {cleanup_errors:#?}")),
    }
}

const SCOPE: &str = "<scope:batched-write>";
const RESPONSE_GET: &str = "http::types::future_incoming_response::get";
const POLL: &str = "io::poll::poll";
const BODY_READ: &str = "http::types::incoming_body_stream::blocking_read";

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

fn end_index(entries: &[PublicOplogEntryWithIndex], start: OplogIndex) -> Option<OplogIndex> {
    entries.iter().find_map(|entry| match &entry.entry {
        PublicOplogEntry::End(end) if end.start_index == start => Some(entry.oplog_index),
        _ => None,
    })
}

fn assert_body_child(
    entries: &[PublicOplogEntryWithIndex],
    child: OplogIndex,
    scope: OplogIndex,
) -> anyhow::Result<()> {
    let record = entries
        .iter()
        .find(|entry| entry.oplog_index == child)
        .context("body child Start missing")?;
    let PublicOplogEntry::Start(start) = &record.entry else {
        anyhow::bail!("body child record was not Start: {record:?}");
    };
    ensure!(
        start.function_name == BODY_READ
            && start.parent_start_index == Some(scope)
            && start.observational_owner.is_none()
            && start.request.is_some()
            && matches!(
                &start.durable_function_type,
                PublicDurableFunctionType::ReadRemote(_)
            ),
        "wrong P2 body child: {record:?}"
    );
    Ok(())
}

fn assert_pending_shape(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    open: OplogIndex,
) -> anyhow::Result<OplogIndex> {
    let scopes = starts(entries, SCOPE);
    let reads = starts(entries, BODY_READ);
    ensure!(
        scopes.len() == 1 && reads.len() == 2 && reads[1] == open,
        "P2 HTTP body scope and first completed/second pending direct read: {entries:#?}"
    );
    let scope = scopes[0];
    ensure!(scope < reads[0] && reads[0] < open);
    let response_gets = starts(entries, RESPONSE_GET);
    ensure!(
        !response_gets.is_empty(),
        "P2 response headers were not retrieved"
    );
    for entry in entries {
        if let PublicOplogEntry::Start(start) = &entry.entry {
            if start.function_name == RESPONSE_GET {
                ensure!(
                    start.parent_start_index == Some(scope)
                        && matches!(&start.durable_function_type,
                        PublicDurableFunctionType::WriteRemoteBatched(params) if params.index == Some(scope))
                        && terminal_count(entries, entry.oplog_index) == 1
                );
            }
            if start.function_name == POLL {
                ensure!(
                    matches!(
                        start.durable_function_type,
                        PublicDurableFunctionType::ReadLocal(_)
                    ) && terminal_count(entries, entry.oplog_index) == 1
                );
            }
        }
    }
    let scope_record = entries
        .iter()
        .find(|entry| entry.oplog_index == scope)
        .unwrap();
    ensure!(matches!(&scope_record.entry, PublicOplogEntry::Start(start)
        if start.parent_start_index.is_none() && start.observational_owner.is_none()
            && start.request.is_none()
            && matches!(&start.durable_function_type, PublicDurableFunctionType::WriteRemoteBatched(params) if params.index.is_none())));
    for child in reads.iter().copied() {
        assert_body_child(entries, child, scope)?;
    }
    let first_end =
        end_index(entries, reads[0]).context("first body byte not durably completed")?;
    ensure!(first_end < open && terminal_count(entries, reads[0]) == 1);
    ensure!(terminal_count(entries, scope) == 0 && terminal_count(entries, open) == 0);
    ensure!(entries.iter().all(|entry| !matches!(&entry.entry,
        PublicOplogEntry::Start(start) if start.function_name == "http::client::send")));
    ensure!(entries.iter().all(|entry| !matches!(
        entry.entry,
        PublicOplogEntry::Jump(_)
            | PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::CompletionDelivered(_)
            | PublicOplogEntry::CompletionDiscarded(_)
    )));
    for entry in entries {
        if matches!(entry.entry, PublicOplogEntry::Start(_))
            && entry.oplog_index != scope
            && entry.oplog_index != open
        {
            ensure!(
                terminal_count(entries, entry.oplog_index) == 1,
                "unexpected incomplete non-body child: {entry:?}"
            );
        }
    }
    assert_invocation(entries, key, 0)?;
    Ok(scope)
}

fn assert_completed_resend(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    scope: OplogIndex,
    second_read: OplogIndex,
) -> anyhow::Result<()> {
    let reads = starts(entries, BODY_READ);
    ensure!(starts(entries, SCOPE) == [scope] && reads.len() >= 3 && reads[1] == second_read);
    for child in reads {
        assert_body_child(entries, child, scope)?;
    }
    let scope_end = end_index(entries, scope).context("successful request scope not closed")?;
    let finished = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("completed invocation missing Finished")?
        .oplog_index;
    ensure!(scope_end < finished);
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_) => ensure!(
                terminal_count(entries, entry.oplog_index) == 1 && entry.oplog_index < finished,
                "unsettled successful child: {entry:?}"
            ),
            PublicOplogEntry::End(_) => ensure!(entry.oplog_index < finished),
            PublicOplogEntry::Error(error) if error.error == "in-function retry" => {}
            PublicOplogEntry::Jump(_)
            | PublicOplogEntry::Suspend(_)
            | PublicOplogEntry::Cancelled(_)
            | PublicOplogEntry::Error(_) => {
                anyhow::bail!("unexpected successful resend entry: {entry:?}")
            }
            _ => {}
        }
    }
    assert_invocation(entries, key, 1)
}

fn assert_repaired_shape(
    entries: &[PublicOplogEntryWithIndex],
    key: &IdempotencyKey,
    scope: OplogIndex,
    abandoned: OplogIndex,
) -> anyhow::Result<()> {
    let jumps = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Jump(jump) => Some((entry.oplog_index, jump)),
            _ => None,
        })
        .collect::<Vec<_>>();
    ensure!(
        jumps.len() == 1,
        "expected one replacement of the interrupted P2 request: {jumps:?}"
    );
    let (jump_idx, jump) = jumps[0];
    ensure!(
        jump.jump.start == scope.next() && jump.jump.end.next() == jump_idx,
        "replace the complete attempt suffix through the pre-Jump horizon: {jump:?}"
    );
    ensure!(scope < abandoned && abandoned < jump_idx && terminal_count(entries, abandoned) == 0);
    assert_repair_suffix(entries, scope, abandoned, jump_idx)?;
    info!(%scope, %abandoned, %jump_idx, deleted_start = %jump.jump.start,
        deleted_end = %jump.jump.end, "P2 body request scope retained and interrupted child Jump-replaced");
    let reads = starts(entries, BODY_READ);
    ensure!(
        reads.len() >= 4 && reads[1] == abandoned && reads[2] > jump_idx,
        "replayed P2 request did not replace body reads: {reads:?}"
    );
    ensure!(
        starts(entries, SCOPE) == [scope]
            && terminal_count(entries, reads[0]) == 1
            && end_index(entries, reads[0]).is_some_and(|end| end < abandoned)
    );
    for child in reads.iter().copied() {
        assert_body_child(entries, child, scope)?;
    }
    for child in reads.iter().copied().filter(|child| *child != abandoned) {
        ensure!(
            terminal_count(entries, child) == 1,
            "unsettled replacement child {child}"
        );
    }
    let scope_end = end_index(entries, scope).context("retained request scope not closed")?;
    let finished = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("retained invocation not finished")?
        .oplog_index;
    ensure!(
        terminal_count(entries, scope) == 1
            && reads
                .iter()
                .copied()
                .filter(|read| *read > jump_idx)
                .all(|read| end_index(entries, read).is_some_and(|end| end < scope_end))
            && scope_end < finished
    );
    assert_invocation(entries, key, 1)
}

fn assert_settled_history(
    entries: &[PublicOplogEntryWithIndex],
    deleted: OplogIndex,
) -> anyhow::Result<()> {
    assert_surviving_references(entries)?;
    let finished = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .context("last invocation Finished missing")?
        .oplog_index;
    for entry in entries {
        match &entry.entry {
            PublicOplogEntry::Start(_) => {
                ensure!(
                    terminal_count(entries, entry.oplog_index)
                        == usize::from(entry.oplog_index != deleted),
                    "unsettled/duplicate Start: {entry:?}"
                );
                ensure!(entry.oplog_index < finished);
            }
            PublicOplogEntry::End(_) => ensure!(entry.oplog_index < finished),
            PublicOplogEntry::Error(error) if error.error == "in-function retry" => {}
            PublicOplogEntry::Cancelled(_) | PublicOplogEntry::Error(_) => {
                anyhow::bail!("unexpected terminal: {entry:?}");
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
