// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Counts the `ArchiveOplog` rows an ephemeral workload leaves in scheduler storage. With the oplog
//! sweep enabled there are none, while a durable agent in the same component, the control, still
//! registers one.

use crate::Tracing;

use bytes::Bytes;
use futures::FutureExt;
use futures::stream::BoxStream;
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::worker::{
    InvocationRequest, InvocationStart, invocation_request, invocation_response,
    invocation_session_completion, stream_terminal,
};
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentDto;
use golem_common::model::durable_stream::{
    AttachmentId, DURABLE_STREAM_FORMAT_VERSION, StreamAttachmentKey,
    StreamAttachmentPreparedRecord, StreamId, StreamRegisteredRecord, StreamSessionRecord,
};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    LogLevel, OplogEntry, OplogIndex, PublicAgentInvocation, PublicOplogEntry,
};
use golem_common::model::{
    AgentFingerprint, AgentInvocation, AgentStatus, IdempotencyKey, InvocationId, OwnedAgentId,
    ShardEpoch, ShardLeaseClock, ShardLeaseRevision, Timestamp,
};
use golem_common::schema::{FromSchema, SchemaValue};
use golem_common::{agent_key, data_value};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobPageEntry, BlobRangeStream, BlobStorage, BlobStorageBackend,
    BlobStorageLabelledApi, BlobStorageNamespace, ExistsResult, ListedBlob, NormalizedBlobPath,
    PutIfAbsent, agent_path_segment,
};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::derive_disjoint_sqlite_config;
use golem_worker_executor::services::oplog::{ArchiveWait, CommitLevel, DurableStreamOplogRecord};
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor::services::{HasOplog, HasOplogService, HasWorkerActivator};
use golem_worker_executor::storage::indexed::sqlite::SqliteIndexedStorage;
use golem_worker_executor::storage::indexed::{IndexedStorage, IndexedStorageNamespace};
use golem_worker_executor::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, callers, scheduler_sqlite_storage_config,
    sqlite_storage_config, start, start_with_overrides,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_sdk_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// Large enough that "one row per invocation" cannot be mistaken for incidental scheduler traffic.
const INVOCATIONS: usize = 30;

