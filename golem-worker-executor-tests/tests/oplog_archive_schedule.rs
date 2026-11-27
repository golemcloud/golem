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
use golem_common::config::DbPostgresConfig;
use golem_common::model::agent::{AgentMode, ScheduledAction};
use golem_common::model::component::ComponentDto;
use golem_common::model::durable_stream::{
    AttachmentId, DURABLE_STREAM_FORMAT_VERSION, StreamAttachmentKey,
    StreamAttachmentPreparedRecord, StreamCascadeDependentResult, StreamError, StreamId,
    StreamRegisteredRecord, StreamSessionRecord, StreamTerminal, StreamTerminalAuthor,
    StreamTerminalOrigin, StreamTerminalRecord,
};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    LogLevel, OplogEntry, OplogIndex, PublicAgentInvocation, PublicOplogEntry,
    PublicOplogEntryWithIndex,
};
use golem_common::model::{
    AgentFingerprint, AgentInvocation, AgentStatus, IdempotencyKey, InvocationId, OwnedAgentId,
    PromiseId, ShardEpoch, ShardLeaseClock, ShardLeaseRevision, Timestamp,
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
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::derive_disjoint_sqlite_config;
use golem_worker_executor::services::golem_config::{
    IndexedStorageConfig, IndexedStoragePostgresConfig,
};
use golem_worker_executor::services::oplog::{ArchiveWait, CommitLevel, DurableStreamOplogRecord};
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor::services::{HasOplog, HasOplogService, HasWorkerActivator};
use golem_worker_executor::storage::indexed::postgres::PostgresIndexedStorage;
use golem_worker_executor::storage::indexed::sqlite::SqliteIndexedStorage;
use golem_worker_executor::storage::indexed::{IndexedStorage, IndexedStorageNamespace};
use golem_worker_executor::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use golem_worker_executor::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageError, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use golem_worker_executor::worker::{WorkerDeletionHook, WorkerDeletionStage};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, callers, scheduler_sqlite_storage_config,
    sqlite_storage_config, start, start_with_overrides,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
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
    sealed_duplicate(
        last_unique_id,
        deps,
        agent_sdk_rust,
        method,
        leave_prepared_attachment,
        AgentMode::Ephemeral,
        None,
    )
    .await
}

#[test]
#[timeout("8m")]
async fn sealed_stream_deletion_replaces_frozen_duplicates_and_fences_activation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    sealed_duplicate(
        last_unique_id,
        deps,
        agent_sdk_rust,
        "identity",
        true,
        AgentMode::Ephemeral,
        Some(SealedDeletionCheck::Delete),
    )
    .await
}

#[test]
#[timeout("8m")]
async fn sealed_durable_stream_deletion_from_uncached_source_fences_activation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    sealed_duplicate(
        last_unique_id,
        deps,
        agent_sdk_rust,
        "identity",
        true,
        AgentMode::Durable,
        Some(SealedDeletionCheck::Delete),
    )
    .await
}

#[test]
#[timeout("8m")]
async fn sealed_deletion_without_stream_history_never_restores_indexed_authority(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        sealed_duplicate(
            last_unique_id,
            deps,
            agent_sdk_rust,
            "instance",
            false,
            mode,
            Some(SealedDeletionCheck::Delete),
        )
        .await?;
    }
    Ok(())
}

#[test]
#[timeout("8m")]
async fn sealed_deletion_at_a_lower_epoch_refuses_without_indexed_writes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        for method in ["identity", "instance"] {
            sealed_duplicate(
                last_unique_id,
                deps,
                agent_sdk_rust,
                method,
                method == "identity",
                mode,
                Some(SealedDeletionCheck::StaleEpoch),
            )
            .await?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SealedDeletionCheck {
    Delete,
    StaleEpoch,
    PostgresDelete,
    NoHistoryOverlap,
    NoHistoryTombstoneRetry,
    UnresolvedAdmission,
}

#[test]
#[timeout("8m")]
async fn sealed_no_history_deletion_overlap_shares_failure_then_explicit_retry_succeeds(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        sealed_duplicate(
            last_unique_id,
            deps,
            fixture,
            "instance",
            false,
            mode,
            Some(SealedDeletionCheck::NoHistoryOverlap),
        )
        .await?;
    }
    Ok(())
}

#[test]
#[timeout("8m")]
async fn sealed_no_history_deletion_retries_cleanup_after_tombstone_publication(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        sealed_duplicate(
            last_unique_id,
            deps,
            fixture,
            "instance",
            false,
            mode,
            Some(SealedDeletionCheck::NoHistoryTombstoneRetry),
        )
        .await?;
    }
    Ok(())
}

#[test]
#[timeout("8m")]
async fn sealed_deletion_joins_unresolved_admission_without_starting_its_replacement(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        sealed_duplicate(
            last_unique_id,
            deps,
            fixture,
            "identity",
            true,
            mode,
            Some(SealedDeletionCheck::UnresolvedAdmission),
        )
        .await?;
    }
    Ok(())
}

#[test]
#[timeout("8m")]
async fn postgres_sealed_stream_deletion_prefix_recovers_after_executor_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        sealed_duplicate(
            last_unique_id,
            deps,
            agent_sdk_rust,
            "identity",
            true,
            mode,
            Some(SealedDeletionCheck::PostgresDelete),
        )
        .await?;
    }
    Ok(())
}

