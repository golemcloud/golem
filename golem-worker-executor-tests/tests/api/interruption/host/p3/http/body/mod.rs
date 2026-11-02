mod history;
mod peer;

use super::*;
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry, OplogErrorKind};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::{HasOplog, UsesAllDeps};
use golem_worker_executor::worker::Worker;
use test_r::test;

use crate::api::interruption::support::{FUEL_BUDGET, Resource, check_accounting};
use golem_worker_executor::services::golem_config::{HttpClientConfig, HttpClientEnabledConfig};
use history::{assert_invocation, pending_body, recovered_body};
use peer::Peer;

macro_rules! pending_case {
    ($name:ident, Storage, $mode:ident) => {
        pending_case!(#[cfg(target_os = "linux")] #[ignore = "requires the privileged managed XFS test runner"] $name, Storage, $mode);
    };
    ($(#[$attr:meta])* $name:ident, $resource:ident, $mode:ident) => {
        $(#[$attr])*
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            pending_response_body(
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

pending_case!(durable_p3_http_body_memory_quota, Memory, Durable);
pending_case!(ephemeral_p3_http_body_memory_quota, Memory, Ephemeral);
pending_case!(durable_p3_http_body_compute_quota_prepaid, Compute, Durable);
pending_case!(
    ephemeral_p3_http_body_compute_quota_prepaid,
    Compute,
    Ephemeral
);
pending_case!(
    durable_p3_http_body_managed_xfs_storage_quota,
    Storage,
    Durable
);
pending_case!(
    ephemeral_p3_http_body_managed_xfs_storage_quota,
    Storage,
    Ephemeral
);

async fn pending_response_body(
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
    let filesystem = managed_filesystem_config(metering)?;
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| {
            config.resource_usage_metering = metering;
            if let Some(filesystem) = &filesystem {
                config.filesystem_storage.mode = filesystem.clone();
            }
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
        while !seed.stop_if_idle().await {
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
    let admitted_revision = registry.current_limits().monthly_policy_revision;
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
                    "get_for_quota_wait",
                    data_value!(url),
                )
                .await
        }
    });
    let result = async {
        let first = peer.request().await?;
        ensure!(first.number == 1);
        ensure!(first.range.is_none());
        let original = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                executor.commit_oplog(&id).await?;
                let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                if let Some(pending) = pending_body(&entries, &key)? {
                    ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
                    info!(?entries, ?pending, "Committed first chunk delivered; next child and consume-body parent open");
                    return Ok::<_, anyhow::Error>(pending);
                }
                tokio::task::yield_now().await;
            }
        }).await.context("committed delivered first chunk and open next child under unfinished method")??;
        let committed_prefix = executor.stored_oplog(&id).await;
        let mut raw = worker.raw_interrupt_for_test();
        let account = limits.initialize_account(context.account_id).await?;
        peer.assert_counts(1, 0, 0)?;
        ensure!(worker.is_loaded().await);
        ensure!(!invocation.is_finished());
        info!(
            ?resource, ?mode, %key, ?id, ?original, http_key = %first.idempotency_key, requests = 1,
            "Peer sent headers and one byte; committed delivered chunk; next read and parent open; peer holds remaining bytes and EOF"
        );

        ensure!(!invocation.is_finished());
        peer.assert_counts(1, 0, 0)?;
        if resource == Resource::Storage {
            allocate_storage_with_billing_refresh(&owned, &limits, &registry, &policy, durable).await?;
        }
        let mut exhausted = policy.clone();
        match resource {
            Resource::Memory => exhausted.available_memory_gb_seconds = 0,
            Resource::Compute => exhausted.available_fuel = 0,
            Resource::Storage if durable => exhausted.available_durable_storage_byte_seconds = 0,
            Resource::Storage => exhausted.available_ephemeral_storage_byte_seconds = 0,
        }
        registry.set_policy(exhausted);
        let refresh = Some(applied_refresh(&limits, &registry, context.account_id).await?);
        let signal = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        ensure!(matches!(signal, InterruptKind::Suspend(_)));
        ensure!(worker.selected_owner_failure_for_test().await == Some(ToolOwnerFailureMetadata::Lifecycle));
        if resource == Resource::Storage {
            let other = if durable {
                AgentMode::Ephemeral
            } else {
                AgentMode::Durable
            };
            ensure!(!account.monthly_capacity_is_exhausted_for_test(other));
        }
        // Keep the remaining bytes and EOF withheld until physical unload.
        peer.first_closed().await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await {
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
        drop(first.finish_body);
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        assert_invocation(&stopped, &key, 0)?;
        ensure!(pending_body(&stopped, &key)? == Some(original), "stop changed the retained body prefix: {stopped:#?}");
        peer.assert_counts(1, 0, 1)?;
        let entries = worker
            .oplog()
            .read_exact(
                OplogIndex::INITIAL,
                worker.oplog().current_oplog_index().await.as_u64(),
            )
            .await?;
        for entry in entries.values() {
            match entry {
                OplogEntry::Error { kind, entity_parent_start_index, .. } => {
                    ensure!(*kind == OplogErrorKind::Invocation && entity_parent_start_index.is_none(), "primary invocation error, not recovery: {entry:?}");
                }
                OplogEntry::Interrupted { .. } | OplogEntry::Exited { .. } => {
                    anyhow::bail!("unexpected lifecycle terminal: {entry:?}");
                }
                _ => {}
            }
        }
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
        flush_stopped_usage(&limits, &registry, resource).await?;
        let settled = registry.applied_updates();
        check_accounting(
            &settled,
            resource,
            durable,
            admitted_revision + u64::from(resource == Resource::Storage),
            policy.period,
        )?;
        info!(?resource, ?mode, ?settled, "HTTP stopped execution window settlement");
        let settled_count = settled.len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        limits.run_batch_for_test().await;
        if resource == Resource::Storage { assert_storage_deleted(&owned)?; }
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
            ?resource, ?mode, ?original, requests = 1,
            "Monthly stop closed partial HTTP body without remaining bytes; parent and next child remain open"
        );

        if durable {
            let mut grant = policy;
            if resource == Resource::Storage {
                grant.available_durable_storage_byte_seconds = u64::MAX;
            }
            registry.set_policy(grant);
            applied_refresh(&limits, &registry, context.account_id).await?.await?;
            executor.resume(&id, false).await?;
            let second = peer.request().await?;
            ensure!(second.number == 2);
            ensure!(second.idempotency_key == first.idempotency_key);
            ensure!(second.range.as_deref() == Some("bytes=1-"));
            let repaired_pending = tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    executor.commit_oplog(&id).await?;
                    let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
                    if let Some(pending) = history::retained_pending(&entries, &key, original)? {
                        ensure!(count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL) == (2, 1));
                        info!(?entries, ?pending, "Body repair retains the original parent and chunk Starts");
                        return Ok::<_, anyhow::Error>(pending);
                    }
                    tokio::task::yield_now().await;
                }
            }).await.context("original body parent and chunk identities after reissue")??;
            peer.assert_counts(2, 0, 1)?;
            second.finish_body.send(()).map_err(|_| anyhow::anyhow!("reissued HTTP body gate closed"))?;
            let resumed = tokio::time::timeout(Duration::from_secs(15), &mut invocation)
                .await.context("retained HTTP body invocation and mandatory finalization")???;
            ensure!(resumed.into_typed::<Result<String, String>>()? == Ok("200 hello-body".to_string()));
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            let raw_recovered = executor.stored_oplog(&id).await;
            ensure!(raw_recovered.len() >= committed_prefix.len());
            ensure!(&raw_recovered[..committed_prefix.len()] == committed_prefix.as_slice(), "complete committed prefix must survive body repair");
            info!(?recovered, "HTTP body recovery oplog after mandatory finalization");
            recovered_body(&recovered, &key, original, repaired_pending)?;
            assert_invocation(&recovered, &key, 1)?;
            ensure!(count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            let cached = executor.invoke_and_await_agent_with_key(&component, &name, &key, "get_for_quota_wait", data_value!(peer.url.clone())).await?;
            ensure!(cached.into_typed::<Result<String, String>>()? == Ok("200 hello-body".to_string()));
            peer.assert_counts(2, 1, 1)?;
            info!(?resource, %key, ?original, ?repaired_pending, requests = 2,
                "Retained method finished once with one body; stable HTTP key across two external requests, not external exactly-once");

            tokio::time::timeout(Duration::from_secs(10), async {
                while !worker.stop_if_idle().await { tokio::task::yield_now().await; }
            }).await.context("idle eviction before completed HTTP body replay")?;
            tokio::time::timeout(Duration::from_secs(10), async {
                while worker.is_loaded().await { tokio::task::yield_now().await; }
            }).await?;
            tokio::time::timeout(Duration::from_secs(10), worker.join_accepted_stops_for_test()).await??;
            tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test()).await??;
            let stored = tokio::time::timeout(Duration::from_secs(15), executor.invoke_and_await_agent(&component, &name, "stored_send_error", data_value!())).await??;
            ensure!(stored.into_typed::<String>()? == "none");
            peer.assert_counts(2, 1, 1)?;
            let completed_history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            recovered_body(&completed_history, &key, original, repaired_pending)?;
            ensure!(count_agent_invocation_pair_since(&completed_history, OplogIndex::INITIAL) == (3, 3));

            let fresh_key = IdempotencyKey::fresh();
            let fresh = executor.invoke_and_await_agent_with_key(&component, &name, &fresh_key, "get_for_quota_wait", data_value!(peer.url.clone()));
            let response = async {
                let third = peer.request().await?;
                ensure!(third.number == 3);
                ensure!(third.range.is_none());
                ensure!(third.idempotency_key != first.idempotency_key, "completed HTTP body history must not reissue a request");
                third.finish_body.send(()).map_err(|_| anyhow::anyhow!("fresh HTTP body gate closed"))?;
                Ok::<_, anyhow::Error>(())
            };
            let (fresh, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(fresh, response) }).await??;
            ensure!(fresh.into_typed::<Result<String, String>>()? == Ok("200 hello-body".to_string()));
            peer.assert_counts(3, 2, 1)?;
            let history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(count_agent_invocation_pair_since(&history, OplogIndex::INITIAL) == (4, 4));
            history::assert_settled_history(&history, original)?;
            assert_invocation(&history, &key, 1)?;
            assert_invocation(&history, &fresh_key, 1)?;
            ensure!(history::starts(&history, history::SEND).len() == 2);
            info!(?resource, %key, ?original, requests = 3, "Completed body history replayed offline; fresh HTTP invocation completed");
        } else {
            let mut grant = policy;
            grant.available_durable_storage_byte_seconds = u64::MAX;
            grant.available_ephemeral_storage_byte_seconds = u64::MAX;
            registry.set_policy(grant);
            applied_refresh(&limits, &registry, context.account_id).await?.await?;
            ensure!(tokio::time::timeout(Duration::from_secs(10), executor.invoke_and_await_agent_with_key(&component, &name, &key, "get_for_quota_wait", data_value!(peer.url.clone()))).await?.is_err());
            peer.assert_counts(1, 0, 1)?;
            // Only an independent durable invocation may reuse the single HTTP pool slot.
            let probe_name = agent_id!("HttpClient4");
            let probe_key = IdempotencyKey::fresh();
            let probe = executor.invoke_and_await_agent_with_key(&component, &probe_name, &probe_key, "get_for_quota_wait", data_value!(peer.url.clone()));
            let response = async {
                let second = peer.request().await?;
                ensure!(second.number == 2);
                ensure!(second.idempotency_key != first.idempotency_key);
                ensure!(second.range.is_none());
                second.finish_body.send(()).map_err(|_| anyhow::anyhow!("pool probe body gate closed"))?;
                Ok::<_, anyhow::Error>(())
            };
            let (probe, ()) = tokio::time::timeout(Duration::from_secs(15), async { tokio::try_join!(probe, response) }).await??;
            ensure!(probe.into_typed::<Result<String, String>>()? == Ok("200 hello-body".to_string()));
            let failed = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(pending_body(&failed, &key)? == Some(original));
            assert_invocation(&failed, &key, 0)?;
            ensure!(count_agent_invocation_pair_since(&failed, OplogIndex::INITIAL) == (2, 1));
            ensure!(failed.iter().filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))).count() == 1);
            ensure!(!failed.iter().any(|entry| matches!(entry.entry, PublicOplogEntry::Suspend(_))));
            executor.wait_for_status(&id, AgentStatus::Failed, Duration::from_secs(10)).await?;
            ensure!(!worker.is_loaded().await);
            peer.assert_counts(2, 1, 1)?;
            info!(?resource, %key, ?original, requests = 2, "Ephemeral HTTP body failure stayed terminal; independent durable Worker reused single pool slot");
        }
        // The accept loop remains active to reject extra network attempts after completion.
        tokio::time::sleep(Duration::from_millis(100)).await;
        peer.assert_counts(if durable { 3 } else { 2 }, if durable { 2 } else { 1 }, 1)?;
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