/// Reads scheduler storage directly. This workload schedules nothing but `ArchiveOplog`.
async fn scheduled_action_count(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> anyhow::Result<i64> {
    let config = scheduler_sqlite_storage_config(deps, context);
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&config.database)
        .create_if_missing(false);
    let pool = sqlx::SqlitePool::connect_with(options).await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scheduled_actions")
        .fetch_one(&pool)
        .await?;
    pool.close().await;
    Ok(count)
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn ephemeral_invocations_schedule_no_oplog_archive(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_key!("EphemeralCounter", "archive-schedule");
    executor
        .create_unkeyed_agent(
            &component.id,
            None,
            agent_id.clone(),
            std::collections::HashMap::new(),
        )
        .await?;

    for _ in 0..INVOCATIONS {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }

    // Settling time, so a registration that merely arrived late still fails the assertion.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let count = scheduled_action_count(deps, &context).await?;
    assert_eq!(
        count, 0,
        "{INVOCATIONS} ephemeral invocations left {count} scheduled actions behind"
    );

    // The control: `Counter` is durable and in the same component, so only the agent mode differs.
    let durable_agent_id = agent_key!("Counter", "archive-schedule-durable");
    executor
        .start_agent(&component.id, durable_agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &durable_agent_id, "increment", data_value!())
        .await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let with_durable = scheduled_action_count(deps, &context).await?;
    assert!(
        with_durable > 0,
        "a durable agent must still register its archive, but the table holds {with_durable} rows"
    );
    Ok(())
}

/// Ephemeral retirement must leave frozen results, not an owner that a duplicate can restart.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn sealed_ephemeral_duplicate_returns_original_result_without_indexed_writes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    sealed_ephemeral_duplicate(last_unique_id, deps, agent_sdk_rust, "instance", false).await
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn sealed_ephemeral_stream_duplicate_preserves_recorded_attachment_without_indexed_writes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let result = std::panic::AssertUnwindSafe(sealed_ephemeral_duplicate(
        last_unique_id,
        deps,
        agent_sdk_rust,
        "identity",
        false,
    ))
    .catch_unwind()
    .await;
    if !matches!(result, Ok(Ok(()))) {
        let data_root = deps.blob_storage_root().parent().unwrap().to_path_buf();
        std::fs::write(data_root.join(".retain-on-failure"), b"")?;
        eprintln!("Retained failed stream fixture at {}", data_root.display());
    }
    match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn sealed_ephemeral_prepared_attachment_cold_duplicate_writes_no_storage(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    sealed_ephemeral_duplicate(last_unique_id, deps, agent_sdk_rust, "identity", true).await
}

async fn sealed_ephemeral_duplicate(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_sdk_rust: &PrecompiledComponent,
    method: &str,
    leave_prepared_attachment: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    let kv = Arc::new(OnceLock::<Arc<dyn KeyValueStorage + Send + Sync>>::new());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.indexed_storage_layers = 2;
                config.oplog.blob_storage_layers = 1;
                config.oplog.entry_count_limit = 1_000_000;
                config.oplog.shard_lease_max_ttl = Duration::from_secs(300);
                config.oplog.skew_margin = Duration::from_secs(1);
            })),
            wrap_shard_service: Some(Arc::new({
                let shard = shard.clone();
                move |inner| {
                    assert!(shard.set(inner.clone()).is_ok());
                    inner
                }
            })),
            wrap_key_value_storage: Some(Arc::new({
                let kv = kv.clone();
                move |inner| {
                    assert!(kv.set(inner.clone()).is_ok());
                    inner
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let constructor = agent_key!("EphemeralStreamAgent");
    let invocation_key = IdempotencyKey::fresh();
    // A new attempt is an attachment conflict, not a reload of the retained response.
    // Clone the complete request so acceptance identity and execution configuration also match.
    let stream_request = if method == "identity" {
        Some(ephemeral_duplicate_stream_request(
            &executor,
            &component,
            &invocation_key,
            uuid::Uuid::new_v4(),
        )?)
    } else {
        None
    };
    let owned = crate::agent::ephemeral_agent_id(
        &executor,
        component.environment_id,
        &constructor,
        &invocation_key,
        method,
    );
    // Retirement runs immediately after the response. Its lease must already be sealable.
    let shard = shard.get().unwrap();
    let assignment = shard.current_assignment()?;
    shard.update_lease(
        &assignment.shard_epochs,
        ShardLeaseClock {
            sent_at: Instant::now(),
            wall_anchor: SystemTime::now(),
            ttl: Duration::from_secs(300),
        },
        ShardLeaseRevision {
            incarnation: uuid::Uuid::new_v4(),
            number: 1,
        },
        assignment.executor_count,
    )?;
    let (original, prepared_attachment) = if leave_prepared_attachment {
        let mut success = executor.gate_next_agent_function_success(&owned.agent_id, "identity");
        let original = tokio::spawn({
            let executor = executor.clone();
            let component = component.clone();
            let invocation_key = invocation_key.clone();
            let stream_request = stream_request.clone();
            async move {
                ephemeral_duplicate_result(
                    &executor,
                    &component,
                    &invocation_key,
                    stream_request.as_ref(),
                )
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(30), success.entered()).await?;
        let worker = executor.active_agents().try_get(&owned).await.unwrap();
        let fingerprint = worker.get_initial_worker_metadata().fingerprint;
        let history = executor.get_oplog(&owned, OplogIndex::INITIAL).await?;
        let (registration_index, registration) = history
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::StreamRegistered(params) => Some((
                    entry.oplog_index,
                    StreamRegisteredRecord::from_value(params.record.value()).unwrap(),
                )),
                _ => None,
            })
            .expect("identity must register its real output stream before returning");
        let session = registration.source_invocation.qualify(
            owned.environment_id,
            &owned.agent_id,
            fingerprint,
        );
        let epoch = history
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::StreamSession(params) => {
                    match StreamSessionRecord::from_value(params.record.value()).unwrap() {
                        StreamSessionRecord::Attached(attached)
                            if attached.session_key == session.idempotency_key =>
                        {
                            Some(attached.epoch)
                        }
                        _ => None,
                    }
                }
                _ => None,
            })
            .expect("the output stream must retain its original attachment epoch");
        let key = StreamAttachmentKey {
            attachment_id: AttachmentId::primary(
                session.callee_environment_id,
                &session.callee,
                &session.idempotency_key,
            )?,
            stream_id: StreamId::derive(
                owned.environment_id,
                &owned.agent_id,
                fingerprint,
                registration_index,
            ),
            epoch,
            session_key: session.clone(),
            producer_environment_id: owned.environment_id,
            producer: owned.agent_id,
            expected_producer_fingerprint: fingerprint,
            consumer_environment_id: owned.environment_id,
            consumer: owned.agent_id,
            expected_consumer_fingerprint: fingerprint,
            consumer_invocation: session,
        };
        // Preserve a crash prefix with producer Prepare but no consumer topology commit.
        // Live reconciliation would inspect this candidate through the shared session index,
        // even while the prepare lease is still young. Frozen observation must not do so.
        let prepared_at_millis = Timestamp::now_utc().to_millis();
        let prepared = StreamAttachmentPreparedRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            key,
            prepared_at_millis,
            lease_expires_at_millis: prepared_at_millis + 300_000,
        };
        let persisted = prepared.clone();
        worker
            .oplog()
            .add_durable_stream_batch(Box::new(move |_| {
                vec![DurableStreamOplogRecord::Session(
                    None,
                    Box::new(StreamSessionRecord::AttachmentPrepared(persisted)),
                )]
            }))
            .await?;
        worker
            .commit_oplog_and_update_state(CommitLevel::Always)
            .await?;
        success.release();
        (original.await??, Some(prepared))
    } else {
        let result = ephemeral_duplicate_result(
            &executor,
            &component,
            &invocation_key,
            stream_request.as_ref(),
        )
        .await;
        let original = match result {
            Ok(original) => original,
            Err(error) => {
                let data_root = deps.blob_storage_root().parent().unwrap().to_path_buf();
                let history = executor.get_oplog(&owned, OplogIndex::INITIAL).await;
                anyhow::bail!(
                    "original ephemeral response failed: {error}; retained data: {}; recorded history: {history:?}",
                    data_root.display()
                );
            }
        };
        (original, None)
    };
    let mut expected = Vec::new();
    if method == "identity" {
        expected.push("sealed-stream-label".to_string());
    }
    expected.push(owned.agent_id.as_uuid().to_string());
    assert_eq!(original, EphemeralResponse::Items(expected));

    let indexed_config =
        derive_disjoint_sqlite_config(&sqlite_storage_config(deps, &context), "indexed");
    let indexed = SqliteIndexedStorage::configured(&indexed_config).await?;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let observation = indexed
                .observe_agent("test", "ephemeral_sealed", &owned)
                .await?;
            if !executor.worker_is_cached(&owned).await && !observation.has_data {
                assert_eq!(observation.durable_marker, None);
                assert_eq!(observation.ephemeral_marker, None);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    assert!(!executor.worker_is_loaded(&owned).await);
    let metadata = executor.get_worker_metadata(&owned).await?;
    assert_sealed_derived_kv_absent(kv.get().unwrap().as_ref(), &owned, metadata.fingerprint)
        .await?;
    let history = executor.get_oplog(&owned, OplogIndex::INITIAL).await?;
    assert!(!history.is_empty(), "retirement must retain sealed history");
    if let Some(prepared) = &prepared_attachment {
        let records: Vec<_> = history
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::StreamSession(params) => {
                    Some(StreamSessionRecord::from_value(params.record.value()).unwrap())
                }
                _ => None,
            })
            .collect();
        assert!(
            records.contains(&StreamSessionRecord::AttachmentPrepared(prepared.clone())),
            "the reconcilable Prepare must survive in sealed history"
        );
        assert!(
            !records.iter().any(|record| matches!(record,
                StreamSessionRecord::AttachmentFinalized(finalized) if finalized.key == prepared.key)),
            "the attachment must still need reconciliation at cold acquisition"
        );
    }
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&indexed_config.database)
            .create_if_missing(false),
    )
    .await?;
    let key = owned.storage_key();
    for table in ["index_storage", "indexed_key_epoch"] {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {table} WHERE key = ? OR (key >= ? || '#' AND key < ? || '$')"
        ))
        .bind(&key)
        .bind(&key)
        .bind(&key)
        .fetch_one(&pool)
        .await?;
        assert_eq!(count, 0, "retirement must release every row in {table}");
    }

    // Audit committed writes, not only the final row count: an insert followed by a delete
    // inside an absent claim would otherwise look like a read-only duplicate lookup.
    sqlx::query("CREATE TABLE sealed_duplicate_target (key TEXT NOT NULL)")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO sealed_duplicate_target VALUES (?)")
        .bind(&key)
        .execute(&pool)
        .await?;
    sqlx::query("CREATE TABLE sealed_duplicate_writes (source TEXT NOT NULL)")
        .execute(&pool)
        .await?;
    for table in ["index_storage", "indexed_key_epoch"] {
        for (operation, row) in [("INSERT", "NEW"), ("UPDATE", "NEW"), ("DELETE", "OLD")] {
            sqlx::query(&format!(
                "CREATE TRIGGER audit_{table}_{operation} AFTER {operation} ON {table}
                 WHEN EXISTS (SELECT 1 FROM sealed_duplicate_target AS target
                   WHERE {row}.key = target.key
                     OR ({row}.key >= target.key || '#' AND {row}.key < target.key || '$'))
                 BEGIN INSERT INTO sealed_duplicate_writes VALUES ('{table}:{operation}'); END"
            ))
            .execute(&pool)
            .await?;
        }
    }

    let kv_config = sqlite_storage_config(deps, &context);
    let kv_pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&kv_config.database)
            .create_if_missing(false),
    )
    .await?;
    sqlx::query("CREATE TABLE sealed_duplicate_kv_targets (namespace TEXT NOT NULL)")
        .execute(&kv_pool)
        .await?;
    // These are the actual SQLite namespace encodings, not debug representations of the enum.
    for prefix in [
        "agent-status",
        "agent-status-checkpoint",
        "agent-invocation-result-index",
        "agent:durable_stream_session_index",
    ] {
        sqlx::query("INSERT INTO sealed_duplicate_kv_targets VALUES (?)")
            .bind(format!("{prefix}:{key}:{}", metadata.fingerprint))
            .execute(&kv_pool)
            .await?;
    }
    sqlx::query("CREATE TABLE sealed_duplicate_kv_writes (source TEXT NOT NULL)")
        .execute(&kv_pool)
        .await?;
    for (operation, row) in [("INSERT", "NEW"), ("UPDATE", "NEW"), ("DELETE", "OLD")] {
        sqlx::query(&format!(
            "CREATE TRIGGER audit_kv_storage_{operation} AFTER {operation} ON kv_storage
             WHEN EXISTS (SELECT 1 FROM sealed_duplicate_kv_targets AS target
               WHERE {row}.namespace = target.namespace)
             BEGIN INSERT INTO sealed_duplicate_kv_writes VALUES ('kv_storage:{operation}'); END"
        ))
        .execute(&kv_pool)
        .await?;
    }

    // There is no Worker shell left. Explicit eviction also keeps this a cold lookup if a
    // metadata read starts caching observation-only shells in the future.
    executor.retire_unloaded_worker(&owned).await?;
    assert!(!executor.worker_is_cached(&owned).await);
    let duplicate = ephemeral_duplicate_result(
        &executor,
        &component,
        &invocation_key,
        stream_request.as_ref(),
    )
    .await
    .map_err(|error| {
        let sessions: Vec<_> = history
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::StreamSession(params) => {
                    Some(StreamSessionRecord::from_value(params.record.value()))
                }
                _ => None,
            })
            .collect();
        anyhow::anyhow!("cold sealed duplicate failed: {error}; recorded sessions: {sessions:?}")
    })?;
    // Ordinary transport completion can detach the original attachment before retirement.
    // Exact retries preserve that recorded refusal, even with an unchanged epoch and attempt.
    let detached = history.iter().any(|entry| match &entry.entry {
        PublicOplogEntry::StreamSession(params) => matches!(
            StreamSessionRecord::from_value(params.record.value()).unwrap(),
            StreamSessionRecord::Detached(_)
        ),
        _ => false,
    });
    if detached {
        assert_eq!(duplicate, EphemeralResponse::AttachmentRevoked);
    } else {
        assert_eq!(duplicate, original);
    }
    assert!(!executor.worker_is_loaded(&owned).await);
    assert_sealed_derived_kv_absent(kv.get().unwrap().as_ref(), &owned, metadata.fingerprint)
        .await?;
    assert!(
        !indexed
            .observe_agent("test", "duplicate", &owned)
            .await?
            .has_data
    );
    assert_eq!(
        executor.get_worker_metadata(&owned).await?.fingerprint,
        metadata.fingerprint
    );
    assert_eq!(
        executor.get_oplog(&owned, OplogIndex::INITIAL).await?,
        history
    );
    let writes: Vec<String> = sqlx::query_scalar("SELECT source FROM sealed_duplicate_writes")
        .fetch_all(&pool)
        .await?;
    assert!(
        writes.is_empty(),
        "sealed duplicate wrote indexed storage: {writes:?}"
    );
    let kv_writes: Vec<String> =
        sqlx::query_scalar("SELECT source FROM sealed_duplicate_kv_writes")
            .fetch_all(&kv_pool)
            .await?;
    assert!(
        kv_writes.is_empty(),
        "sealed duplicate wrote derived KV storage: {kv_writes:?}"
    );
    kv_pool.close().await;
    pool.close().await;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum EphemeralResponse {
    Items(Vec<String>),
    AttachmentRevoked,
}