#[test]
#[timeout("6m")]
async fn sealed_open_producer_deletion_preserves_ordinary_attached_consumer_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut resident_failure = None;
    let mut resident_retry_failure = None;
    for seal_producer in [false, true] {
        let context = TestContext::new(last_unique_id);
        let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
        let executor = start_with_overrides(
            deps,
            &context,
            TestExecutorOverrides {
                configure: Some(Arc::new(|config| {
                    config.oplog.indexed_storage_layers = 2;
                    config.oplog.blob_storage_layers = 1;
                    config.oplog.entry_count_limit = 1_000_000;
                    config.oplog.archive_interval = Duration::from_secs(3600);
                    config.oplog.shard_lease_max_ttl = Duration::from_secs(300);
                    config.oplog.skew_margin = Duration::from_secs(1);
                    config.suspend.wait_suspend_grace = Duration::from_millis(100);
                    config.suspend.wait_suspend_check_interval = Duration::from_millis(100);
                    config.suspend.rpc_resume_after = Duration::from_secs(3600);
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
        install_sealable_lease(shard.get().unwrap().as_ref())?;
        let component = executor
            .component_dep(&context.default_environment_id, fixture)
            .store()
            .await?;
        let consumer_key = agent_key!("StreamingRpcCaller", "sealed-open-producer");
        let producer_key = agent_key!("StreamingRpcTarget", "sealed-open-producer");
        let consumer = executor
            .start_agent(&component.id, consumer_key.clone())
            .await?;
        let producer = executor
            .start_agent(&component.id, producer_key.clone())
            .await?;
        let promise = executor
            .invoke_and_await_agent(
                &component,
                &producer_key,
                "create_output_gate",
                data_value!(),
            )
            .await?
            .into_typed::<PromiseId>()?;
        let key = IdempotencyKey::fresh();
        let invocation = tokio::spawn({
            let executor = executor.clone();
            let component = component.clone();
            let consumer_key = consumer_key.clone();
            let key = key.clone();
            let consumer = consumer.clone();
            let promise = promise.clone();
            async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &consumer_key,
                        &key,
                        "fork_drop_inherited_output",
                        data_value!(promise, consumer.agent_id.to_string(), false),
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let history = executor.get_oplog(&consumer, OplogIndex::INITIAL).await?;
                let consumed = history.iter().any(|entry| match &entry.entry {
                    PublicOplogEntry::StreamSession(params) => matches!(
                        StreamSessionRecord::from_value(params.record.value()).unwrap(),
                        StreamSessionRecord::ConsumerItemValue(_)
                    ),
                    _ => false,
                });
                if consumed
                    && !invocation.is_finished()
                    && executor.get_worker_metadata(&producer).await?.status
                        == AgentStatus::Suspended
                    && executor.get_worker_metadata(&consumer).await?.status
                        == AgentStatus::Suspended
                    && !executor.worker_is_loaded(&producer).await
                    && !executor.worker_is_loaded(&consumer).await
                {
                    return anyhow::Ok(());
                }
                assert!(
                    !invocation.is_finished(),
                    "the open stream must wait for its producer promise"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        let history = executor.get_oplog(&producer, OplogIndex::INITIAL).await?;
        let attachments: Vec<_> = history
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::StreamSession(params) => {
                    match StreamSessionRecord::from_value(params.record.value()).unwrap() {
                        StreamSessionRecord::AttachmentActivated(record)
                            if record.key.consumer == consumer.agent_id =>
                        {
                            Some(record.key)
                        }
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect();
        assert!(
            !attachments.is_empty(),
            "the producer must retain an active attachment to the independent consumer"
        );
        assert!(
            !history
                .iter()
                .any(|entry| matches!(entry.entry, PublicOplogEntry::StreamTerminal(_))),
            "the producer output must still be open before deletion"
        );
        let indexed = SqliteIndexedStorage::configured(&derive_disjoint_sqlite_config(
            &sqlite_storage_config(deps, &context),
            "indexed",
        ))
        .await?;
        if seal_producer {
            let worker = executor.active_agents().try_get(&producer).await.unwrap();
            let last = history.last().unwrap().oplog_index;
            tokio::time::timeout(Duration::from_secs(30), async {
                while indexed
                    .observe_agent("test", "seal_open_producer", &producer)
                    .await?
                    .has_data
                {
                    worker
                        .worker_activator()
                        .archive_oplog(&producer, last, ArchiveWait::Finished)
                        .await?;
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                anyhow::Ok(())
            })
            .await??;
            assert!(!executor.worker_is_cached(&producer).await);
            assert_eq!(
                executor.get_oplog(&producer, OplogIndex::INITIAL).await?,
                history
            );
        }
        let gate = Arc::new(SealedDeletionGate {
            target: producer.clone(),
            fail_preparation: false,
            fail_cache_removal: AtomicBool::new(false),
            preparation_calls: AtomicUsize::new(1),
            removal_calls: AtomicUsize::new(0),
            claims: tokio::sync::Semaphore::new(0),
            started_claims: AtomicUsize::new(0),
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        });
        executor.set_worker_deletion_hook(gate.clone());
        let deletion = tokio::spawn({
            let executor = executor.clone();
            let producer = producer.clone();
            async move { executor.delete_worker(&producer).await }
        });
        tokio::time::timeout(Duration::from_secs(30), gate.entered.acquire())
            .await??
            .forget();
        let cascading = executor.get_oplog(&producer, OplogIndex::INITIAL).await?;
        assert_eq!(&cascading[..history.len()], history);
        let records: Vec<_> = cascading[history.len()..]
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::StreamSession(params) => {
                    Some(StreamSessionRecord::from_value(params.record.value()).unwrap())
                }
                _ => None,
            })
            .collect();
        assert!(records.iter().any(|record| matches!(record,
            StreamSessionRecord::ProducerDeleting(record)
                if record.producer == producer.agent_id
                    && record.producer_environment_id == producer.environment_id
                    && record.producer_fingerprint == attachments[0].expected_producer_fingerprint
        )), "both resident and sealed producers must commit their deletion barrier before removal: {records:?}");
        let terminals: Vec<_> = cascading[history.len()..]
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::StreamTerminal(params) => {
                    Some(StreamTerminalRecord::from_value(params.record.value()).unwrap())
                }
                _ => None,
            })
            .collect();
        let consumer_history = executor.get_oplog(&consumer, OplogIndex::INITIAL).await?;
        for attachment in &attachments {
            let terminal = terminals
                .iter()
                .find(|terminal| {
                    StreamId::derive(
                        producer.environment_id,
                        &producer.agent_id,
                        attachment.expected_producer_fingerprint,
                        terminal.stream_id.0,
                    ) == attachment.stream_id
                })
                .expect(
                    "the previously open attached output must have a committed deletion terminal",
                );
            assert_eq!(terminal.authored_by, StreamTerminalAuthor::Host);
            assert_eq!(terminal.origin, StreamTerminalOrigin::Control);
            assert_eq!(
                terminal.terminal,
                StreamTerminal::Failed(StreamError::Interrupted("producer deleting".into()))
            );
            let outbox = records.iter().find_map(|record| match record {
                StreamSessionRecord::CascadeOutbox(record) if record.key == *attachment => {
                    Some(record)
                }
                _ => None,
            }).expect("the independent active attachment must have committed cascade completion before removal");
            let StreamCascadeDependentResult::SourceUnavailable {
                first_unjournaled_offset,
            } = &outbox.result
            else {
                panic!(
                    "the suspended consumer must retain deterministic loss of the open stream: {outbox:?}"
                );
            };
            assert!(*first_unjournaled_offset <= terminal.offset);
            assert!(consumer_history.iter().any(|entry| match &entry.entry {
                PublicOplogEntry::StreamSession(params) => matches!(
                    StreamSessionRecord::from_value(params.record.value()).unwrap(),
                    StreamSessionRecord::SourceUnavailable(record)
                        if record.source_offset == *first_unjournaled_offset
                            && record.session_key.idempotency_key() == &attachment.session_key.idempotency_key
                ),
                _ => false,
            }), "cascade outbox must follow the independent consumer's committed source-unavailable journal");
        }
        gate.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(30), deletion).await???;
        assert!(executor.get_worker_metadata_opt(&producer).await?.is_none());
        // The consumer's durable recheck is deliberately far in the future. Resume it explicitly
        // so both the resident control and sealed producer exercise the same missing-source read.
        if executor.get_worker_metadata(&consumer).await?.status == AgentStatus::Suspended {
            executor.resume(&consumer, false).await?;
        }
        let failure = tokio::time::timeout(Duration::from_secs(30), invocation)
            .await??
            .unwrap_err();
        let failure = failure.to_string();
        // This fixture surfaces the missing-source failure as a guest trap.
        // The committed terminal and consumer journal above identify the missing source.
        assert!(
            failure.contains("DeterministicTrap") && failure.contains("StreamingRpcCallerImpl"),
            "{failure}"
        );
        if seal_producer {
            assert_eq!(
                Some(&failure),
                resident_failure.as_ref(),
                "sealing must preserve the resident producer-deletion outcome"
            );
        } else {
            resident_failure = Some(failure.clone());
        }
        assert_eq!(
            executor.get_worker_metadata(&consumer).await?.status,
            AgentStatus::Failed
        );
        let history = executor.get_oplog(&consumer, OplogIndex::INITIAL).await?;
        let duplicate = executor
            .invoke_and_await_agent_with_key(
                &component,
                &consumer_key,
                &key,
                "fork_drop_inherited_output",
                data_value!(promise, consumer.agent_id.to_string(), false),
            )
            .await
            .unwrap_err();
        let duplicate = duplicate.to_string();
        assert!(
            duplicate.contains("PreviousInvocationFailed")
                && duplicate.contains("DeterministicTrap")
                && duplicate.contains("StreamingRpcCallerImpl"),
            "an exact retry must observe the failed consumer's health: {duplicate}"
        );
        if seal_producer {
            assert_eq!(
                Some(&duplicate),
                resident_retry_failure.as_ref(),
                "sealing must preserve the resident producer-deletion retry outcome"
            );
        } else {
            resident_retry_failure = Some(duplicate);
        }
        assert_eq!(
            executor.get_oplog(&consumer, OplogIndex::INITIAL).await?,
            history,
            "an exact retry must not execute the failed consumer again"
        );
    }
    Ok(())
}

async fn sealed_duplicate(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_sdk_rust: &PrecompiledComponent,
    method: &str,
    leave_prepared_attachment: bool,
    mode: AgentMode,
    deletion: Option<SealedDeletionCheck>,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let postgres = if deletion == Some(SealedDeletionCheck::PostgresDelete) {
        Some(DockerPostgresRdb::new(&uuid::Uuid::new_v4().to_string(), false).await)
    } else {
        None
    };
    let postgres_config = postgres.as_ref().map(|postgres| {
        let url = url::Url::parse(&postgres.public_connection_string()).unwrap();
        IndexedStoragePostgresConfig {
            postgres: DbPostgresConfig {
                host: url.host_str().unwrap().to_string(),
                port: url.port().unwrap(),
                database: url.path().trim_start_matches('/').to_string(),
                username: url.username().to_string(),
                password: url.password().unwrap().to_string(),
                max_connections: 10,
                schema: None,
                acquire_timeout: None,
            },
            drop_prefix_delete_batch_size: 1024,
            max_concurrent_ops: None,
        }
    });
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    let kv = Arc::new(OnceLock::<Arc<dyn KeyValueStorage + Send + Sync>>::new());
    let deletion_lists = Arc::new(SealWriteFault::default());
    let collect_stream_deletion = method == "identity"
        && matches!(
            deletion,
            Some(
                SealedDeletionCheck::Delete
                    | SealedDeletionCheck::PostgresDelete
                    | SealedDeletionCheck::UnresolvedAdmission
            )
        );
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new({
            let postgres_config = postgres_config.clone();
            move |config| {
                if let Some(postgres_config) = &postgres_config {
                    config.indexed_storage =
                        IndexedStorageConfig::Postgres(postgres_config.clone());
                }
                config.oplog.indexed_storage_layers = 2;
                config.oplog.blob_storage_layers = 1;
                config.oplog.entry_count_limit = 1_000_000;
                config.oplog.shard_lease_max_ttl = Duration::from_secs(300);
                config.oplog.skew_margin = Duration::from_secs(1);
                if deletion.is_some() {
                    config.oplog.sweep.enabled = true;
                    config.oplog.archive_interval = Duration::from_secs(3600);
                }
                if collect_stream_deletion {
                    config.oplog.shard_lease_max_ttl = Duration::from_secs(30);
                    config.oplog.seal_put_timeout = Duration::from_secs(1);
                    config.oplog.server_commit_bound = Duration::from_secs(1);
                    config.oplog.archive_interval = Duration::from_secs(1);
                    // Preserve the exact recovery action until the deletion assertions finish.
                    // A reconstructed executor below runs the real scheduler through GC.
                    config.scheduler.refresh_interval = Duration::from_secs(3600);
                }
            }
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
        wrap_blob_storage: Some(Arc::new({
            let fault = deletion_lists.clone();
            move |inner| {
                Arc::new(FailingSealStorage {
                    inner,
                    fault: fault.clone(),
                })
            }
        })),
        ..Default::default()
    };
    let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let durable = mode == AgentMode::Durable;
    let constructor = if durable {
        agent_key!("DurableStreamAgent", "sealed-deletion")
    } else {
        agent_key!("EphemeralStreamAgent")
    };
    let actual_method = if durable && method == "identity" {
        "json"
    } else {
        method
    };
    let invocation_key = IdempotencyKey::fresh();
    // A new attempt is an attachment conflict, not a reload of the retained response.
    // Clone the complete request so acceptance identity and execution configuration also match.
    let stream_request = if method == "identity" {
        Some(ephemeral_duplicate_stream_request(
            &executor,
            &component,
            &invocation_key,
            uuid::Uuid::new_v4(),
            durable,
        )?)
    } else {
        None
    };
    let shard = shard.get().unwrap();
    if deletion == Some(SealedDeletionCheck::StaleEpoch) {
        let assignment = shard.current_assignment()?;
        let epochs = assignment
            .shard_epochs
            .keys()
            .map(|id| (*id, ShardEpoch(100)))
            .collect();
        shard.install_unexpiring(assignment.number_of_shards, &epochs);
    }
    let owned = if durable {
        executor
            .start_agent(&component.id, constructor.clone())
            .await?
    } else {
        crate::agent::ephemeral_agent_id(
            &executor,
            component.environment_id,
            &constructor,
            &invocation_key,
            method,
        )
    };
    // Retirement runs immediately after the response. Its lease must already be sealable.
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
    let (original, prepared_attachment, fingerprint) = if leave_prepared_attachment {
        let mut success = executor.gate_next_agent_function_success(&owned.agent_id, actual_method);
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
                    durable,
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
        if collect_stream_deletion {
            install_sealable_lease(shard.as_ref())?;
        }
        success.release();
        (original.await??, Some(prepared), fingerprint)
    } else {
        let result = ephemeral_duplicate_result(
            &executor,
            &component,
            &invocation_key,
            stream_request.as_ref(),
            durable,
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
        let fingerprint = executor.get_worker_metadata(&owned).await?.fingerprint;
        (original, None, fingerprint)
    };
    let mut expected = Vec::new();
    if durable && method == "identity" {
        expected.push("message-0000".to_string());
    } else if method == "identity" {
        expected.push("sealed-stream-label".to_string());
    }
    if !durable || method == "instance" {
        expected.push(owned.agent_id.as_uuid().to_string());
    }
    assert_eq!(original, EphemeralResponse::Items(expected));

    if durable {
        let worker = executor.active_agents().try_get(&owned).await.unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            while executor.worker_is_loaded(&owned).await {
                executor.stop_worker_if_idle(&owned).await?;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            worker.wait_for_unload_cleanup().await?;
            if collect_stream_deletion {
                install_sealable_lease(shard.as_ref())?;
            }
            let last = worker.oplog_service().get_last_index(&owned, mode).await?;
            loop {
                match worker
                    .worker_activator()
                    .archive_oplog(&owned, last, ArchiveWait::Finished)
                    .await?
                {
                    Some(true) => tokio::time::sleep(Duration::from_millis(10)).await,
                    Some(false) => break,
                    None => anyhow::bail!("durable fixture disappeared before sealing"),
                }
            }
            anyhow::Ok(())
        })
        .await??;
    }
    let indexed_config =
        derive_disjoint_sqlite_config(&sqlite_storage_config(deps, &context), "indexed");
    let indexed: Arc<dyn IndexedStorage + Send + Sync> = if let Some(config) = &postgres_config {
        Arc::new(
            PostgresIndexedStorage::configured(config)
                .await
                .map_err(anyhow::Error::msg)?,
        )
    } else {
        Arc::new(SqliteIndexedStorage::configured(&indexed_config).await?)
    };
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
    assert_sealed_derived_kv_absent(kv.get().unwrap().as_ref(), &owned, fingerprint)
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "{mode:?} {method}: seal cleanup before read-only metadata observation: {error}"
            )
        })?;
    let metadata = executor.get_worker_metadata(&owned).await?;
    assert_eq!(metadata.fingerprint, fingerprint);
    assert_sealed_derived_kv_absent(kv.get().unwrap().as_ref(), &owned, metadata.fingerprint)
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "{mode:?} {method}: after read-only sealed metadata observation: {error}"
            )
        })?;
    let history = executor.get_oplog(&owned, OplogIndex::INITIAL).await?;
    assert!(!history.is_empty(), "retirement must retain sealed history");
    assert_sealed_derived_kv_absent(kv.get().unwrap().as_ref(), &owned, fingerprint)
        .await
        .map_err(|error| {
            anyhow::anyhow!("{mode:?} {method}: after read-only sealed oplog observation: {error}")
        })?;
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
    if postgres_config.is_some() {
        executor.retire_unloaded_worker(&owned).await?;
        assert!(!executor.worker_is_cached(&owned).await);
        if !durable {
            let duplicate = ephemeral_duplicate_result(
                &executor,
                &component,
                &invocation_key,
                stream_request.as_ref(),
                durable,
            )
            .await?;
            if history.iter().any(|entry| match &entry.entry {
                PublicOplogEntry::StreamSession(params) => matches!(
                    StreamSessionRecord::from_value(params.record.value()).unwrap(),
                    StreamSessionRecord::Detached(_)
                ),
                _ => false,
            }) {
                assert_eq!(duplicate, EphemeralResponse::AttachmentRevoked);
            } else {
                assert_eq!(duplicate, original);
            }
        }
        assert!(
            !indexed
                .observe_agent("test", "postgres_sealed_observation", &owned)
                .await?
                .has_data
        );
        assert_eq!(
            executor.get_oplog(&owned, OplogIndex::INITIAL).await?,
            history
        );
        executor = delete_sealed_stream_after_duplicate(
            executor,
            &owned,
            mode,
            &history,
            indexed.as_ref(),
            deps,
            &context,
            TestExecutorOverrides {
                configure: overrides.configure.clone(),
                ..Default::default()
            },
        )
        .await?;
        assert!(executor.get_worker_metadata_opt(&owned).await?.is_none());
        assert!(
            !indexed
                .observe_agent("test", "postgres_deleted", &owned)
                .await?
                .has_data
        );
        collect_deleted_stream_storage(
            executor,
            &owned,
            mode,
            deps,
            &context,
            TestExecutorOverrides {
                configure: overrides.configure.clone(),
                ..Default::default()
            },
            postgres_config.as_ref(),
        )
        .await?;
        return Ok(());
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
    if !durable {
        let duplicate = ephemeral_duplicate_result(
            &executor,
            &component,
            &invocation_key,
            stream_request.as_ref(),
            durable,
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
            anyhow::anyhow!(
                "cold sealed duplicate failed: {error}; recorded sessions: {sessions:?}"
            )
        })?;
        // Transport completion can detach the original attachment before retirement.
        // An exact retry preserves that recorded refusal without activating the ephemeral agent.
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
    } else {
        // Durable invocation ingress unseals even an exact duplicate. Observe the natural seal
        // through metadata and oplog reads instead; neither API constructs a cached Worker.
        assert_eq!(
            executor.get_worker_metadata(&owned).await?.fingerprint,
            fingerprint
        );
        assert_eq!(
            executor.get_oplog(&owned, OplogIndex::INITIAL).await?,
            history
        );
        assert!(!executor.worker_is_cached(&owned).await);
    }
    assert!(!executor.worker_is_loaded(&owned).await);
    assert_sealed_derived_kv_absent(kv.get().unwrap().as_ref(), &owned, metadata.fingerprint)
        .await?;
    assert!(
        !indexed
            .observe_agent("test", "sealed_observation", &owned)
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
        "sealed observation or ephemeral duplicate wrote indexed storage: {writes:?}"
    );
    let kv_writes: Vec<String> =
        sqlx::query_scalar("SELECT source FROM sealed_duplicate_kv_writes")
            .fetch_all(&kv_pool)
            .await?;
    assert!(
        kv_writes.is_empty(),
        "sealed observation or ephemeral duplicate wrote derived KV storage: {kv_writes:?}"
    );
    if deletion == Some(SealedDeletionCheck::StaleEpoch) {
        let epochs = assignment
            .shard_epochs
            .keys()
            .map(|id| (*id, ShardEpoch(99)))
            .collect();
        shard.install_unexpiring(assignment.number_of_shards, &epochs);
        install_sealable_lease(shard.as_ref())?;
        assert!(
            executor.delete_worker(&owned).await.is_err(),
            "a lower epoch must not delete sealed history"
        );
        assert_eq!(
            executor.get_oplog(&owned, OplogIndex::INITIAL).await?,
            history
        );
        assert!(
            !indexed
                .observe_agent("test", "stale_delete", &owned)
                .await?
                .has_data
        );
        let writes: Vec<String> = sqlx::query_scalar("SELECT source FROM sealed_duplicate_writes")
            .fetch_all(&pool)
            .await?;
        assert!(
            writes.is_empty(),
            "a stale delete must not write even a temporary epoch row: {writes:?}"
        );
        let epochs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM indexed_key_epoch WHERE key = ? OR (key >= ? || '#' AND key < ? || '$')")
            .bind(&key).bind(&key).bind(&key).fetch_one(&pool).await?;
        assert_eq!(epochs, 0);
    } else if matches!(
        deletion,
        Some(
            SealedDeletionCheck::Delete
                | SealedDeletionCheck::NoHistoryOverlap
                | SealedDeletionCheck::NoHistoryTombstoneRetry
                | SealedDeletionCheck::UnresolvedAdmission
        )
    ) {
        let list_start = deletion_lists.lists.lock().unwrap().len();
        if method == "identity" {
            if deletion == Some(SealedDeletionCheck::UnresolvedAdmission) {
                delete_with_unresolved_admission(&executor, &owned, &history).await?;
            } else {
                executor = delete_sealed_stream_after_duplicate(
                    executor,
                    &owned,
                    mode,
                    &history,
                    indexed.as_ref(),
                    deps,
                    &context,
                    TestExecutorOverrides {
                        configure: overrides.configure.clone(),
                        ..Default::default()
                    },
                )
                .await?;
            }
            let writes: Vec<String> =
                sqlx::query_scalar("SELECT source FROM sealed_duplicate_writes")
                    .fetch_all(&pool)
                    .await?;
            assert!(
                writes.iter().any(|write| write == "index_storage:INSERT"),
                "stream deletion must restore a writable primary before cascade: {writes:?}"
            );
        } else {
            if deletion == Some(SealedDeletionCheck::NoHistoryOverlap) {
                delete_sealed_no_history_with_overlap(&executor, &owned, &history).await?;
            } else if deletion == Some(SealedDeletionCheck::NoHistoryTombstoneRetry) {
                delete_sealed_no_history_after_tombstone(&executor, &owned, &history).await?;
            } else {
                executor.delete_worker(&owned).await?;
            }
            let writes: Vec<String> =
                sqlx::query_scalar("SELECT source FROM sealed_duplicate_writes")
                    .fetch_all(&pool)
                    .await?;
            assert!(
                writes
                    .iter()
                    .all(|write| !write.starts_with("index_storage:")),
                "no-stream deletion must not restore indexed authority: {writes:?}"
            );
            let shard_id = golem_common::model::ShardId::from_agent_id(
                &owned.agent_id,
                assignment.number_of_shards,
            );
            let epoch = assignment.shard_epochs[&shard_id];
            let namespace = match mode {
                AgentMode::Durable => "durable-worker-oplog",
                AgentMode::Ephemeral => "ephemeral-worker-c1-oplog",
            };
            let fences: Vec<(String, String, i64)> = sqlx::query_as(
                "SELECT namespace, key, epoch FROM indexed_key_epoch
                 WHERE key = ? OR (key >= ? || '#' AND key < ? || '$')",
            )
            .bind(&key)
            .bind(&key)
            .bind(&key)
            .fetch_all(&pool)
            .await?;
            assert_eq!(
                fences,
                vec![(namespace.to_string(), key.clone(), i64::try_from(epoch.0)?)],
                "no-stream deletion must retain exactly its owning fence until tombstone collection"
            );
        }
        if mode == AgentMode::Ephemeral && deletion == Some(SealedDeletionCheck::Delete) {
            let calls = deletion_lists.lists.lock().unwrap();
            assert!(
                calls[list_start..].iter().any(|(_, _, api, namespace, _)| {
                    *api == "deletion"
                        && matches!(
                            namespace,
                            BlobStorageNamespace::SealRecords {
                                agent_mode: AgentMode::Ephemeral,
                                ..
                            }
                        )
                }),
                "deleting the cached frozen duplicate must list its sealed history: {:?}",
                &calls[list_start..]
            );
        }
        assert!(executor.get_worker_metadata_opt(&owned).await?.is_none());
        assert!(!executor.worker_is_cached(&owned).await);
        assert!(
            !indexed
                .observe_agent("test", "deleted", &owned)
                .await?
                .has_data
        );
    }
    kv_pool.close().await;
    pool.close().await;
    if collect_stream_deletion {
        collect_deleted_stream_storage(
            executor,
            &owned,
            mode,
            deps,
            &context,
            TestExecutorOverrides {
                configure: overrides.configure.clone(),
                ..Default::default()
            },
            None,
        )
        .await?;
    }
    Ok(())
}

