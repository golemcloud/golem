use super::*;
use anyhow::{Context as _, ensure};
use golem_common::model::PromiseId;
use golem_common::model::oplog::host_functions::GolemApiGenerateIdempotencyKey;
use golem_common::model::oplog::payload::HostPayloadPair;
use golem_common::model::oplog::{DurableInvocationId, OplogEntry};
use golem_worker_executor::services::HasOplog;
use golem_worker_executor::services::oplog::CommitLevel;
use golem_worker_executor::worker::Worker;
use golem_worker_executor::workerctx::default::Context;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use test_r::test;

const CUSTOM_WAIT: &str = "golem-it::test-promise";

async fn history(worker: &Worker<Context>) -> BTreeMap<OplogIndex, OplogEntry> {
    worker
        .oplog()
        .read_exact(
            OplogIndex::INITIAL,
            worker.oplog().current_oplog_index().await.as_u64(),
        )
        .await
}

fn roots(
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    key: &IdempotencyKey,
) -> Vec<(OplogIndex, DurableInvocationId)> {
    let anchors: Vec<_> = entries
        .iter()
        .filter_map(|(index, entry)| match entry {
            OplogEntry::AgentInvocationStarted {
                idempotency_key, ..
            } if idempotency_key == key => Some(*index),
            _ => None,
        })
        .collect();
    assert!(
        anchors.len() <= 1,
        "reconstruction must retain the original invocation acceptance"
    );
    let Some(anchor) = anchors.first() else {
        return Vec::new();
    };
    let mut calls: Vec<_> = entries
        .iter()
        .filter_map(|(index, entry)| match entry {
            OplogEntry::Start {
                function_name,
                invocation_id: Some(id),
                parent_start_index,
                observational_owner,
                ..
            } if function_name.to_string() == CUSTOM_WAIT && id.anchor == *anchor => {
                assert!(
                    parent_start_index.is_none(),
                    "concurrent custom operations must remain siblings"
                );
                assert!(observational_owner.is_none());
                Some((*index, *id))
            }
            _ => None,
        })
        .collect();
    calls.sort_by_key(|(_, id)| id.ordinal);
    assert!(
        calls.len() <= 2,
        "repair must not begin another custom invocation"
    );
    if calls.len() == 2 {
        assert_ne!(calls[0].1, calls[1].1);
    }
    calls
}

fn observations(entries: &BTreeMap<OplogIndex, OplogEntry>, owner: OplogIndex) -> Vec<OplogIndex> {
    entries
        .iter()
        .filter_map(|(index, entry)| match entry {
            OplogEntry::Start {
                observational_owner: Some(actual),
                ..
            } if *actual == owner => Some(*index),
            _ => None,
        })
        .collect()
}

fn finished_for_key(
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    key: &IdempotencyKey,
) -> Vec<OplogIndex> {
    let mut current = false;
    let mut finished = Vec::new();
    for (index, entry) in entries {
        match entry {
            OplogEntry::AgentInvocationStarted {
                idempotency_key, ..
            } => {
                current = idempotency_key == key;
            }
            OplogEntry::AgentInvocationFinished { .. } if current => finished.push(*index),
            _ => {}
        }
    }
    finished
}

fn assert_completed(
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    original: &[(OplogIndex, DurableInvocationId)],
    key: &IdempotencyKey,
) {
    assert_eq!(roots(entries, key), original);
    for (start, _) in original {
        assert_eq!(entries.values().filter(|entry| matches!(entry, OplogEntry::End { start_index, .. } if start_index == start)).count(), 1);
        assert_eq!(entries.values().filter(|entry| matches!(entry, OplogEntry::CompletionDelivered { start_index, .. } if start_index == start)).count(), 1);
        assert!(!entries.values().any(|entry| matches!(entry, OplogEntry::Cancelled { start_index, .. } | OplogEntry::CompletionDiscarded { start_index, .. } if start_index == start)));
    }
    let root_calls: Vec<_> = entries
        .values()
        .filter_map(|entry| match entry {
            OplogEntry::Start {
                function_name,
                invocation_id: Some(id),
                parent_start_index,
                observational_owner,
                ..
            } if function_name == &GolemApiGenerateIdempotencyKey::HOST_FUNCTION_NAME
                && id.anchor == original[0].1.anchor =>
            {
                assert!(parent_start_index.is_none() && observational_owner.is_none());
                Some(*id)
            }
            _ => None,
        })
        .collect();
    assert_eq!(root_calls.len(), 1);
    assert_eq!(root_calls[0].ordinal, original[1].1.ordinal + 1);
    let finished = finished_for_key(entries, key);
    assert_eq!(finished.len(), 1);
    assert!(!entries.iter().any(|(index, entry)| *index > finished[0]
        && matches!(entry, OplogEntry::Start { .. } | OplogEntry::End { .. })));
    assert!(
        !entries
            .values()
            .any(|entry| matches!(entry, OplogEntry::Jump { .. }))
    );
}