async fn ephemeral_duplicate_result(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    key: &IdempotencyKey,
    stream_request: Option<&InvocationRequest>,
) -> anyhow::Result<EphemeralResponse> {
    let constructor = agent_key!("EphemeralStreamAgent");
    let Some(request) = stream_request else {
        return Ok(EphemeralResponse::Items(vec![
            executor
                .invoke_and_await_agent_with_key(
                    component,
                    &constructor,
                    key,
                    "instance",
                    data_value!(),
                )
                .await?
                .into_typed::<String>()?,
        ]));
    };
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(request)
        .map_err(anyhow::Error::msg)?;
    let (frames, receiver) = tokio::sync::mpsc::channel(1);
    frames.send(request.clone()).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(tokio_stream::wrappers::ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let mut items = Vec::new();
    let mut ends = 0;
    let mut finished = false;
    while let Some(response) = responses.message().await? {
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::OutputItem(item)) => {
                match SchemaValue::try_from(item.value.expect("output item value"))
                    .map_err(anyhow::Error::msg)?
                {
                    SchemaValue::String(value) => items.push(value),
                    other => anyhow::bail!("expected string stream item, got {other:?}"),
                }
            }
            Some(invocation_response::Response::StreamEnd(end)) => {
                assert!(matches!(
                    end.outcome.and_then(|outcome| outcome.terminal),
                    Some(stream_terminal::Terminal::Finished(_))
                ));
                ends += 1;
            }
            Some(invocation_response::Response::Finished(completion)) => {
                assert!(matches!(
                    completion.outcome,
                    Some(invocation_session_completion::Outcome::Success(_))
                ));
                finished = true;
            }
            Some(invocation_response::Response::Accepted(_))
            | Some(invocation_response::Response::Ready(_))
            | Some(invocation_response::Response::Result(_)) => {}
            Some(invocation_response::Response::AttachmentRevoked(_)) => {
                assert!(items.is_empty() && ends == 0 && !finished);
                assert!(state.is_complete());
                return Ok(EphemeralResponse::AttachmentRevoked);
            }
            other => anyhow::bail!("unexpected ephemeral stream response: {other:?}"),
        }
    }
    anyhow::ensure!(
        ends == 1 && finished && state.is_complete(),
        "incomplete ephemeral stream response: ends={ends}, finished={finished}, complete={}, items={items:?}",
        state.is_complete()
    );
    drop(frames);
    Ok(EphemeralResponse::Items(items))
}