async fn scheduled_actions_for_agent(
    pool: &sqlx::SqlitePool,
    owned: &OwnedAgentId,
) -> anyhow::Result<Vec<ScheduledAction>> {
    let rows: Vec<Vec<u8>> = sqlx::query_scalar("SELECT action FROM scheduled_actions")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|bytes| golem_common::serialization::deserialize::<ScheduledAction>(&bytes).unwrap())
        .filter(|action| action.owned_agent_id() == *owned)
        .collect())
}

async fn collect_deleted_stream_storage(
    executor: TestWorkerExecutor,
    owned: &OwnedAgentId,
    mode: AgentMode,
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    mut overrides: TestExecutorOverrides,
    postgres: Option<&IndexedStoragePostgresConfig>,
) -> anyhow::Result<()> {
    let scheduler_config = scheduler_sqlite_storage_config(deps, context);
    let scheduler = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&scheduler_config.database)
            .create_if_missing(false),
    )
    .await?;
    let pending = scheduled_actions_for_agent(&scheduler, owned).await?;
    assert!(
        pending.iter().any(|action| matches!(action,
            ScheduledAction::CollectTombstone { agent_mode, .. } if *agent_mode == mode
        )),
        "deletion must leave durable target-specific collection work: {pending:?}"
    );
    assert!(
        pending.iter().any(|action| matches!(action,
            ScheduledAction::ArchiveOplog { agent_mode, .. } if *agent_mode == mode
        )),
        "the restored history's archive action must also be drained: {pending:?}"
    );

    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let configure = overrides.configure.take().unwrap();
    overrides.configure = Some(Arc::new(move |config| {
        configure(config);
        config.scheduler.refresh_interval = Duration::from_millis(100);
    }));
    let blobs = Arc::new(OnceLock::<Arc<dyn BlobStorage>>::new());
    overrides.wrap_blob_storage = Some(Arc::new({
        let blobs = blobs.clone();
        move |inner| {
            assert!(blobs.set(inner.clone()).is_ok());
            inner
        }
    }));
    let executor = start_with_overrides(deps, context, overrides).await?;
    let blobs = blobs.get().unwrap();
    let seals = std::path::Path::new("seals").join(agent_path_segment(&owned.agent_id));
    let chunks = std::path::Path::new("chunks").join(agent_path_segment(&owned.agent_id));
    let records = blobs
        .with("test", "before_deleted_gc")
        .list_page(
            BlobStorageNamespace::SealRecords {
                environment_id: owned.environment_id,
                type_tag: owned.agent_id.type_tag(),
                agent_mode: mode,
            },
            &seals,
            None,
            1000,
        )
        .await?;
    assert!(
        records.iter().any(|record| record
            .path
            .extension()
            .is_some_and(|extension| extension == "deleted")),
        "GC must start from a real tombstone, not only logical absence: {records:?}"
    );
    let gc_deadline_millis = records
        .iter()
        .filter(|record| {
            record
                .path
                .extension()
                .is_some_and(|extension| extension == "deleted")
        })
        .map(|record| record.last_modified_at.to_millis() + 34_000)
        .max()
        .unwrap();
    let pg = if let Some(config) = postgres {
        Some(
            sqlx::PgPool::connect_with(
                sqlx::postgres::PgConnectOptions::new()
                    .host(&config.postgres.host)
                    .port(config.postgres.port)
                    .database(&config.postgres.database)
                    .username(&config.postgres.username)
                    .password(&config.postgres.password),
            )
            .await?,
        )
    } else {
        None
    };
    let sqlite = if pg.is_none() {
        let config =
            derive_disjoint_sqlite_config(&sqlite_storage_config(deps, context), "indexed");
        Some(
            sqlx::SqlitePool::connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&config.database)
                    .create_if_missing(false),
            )
            .await?,
        )
    } else {
        None
    };
    // T_gc is 30s lease TTL + 1s PUT timeout + 1s commit bound + twice the 1s skew.
    // Do not delete test data or acknowledge actions here: the production scheduler must
    // collect the tombstone and drain its own durable work after eligibility.
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let mut remaining_indexed = Vec::new();
            for table in ["index_storage", "indexed_key_epoch"] {
                let count: i64 = if let Some(pg) = &pg {
                    sqlx::query_scalar(&format!(
                        "SELECT COUNT(*) FROM {table} WHERE env = $1 AND agent = $2"
                    ))
                    .bind(owned.environment_id.0)
                    .bind(owned.agent_id.as_uuid())
                    .fetch_one(pg)
                    .await?
                } else {
                    let key = owned.storage_key();
                    sqlx::query_scalar(&format!(
                        "SELECT COUNT(*) FROM {table} WHERE key = ? OR (key >= ? || '#' AND key < ? || '$')"
                    ))
                    .bind(&key)
                    .bind(&key)
                    .bind(&key)
                    .fetch_one(sqlite.as_ref().unwrap())
                    .await?
                };
                remaining_indexed.push((table, count));
            }
            let mut remaining_blobs = Vec::new();
            for agent_mode in [AgentMode::Durable, AgentMode::Ephemeral] {
                for (namespace, prefix) in [
                    (BlobStorageNamespace::SealRecords {
                        environment_id: owned.environment_id,
                        type_tag: owned.agent_id.type_tag(),
                        agent_mode,
                    }, &seals),
                    (BlobStorageNamespace::CompressedOplog {
                        environment_id: owned.environment_id,
                        type_tag: owned.agent_id.type_tag(),
                        agent_mode,
                        level: 0,
                    }, &chunks),
                ] {
                    remaining_blobs.extend(blobs.with("test", "after_deleted_gc")
                        .list_page(namespace, prefix, None, 1).await?);
                }
            }
            let remaining_actions = scheduled_actions_for_agent(&scheduler, owned).await?;
            if remaining_indexed.iter().all(|(_, count)| *count == 0)
                && remaining_blobs.is_empty()
                && remaining_actions.is_empty()
            {
                assert!(
                    Timestamp::now_utc().to_millis() >= gc_deadline_millis,
                    "the target must not be collected before its tombstones become eligible"
                );
                break;
            }
            anyhow::ensure!(
                started.elapsed() < Duration::from_secs(85),
                "post-T_gc target residue: indexed={remaining_indexed:?}, blobs={remaining_blobs:?}, actions={remaining_actions:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        anyhow::Ok(())
    }).await??;
    assert!(executor.get_worker_metadata_opt(owned).await?.is_none());
    assert!(
        executor
            .get_oplog(owned, OplogIndex::INITIAL)
            .await
            .is_err()
    );
    assert!(!executor.worker_is_cached(owned).await);
    assert!(!executor.worker_is_loaded(owned).await);
    assert_eq!(executor.instance_load_count(&owned.agent_id), 0);
    scheduler.close().await;
    if let Some(pg) = pg {
        pg.close().await;
    }
    if let Some(sqlite) = sqlite {
        sqlite.close().await;
    }
    Ok(())
}