async fn physical_stop(worker: &Worker<Context>) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(15), async {
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
        worker.join_accepted_stops_for_test().await?;
        worker.retained_cleanup_for_test().await
    })
    .await
    .context("custom-operation Store and permit were not released")??;
    Ok(())
}

#[test]
#[timeout("2m")]
async fn concurrent_custom_promises_auto_suspend_and_reconstruct_original_scopes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let registry = Arc::new(MutableResourceLimitsRegistry::new(available_quota_policy()));
    let shutdown = CancellationToken::new();
    let _shutdown = shutdown.clone().drop_guard();
    let metering = ResourceUsageMeteringConfig {
        compute: false,
        memory: false,
        filesystem: false,
    };
    let limits = ResourceLimitsGrpc::new(
        registry,
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown,
    );
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits,
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let name = agent_id!("CustomDurability", "custom-promise-auto-suspend");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    let first = executor
        .invoke_and_await_agent(&component, &name, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let second = executor
        .invoke_and_await_agent(&component, &name, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    assert_eq!(first.agent_id, id);
    assert_eq!(second.agent_id, id);
    let worker = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
        .await
        .context("custom owner not cached")?
        .primary();
    let generation = worker.resident_generation_for_test();
    let key = IdempotencyKey::fresh();
    let inputs = (first.clone(), second.clone());
    let invocation = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
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
                    "concurrent_promises",
                    data_value!(inputs.0, inputs.1),
                )
                .await
        }
    }));
    executor
        .wait_for_status(&id, AgentStatus::Suspended, Duration::from_secs(20))
        .await?;
    physical_stop(&worker).await?;
    assert!(!invocation.is_finished());
    let stopped = history(&worker).await;
    let original = roots(&stopped, &key);
    assert_eq!(original.len(), 2);
    assert_eq!(
        stopped
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
            .count(),
        1
    );
    for (start, _) in &original {
        assert!(!observations(&stopped, *start).is_empty());
        assert!(!stopped.values().any(
            |entry| matches!(entry, OplogEntry::End { start_index, .. } if start_index == start)
        ));
    }
    let prefix = executor.stored_oplog(&id).await;
    executor.complete_promise(&first, b"first".to_vec()).await?;
    executor
        .complete_promise(&second, b"second".to_vec())
        .await?;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(20), invocation)
            .await???
            .into_typed::<Vec<String>>()?,
        vec!["first", "second"]
    );
    assert!(worker.resident_generation_for_test() > generation);
    assert_completed(&history(&worker).await, &original, &key);
    let after = executor.stored_oplog(&id).await;
    assert!(after.len() >= prefix.len());
    assert_eq!(&after[..prefix.len()], prefix.as_slice());
    Ok(())
}

#[test]
#[timeout("2m")]
async fn quota_stop_retains_pending_custom_finish_and_replays_its_markerless_end(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    quota_custom_finish(last_unique_id, deps, fixture, 0).await
}

#[test]
#[timeout("2m")]
async fn quota_stop_retains_second_sibling_pending_custom_finish(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    quota_custom_finish(last_unique_id, deps, fixture, 1).await
}