fn ephemeral_duplicate_stream_request(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    key: &IdempotencyKey,
    attempt_id: uuid::Uuid,
) -> anyhow::Result<InvocationRequest> {
    let constructor = agent_key!("EphemeralStreamAgent");
    let (target, constructor) =
        crate::agent::invocation_target(executor, component, &constructor, key, "identity")?;
    let (_, input) = data_value!("sealed-stream-label".to_string()).into_parts();
    Ok(InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            allow_streams: true,
            target: Some(target),
            constructor,
            component_id: Some(component.id.into()),
            method_name: Some("identity".to_string()),
            input: Some(input.try_into().map_err(anyhow::Error::msg)?),
            idempotency_key: Some(key.clone().into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            caller: Some(callers::anonymous().into()),
            environment_id: Some(component.environment_id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(attempt_id.into()),
            request_key: Some(
                golem_common::model::InvocationRequestKey::external(
                    executor.auth_ctx().idempotency_scope(),
                    Some(key),
                )
                .into(),
            ),
            ..Default::default()
        })),
    })
}

/// A startup restart after `Restart` is committed is terminal, but `Interrupted` is not sealable.
/// Skipping the optional seal must not skip removal of the quiesced Worker shell.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn interrupted_ephemeral_retirement_evicts_without_sealing_under_a_valid_lease(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    let blobs = Arc::new(OnceLock::<Arc<dyn BlobStorage>>::new());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.indexed_storage_layers = 2;
                config.oplog.blob_storage_layers = 1;
                config.oplog.entry_count_limit = 1_000_000;
                config.oplog.shard_lease_max_ttl = Duration::from_secs(300);
                config.oplog.skew_margin = Duration::from_secs(1);
            })),
            wrap_shard_service: Some(Arc::new({
                let shard = shard.clone();
                move |inner| {
                    assert!(shard.set(inner.clone()).is_ok());
                    inner
                }
            })),
            wrap_blob_storage: Some(Arc::new({
                let blobs = blobs.clone();
                move |inner| {
                    assert!(blobs.set(inner.clone()).is_ok());
                    inner
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let constructor = agent_key!("EphemeralStreamAgent");
    let key = IdempotencyKey::fresh();
    let owned = crate::agent::ephemeral_agent_id(
        &executor,
        component.environment_id,
        &constructor,
        &key,
        "instance",
    );
    install_sealable_lease(shard.get().unwrap().as_ref())?;
    let (prepared, release) = executor.gate_next_prepared_instance(&owned.agent_id);
    let invocation = tokio::spawn({
        let executor = executor.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &constructor,
                    &key,
                    "instance",
                    data_value!(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), prepared).await??;
    let worker = executor.active_agents().try_get(&owned).await.unwrap();
    assert!(
        worker
            .get_last_known_status()
            .await
            .ephemeral_execution_begun
    );
    let epoch = worker
        .oplog()
        .shard_epoch()
        .expect("ephemeral owner records its epoch");
    assert!(
        shard
            .get()
            .unwrap()
            .lease_valid_until(&owned.agent_id, epoch, Duration::from_secs(1))
            .is_some()
    );
    let mut acknowledged = worker.set_interrupting(InterruptKind::Restart).await?;
    release.send(()).unwrap();
    if let Some(acknowledged) = acknowledged.as_mut() {
        tokio::time::timeout(Duration::from_secs(30), acknowledged.recv()).await??;
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(30), invocation)
            .await??
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        while executor.worker_is_cached(&owned).await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert!(!executor.worker_is_loaded(&owned).await);
    assert_eq!(
        executor.get_worker_metadata(&owned).await?.status,
        AgentStatus::Interrupted
    );
    let history = executor.get_oplog(&owned, OplogIndex::INITIAL).await?;
    let restarts: Vec<_> = history
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::Restart(_)))
        .map(|entry| entry.oplog_index)
        .collect();
    assert_eq!(
        restarts.len(),
        1,
        "the startup must not retry guest execution: {history:#?}"
    );
    assert!(
        history.iter().any(
            |entry| matches!(entry.entry, PublicOplogEntry::Interrupted(_))
                && entry.oplog_index > restarts[0]
        ),
        "{history:#?}"
    );
    assert!(!history.iter().any(|entry| matches!(
        &entry.entry, PublicOplogEntry::AgentInvocationStarted(started)
            if matches!(&started.invocation, PublicAgentInvocation::AgentMethodInvocation(method)
                if method.method_name == "instance")
    )), "the interrupt must reach startup, not the method: {history:#?}");
    assert_no_seal_records(blobs.get().unwrap().as_ref(), &owned).await?;
    let indexed = SqliteIndexedStorage::configured(&derive_disjoint_sqlite_config(
        &sqlite_storage_config(deps, &context),
        "indexed",
    ))
    .await?;
    assert!(
        indexed
            .observe_agent("test", "interrupted_not_sealed", &owned)
            .await?
            .has_data
    );
    // Holding the old Worker must not keep it in ActiveAgents.
    assert!(!executor.worker_is_cached(&owned).await);
    assert_eq!(
        executor.get_oplog(&owned, OplogIndex::INITIAL).await?,
        history
    );
    Ok(())
}

/// Failure of optional seal storage must not retain a quiesced ephemeral Worker in the cache.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn ephemeral_retirement_evicts_when_seal_storage_fails(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    let blobs = Arc::new(OnceLock::<Arc<dyn BlobStorage>>::new());
    let fault = Arc::new(SealWriteFault::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.indexed_storage_layers = 2;
                config.oplog.blob_storage_layers = 1;
                config.oplog.entry_count_limit = 1_000_000;
                config.oplog.shard_lease_max_ttl = Duration::from_secs(300);
                config.oplog.skew_margin = Duration::from_secs(1);
            })),
            wrap_shard_service: Some(Arc::new({
                let shard = shard.clone();
                move |inner| {
                    assert!(shard.set(inner.clone()).is_ok());
                    inner
                }
            })),
            wrap_blob_storage: Some(Arc::new({
                let blobs = blobs.clone();
                let fault = fault.clone();
                move |inner| {
                    assert!(blobs.set(inner.clone()).is_ok());
                    Arc::new(FailingSealStorage {
                        inner,
                        fault: fault.clone(),
                    })
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let constructor = agent_key!("EphemeralStreamAgent");
    let key = IdempotencyKey::fresh();
    let owned = crate::agent::ephemeral_agent_id(
        &executor,
        component.environment_id,
        &constructor,
        &key,
        "instance",
    );
    install_sealable_lease(shard.get().unwrap().as_ref())?;
    let (prepared, release) = executor.gate_next_prepared_instance(&owned.agent_id);
    let invocation = tokio::spawn({
        let executor = executor.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &constructor,
                    &key,
                    "instance",
                    data_value!(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), prepared).await??;
    let worker = executor.active_agents().try_get(&owned).await.unwrap();
    let epoch = worker
        .oplog()
        .shard_epoch()
        .expect("ephemeral owner records its epoch");
    assert!(
        shard
            .get()
            .unwrap()
            .lease_valid_until(&owned.agent_id, epoch, Duration::from_secs(1))
            .is_some()
    );
    // Arm only after reconstruction. The wrapper refuses this agent's single-attempt seal PUT,
    // while real archival, payload reads and seal authority observations keep using storage.
    assert!(fault.target.set(owned.clone()).is_ok());
    let blobs = blobs.get().unwrap();
    release.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), invocation)
            .await???
            .into_typed::<String>()?,
        owned.agent_id.as_uuid().to_string()
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        while fault.hits.load(Ordering::Acquire) == 0 || executor.worker_is_cached(&owned).await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert!(!executor.worker_is_loaded(&owned).await);
    assert!(
        fault.hits.load(Ordering::Acquire) > 0,
        "retirement must attempt the refused seal PUT"
    );
    assert_no_seal_records(blobs.as_ref(), &owned).await?;
    assert_eq!(
        executor.get_worker_metadata(&owned).await?.status,
        AgentStatus::Idle
    );
    assert!(
        !executor
            .get_oplog(&owned, OplogIndex::INITIAL)
            .await?
            .is_empty()
    );
    let indexed = SqliteIndexedStorage::configured(&derive_disjoint_sqlite_config(
        &sqlite_storage_config(deps, &context),
        "indexed",
    ))
    .await?;
    assert!(
        indexed
            .observe_agent("test", "seal_error_fallback", &owned)
            .await?
            .has_data
    );
    Ok(())
}

/// Sweep acquisition uses this same real archive path. A stale cold open must not turn the
/// archive's `Some(false)` fence result into permission to publish a frozen record.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn fenced_cold_ephemeral_archive_does_not_publish_a_seal_record(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    let blobs = Arc::new(OnceLock::<Arc<dyn BlobStorage>>::new());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.indexed_storage_layers = 2;
                config.oplog.blob_storage_layers = 1;
                config.oplog.entry_count_limit = 1_000_000;
                config.oplog.shard_lease_max_ttl = Duration::from_secs(300);
                config.oplog.skew_margin = Duration::from_secs(1);
            })),
            wrap_shard_service: Some(Arc::new({
                let shard = shard.clone();
                move |inner| {
                    assert!(shard.set(inner.clone()).is_ok());
                    inner
                }
            })),
            wrap_blob_storage: Some(Arc::new({
                let blobs = blobs.clone();
                move |inner| {
                    assert!(blobs.set(inner.clone()).is_ok());
                    inner
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let constructor = agent_key!("EphemeralStreamAgent");
    let key = IdempotencyKey::fresh();
    let owned = crate::agent::ephemeral_agent_id(
        &executor,
        component.environment_id,
        &constructor,
        &key,
        "instance",
    );
    // The harness's initial unexpiring grant lets retirement archive, but not seal. Capture
    // the real owner before it exits so the later call uses its production WorkerActivator.
    let (prepared, release) = executor.gate_next_prepared_instance(&owned.agent_id);
    let invocation = tokio::spawn({
        let executor = executor.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &constructor,
                    &key,
                    "instance",
                    data_value!(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), prepared).await??;
    let worker = executor.active_agents().try_get(&owned).await.unwrap();
    let activator = worker.worker_activator();
    let oplog_service = worker.oplog_service();
    let epoch = worker
        .oplog()
        .shard_epoch()
        .expect("ephemeral owner records its epoch");
    release.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), invocation)
            .await???
            .into_typed::<String>()?,
        owned.agent_id.as_uuid().to_string()
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        while executor.worker_is_cached(&owned).await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    executor.retire_unloaded_worker(&owned).await?;
    assert_no_seal_records(blobs.get().unwrap().as_ref(), &owned).await?;
    let last = oplog_service
        .get_last_index(&owned, AgentMode::Ephemeral)
        .await?;
    let history = executor.get_oplog(&owned, OplogIndex::INITIAL).await?;
    assert_eq!(
        executor.get_worker_metadata(&owned).await?.status,
        AgentStatus::Idle
    );
    install_sealable_lease(shard.get().unwrap().as_ref())?;
    let indexed = SqliteIndexedStorage::configured(&derive_disjoint_sqlite_config(
        &sqlite_storage_config(deps, &context),
        "indexed",
    ))
    .await?;
    let newer_epoch = ShardEpoch(epoch.0.checked_add(1).unwrap());
    {
        let _lifecycle = oplog_service.lock_lifecycle(&owned.agent_id).await;
        // The retained old Worker also retains its oplog. Close that handle so acquisition
        // cannot reuse it and must claim the first lower again, without starting a Store.
        worker
            .oplog()
            .stop_and_wait()
            .await
            .map_err(anyhow::Error::msg)?;
        indexed
            .set_key_epoch(
                "test",
                "take_first_lower",
                IndexedStorageNamespace::CompressedOpLog {
                    owned_agent_id: owned.clone(),
                    agent_mode: AgentMode::Ephemeral,
                    level: 1,
                },
                &owned.storage_key(),
                newer_epoch,
                &[],
                false,
            )
            .await?;
    }
    assert_eq!(
        activator
            .archive_oplog(&owned, last, ArchiveWait::Finished)
            .await?,
        Some(false)
    );
    let archived = executor.active_agents().try_get(&owned).await.unwrap();
    assert!(
        !Arc::ptr_eq(&worker, &archived),
        "archive must acquire a cold Worker"
    );
    let fence = archived
        .oplog()
        .fence()
        .expect("cold archive open must latch the refusal");
    assert_eq!(fence.expected_epoch, epoch);
    assert_eq!(fence.actual_epoch, Some(newer_epoch));
    assert!(!executor.worker_is_loaded(&owned).await);
    assert_no_seal_records(blobs.get().unwrap().as_ref(), &owned).await?;
    assert!(
        indexed
            .observe_agent("test", "fenced_archive", &owned)
            .await?
            .has_data
    );
    assert_eq!(
        executor.get_oplog(&owned, OplogIndex::INITIAL).await?,
        history
    );
    Ok(())
}

fn install_sealable_lease(shard: &dyn ShardService) -> anyhow::Result<()> {
    let assignment = shard.current_assignment()?;
    shard.update_lease(
        &assignment.shard_epochs,
        ShardLeaseClock {
            sent_at: Instant::now(),
            wall_anchor: SystemTime::now(),
            ttl: Duration::from_secs(300),
        },
        ShardLeaseRevision {
            incarnation: uuid::Uuid::new_v4(),
            number: 1,
        },
        assignment.executor_count,
    )?;
    Ok(())
}

async fn assert_no_seal_records(
    blobs: &dyn BlobStorage,
    owned: &OwnedAgentId,
) -> anyhow::Result<()> {
    let prefix = std::path::Path::new("seals").join(agent_path_segment(&owned.agent_id));
    let records = blobs
        .with("test", "assert_no_seal")
        .list_page(
            BlobStorageNamespace::SealRecords {
                environment_id: owned.environment_id(),
                type_tag: owned.agent_id.type_tag(),
            },
            &prefix,
            None,
            1,
        )
        .await?;
    assert!(
        records.is_empty(),
        "unexpected frozen record for {owned}: {records:?}"
    );
    Ok(())
}

#[derive(Debug, Default)]
struct SealWriteFault {
    target: OnceLock<OwnedAgentId>,
    hits: AtomicUsize,
}

#[derive(Debug)]
struct FailingSealStorage {
    inner: Arc<dyn BlobStorage>,
    fault: Arc<SealWriteFault>,
}

#[async_trait::async_trait]
impl BlobStorageBackend for FailingSealStorage {
    async fn list_page_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        prefix: &NormalizedBlobPath<'_>,
        after: Option<&NormalizedBlobPath<'_>>,
        max_keys: u32,
    ) -> anyhow::Result<Vec<BlobPageEntry>> {
        self.inner
            .list_page(
                svc,
                api,
                namespace,
                prefix,
                after.map(|path| path.as_ref()),
                max_keys,
            )
            .await
    }

    async fn put_raw_if_absent_once_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
        timeout: Duration,
    ) -> anyhow::Result<PutIfAbsent> {
        if let Some(target) = self.fault.target.get()
            && namespace
                == (BlobStorageNamespace::SealRecords {
                    environment_id: target.environment_id(),
                    type_tag: target.agent_id.type_tag(),
                })
            && path.as_ref().starts_with(
                std::path::Path::new("seals").join(agent_path_segment(&target.agent_id)),
            )
        {
            self.fault.hits.fetch_add(1, Ordering::AcqRel);
            anyhow::bail!("injected seal-record PUT failure");
        }
        self.inner
            .put_raw_if_absent_once(svc, api, namespace, path, data, timeout)
            .await
    }

    async fn get_raw_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner.get_raw(svc, api, namespace, path).await
    }

    async fn get_stream_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<BoxStream<'static, anyhow::Result<Bytes>>>> {
        self.inner.get_stream(svc, api, namespace, path).await
    }

    async fn get_range_stream_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> anyhow::Result<Option<BlobRangeStream>> {
        self.inner
            .get_range_stream(svc, api, namespace, path, offset, length)
            .await
    }

    async fn get_raw_slice_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner
            .get_raw_slice(svc, api, namespace, path, start, end)
            .await
    }

    async fn get_metadata_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.inner.get_metadata(svc, api, namespace, path).await
    }

    async fn put_raw_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<()> {
        self.inner.put_raw(svc, api, namespace, path, data).await
    }

    async fn put_raw_if_absent_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        self.inner
            .put_raw_if_absent(svc, api, namespace, path, data)
            .await
    }

    async fn put_stream_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        stream: &dyn ErasedReplayableStream<Item = anyhow::Result<Vec<u8>>, Error = anyhow::Error>,
    ) -> anyhow::Result<()> {
        self.inner
            .put_stream(svc, api, namespace, path, stream)
            .await
    }

    async fn delete_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.inner.delete(svc, api, namespace, path).await
    }

    async fn delete_many_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[NormalizedBlobPath<'_>],
    ) -> anyhow::Result<()> {
        let paths = paths
            .iter()
            .map(|path| path.to_path_buf())
            .collect::<Vec<_>>();
        self.inner.delete_many(svc, api, namespace, &paths).await
    }

    async fn create_dir_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.inner.create_dir(svc, api, namespace, path).await
    }

    async fn list_dir_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.inner.list_dir(svc, api, namespace, path).await
    }

    async fn list_blobs_below_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.inner.list_blobs_below(svc, api, namespace, path).await
    }

    async fn delete_dir_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<u64>> {
        self.inner.delete_dir(svc, api, namespace, path).await
    }

    async fn exists_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<ExistsResult> {
        self.inner.exists(svc, api, namespace, path).await
    }

    async fn copy_between_at(
        &self,
        svc: &'static str,
        api: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<bool> {
        match self
            .inner
            .copy_between(svc, api, from_namespace, from, to_namespace, to)
            .await
        {
            Ok(()) => Ok(true),
            Err(error) if error.is::<golem_service_base::storage::blob::BlobMissingError>() => {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
}

async fn assert_sealed_derived_kv_absent(
    kv: &(dyn KeyValueStorage + Send + Sync),
    owned: &OwnedAgentId,
    fingerprint: AgentFingerprint,
) -> anyhow::Result<()> {
    for namespace in [
        KeyValueStorageNamespace::AgentStatus {
            owned_agent_id: owned.clone(),
            fingerprint,
        },
        KeyValueStorageNamespace::AgentStatusCheckpoint {
            owned_agent_id: owned.clone(),
            fingerprint,
        },
        KeyValueStorageNamespace::AgentInvocationResultIndex {
            owned_agent_id: owned.clone(),
            fingerprint,
        },
        KeyValueStorageNamespace::AgentDurableStreamSessionIndex {
            owned_agent_id: owned.clone(),
            fingerprint,
        },
    ] {
        let rows = kv
            .with_entity("test", "sealed_derived_kv", "agent")
            .get_all_raw(namespace.clone())
            .await
            .map_err(anyhow::Error::msg)?;
        assert!(
            rows.is_empty(),
            "sealed history retained derived rows in {namespace:?}"
        );
    }
    Ok(())
}

/// With the sweep disabled nothing else would archive an ephemeral oplog stranded by a crashed
/// pod, so the registration has to happen again.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn disabling_the_sweep_restores_the_ephemeral_archive_registration(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.sweep.enabled = false;
            })),
            ..Default::default()
        },
    )
    .await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent_id = agent_key!("EphemeralCounter", "archive-schedule-sweep-off");
    executor
        .create_unkeyed_agent(
            &component.id,
            None,
            agent_id.clone(),
            std::collections::HashMap::new(),
        )
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    // Settling time, as in the positive case.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let count = scheduled_action_count(deps, &context).await?;
    assert!(
        count > 0,
        "with the sweep off an ephemeral agent must register its archive again, \
         but the table holds {count} rows"
    );
    Ok(())
}