struct SealedDeletionGate {
    target: OwnedAgentId,
    fail_preparation: bool,
    fail_cache_removal: AtomicBool,
    preparation_calls: AtomicUsize,
    removal_calls: AtomicUsize,
    claims: tokio::sync::Semaphore,
    started_claims: AtomicUsize,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

#[async_trait::async_trait]
impl WorkerDeletionHook for SealedDeletionGate {
    fn claimed(&self, target: &OwnedAgentId, started: bool) {
        if target == &self.target {
            if started {
                self.started_claims.fetch_add(1, Ordering::SeqCst);
            }
            self.claims.add_permits(1);
        }
    }

    async fn before_stage(
        &self,
        target: &OwnedAgentId,
        stage: WorkerDeletionStage,
    ) -> Result<(), WorkerExecutorError> {
        if target != &self.target {
            return Ok(());
        }
        let preparation = stage == WorkerDeletionStage::ExecutionFenced
            && self.preparation_calls.fetch_add(1, Ordering::SeqCst) == 0;
        let inspect_cascade = stage == WorkerDeletionStage::DurableStateRemoved
            && self.removal_calls.fetch_add(1, Ordering::SeqCst) == 0;
        if preparation || inspect_cascade {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        if preparation && self.fail_preparation {
            return Err(WorkerExecutorError::runtime(
                "injected failure after sealed deletion preparation",
            ));
        }
        if stage == WorkerDeletionStage::CacheRemoved
            && self.fail_cache_removal.swap(false, Ordering::SeqCst)
        {
            return Err(WorkerExecutorError::runtime(
                "injected failure after sealed deletion tombstone publication",
            ));
        }
        Ok(())
    }
}

async fn delete_sealed_no_history_with_overlap(
    executor: &TestWorkerExecutor,
    owned: &OwnedAgentId,
    history: &[PublicOplogEntryWithIndex],
) -> anyhow::Result<()> {
    let loads = executor.instance_load_count(&owned.agent_id);
    let gate = Arc::new(SealedDeletionGate {
        target: owned.clone(),
        fail_preparation: true,
        fail_cache_removal: AtomicBool::new(false),
        preparation_calls: AtomicUsize::new(0),
        removal_calls: AtomicUsize::new(0),
        claims: tokio::sync::Semaphore::new(0),
        started_claims: AtomicUsize::new(0),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    executor.set_worker_deletion_hook(gate.clone());
    let first = tokio::spawn({
        let executor = executor.clone();
        let owned = owned.clone();
        async move { executor.delete_worker(&owned).await }
    });
    tokio::time::timeout(Duration::from_secs(30), gate.entered.acquire())
        .await??
        .forget();
    tokio::time::timeout(Duration::from_secs(10), gate.claims.acquire())
        .await??
        .forget();
    let deleting = executor.active_agents().try_get(owned).await.unwrap();
    let overlapping = tokio::spawn({
        let executor = executor.clone();
        let owned = owned.clone();
        async move { executor.delete_worker(&owned).await }
    });
    // Wait for the second caller to join the retained handle, not merely to start its RPC.
    tokio::time::timeout(Duration::from_secs(10), gate.claims.acquire())
        .await??
        .forget();
    assert_eq!(gate.started_claims.load(Ordering::SeqCst), 1);
    assert_eq!(gate.preparation_calls.load(Ordering::SeqCst), 1);
    assert!(Arc::ptr_eq(
        &deleting,
        &executor.active_agents().try_get(owned).await.unwrap()
    ));
    gate.release.add_permits(1);
    let first = tokio::time::timeout(Duration::from_secs(30), first)
        .await??
        .unwrap_err();
    let overlapping = tokio::time::timeout(Duration::from_secs(30), overlapping)
        .await??
        .unwrap_err();
    assert!(
        first
            .to_string()
            .contains("injected failure after sealed deletion preparation"),
        "{first}"
    );
    assert_eq!(
        overlapping.to_string(),
        first.to_string(),
        "overlapping callers must share the first attempt's failure"
    );
    assert_eq!(gate.started_claims.load(Ordering::SeqCst), 1);
    assert_eq!(
        executor.get_oplog(owned, OplogIndex::INITIAL).await?,
        history
    );
    assert_eq!(executor.instance_load_count(&owned.agent_id), loads);

    let retry = tokio::spawn({
        let executor = executor.clone();
        let owned = owned.clone();
        async move { executor.delete_worker(&owned).await }
    });
    tokio::time::timeout(Duration::from_secs(30), gate.entered.acquire())
        .await??
        .forget();
    assert_eq!(
        gate.started_claims.load(Ordering::SeqCst),
        2,
        "only a later explicit call may claim the retry"
    );
    assert!(
        Arc::ptr_eq(
            &deleting,
            &executor.active_agents().try_get(owned).await.unwrap()
        ),
        "a sealed no-history deletion must retain its generation across overlap and retry"
    );
    assert_eq!(
        executor.get_oplog(owned, OplogIndex::INITIAL).await?,
        history,
        "the no-history retry must not append maintenance or guest execution records"
    );
    assert_eq!(executor.instance_load_count(&owned.agent_id), loads);
    gate.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(30), retry).await???;
    Ok(())
}

async fn delete_sealed_no_history_after_tombstone(
    executor: &TestWorkerExecutor,
    owned: &OwnedAgentId,
    history: &[PublicOplogEntryWithIndex],
) -> anyhow::Result<()> {
    let fingerprint = executor.get_worker_metadata(owned).await?.fingerprint;
    let loads = executor.instance_load_count(&owned.agent_id);
    let gate = Arc::new(SealedDeletionGate {
        target: owned.clone(),
        fail_preparation: false,
        fail_cache_removal: AtomicBool::new(true),
        preparation_calls: AtomicUsize::new(1),
        removal_calls: AtomicUsize::new(0),
        claims: tokio::sync::Semaphore::new(0),
        started_claims: AtomicUsize::new(0),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    executor.set_worker_deletion_hook(gate.clone());
    let first = tokio::spawn({
        let executor = executor.clone();
        let owned = owned.clone();
        async move { executor.delete_worker(&owned).await }
    });
    tokio::time::timeout(Duration::from_secs(30), gate.entered.acquire())
        .await??
        .forget();
    let deleting = executor.active_agents().try_get(owned).await.unwrap();
    assert_eq!(
        executor.get_oplog(owned, OplogIndex::INITIAL).await?,
        history
    );
    gate.release.add_permits(1);
    let failure = tokio::time::timeout(Duration::from_secs(30), first)
        .await??
        .unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("injected failure after sealed deletion tombstone publication"),
        "{failure}"
    );
    assert_eq!(gate.started_claims.load(Ordering::SeqCst), 1);
    assert!(executor.get_worker_metadata_opt(owned).await?.is_none());
    assert!(
        executor
            .get_oplog(owned, OplogIndex::INITIAL)
            .await
            .is_err(),
        "the published tombstone must hide the sealed history before cache cleanup retries"
    );
    assert!(
        executor.worker_is_cached(owned).await,
        "the failed final stage must leave cleanup owned by the cached deletion generation"
    );
    assert!(Arc::ptr_eq(
        &deleting,
        &executor.active_agents().try_get(owned).await.unwrap()
    ));
    let preparations = gate.preparation_calls.load(Ordering::SeqCst);
    tokio::time::timeout(
        Duration::from_secs(30),
        executor.delete_worker_with_fingerprint(owned, fingerprint),
    )
    .await??;
    assert_eq!(
        gate.started_claims.load(Ordering::SeqCst),
        2,
        "the tombstone must not turn explicit cleanup retry into a no-op"
    );
    assert_eq!(gate.preparation_calls.load(Ordering::SeqCst), preparations);
    assert_eq!(
        gate.removal_calls.load(Ordering::SeqCst),
        1,
        "retry must preserve completed durable removal and finish only the remaining cleanup"
    );
    assert!(!executor.worker_is_cached(owned).await);
    assert!(executor.get_worker_metadata_opt(owned).await?.is_none());
    assert_eq!(executor.instance_load_count(&owned.agent_id), loads);
    Ok(())
}

async fn delete_with_unresolved_admission(
    executor: &TestWorkerExecutor,
    owned: &OwnedAgentId,
    history: &[PublicOplogEntryWithIndex],
) -> anyhow::Result<()> {
    executor.retire_unloaded_worker(owned).await?;
    assert!(!executor.worker_is_cached(owned).await);
    let loads = executor.instance_load_count(&owned.agent_id);
    let selected = Arc::new(tokio::sync::Semaphore::new(0));
    let resolve = Arc::new(tokio::sync::Semaphore::new(0));
    let acquisitions = Arc::new(AtomicUsize::new(0));
    executor.set_existing_worker_acquisition_hook(Arc::new({
        let owned = owned.clone();
        let selected = selected.clone();
        let resolve = resolve.clone();
        let acquisitions = acquisitions.clone();
        move |id| {
            let pause = id == owned && acquisitions.fetch_add(1, Ordering::SeqCst) == 0;
            let selected = selected.clone();
            let resolve = resolve.clone();
            async move {
                if pause {
                    selected.add_permits(1);
                    resolve.acquire().await.unwrap().forget();
                }
            }
            .boxed()
        }
    }));
    let admission = tokio::spawn({
        let executor = executor.clone();
        let owned = owned.clone();
        async move { executor.resume(&owned, false).await }
    });
    // This hook runs after cold unresolved insertion and before ensure_existing initializes it.
    tokio::time::timeout(Duration::from_secs(30), selected.acquire())
        .await??
        .forget();
    assert!(executor.active_agents().try_get(owned).await.is_none());
    assert_eq!(executor.instance_load_count(&owned.agent_id), loads);
    let gate = Arc::new(SealedDeletionGate {
        target: owned.clone(),
        fail_preparation: false,
        fail_cache_removal: AtomicBool::new(false),
        preparation_calls: AtomicUsize::new(0),
        removal_calls: AtomicUsize::new(0),
        claims: tokio::sync::Semaphore::new(0),
        started_claims: AtomicUsize::new(0),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    executor.set_worker_deletion_hook(gate.clone());
    let deletion = tokio::spawn({
        let executor = executor.clone();
        let owned = owned.clone();
        async move { executor.delete_worker(&owned).await }
    });
    tokio::time::timeout(Duration::from_secs(30), gate.entered.acquire())
        .await??
        .forget();
    let reserved = executor.active_agents().try_get(owned).await.unwrap();
    assert_eq!(
        executor.get_oplog(owned, OplogIndex::INITIAL).await?,
        history
    );
    resolve.add_permits(1);
    let refusal = tokio::time::timeout(Duration::from_secs(30), admission)
        .await??
        .unwrap_err();
    let message = refusal.to_string().to_lowercase();
    assert!(
        message.contains("being deleted") || message.contains("not found"),
        "{refusal}"
    );
    assert!(Arc::ptr_eq(
        &reserved,
        &executor.active_agents().try_get(owned).await.unwrap()
    ));
    assert!(!executor.worker_is_loaded(owned).await);
    assert_eq!(
        executor.instance_load_count(&owned.agent_id),
        loads,
        "ordinary acquisition must not start a guest through deletion's reserved replacement"
    );
    assert_eq!(
        executor.get_oplog(owned, OplogIndex::INITIAL).await?,
        history
    );
    gate.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(30), gate.entered.acquire())
        .await??
        .forget();
    let cascading = executor.get_oplog(owned, OplogIndex::INITIAL).await?;
    assert_eq!(&cascading[..history.len()], history);
    assert!(
        !cascading[history.len()..]
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
    );
    assert_eq!(executor.instance_load_count(&owned.agent_id), loads);
    gate.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(30), deletion).await???;
    Ok(())
}

async fn delete_sealed_stream_after_duplicate(
    executor: TestWorkerExecutor,
    owned: &OwnedAgentId,
    mode: AgentMode,
    history: &[PublicOplogEntryWithIndex],
    indexed: &dyn IndexedStorage,
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    mut restart_overrides: TestExecutorOverrides,
) -> anyhow::Result<TestWorkerExecutor> {
    let frozen = executor.active_agents().try_get(owned).await;
    if mode == AgentMode::Ephemeral {
        assert!(
            frozen.is_some(),
            "the real duplicate must cache its frozen ephemeral generation"
        );
    } else {
        assert!(
            frozen.is_none(),
            "read-only durable observation must leave the naturally sealed source uncached"
        );
    }
    assert!(
        !indexed
            .observe_agent("test", "before_sealed_delete", owned)
            .await?
            .has_data
    );
    let loads = executor.instance_load_count(&owned.agent_id);
    let config = scheduler_sqlite_storage_config(deps, context);
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&config.database)
            .create_if_missing(false),
    )
    .await?;
    // A durable invocation can leave an older archive row. It must not satisfy the unseal check.
    let existing: Vec<(String, Vec<u8>)> =
        sqlx::query_as("SELECT schedule_id, action FROM scheduled_actions")
            .fetch_all(&pool)
            .await?;
    for (id, bytes) in existing {
        if matches!(golem_common::serialization::deserialize::<ScheduledAction>(&bytes),
            Ok(ScheduledAction::ArchiveOplog { owned_agent_id, .. }) if owned_agent_id == *owned)
        {
            sqlx::query("DELETE FROM scheduled_actions WHERE schedule_id = ?")
                .bind(id)
                .execute(&pool)
                .await?;
        }
    }
    let gate = Arc::new(SealedDeletionGate {
        target: owned.clone(),
        fail_preparation: true,
        fail_cache_removal: AtomicBool::new(false),
        preparation_calls: AtomicUsize::new(0),
        removal_calls: AtomicUsize::new(0),
        claims: tokio::sync::Semaphore::new(0),
        started_claims: AtomicUsize::new(0),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    executor.set_worker_deletion_hook(gate.clone());
    let deleting = tokio::spawn({
        let executor = executor.clone();
        let owned = owned.clone();
        async move { executor.delete_worker(&owned).await }
    });
    tokio::time::timeout(Duration::from_secs(30), gate.entered.acquire())
        .await??
        .forget();
    let writable = executor.active_agents().try_get(owned).await.unwrap();
    assert_eq!(writable.agent_mode(), mode);
    let fingerprint = writable.get_initial_worker_metadata().fingerprint;
    if let Some(frozen) = &frozen {
        assert!(
            !Arc::ptr_eq(frozen, &writable),
            "Delete must evict the frozen ephemeral generation"
        );
    }
    assert!(
        !executor.worker_is_loaded(owned).await,
        "Delete must not start the guest"
    );
    assert_eq!(executor.instance_load_count(&owned.agent_id), loads);
    assert!(
        indexed
            .observe_agent("test", "deletion_prepared", owned)
            .await?
            .has_data
    );
    assert_eq!(
        executor.get_oplog(owned, OplogIndex::INITIAL).await?,
        history
    );

    // These operations run after preparation returned its lifecycle and gate locks.
    let activation = tokio::time::timeout(
        Duration::from_secs(10),
        golem_worker_executor::worker::Worker::start_if_needed(writable.clone()),
    )
    .await?
    .unwrap_err();
    assert!(
        activation.to_string().contains("being deleted"),
        "{activation}"
    );
    let last = history.last().unwrap().oplog_index;
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(10),
            writable
                .worker_activator()
                .archive_oplog(owned, last, ArchiveWait::Finished),
        )
        .await??,
        None
    );
    assert!(
        indexed
            .observe_agent("test", "refused_seal", owned)
            .await?
            .has_data
    );
    assert!(Arc::ptr_eq(
        &writable,
        &executor.active_agents().try_get(owned).await.unwrap()
    ));

    gate.release.add_permits(1);
    let failure = tokio::time::timeout(Duration::from_secs(30), deleting)
        .await??
        .unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("injected failure after sealed deletion preparation"),
        "{failure}"
    );
    assert_eq!(
        executor.get_oplog(owned, OplogIndex::INITIAL).await?,
        history,
        "failed preparation must leave the restored history readable"
    );
    let actions: Vec<Vec<u8>> = sqlx::query_scalar("SELECT action FROM scheduled_actions")
        .fetch_all(&pool)
        .await?;
    pool.close().await;
    assert!(
        actions.iter().any(|bytes| matches!(
            golem_common::serialization::deserialize::<ScheduledAction>(bytes),
            Ok(ScheduledAction::ArchiveOplog { owned_agent_id, agent_mode, .. })
                if owned_agent_id == *owned && agent_mode == mode
        )),
        "unsealed deletion must register ArchiveOplog even for ephemeral sweep-enabled agents"
    );