async fn quota_custom_finish(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    completing_sibling: usize,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let available = available_quota_policy();
    let registry = Arc::new(MutableResourceLimitsRegistry::new(available.clone()));
    let shutdown = CancellationToken::new();
    let _shutdown = shutdown.clone().drop_guard();
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
        shutdown,
    );
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits.clone(),
        Arc::new(move |config| {
            config.resource_usage_metering = metering;
            config.suspend.wait_suspend_grace = Duration::from_secs(300);
        }),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let name = agent_id!("CustomDurability", "custom-promise-quota-finish");
    let id = executor.start_agent(&component.id, name.clone()).await?;
    let first = executor
        .invoke_and_await_agent(&component, &name, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let second = executor
        .invoke_and_await_agent(&component, &name, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let worker = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
        .await
        .context("custom owner not cached")?
        .primary();
    let generation = worker.resident_generation_for_test();
    let key = IdempotencyKey::fresh();
    let inputs = (first.clone(), second.clone());
    let invocation = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
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
                    "concurrent_promises",
                    data_value!(inputs.0, inputs.1),
                )
                .await
        }
    }));
    let original = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            worker.commit_oplog_and_update_state(CommitLevel::Always).await?;
            let entries = history(&worker).await;
            let calls = roots(&entries, &key);
            if calls.len() == 2 && calls.iter().all(|(start, _)| entries.values().any(|entry| matches!(entry,
                OplogEntry::Start { function_name, observational_owner: Some(owner), .. }
                    if owner == start && function_name.to_string() == "golem::api::get_promise_result"))) {
                return Ok::<_, anyhow::Error>(calls);
            }
            ensure!(!invocation.is_finished(), "custom body returned before both promise waits");
            tokio::task::yield_now().await;
        }
    }).await.context("both scoped promise waits must enter before quota exhaustion")??;
    let (accepted, release) = executor.gate_oplog_function_end_at(
        &id,
        golem_common::model::oplog::host_functions::HostFunctionName::Custom(
            CUSTOM_WAIT.to_string(),
        ),
        original[completing_sibling].0,
        false,
    );
    let prefix = executor.stored_oplog(&id).await;
    let (completing, waiting, value, remaining) = if completing_sibling == 0 {
        (&first, &second, b"first".to_vec(), b"second".to_vec())
    } else {
        (&second, &first, b"second".to_vec(), b"first".to_vec())
    };
    executor.complete_promise(completing, value).await?;
    tokio::time::timeout(Duration::from_secs(20), accepted)
        .await
        .context(
            "selected promise completed but its custom End did not reach the dependency gate",
        )??;
    let mut attempts =
        crate::monthly_acceptance::observe(worker.get_initial_worker_metadata().fingerprint);
    let mut exhausted = available.clone();
    exhausted.available_memory_gb_seconds = 0;
    registry.set_policy(exhausted);
    let refresh = applied_refresh(&limits, &registry, context.account_id).await?;
    let attempt = tokio::time::timeout(Duration::from_secs(15), attempts.recv())
        .await
        .context("monthly stop acceptance while custom End is held")?
        .context("quota acceptance during custom finish")?;
    assert!(attempt.accepted);
    let kind = worker
        .monthly_stop_for_test()
        .context("accepted custom-finish quota stop")?;
    assert!(matches!(
        kind,
        golem_service_base::error::worker_executor::InterruptKind::Suspend(_)
    ));
    assert_eq!(
        worker.monthly_stop_reason_for_test(),
        Some("monthly memory exhausted")
    );
    assert!(worker.concurrent_agent_permit_is_held().await);
    assert!(!invocation.is_finished());
    release.send(()).unwrap();
    executor
        .wait_for_status(&id, AgentStatus::Suspended, Duration::from_secs(20))
        .await?;
    physical_stop(&worker).await?;
    refresh.await?;
    assert!(!worker.monthly_window_active_for_test());
    let stopped = history(&worker).await;
    assert_eq!(roots(&stopped, &key), original);
    let completed: Vec<_> = original.iter().filter(|(start, _)| stopped.values().any(|entry| matches!(entry, OplogEntry::End { start_index, .. } if start_index == start))).map(|(start, _)| *start).collect();
    assert_eq!(
        completed,
        vec![original[completing_sibling].0],
        "only the selected promise's accepted custom finish may have End"
    );
    assert!(!stopped.values().any(|entry| matches!(entry, OplogEntry::Cancelled { start_index, .. } if original.iter().any(|(start, _)| start == start_index))));
    assert!(!stopped.values().any(|entry| matches!(entry, OplogEntry::CompletionDelivered { start_index, .. } | OplogEntry::CompletionDiscarded { start_index, .. } if *start_index == completed[0])));
    assert_eq!(
        stopped
            .values()
            .filter(|entry| matches!(entry, OplogEntry::Suspend { .. }))
            .count(),
        1
    );
    assert!(finished_for_key(&stopped, &key).is_empty());
    let stopped_prefix = executor.stored_oplog(&id).await;
    assert!(stopped_prefix.len() >= prefix.len());
    assert_eq!(&stopped_prefix[..prefix.len()], prefix.as_slice());
    limits.run_batch_for_test().await;
    let settled = registry.applied_memory_byte_nanoseconds();
    assert!(settled > 0);
    limits.run_batch_for_test().await;
    assert_eq!(registry.applied_memory_byte_nanoseconds(), settled);
    registry.set_policy(available);
    limits.run_batch_for_test().await;
    executor.resume(&id, false).await?;
    executor.complete_promise(waiting, remaining).await?;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(20), invocation)
            .await
            .context("original custom invocation did not finish after quota recovery")???
            .into_typed::<Vec<String>>()?,
        vec!["first", "second"]
    );
    assert!(worker.resident_generation_for_test() > generation);
    let after = history(&worker).await;
    assert_completed(&after, &original, &key);
    assert_eq!(
        observations(&after, completed[0]),
        observations(&stopped, completed[0]),
        "completed custom replay must not execute its promise body again"
    );
    let raw = executor.stored_oplog(&id).await;
    assert!(raw.len() >= stopped_prefix.len());
    assert_eq!(&raw[..stopped_prefix.len()], stopped_prefix.as_slice());
    Ok(())
}