/// A sealed owner must lose its cached Worker, not just its Store. Reusing that Worker after
/// restoration at the same shard epoch must refuse both invocation admission and oplog writes.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn sealed_cached_worker_is_reacquired_without_reusing_its_old_handle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.indexed_storage_layers = 1;
                config.oplog.blob_storage_layers = 1;
                config.oplog.entry_count_limit = 1_000_000;
                config.oplog.shard_lease_max_ttl = Duration::from_secs(300);
                config.oplog.skew_margin = Duration::from_secs(1);
            })),
            wrap_shard_service: Some(Arc::new({
                let shard = shard.clone();
                move |inner| {
                    assert!(shard.set(inner.clone()).is_ok());
                    inner
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_key!("GolemWasiHttp", "seal-cached-worker");
    let owned = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    for _ in 0..2 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
            .await?;
    }
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
            .await?
            .into_typed::<u64>()?,
        2
    );
    let old = executor.active_agents().try_get(&owned).await.unwrap();
    let old_oplog = old.oplog();
    let fingerprint = old.get_initial_worker_metadata().fingerprint;
    tokio::time::timeout(Duration::from_secs(30), async {
        while executor.worker_is_loaded(&owned).await {
            executor.stop_worker_if_idle(&owned).await?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    assert!(executor.worker_is_cached(&owned).await);
    assert!(Arc::ptr_eq(
        &old,
        &executor.active_agents().try_get(&owned).await.unwrap()
    ));

    // Replace the harness's unexpiring grant with a real lease without changing its shard epochs.
    let shard = shard.get().unwrap();
    let assignment = shard.current_assignment()?;
    shard.update_lease(
        &assignment.shard_epochs,
        ShardLeaseClock {
            sent_at: Instant::now(),
            wall_anchor: SystemTime::now(),
            ttl: Duration::from_secs(300),
        },
        ShardLeaseRevision {
            incarnation: uuid::Uuid::new_v4(),
            number: 1,
        },
        assignment.executor_count,
    )?;
    let epoch = old_oplog.shard_epoch().expect("durable oplog has an epoch");
    assert!(
        shard
            .lease_valid_until(&owned.agent_id, epoch, Duration::from_secs(1))
            .is_some()
    );
    let indexed_config =
        derive_disjoint_sqlite_config(&sqlite_storage_config(deps, &context), "indexed");
    let indexed = SqliteIndexedStorage::configured(&indexed_config).await?;
    let resident = indexed.observe_agent("test", "before_seal", &owned).await?;
    assert!(resident.has_data);
    let history = executor.stored_oplog(&owned.agent_id).await;
    let oplog_service = old.oplog_service();
    let last = oplog_service
        .get_last_index(&owned, AgentMode::Durable)
        .await?;
    let activator = old.worker_activator();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match activator
                .archive_oplog(&owned, last, ArchiveWait::Finished)
                .await?
            {
                Some(true) => tokio::time::sleep(Duration::from_millis(10)).await,
                Some(false) => break,
                None => anyhow::bail!("the current cached generation was not archived"),
            }
        }
        anyhow::Ok(())
    })
    .await??;

    let sealed = indexed.observe_agent("test", "after_seal", &owned).await?;
    assert!(!sealed.has_data, "seal must remove all indexed authority");
    assert_eq!(sealed.durable_marker, None);
    assert_eq!(sealed.ephemeral_marker, None);
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&indexed_config.database)
            .create_if_missing(false),
    )
    .await?;
    let key = owned.storage_key();
    let epochs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM indexed_key_epoch WHERE key = ? OR (key >= ? || '#' AND key < ? || '$')",
    )
    .bind(&key)
    .bind(&key)
    .bind(&key)
    .fetch_one(&pool)
    .await?;
    pool.close().await;
    assert_eq!(epochs, 0, "seal must release every indexed epoch row too");
    assert!(!executor.worker_is_cached(&owned).await);
    assert!(old_oplog.is_retired());
    assert_eq!(executor.stored_oplog(&owned.agent_id).await, history);

    // An archive redelivery observes frozen history without constructing or unsealing an owner.
    assert_eq!(
        activator
            .archive_oplog(&owned, last, ArchiveWait::Finished)
            .await?,
        None
    );
    assert!(!executor.worker_is_cached(&owned).await);
    assert!(
        !indexed
            .observe_agent("test", "archive_redelivery", &owned)
            .await?
            .has_data
    );
    let invocation = || AgentInvocation::AgentMethod {
        idempotency_key: InvocationId(uuid::Uuid::new_v4()),
        method_name: "increment".to_string(),
        input: data_value!().into_parts().1.try_into().unwrap(),
        allow_streams: false,
        invocation_context: InvocationContextStack::fresh(),
        caller: callers::anonymous(),
        scope_card: None,
    };
    assert!(matches!(
        old.clone().invoke(invocation()).await,
        Err(WorkerExecutorError::ShardingNotReady)
    ));
    assert_eq!(executor.stored_oplog(&owned.agent_id).await, history);

    // Ordinary invocation must restore presence, construct a different Worker and replay its state.
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
            .await?
            .into_typed::<u64>()?,
        2
    );
    let new = executor.active_agents().try_get(&owned).await.unwrap();
    assert!(!Arc::ptr_eq(&old, &new));
    assert_eq!(new.get_initial_worker_metadata().fingerprint, fingerprint);
    assert_eq!(new.oplog().shard_epoch(), Some(epoch));
    let restored = indexed
        .observe_agent("test", "after_unseal", &owned)
        .await?;
    assert!(restored.has_data);
    assert!(restored.durable_marker.is_some());
    assert_ne!(restored.durable_marker, resident.durable_marker);
    // Await settlement before comparing history around refused stale-generation operations.
    tokio::time::timeout(Duration::from_secs(30), async {
        while executor.worker_is_loaded(&owned).await {
            executor.stop_worker_if_idle(&owned).await?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    let continued = executor.stored_oplog(&owned.agent_id).await;
    assert!(continued.len() > history.len());
    assert_eq!(&continued[..history.len()], history.as_slice());

    assert!(matches!(
        old.clone().invoke(invocation()).await,
        Err(WorkerExecutorError::ShardingNotReady)
    ));
    assert!(
        std::panic::AssertUnwindSafe(async {
            old_oplog
                .add(OplogEntry::log(
                    None,
                    LogLevel::Info,
                    "stale-generation".to_string(),
                    "must not be written".to_string(),
                    None,
                ))
                .await
        })
        .catch_unwind()
        .await
        .is_err(),
        "restoration at the same epoch must not revive the old oplog handle"
    );
    assert_eq!(executor.stored_oplog(&owned.agent_id).await, continued);
    executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "get_count", data_value!())
            .await?
            .into_typed::<u64>()?,
        3
    );
    Ok(())
}