    // The injected failure leaves the durable prefix of a crash after unseal, before cascade.
    // Shutdown and reconstruction discard the retained Deleting generation; this is not an
    // abrupt process crash, and the recovered executor must need no in-memory deletion state.
    drop(frozen);
    drop(writable);
    drop(gate);
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    restart_overrides.wrap_shard_service = Some(Arc::new({
        let shard = shard.clone();
        move |inner| {
            assert!(shard.set(inner.clone()).is_ok());
            inner
        }
    }));
    let executor = start_with_overrides(deps, context, restart_overrides).await?;
    install_sealable_lease(shard.get().unwrap().as_ref())?;
    assert!(!executor.worker_is_cached(owned).await);
    assert!(!executor.worker_is_loaded(owned).await);
    assert!(
        indexed
            .observe_agent("test", "reconstructed_delete_prefix", owned)
            .await?
            .has_data
    );
    assert_eq!(
        executor.get_oplog(owned, OplogIndex::INITIAL).await?,
        history,
        "the reconstructed executor must read the entire restored pre-cascade history"
    );
    assert_eq!(
        executor.get_worker_metadata(owned).await?.fingerprint,
        fingerprint
    );
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&config.database)
            .create_if_missing(false),
    )
    .await?;
    let recovered_actions: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT action FROM scheduled_actions")
            .fetch_all(&pool)
            .await?;
    pool.close().await;
    assert!(
        recovered_actions.iter().any(|bytes| matches!(
            golem_common::serialization::deserialize::<ScheduledAction>(bytes),
            Ok(ScheduledAction::ArchiveOplog { owned_agent_id, agent_mode, .. })
                if owned_agent_id == *owned && agent_mode == mode && actions.contains(bytes)
        )),
        "the reconstructed executor must retain target-specific ArchiveOplog before explicit retry"
    );
    let gate = Arc::new(SealedDeletionGate {
        target: owned.clone(),
        fail_preparation: true,
        fail_cache_removal: AtomicBool::new(false),
        preparation_calls: AtomicUsize::new(1),
        removal_calls: AtomicUsize::new(0),
        claims: tokio::sync::Semaphore::new(0),
        started_claims: AtomicUsize::new(0),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    executor.set_worker_deletion_hook(gate.clone());
    let retry = tokio::spawn({
        let executor = executor.clone();
        let owned = owned.clone();
        async move { executor.delete_worker(&owned).await }
    });
    tokio::time::timeout(Duration::from_secs(30), gate.entered.acquire())
        .await??
        .forget();
    let cascading = executor.get_oplog(owned, OplogIndex::INITIAL).await?;
    assert_eq!(&cascading[..history.len()], history);
    let records: Vec<_> = cascading[history.len()..]
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::StreamSession(params) => {
                Some(StreamSessionRecord::from_value(params.record.value()).unwrap())
            }
            _ => None,
        })
        .collect();
    assert!(
        records
            .iter()
            .any(|record| matches!(record, StreamSessionRecord::ProducerDeleting(_))),
        "cascade must commit ProducerDeleting before removal: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|record| matches!(record, StreamSessionRecord::AttachmentFinalized(_))),
        "deletion must reconcile the retained prepared attachment: {records:?}"
    );
    assert!(
        !cascading[history.len()..]
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_))),
        "deletion maintenance must not execute the guest"
    );
    gate.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(30), retry).await???;
    Ok(executor)
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
    durable: bool,
) -> anyhow::Result<EphemeralResponse> {
    let constructor = if durable {
        agent_key!("DurableStreamAgent", "sealed-deletion")
    } else {
        agent_key!("EphemeralStreamAgent")
    };
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
    durable: bool,
) -> anyhow::Result<InvocationRequest> {
    let constructor = if durable {
        agent_key!("DurableStreamAgent", "sealed-deletion")
    } else {
        agent_key!("EphemeralStreamAgent")
    };
    let method = if durable { "json" } else { "identity" };
    let (target, constructor) =
        crate::agent::invocation_target(executor, component, &constructor, key, method)?;
    let (_, input) = if durable {
        data_value!(1u32, 0u64).into_parts()
    } else {
        data_value!("sealed-stream-label".to_string()).into_parts()
    };
    Ok(InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            allow_streams: true,
            target: Some(target),
            constructor,
            component_id: Some(component.id.into()),
            method_name: Some(method.to_string()),
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

/// The first invocation has no metadata or indexed authority to reuse. Its known mode must
/// survive session ingress and acquisition, without repeating the seal selection at startup.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn new_ephemeral_may_exist_session_lists_seal_once_in_its_known_mode(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let fault = Arc::new(SealWriteFault::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.indexed_storage_layers = 2;
                config.oplog.blob_storage_layers = 1;
                config.oplog.entry_count_limit = 1_000_000;
            })),
            wrap_blob_storage: Some(Arc::new({
                let fault = fault.clone();
                move |inner| {
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
    let key = IdempotencyKey::fresh();
    let owned = crate::agent::ephemeral_agent_id(
        &executor,
        component.environment_id,
        &agent_key!("EphemeralStreamAgent"),
        &key,
        "identity",
    );
    let request = ephemeral_duplicate_stream_request(
        &executor,
        &component,
        &key,
        uuid::Uuid::new_v4(),
        false,
    )?;
    assert!(!executor.worker_is_cached(&owned).await);
    fault.lists.lock().unwrap().clear();
    let (prepared, release) = executor.gate_next_prepared_instance(&owned.agent_id);
    let invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let key = key.clone();
        async move {
            ephemeral_duplicate_result(&executor, &component, &key, Some(&request), false).await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), prepared).await??;
    // Snapshot before guest execution and retirement. The default unexpiring shard grant also
    // prevents a later retirement seal from contributing a different kind of LIST.
    let lists = fault.lists.lock().unwrap().clone();
    release.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), invocation).await???,
        EphemeralResponse::Items(vec![
            "sealed-stream-label".to_string(),
            owned.agent_id.as_uuid().to_string(),
        ])
    );
    let prefix = std::path::Path::new("seals").join(agent_path_segment(&owned.agent_id));
    let seal_lists: Vec<_> = lists
        .iter()
        .filter(|(_, _, _, namespace, path)| {
            matches!(namespace, BlobStorageNamespace::SealRecords { environment_id, type_tag, .. }
            if *environment_id == owned.environment_id() && *type_tag == owned.agent_id.type_tag())
                && path == &prefix
        })
        .collect();
    assert_eq!(seal_lists.len(), 1, "startup LISTs: {lists:#?}");
    let (operation, svc, api, namespace, _) = seal_lists[0];
    assert_eq!(
        (*operation, *svc, *api),
        ("list_page", "oplog_seal", "read_record")
    );
    assert!(
        matches!(
            namespace,
            BlobStorageNamespace::SealRecords {
                agent_mode: AgentMode::Ephemeral,
                ..
            }
        ),
        "startup LISTs: {lists:#?}"
    );
    assert!(
        lists.iter().all(|(_, _, api, _, _)| *api != "moved_data"),
        "startup must reuse its authority selection: {lists:#?}"
    );
    Ok(())
}

/// A lost shard takes priority over cold metadata and freshness reads. Validation that does
/// not need storage must still reject an invalid request, including KnownFresh requests.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn may_exist_session_refuses_lost_shard_before_failing_cold_storage(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::create_worker_response;

    let context = TestContext::new(last_unique_id);
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    let fault = Arc::new(SealWriteFault::default());
    let kv_faults = KeyValueStorageFaults::default();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.indexed_storage_layers = 2;
                config.oplog.blob_storage_layers = 1;
                config.oplog.entry_count_limit = 1_000_000;
            })),
            wrap_shard_service: Some(Arc::new({
                let shard = shard.clone();
                move |inner| {
                    assert!(shard.set(inner.clone()).is_ok());
                    inner
                }
            })),
            wrap_key_value_storage: Some(Arc::new({
                let faults = kv_faults.clone();
                move |inner| Arc::new(FaultInjectingKeyValueStorage::new(inner, faults.clone()))
            })),
            wrap_blob_storage: Some(Arc::new({
                let fault = fault.clone();
                move |inner| {
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
    let request = ephemeral_duplicate_stream_request(
        &executor,
        &component,
        &IdempotencyKey::fresh(),
        uuid::Uuid::new_v4(),
        false,
    )?;
    kv_faults.fail_all(
        usize::MAX,
        KeyValueStorageError::other("injected cold storage lookup failure"),
    );
    fault.fail_lists.store(true, Ordering::Release);
    // A healthy-shard control proves this exact cold request would encounter the outage.
    let error = rejected_session(&executor, request.clone()).await?;
    assert!(
        error.contains("injected cold storage lookup failure"),
        "{error}"
    );

    // Without an invocation key, creation must validate caller and local config eagerly,
    // before acquiring an owner or consulting the failing cold storage.
    let mut create = executor.create_keyed_worker_request(
        &component.id,
        &agent_key!("DurableStreamAgent", "no-key-create-validation"),
    )?;
    create.idempotency_key = None;
    create.freshness_disposition =
        golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist as i32;
    create.caller = None;
    create.config = vec![golem_api_grpc::proto::golem::worker::AgentConfigEntryDto {
        path: vec!["invalid-local-config".to_string()],
        value: "not-json".to_string(),
    }];
    let response = executor
        .client
        .clone()
        .create_worker(create.clone())
        .await?
        .into_inner();
    let Some(create_worker_response::Result::Failure(failure)) = response.result else {
        anyhow::bail!("no-key creation without a caller must fail: {response:?}");
    };
    assert!(
        format!("{failure:?}").contains("caller is missing"),
        "unexpected no-key create failure: {failure:?}"
    );

    create.caller = Some(callers::anonymous().into());
    let response = executor
        .client
        .clone()
        .create_worker(create)
        .await?
        .into_inner();
    let Some(create_worker_response::Result::Failure(failure)) = response.result else {
        anyhow::bail!("no-key creation with malformed local config must fail: {response:?}");
    };
    assert!(
        format!("{failure:?}").contains("Failed parsing local agent config"),
        "unexpected no-key create failure: {failure:?}"
    );

    shard.get().unwrap().clear_assignment();
    fault.lists.lock().unwrap().clear();
    let error = rejected_session(&executor, request.clone()).await?;
    assert!(error.contains("ShardingNotReady"), "{error}");
    assert!(
        fault.lists.lock().unwrap().is_empty(),
        "a lost shard must not LIST storage"
    );

    let mut invalid = request.clone();
    let Some(invocation_request::Request::Start(start)) = invalid.request.as_mut() else {
        unreachable!()
    };
    start.component_owner_account_id = None;
    let error = rejected_session(&executor, invalid).await?;
    assert!(error.contains("account_id not found"), "{error}");

    let mut known_fresh = request.clone();
    let Some(invocation_request::Request::Start(start)) = known_fresh.request.as_mut() else {
        unreachable!()
    };
    start.freshness_disposition =
        golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh as i32;
    start.constructor = None;
    let error = rejected_session(&executor, known_fresh).await?;
    assert!(
        error.contains("KnownFresh requires the constructor"),
        "{error}"
    );
    let mut known_fresh = request;
    let Some(invocation_request::Request::Start(start)) = known_fresh.request.as_mut() else {
        unreachable!()
    };
    start.freshness_disposition =
        golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh as i32;
    start.caller = None;
    let error = rejected_session(&executor, known_fresh).await?;
    assert!(error.contains("caller is missing"), "{error}");
    Ok(())
}

async fn rejected_session(
    executor: &TestWorkerExecutor,
    request: InvocationRequest,
) -> anyhow::Result<String> {
    let (frames, receiver) = tokio::sync::mpsc::channel(1);
    frames.send(request).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(tokio_stream::wrappers::ReceiverStream::new(receiver))
        .await?
        .into_inner();
    if let Some(response) = responses.message().await? {
        if let Some(invocation_response::Response::Rejected(rejected)) = &response.response {
            return Ok(format!("{} {:?}", rejected.error, rejected.worker_error));
        }
        anyhow::bail!("request must be rejected before acceptance: {response:?}");
    }
    drop(frames);
    anyhow::bail!("session ended without rejecting the invocation")
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
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let records = blobs
            .with("test", "assert_no_seal")
            .list_page(
                BlobStorageNamespace::SealRecords {
                    environment_id: owned.environment_id(),
                    type_tag: owned.agent_id.type_tag(),
                    agent_mode: mode,
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
    }
    Ok(())
}

#[derive(Debug, Default)]
struct SealWriteFault {
    target: OnceLock<OwnedAgentId>,
    hits: AtomicUsize,
    lists: Mutex<
        Vec<(
            &'static str,
            &'static str,
            &'static str,
            BlobStorageNamespace,
            PathBuf,
        )>,
    >,
    fail_lists: AtomicBool,
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
        self.fault.lists.lock().unwrap().push((
            "list_page",
            svc,
            api,
            namespace.clone(),
            prefix.to_path_buf(),
        ));
        if self.fault.fail_lists.load(Ordering::Acquire) {
            anyhow::bail!("injected cold storage lookup failure");
        }
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
            && matches!(&namespace, BlobStorageNamespace::SealRecords { environment_id, type_tag, .. }
                if *environment_id == target.environment_id() && *type_tag == target.agent_id.type_tag())
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
        self.fault.lists.lock().unwrap().push((
            "list_dir",
            svc,
            api,
            namespace.clone(),
            path.to_path_buf(),
        ));
        if self.fault.fail_lists.load(Ordering::Acquire) {
            anyhow::bail!("injected cold storage lookup failure");
        }
        self.inner.list_dir(svc, api, namespace, path).await
    }

    async fn list_blobs_below_at(
        &self,
        svc: &'static str,
        api: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.fault.lists.lock().unwrap().push((
            "list_blobs_below",
            svc,
            api,
            namespace.clone(),
            path.to_path_buf(),
        ));
        if self.fault.fail_lists.load(Ordering::Acquire) {
            anyhow::bail!("injected cold storage lookup failure");
        }
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
        anyhow::ensure!(
            rows.is_empty(),
            "sealed history retained derived rows in {namespace:?}; fields: {:?}",
            rows.iter().map(|(key, _)| key).collect::<Vec<_>>()
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

#[test]
#[timeout("4m")]
async fn fully_archived_running_agent_stops_scheduling_and_seals_when_idle(
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
                config.oplog.archive_interval = Duration::from_millis(100);
                config.scheduler.refresh_interval = Duration::from_millis(50);
                config.oplog.shard_lease_max_ttl = Duration::from_secs(300);
                config.oplog.skew_margin = Duration::from_secs(1);
                config.suspend.suspend_after = Duration::from_secs(60);
                config.suspend.wait_suspend_grace = Duration::from_secs(60);
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
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let key = agent_key!("Clocks", "archive-running-then-idle");
    let id = executor.start_agent(&component.id, key.clone()).await?;
    executor
        .invoke_agent(&component, &key, "sleep_for", data_value!(10.0f64))
        .await?;
    executor
        .wait_for_status(&id, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    let worker = executor.active_agents().try_get(&id).await.unwrap();
    let service = worker.oplog_service();
    let activator = worker.worker_activator();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let last = service.get_last_index(&id, AgentMode::Durable).await?;
            if activator
                .archive_oplog(&id, last, ArchiveWait::Finished)
                .await?
                == Some(false)
                && scheduled_action_count(deps, &context).await? == 0
            {
                break anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    assert_eq!(
        executor.get_worker_metadata(&id).await?.status,
        AgentStatus::Running
    );
    // More than one archive interval must pass without an eligibility polling row.
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(scheduled_action_count(deps, &context).await?, 0);
    let indexed_config =
        derive_disjoint_sqlite_config(&sqlite_storage_config(deps, &context), "indexed");
    let indexed = SqliteIndexedStorage::configured(&indexed_config).await?;
    assert!(
        indexed
            .observe_agent("test", "running", &id)
            .await?
            .has_data
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        while indexed
            .observe_agent("test", "after_idle", &id)
            .await?
            .has_data
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    assert_eq!(
        executor.get_worker_metadata(&id).await?.status,
        AgentStatus::Idle
    );
    assert!(!executor.worker_is_cached(&id).await);
    executor.shutdown_and_wait_for_invocation_loops().await?;
    Ok(())
}

#[test]
#[timeout("3m")]
async fn seal_declines_a_suspended_generation_waiting_for_permit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    seal_declines_starting_generation(last_unique_id, deps, host, false).await
}

#[test]
#[timeout("3m")]
async fn seal_declines_a_suspended_generation_replaying(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    seal_declines_starting_generation(last_unique_id, deps, host, true).await
}

async fn seal_declines_starting_generation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host: &PrecompiledComponent,
    replay: bool,
) -> anyhow::Result<()> {
    use golem_common::model::account::AccountId;
    use golem_worker_executor::services::resource_limits::{AtomicResourceEntry, ResourceLimits};
    use golem_worker_executor::worker::Worker;
    use golem_worker_executor_test_utils::ReplayAdmissionStage;

    struct Limits(Arc<AtomicResourceEntry>);

    #[async_trait::async_trait]
    impl ResourceLimits for Limits {
        async fn initialize_account(
            &self,
            _account_id: AccountId,
        ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError> {
            Ok(self.0.clone())
        }
    }

    let context = TestContext::new(last_unique_id);
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    let limits = Arc::new(AtomicResourceEntry::new(
        u64::MAX,
        usize::MAX,
        usize::MAX,
        u64::MAX,
        1,
    ));
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            resource_limits: Some(Arc::new(Limits(limits.clone()))),
            configure: Some(Arc::new(|config| {
                config.oplog.indexed_storage_layers = 1;
                config.oplog.blob_storage_layers = 1;
                config.oplog.entry_count_limit = 1_000_000;
                config.oplog.archive_interval = Duration::from_millis(100);
                config.oplog.shard_lease_max_ttl = Duration::from_secs(300);
                config.oplog.skew_margin = Duration::from_secs(1);
                config.scheduler.refresh_interval = Duration::from_millis(25);
                config.suspend.suspend_after = Duration::from_millis(100);
                config.suspend.wait_suspend_grace = Duration::from_millis(100);
                config.suspend.wait_suspend_check_interval = Duration::from_millis(25);
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
        .component_dep(&context.default_environment_id, host)
        .store()
        .await?;
    let key = agent_key!("GolemHostApi", "seal-starting-generation");
    let id = executor.start_agent(&component.id, key.clone()).await?;
    let promise = executor
        .invoke_and_await_agent(&component, &key, "create_promise", data_value!())
        .await?
        .into_return_value()
        .unwrap();
    executor
        .invoke_agent(
            &component,
            &key,
            "local_calls_then_await_promise",
            crate::raw_params(vec![SchemaValue::U64(2), promise]),
        )
        .await?;
    executor
        .wait_for_status(&id, AgentStatus::Suspended, Duration::from_secs(30))
        .await?;
    let worker = executor.active_agents().try_get(&id).await.unwrap();
    worker.await_ready_to_process_commands().await?;
    assert!(!worker.is_loaded().await);
    // The harness grant has no expiry, so maintenance can fully archive without sealing.
    tokio::time::timeout(Duration::from_secs(30), async {
        while scheduled_action_count(deps, &context).await? != 0 {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        anyhow::Ok(())
    })
    .await??;
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
    let held_permit = if replay {
        None
    } else {
        Some(
            executor
                .active_agents()
                .register_account_concurrency(context.account_id, limits)
                .await
                .acquire(id.agent_id)
                .await,
        )
    };
    let mut queued = crate::scheduler_waits::queued(&context.account_id, &id.agent_id);
    let mut replay_gate = replay.then(|| {
        executor.gate_next_replay_access_admission(
            &id.agent_id,
            "golem::api::create_promise",
            ReplayAdmissionStage::BeforeScope,
        )
    });
    Worker::start_if_needed(worker.clone()).await?;
    if let Some(gate) = replay_gate.as_mut() {
        tokio::time::timeout(Duration::from_secs(15), gate.entered()).await?;
    } else {
        tokio::time::timeout(Duration::from_secs(15), queued.entered()).await??;
    }
    assert_eq!(
        worker.get_last_known_status().await.status,
        AgentStatus::Suspended
    );
    let last = worker
        .oplog_service()
        .get_last_index(&id, AgentMode::Durable)
        .await?;
    assert_eq!(
        worker
            .worker_activator()
            .archive_oplog(&id, last, ArchiveWait::Finished)
            .await?,
        Some(false),
        "busy resident admission must decline without a routing error"
    );
    assert!(Arc::ptr_eq(
        &worker,
        &executor.active_agents().try_get(&id).await.unwrap()
    ));
    assert!(!worker.oplog().is_retired());
    assert_eq!(scheduled_action_count(deps, &context).await?, 0);
    let readiness = tokio::spawn({
        let worker = worker.clone();
        async move { worker.await_ready_to_process_commands().await }
    });
    drop(held_permit);
    if let Some(gate) = replay_gate {
        gate.release();
    }
    tokio::time::timeout(Duration::from_secs(30), readiness).await???;
    let indexed_config =
        derive_disjoint_sqlite_config(&sqlite_storage_config(deps, &context), "indexed");
    let indexed = SqliteIndexedStorage::configured(&indexed_config).await?;
    tokio::time::timeout(Duration::from_secs(30), async {
        while indexed
            .observe_agent("test", "idle_retrigger", &id)
            .await?
            .has_data
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    assert!(!executor.worker_is_cached(&id).await);
    assert_eq!(
        executor.get_worker_metadata(&id).await?.status,
        AgentStatus::Suspended
    );
    executor.shutdown_and_wait_for_invocation_loops().await?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeletionRace {
    Resident,
    SealBeforeClaim,
    ClaimBeforeSeal,
    ActivationBeforePreparation,
    ExhaustAcquisitions,
}

#[test]
#[timeout("4m")]
async fn resident_deletion_reaches_removal_without_listing_seal_records(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    deletion_race(
        last_unique_id,
        deps,
        fixture,
        AgentMode::Durable,
        DeletionRace::Resident,
    )
    .await
}

#[test]
#[timeout("4m")]
async fn deletion_reacquires_after_seal_wins_its_claim(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        deletion_race(
            last_unique_id,
            deps,
            fixture,
            mode,
            DeletionRace::SealBeforeClaim,
        )
        .await?;
    }
    Ok(())
}

#[test]
#[timeout("4m")]
async fn deletion_claim_refuses_seal_without_holding_preparation_locks(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    deletion_race(
        last_unique_id,
        deps,
        fixture,
        AgentMode::Durable,
        DeletionRace::ClaimBeforeSeal,
    )
    .await
}

#[test]
#[timeout("4m")]
async fn sealed_deletion_rechecks_activation_before_preparation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    deletion_race(
        last_unique_id,
        deps,
        fixture,
        AgentMode::Durable,
        DeletionRace::ActivationBeforePreparation,
    )
    .await
}

#[test]
#[timeout("4m")]
async fn deletion_stops_after_three_seal_activation_races_and_can_retry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    deletion_race(
        last_unique_id,
        deps,
        fixture,
        AgentMode::Durable,
        DeletionRace::ExhaustAcquisitions,
    )
    .await
}

struct DeletionClaimPause {
    target: OwnedAgentId,
    enabled: AtomicBool,
    calls: AtomicUsize,
    started: AtomicUsize,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

#[async_trait::async_trait]
impl WorkerDeletionHook for DeletionClaimPause {
    async fn before_claim(&self, target: &OwnedAgentId) {
        if target == &self.target && self.enabled.load(Ordering::SeqCst) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
    }

    fn claimed(&self, target: &OwnedAgentId, started: bool) {
        if target == &self.target && started {
            self.started.fetch_add(1, Ordering::SeqCst);
        }
    }

    async fn before_stage(
        &self,
        _: &OwnedAgentId,
        _: WorkerDeletionStage,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }
}

async fn seal_deletion_race_generation(
    executor: &TestWorkerExecutor,
    indexed: &dyn IndexedStorage,
    owned: &OwnedAgentId,
    history: &[PublicOplogEntryWithIndex],
) -> anyhow::Result<()> {
    let old = executor.active_agents().try_get(owned).await.unwrap();
    let oplog = old.oplog();
    let last = oplog.current_oplog_index().await;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match old
                .worker_activator()
                .archive_oplog(owned, last, ArchiveWait::Finished)
                .await?
            {
                Some(true) => {}
                Some(false) => break,
                None => anyhow::bail!("the current generation refused archival"),
            }
        }
        anyhow::Ok(())
    })
    .await??;
    let sealed = indexed
        .observe_agent("test", "seal_claim_race", owned)
        .await?;
    assert!(
        !sealed.has_data,
        "seal declined: fence={:?}, status={:?}",
        oplog.fence(),
        old.get_last_known_status().await
    );
    assert_eq!(sealed.durable_marker, None);
    assert_eq!(sealed.ephemeral_marker, None);
    assert!(oplog.is_retired());
    assert!(!executor.worker_is_cached(owned).await);
    assert_eq!(
        executor.get_oplog(owned, OplogIndex::INITIAL).await?,
        history
    );
    Ok(())
}

async fn deletion_race(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    mode: AgentMode,
    race: DeletionRace,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    let lists = Arc::new(SealWriteFault::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.indexed_storage_layers = 2;
                config.oplog.blob_storage_layers = 1;
                config.oplog.entry_count_limit = 1_000_000;
                config.oplog.archive_interval = Duration::from_secs(3600);
                config.oplog.shard_lease_max_ttl = Duration::from_secs(300);
                config.oplog.skew_margin = Duration::from_secs(1);
                config.scheduler.refresh_interval = Duration::from_secs(3600);
            })),
            wrap_shard_service: Some(Arc::new({
                let shard = shard.clone();
                move |inner| {
                    assert!(shard.set(inner.clone()).is_ok());
                    inner
                }
            })),
            wrap_blob_storage: Some(Arc::new({
                let lists = lists.clone();
                move |inner| {
                    Arc::new(FailingSealStorage {
                        inner,
                        fault: lists.clone(),
                    })
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let key = IdempotencyKey::fresh();
    let constructor = if mode == AgentMode::Durable {
        agent_key!("DurableStreamAgent", "deletion-race")
    } else {
        agent_key!("EphemeralStreamAgent")
    };
    let owned = if mode == AgentMode::Durable {
        executor
            .start_agent(&component.id, constructor.clone())
            .await?
    } else {
        crate::agent::ephemeral_agent_id(
            &executor,
            component.environment_id,
            &constructor,
            &key,
            "instance",
        )
    };
    executor
        .invoke_and_await_agent_with_key(&component, &constructor, &key, "instance", data_value!())
        .await?;
    tokio::time::timeout(Duration::from_secs(30), async {
        while executor.worker_is_loaded(&owned).await {
            executor.stop_worker_if_idle(&owned).await?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    // An ephemeral retirement may have evicted its shell without sealing under the unexpiring grant.
    executor.attached_agent_status(&owned.agent_id).await?;
    let old = executor.active_agents().try_get(&owned).await.unwrap();
    let fingerprint = old.get_initial_worker_metadata().fingerprint;
    let history = executor.get_oplog(&owned, OplogIndex::INITIAL).await?;
    let indexed_config =
        derive_disjoint_sqlite_config(&sqlite_storage_config(deps, &context), "indexed");
    let indexed = SqliteIndexedStorage::configured(&indexed_config).await?;
    assert!(
        indexed
            .observe_agent("test", "resident_delete_race", &owned)
            .await?
            .has_data
    );
    install_sealable_lease(shard.get().unwrap().as_ref())?;
    if race == DeletionRace::ActivationBeforePreparation {
        seal_deletion_race_generation(&executor, &indexed, &owned, &history).await?;
    }
    let claim = Arc::new(DeletionClaimPause {
        target: owned.clone(),
        enabled: AtomicBool::new(true),
        calls: AtomicUsize::new(0),
        started: AtomicUsize::new(0),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    let stages = Arc::new(SealedDeletionGate {
        target: owned.clone(),
        fail_preparation: false,
        fail_cache_removal: AtomicBool::new(false),
        preparation_calls: AtomicUsize::new(0),
        removal_calls: AtomicUsize::new(0),
        claims: tokio::sync::Semaphore::new(0),
        started_claims: AtomicUsize::new(0),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    let pause_claim = matches!(
        race,
        DeletionRace::SealBeforeClaim | DeletionRace::ExhaustAcquisitions
    );
    if pause_claim {
        executor.set_worker_deletion_hook(claim.clone());
    } else {
        executor.set_worker_deletion_hook(stages.clone());
    }
    let preparation = (race == DeletionRace::ActivationBeforePreparation)
        .then(|| executor.gate_lifecycle_after_next_successful_open(&owned.agent_id));
    let list_start = lists.lists.lock().unwrap().len();
    let deleting = tokio::spawn({
        let executor = executor.clone();
        let owned = owned.clone();
        async move {
            use golem_api_grpc::proto::golem::workerexecutor::v1::{
                DeleteWorkerRequest, delete_worker_response,
            };
            let response = executor
                .client
                .clone()
                .delete_worker(DeleteWorkerRequest {
                    agent_id: Some(owned.agent_id.into()),
                    environment_id: Some(owned.environment_id.into()),
                    auth_ctx: Some(executor.auth_ctx().into()),
                    caller: Some(callers::anonymous().into()),
                    expected_fingerprint: Some(fingerprint.0.into()),
                })
                .await?
                .into_inner();
            anyhow::Ok(match response.result {
                Some(delete_worker_response::Result::Success(_)) => Ok(()),
                Some(delete_worker_response::Result::Failure(error)) => {
                    Err(WorkerExecutorError::try_from(error).map_err(anyhow::Error::msg)?)
                }
                None => anyhow::bail!("deletion returned no result"),
            })
        }
    });
    let mut activated = None;
    if let Some((entered, release)) = preparation {
        tokio::time::timeout(Duration::from_secs(30), entered).await??;
        assert!(
            !indexed
                .observe_agent("test", "frozen_delete_open", &owned)
                .await?
                .has_data
        );
        assert_eq!(stages.started_claims.load(Ordering::SeqCst), 0);
        assert_eq!(
            executor.get_oplog(&owned, OplogIndex::INITIAL).await?,
            history
        );
        assert!(!executor.worker_is_loaded(&owned).await);
        let frozen = executor.active_agents().try_get(&owned).await.unwrap();
        executor.retire_unloaded_worker(&owned).await?;
        executor.attached_agent_status(&owned.agent_id).await?;
        let replacement = executor.active_agents().try_get(&owned).await.unwrap();
        assert!(!Arc::ptr_eq(&frozen, &replacement));
        assert_eq!(
            replacement.get_initial_worker_metadata().fingerprint,
            fingerprint
        );
        assert!(
            indexed
                .observe_agent("test", "activated_delete", &owned)
                .await?
                .has_data
        );
        activated = Some(replacement);
        release.send(()).unwrap();
    }
    if pause_claim {
        let rounds = if race == DeletionRace::ExhaustAcquisitions {
            3
        } else {
            1
        };
        for round in 1..=rounds {
            tokio::time::timeout(Duration::from_secs(30), claim.entered.acquire())
                .await??
                .forget();
            let acquired = executor.active_agents().try_get(&owned).await.unwrap();
            seal_deletion_race_generation(&executor, &indexed, &owned, &history).await?;
            if race == DeletionRace::ExhaustAcquisitions {
                executor.attached_agent_status(&owned.agent_id).await?;
                let replacement = executor.active_agents().try_get(&owned).await.unwrap();
                assert!(!Arc::ptr_eq(&acquired, &replacement));
                assert_eq!(
                    replacement.get_initial_worker_metadata().fingerprint,
                    fingerprint
                );
                assert!(
                    indexed
                        .observe_agent("test", "activation_claim_race", &owned)
                        .await?
                        .has_data
                );
                assert_eq!(
                    executor.get_oplog(&owned, OplogIndex::INITIAL).await?,
                    history
                );
            }
            assert_eq!(claim.calls.load(Ordering::SeqCst), round);
            claim.release.add_permits(1);
        }
    } else {
        tokio::time::timeout(Duration::from_secs(30), stages.entered.acquire())
            .await??
            .forget();
        let owner = executor.active_agents().try_get(&owned).await.unwrap();
        if let Some(activated) = activated {
            assert!(Arc::ptr_eq(&owner, &activated));
        }
        assert_eq!(stages.started_claims.load(Ordering::SeqCst), 1);
        if race == DeletionRace::ClaimBeforeSeal {
            let last = owner.oplog().current_oplog_index().await;
            assert_eq!(
                owner
                    .worker_activator()
                    .archive_oplog(&owned, last, ArchiveWait::Finished)
                    .await?,
                None
            );
            assert!(
                indexed
                    .observe_agent("test", "claim_refuses_seal", &owned)
                    .await?
                    .has_data
            );
            assert!(Arc::ptr_eq(
                &owner,
                &executor.active_agents().try_get(&owned).await.unwrap()
            ));
        }
        stages.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(30), stages.entered.acquire())
            .await??
            .forget();
        if race == DeletionRace::Resident {
            let calls = lists.lists.lock().unwrap();
            assert!(
                calls[list_start..]
                    .iter()
                    .all(|(_, _, _, namespace, _)| !matches!(
                        namespace,
                        BlobStorageNamespace::SealRecords { .. }
                    )),
                "PG-present deletion must not list seal records before DurableStateRemoved: {:?}",
                &calls[list_start..]
            );
        }
        stages.release.add_permits(1);
    }
    let result = tokio::time::timeout(Duration::from_secs(30), deleting).await???;
    if race == DeletionRace::ExhaustAcquisitions {
        assert!(
            matches!(result, Err(WorkerExecutorError::ShardingNotReady)),
            "{result:?}"
        );
        assert_eq!(claim.calls.load(Ordering::SeqCst), 3);
        assert_eq!(claim.started.load(Ordering::SeqCst), 0);
        assert!(
            indexed
                .observe_agent("test", "bounded_delete_retry", &owned)
                .await?
                .has_data
        );
        assert_eq!(
            executor.get_oplog(&owned, OplogIndex::INITIAL).await?,
            history
        );
        claim.enabled.store(false, Ordering::SeqCst);
        executor
            .delete_worker_with_fingerprint(&owned, fingerprint)
            .await?;
    } else {
        result?;
        if pause_claim {
            assert_eq!(claim.started.load(Ordering::SeqCst), 1);
        }
    }
    assert!(executor.get_worker_metadata_opt(&owned).await?.is_none());
    assert!(
        !indexed
            .observe_agent("test", "completed_delete_race", &owned)
            .await?
            .has_data
    );
    assert!(!executor.worker_is_cached(&owned).await);
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
    sealed_cached_worker(last_unique_id, deps, host_api_tests, None, false).await
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn reacquisition_after_seal_does_not_retry_a_shard_refusal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    sealed_cached_worker(last_unique_id, deps, host_api_tests, None, true).await
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn postgres_seal_preserves_frozen_history_and_reacquires_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let postgres = DockerPostgresRdb::new(&uuid::Uuid::new_v4().to_string(), false).await;
    let url = url::Url::parse(&postgres.public_connection_string())?;
    let config = IndexedStoragePostgresConfig {
        postgres: DbPostgresConfig {
            host: url.host_str().unwrap().to_string(),
            port: url.port().unwrap(),
            database: url.path().trim_start_matches('/').to_string(),
            username: url.username().to_string(),
            password: url.password().unwrap().to_string(),
            max_connections: 10,
            schema: None,
            acquire_timeout: None,
        },
        drop_prefix_delete_batch_size: 1024,
        max_concurrent_ops: None,
    };
    sealed_cached_worker(last_unique_id, deps, host_api_tests, Some(config), false).await
}

async fn sealed_cached_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    postgres: Option<IndexedStoragePostgresConfig>,
    lose_shard: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new({
                let postgres = postgres.clone();
                move |config| {
                    if let Some(postgres) = &postgres {
                        config.indexed_storage = IndexedStorageConfig::Postgres(postgres.clone());
                    }
                    config.oplog.indexed_storage_layers = 1;
                    config.oplog.blob_storage_layers = 1;
                    config.oplog.entry_count_limit = 1_000_000;
                    config.oplog.shard_lease_max_ttl = Duration::from_secs(300);
                    config.oplog.skew_margin = Duration::from_secs(1);
                }
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
    let indexed: Arc<dyn IndexedStorage + Send + Sync> = match &postgres {
        Some(config) => Arc::new(
            PostgresIndexedStorage::configured(config)
                .await
                .map_err(anyhow::Error::msg)?,
        ),
        None => Arc::new(SqliteIndexedStorage::configured(&indexed_config).await?),
    };
    let resident = indexed.observe_agent("test", "before_seal", &owned).await?;
    assert!(resident.has_data);
    let history = executor.stored_oplog(&owned.agent_id).await;
    let oplog_service = old.oplog_service();
    let last = oplog_service
        .get_last_index(&owned, AgentMode::Durable)
        .await?;
    let activator = old.worker_activator();
    let selected = Arc::new(tokio::sync::Notify::new());
    let resolve = Arc::new(tokio::sync::Notify::new());
    let acquisitions = Arc::new(AtomicUsize::new(0));
    executor.set_existing_worker_acquisition_hook(Arc::new({
        let owned = owned.clone();
        let selected = selected.clone();
        let resolve = resolve.clone();
        let acquisitions = acquisitions.clone();
        move |id| {
            let pause = id == owned && acquisitions.fetch_add(1, Ordering::SeqCst) == 0;
            let selected = selected.clone();
            let resolve = resolve.clone();
            async move {
                if pause {
                    selected.notify_one();
                    resolve.notified().await;
                }
            }
            .boxed()
        }
    }));
    let acquisition = tokio::spawn({
        let executor = executor.clone();
        let id = owned.agent_id;
        async move { executor.attached_agent_status(&id).await }
    });
    selected.notified().await;
    assert!(Arc::ptr_eq(
        &old,
        &executor.active_agents().try_get(&owned).await.unwrap()
    ));
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
    let epochs: i64 = if let Some(config) = &postgres {
        let pool = sqlx::PgPool::connect_with(
            sqlx::postgres::PgConnectOptions::new()
                .host(&config.postgres.host)
                .port(config.postgres.port)
                .database(&config.postgres.database)
                .username(&config.postgres.username)
                .password(&config.postgres.password),
        )
        .await?;
        let epochs = sqlx::query_scalar(
            "SELECT COUNT(*) FROM indexed_key_epoch WHERE env = $1 AND agent = $2",
        )
        .bind(owned.environment_id.0)
        .bind(owned.agent_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        pool.close().await;
        epochs
    } else {
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
        epochs
    };
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
    shard.check_admission(&owned.agent_id)?;
    if lose_shard {
        shard.clear_assignment();
    }
    resolve.notify_one();
    let acquired = acquisition.await?;
    assert_eq!(acquisitions.load(Ordering::SeqCst), 2);
    if lose_shard {
        assert!(
            acquired
                .unwrap_err()
                .to_string()
                .contains("ShardingNotReady"),
            "the genuine shard refusal must propagate without reacquisition"
        );
        assert!(
            !indexed
                .observe_agent("test", "refused_unseal", &owned)
                .await?
                .has_data
        );
        executor.shutdown_and_wait_for_invocation_loops().await?;
        return Ok(());
    }
    let acquired = acquired?;
    assert_eq!(acquired.status, AgentStatus::Idle);
    assert_eq!(acquired.oplog_idx, last);
    assert!(!Arc::ptr_eq(
        &old,
        &executor.active_agents().try_get(&owned).await.unwrap()
    ));
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
