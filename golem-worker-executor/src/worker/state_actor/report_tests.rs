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

use super::*;
use crate::services::golem_config::*;
use crate::services::oplog::OplogOps;
use crate::services::oplog::tests::{
    ReadCountingBlobStorage, ReadCountingIndexedStorage, create_test_entry,
    default_execution_status, default_last_known_status, make_agent_metadata,
};
use crate::services::oplog::{OplogService, PrimaryOplogService};
use crate::storage::indexed::{
    IndexedStorageError, IndexedStorageLabelledApi, IndexedStorageNamespace,
};
use crate::storage::keyvalue::{KeyValueStorageLabelledApi, KeyValueStorageNamespace};
use crate::test_callers::TEST_ISSUER;
use crate::worker::status::calculate_last_known_status_for_existing_worker;
use crate::worker::{GetOrCreateWorkerResult, ResolvedOwnerContext};
use crate::workerctx::default::Context;
use futures::StreamExt;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent::{AgentMode, AgentTypeName, ParsedAgentKey, Snapshotting};
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::card::{InvocationWalletPin, WalletVersionToken};
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::diff::Hash;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::OplogPayload;
use golem_common::model::{
    AgentId, AgentInvocationPayload, AgentInvocationResult, RetryConfig, SnapshotFiles,
};
use golem_common::schema::agent::{
    AgentConstructorSchema, AgentTypeKind, AgentTypeSchema, InputSchema,
};
use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
use golem_service_base::config::{BlobStorageConfig, InMemoryBlobStorageConfig};
use golem_service_base::model::component::Component;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use std::collections::HashMap;
use std::time::Duration;
use test_r::{test, timeout};

type SharedKeyValueStorage = Arc<dyn crate::storage::keyvalue::KeyValueStorage + Send + Sync>;

struct ReportBootstrap(
    Arc<dyn OplogService>,
    std::sync::OnceLock<SharedKeyValueStorage>,
);

#[async_trait::async_trait]
impl crate::Bootstrap<Context> for ReportBootstrap {
    fn wrap_key_value_storage(&self, storage: SharedKeyValueStorage) -> SharedKeyValueStorage {
        self.1.get_or_init(|| storage).clone()
    }

    fn create_additional_deps(
        &self,
        _: Arc<dyn golem_service_base::clients::registry::RegistryService>,
    ) -> crate::services::NoAdditionalDeps {
        crate::services::NoAdditionalDeps
    }

    fn wrap_oplog_service(&self, _: Arc<dyn OplogService>) -> Arc<dyn OplogService> {
        self.0.clone()
    }
}

#[test]
#[timeout("60s")]
async fn failed_primary_reports_detach_before_same_key_admission_and_keep_storage_receipts() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let indexed = Arc::new(ReadCountingIndexedStorage::new());
    let blob = Arc::new(ReadCountingBlobStorage::new());
    let service = Arc::new(
        PrimaryOplogService::new(
            indexed.clone(),
            blob.clone(),
            0,
            0,
            1024,
            0,
            RetryConfig {
                max_attempts: 2,
                min_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                ..Default::default()
            },
        )
        .await,
    );
    let mut config = GolemConfig {
        key_value_storage: KeyValueStorageConfig::InMemory(KeyValueStorageInMemoryConfig {}),
        indexed_storage: IndexedStorageConfig::InMemory(IndexedStorageInMemoryConfig {}),
        scheduler_storage: SchedulerStorageConfig::InMemory(SchedulerStorageInMemoryConfig {}),
        blob_storage: BlobStorageConfig::InMemory(InMemoryBlobStorageConfig {}),
        resource_usage_metering: ResourceUsageMeteringConfig::default(),
        resource_limits: ResourceLimitsConfig::Disabled(ResourceLimitsDisabledConfig {}),
        ..Default::default()
    };
    config.filesystem_storage.mode = FilesystemStorageMode::Temporary;
    config.engine.enable_fs_cache = false;
    config.oplog.sweep.enabled = false;
    config.set_golem_issuer(TEST_ISSUER.parse().unwrap());
    let shutdown = crate::services::shutdown::Shutdown::new();
    let mut tasks = tokio::task::JoinSet::new();
    let bootstrap = ReportBootstrap(service.clone(), Default::default());
    let (deps, _epoch, _) = crate::create_worker_executor_impl::<Context, _>(
        config,
        &bootstrap,
        tokio::runtime::Handle::current(),
        &Arc::new(crate::services::worker_activator::LazyWorkerActivator::new()),
        shutdown.clone(),
        &mut tasks,
    )
    .await
    .unwrap();
    let key_value_storage = bootstrap.1.get().unwrap().clone();
    crate::services::HasShardService::shard_service(&deps).install_unexpiring(
        1,
        &HashMap::from([(
            golem_common::model::ShardId::new(0),
            golem_common::model::ShardEpoch(0),
        )]),
    );

    // Distinct owners keep the queued-only report independent of the explicit failure.
    futures::stream::iter([false, true])
        .for_each(|queued_only| {
            let deps = deps.clone();
            let service = service.clone();
            let indexed = indexed.clone();
            let blob = blob.clone();
            let key_value_storage = key_value_storage.clone();
            async move {
                let id = OwnedAgentId::new(
                    EnvironmentId::new(),
                    &AgentId::from_parts(7, if queued_only { 2 } else { 1 }),
                );
                let account = AccountId::new();
                let metadata = make_agent_metadata(id.agent_id, account, id.environment_id);
                let fingerprint = metadata.fingerprint;
                let oplog = service
                    .create_fresh(
                        &mut service.lock_lifecycle(&id.agent_id).await,
                        &id,
                        AgentMode::Durable,
                        create_test_entry(
                            id.agent_id,
                            AgentMode::Durable,
                            ComponentRevision::INITIAL,
                            id.environment_id,
                            account,
                            fingerprint.0,
                        ),
                        metadata,
                        default_last_known_status(),
                        default_execution_status(AgentMode::Durable),
                        None,
                    )
                    .await
                    .unwrap();
                let initial = calculate_last_known_status_for_existing_worker(
                    &deps,
                    &id,
                    AgentMode::Durable,
                    None,
                )
                .await
                .unwrap();
                let result_generation = initial.invocation_results.change_generation();
                let revert_generation = initial.invocation_results.revert_generation();
                let current = Arc::new(ArcSwap::from_pointee(initial.clone()));
                let detached = Arc::new(AtomicBool::new(false));
                let lifecycle = Arc::new(Mutex::new(WorkerLifecycle::unresolved()));
                let published_authority_generation = Arc::new(AtomicU64::new(0));
                let flusher = AgentStatusFlusher::new(
                    id.clone(),
                    fingerprint,
                    false,
                    true,
                    deps.worker_service(),
                    deps.active_agents().status_flush_queue(),
                    Some(initial),
                    current.clone(),
                    detached.clone(),
                    tokio::sync::watch::channel(None).1,
                );
                let actor = WorkerStateActor::new(
                    deps.clone(),
                    id.clone(),
                    fingerprint,
                    AgentMode::Durable,
                    account,
                    oplog.clone(),
                    current,
                    detached.clone(),
                    Arc::new(WorkerStatusMetric::new(AgentStatus::Idle)),
                    flusher.clone(),
                    published_authority_generation.clone(),
                    lifecycle.clone(),
                );
                let key = InvocationId(uuid::Uuid::new_v4());
                let (operation, payload) = oplog
                    .upload_request(AgentInvocationPayload::AgentMethod {
                        method_name: "test".into(),
                        allow_streams: false,
                        input: SchemaValue::Tuple { elements: vec![] }.try_into().unwrap(),
                        caller: golem_common::model::caller::Caller::anonymous(),
                        scope_card: None,
                    })
                    .await
                    .unwrap();
                let context = InvocationContextStack::fresh_rounded();
                let entry = OplogEntry::pending_agent_invocation(
                    None,
                    key,
                    Box::new(operation),
                    payload,
                    context.trace_id.clone(),
                    context.trace_states.clone(),
                    context.to_oplog_data(),
                );
                let append = || async {
                    actor
                        .append_invocation_if_version(
                            entry.clone(),
                            None,
                            key,
                            result_generation,
                            revert_generation,
                            lifecycle.clone().lock_owned().await,
                        )
                        .await
                };
                indexed.reset();
                indexed.set_read_error(Some(IndexedStorageError::Transient(
                    "report read unavailable".into(),
                )));
                if queued_only {
                    oplog.add(entry.clone()).await.unwrap();
                } else {
                    let Ok(InvocationAppendOutcome::Appended(StoredAppend::Unreported(unreadable))) =
                        append().await
                    else {
                        panic!("expected the actor to report a stored invocation");
                    };
                    assert_eq!(unreadable.agent_id, id.agent_id);
                    assert!(unreadable.is_transient());
                }
                // The listener enqueues the report before add completes. This FIFO read drains it.
                assert!(actor.status().await.pending_invocations.is_empty());
                assert!(indexed.reads() >= if queued_only { 2 } else { 4 });
                assert!(
                    detached.load(Ordering::Acquire),
                    "a failed report must detach its stale projection"
                );
                flusher.flush(FlushReason::Forced).await.unwrap();
                // A storage receipt succeeds although its full transaction cannot collect a report.
                let (receipt, stored) = oneshot::channel();
                let (done, transaction) = oneshot::channel();
                actor
                    .commit
                    .status_jobs
                    .send(StatusJob::CommitAndUpdateState {
                        level: CommitLevel::Always,
                        committed: Some(CommitReceipt::Refusable(receipt)),
                        done,
                    })
                    .unwrap_or_else(|_| panic!("status actor stopped"));
                assert!(matches!(
                    transaction.await.unwrap(),
                    Err(OplogRefusal::Unreadable(_))
                ));
                assert_eq!(stored.await.unwrap(), Ok(()));
                assert!(detached.load(Ordering::Acquire));

                let before_failed_repair = oplog.current_oplog_index().await;
                assert!(matches!(
                    append().await,
                    Err(WorkerExecutorError::RecoveryRequired { details, .. })
                        if details.contains("report read unavailable")
                ));
                assert_eq!(oplog.current_oplog_index().await, before_failed_repair);

                indexed.set_read_error(None);
                let namespace = IndexedStorageNamespace::OpLog {
                    owned_agent_id: id.clone(),
                    agent_mode: AgentMode::Durable,
                };
                let stored_pending = || async {
                    indexed
                        .with_entity("test", "report", "entry")
                        .read::<OplogEntry>(namespace.clone(), &id.storage_key(), 1, 100)
                        .await
                        .unwrap()
                        .into_iter()
                        .filter(|(_, entry)| {
                            matches!(entry, OplogEntry::PendingAgentInvocation { .. })
                        })
                        .count()
                };
                assert_eq!(stored_pending().await, 1);
                let authority_before_repair =
                    published_authority_generation.load(Ordering::Acquire);
                assert!(matches!(append().await, Ok(InvocationAppendOutcome::Retry)));
                assert_eq!(stored_pending().await, 1);
                assert_eq!(
                    actor
                        .repaired_status()
                        .await
                        .unwrap()
                        .pending_invocations
                        .len(),
                    1
                );
                assert!(!detached.load(Ordering::Acquire));
                let authority_after_repair = published_authority_generation.load(Ordering::Acquire);
                assert!(authority_after_repair > authority_before_repair);
                let repaired_index = actor.status().await.oplog_idx;
                assert_eq!(
                    actor
                        .commit_and_update_state(CommitLevel::Always)
                        .await
                        .unwrap(),
                    repaired_index
                );
                assert_eq!(
                    published_authority_generation.load(Ordering::Acquire),
                    authority_after_repair,
                    "an attached empty commit must not invalidate authority"
                );

                use golem_common::model::durable_stream::{
                    LocalStreamId, SessionStreamRole, StreamCancellationRequest,
                    StreamConsumerCancelAppliedRecord, StreamConsumerCancelIntentRecord,
                    StreamEndpointSide, StreamError, StreamRecordReference,
                    StreamRegistrationInvocation, StreamSessionRecord, StreamTerminal,
                    StreamTerminalAuthor,
                };
                let intent = StreamConsumerCancelIntentRecord {
                    format_version: 1,
                    session_key: StreamRegistrationInvocation::Remote(
                        crate::durable_host::durable_stream::tests::identity().invocation,
                    ),
                    consumer_invocation: key,
                    source: StreamRecordReference::Local(LocalStreamId(OplogIndex::INITIAL)),
                    epoch: 7,
                    cancellation: StreamCancellationRequest {
                        direction: SessionStreamRole::Output,
                        side: StreamEndpointSide::Consumer,
                        authored_by: StreamTerminalAuthor::Host,
                        terminal: StreamTerminal::Failed(StreamError::Interrupted(
                            "stream cancelled".into(),
                        )),
                    },
                };
                // The persisted status cache, as stored fields, after a forced flush.
                let cached = async || {
                    flusher.flush(FlushReason::Forced).await.unwrap();
                    key_value_storage
                        .with_entity("test", "report", "agent_status")
                        .get_all_raw(KeyValueStorageNamespace::AgentStatus {
                            owned_agent_id: id.clone(),
                            fingerprint,
                        })
                        .await
                        .unwrap()
                        .into_iter()
                        .collect::<BTreeMap<_, _>>()
                };
                let retry_report = async |record| {
                    let before_report_cache = cached().await;
                    assert!(!before_report_cache.is_empty());
                    indexed.set_read_error(Some(IndexedStorageError::Transient(
                        "cancellation report unavailable".into(),
                    )));
                    let index = oplog
                        .add(OplogEntry::StreamSession {
                            timestamp: Timestamp::now_utc(),
                            entity_parent_start_index: None,
                            record: OplogPayload::Inline(Box::new(record)),
                            summary: None,
                        })
                        .await
                        .unwrap();
                    assert!(actor.status().await.oplog_idx < index);
                    assert!(detached.load(Ordering::Acquire));

                    indexed.set_read_error(None);
                    let detached_cache = cached().await;
                    assert_eq!(
                        detached_cache, before_report_cache,
                        "a detached projection must not reach the status cache"
                    );
                    let entries = indexed
                        .with_entity("test", "report", "entry")
                        .read::<OplogEntry>(
                            namespace.clone(),
                            &id.storage_key(),
                            index.as_u64(),
                            index.as_u64(),
                        )
                        .await
                        .unwrap();
                    assert!(matches!(
                        entries.as_slice(),
                        [(_, OplogEntry::StreamSession {
                            record: OplogPayload::SerializedInline { cached: None, .. }, ..
                        })]
                    ));
                    let status = actor.repaired_status().await.unwrap();
                    assert_eq!(status.oplog_idx, index);
                    assert!(!detached.load(Ordering::Acquire));
                    let reads_after_repair = indexed.reads();
                    assert!(oplog.commit(CommitLevel::ReportOnly).await.unwrap().is_empty());
                    assert_eq!(indexed.reads(), reads_after_repair);
                    assert_eq!(oplog.current_oplog_index().await, index);
                    assert_ne!(
                        cached().await,
                        detached_cache,
                        "the reattached projection must reach the status cache"
                    );
                    status
                };
                let status = retry_report(StreamSessionRecord::ConsumerCancelIntent(intent.clone()))
                    .await;
                assert_eq!(
                    status.pending_durable_stream_cancellations,
                    HashMap::from([((intent.source.clone(), intent.epoch), vec![intent.clone()])])
                );
                let status = retry_report(StreamSessionRecord::ConsumerCancelApplied(
                    StreamConsumerCancelAppliedRecord { format_version: 1, intent },
                ))
                .await;
                assert!(status.pending_durable_stream_cancellations.is_empty());

                let (previous, own) = (
                    FilesystemSnapshotName::periodic(),
                    FilesystemSnapshotName::periodic(),
                );
                let snapshot = |name: &FilesystemSnapshotName| OplogEntry::Snapshot {
                    timestamp: Timestamp::now_utc(),
                    data: OplogPayload::Inline(Box::new(vec![])),
                    mime_type: "application/octet-stream".to_string(),
                    active_cards: Vec::new(),
                    wallet_generation: 0,
                    filesystem_snapshot: Some(name.clone()),
                    bootstrap: None,
                };
                oplog.add(snapshot(&previous)).await.unwrap();
                assert_eq!(
                    actor
                        .append_confirmation(
                            previous.clone(),
                            lifecycle.clone().lock_owned().await,
                            Box::new(|_| {}),
                        )
                        .await,
                    ConfirmOutcome::Confirmed
                );
                let snapshot_index = oplog.add(snapshot(&own)).await.unwrap();
                assert_eq!(
                    actor
                        .append_confirmation(
                            previous,
                            lifecycle.clone().lock_owned().await,
                            Box::new(|_| {}),
                        )
                        .await,
                    ConfirmOutcome::Superseded
                );
                assert_eq!(oplog.current_oplog_index().await, snapshot_index);
                assert_eq!(
                    actor
                        .append_confirmation(
                            own.clone(),
                            lifecycle.clone().lock_owned().await,
                            Box::new(|_| {}),
                        )
                        .await,
                    ConfirmOutcome::Confirmed
                );
                assert_eq!(
                    actor
                        .status()
                        .await
                        .last_automatic_snapshot
                        .as_ref()
                        .unwrap()
                        .files,
                    SnapshotFiles::Confirmed(own)
                );

                use golem_common::model::oplog::RawOplogPayload;
                use golem_common::model::oplog::host_functions::HostFunctionName;
                let RawOplogPayload::External { payload_id, md5_hash } =
                    oplog.upload_raw_payload(vec![255; 2048]).await.unwrap()
                else {
                    panic!("expected the large test payload to be external");
                };
                indexed.set_read_error(Some(IndexedStorageError::Transient(
                    "report read unavailable".into(),
                )));
                let lookup_index = oplog
                    .add(OplogEntry::Start {
                        timestamp: Timestamp::now_utc(),
                        parent_start_index: None,
                        function_name: HostFunctionName::GolemNetworkDnsResolve,
                        invocation_id: None,
                        observational_owner: None,
                        driver: None,
                        request: Some(OplogPayload::External { payload_id, md5_hash, cached: None }),
                        durable_function_type: golem_common::model::oplog::DurableFunctionType::ReadRemote,
                        span_started: None,
                        denial: None,
                    })
                    .await
                    .unwrap();
                assert!(actor.status().await.oplog_idx < lookup_index);
                indexed.set_read_error(None);

                let (fold_read_started, release_fold_read) = blob.pause_next_read();
                let (receipt, mut stored) = oneshot::channel();
                let (done, mut transaction) = oneshot::channel();
                actor
                    .commit
                    .status_jobs
                    .send(StatusJob::CommitAndUpdateState {
                        level: CommitLevel::Always,
                        committed: Some(CommitReceipt::Refusable(receipt)),
                        done,
                    })
                    .unwrap_or_else(|_| panic!("status actor stopped"));
                fold_read_started.await.unwrap();
                let receipt_during_fold = stored.try_recv();
                let transaction_during_fold = transaction.try_recv();
                release_fold_read.send(()).unwrap();
                assert_eq!(receipt_during_fold, Ok(Ok(())));
                assert!(matches!(
                    transaction_during_fold,
                    Err(oneshot::error::TryRecvError::Empty)
                ));
                let Err(OplogRefusal::Unreadable(error)) = transaction.await.unwrap() else {
                    panic!("expected the fold to refuse the malformed external payload");
                };
                assert_eq!(error.agent_id, id.agent_id);
                assert!(
                    error.to_string().ends_with(&format!(
                        "failed to load payload at oplog index {lookup_index}: unknown serialization version: 255"
                    )),
                    "{error}"
                );
                assert_eq!(oplog.current_oplog_index().await, lookup_index);
                actor.stop_handle().stop_and_wait().await.unwrap();
            }
        })
        .await;
    shutdown
        .cancel_and_wait_for_tracked(Duration::from_secs(5))
        .await;
}

#[test]
#[timeout("60s")]
async fn old_invocation_result_index_preserves_authoritative_read_errors() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let indexed = Arc::new(ReadCountingIndexedStorage::new());
    let service = Arc::new(
        PrimaryOplogService::new(
            indexed.clone(),
            Arc::new(InMemoryBlobStorage::new()),
            0,
            0,
            1024,
            0,
            RetryConfig {
                max_attempts: 2,
                min_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                ..Default::default()
            },
        )
        .await,
    );
    let mut config = GolemConfig {
        key_value_storage: KeyValueStorageConfig::InMemory(KeyValueStorageInMemoryConfig {}),
        indexed_storage: IndexedStorageConfig::InMemory(IndexedStorageInMemoryConfig {}),
        scheduler_storage: SchedulerStorageConfig::InMemory(SchedulerStorageInMemoryConfig {}),
        blob_storage: BlobStorageConfig::InMemory(InMemoryBlobStorageConfig {}),
        resource_limits: ResourceLimitsConfig::Disabled(ResourceLimitsDisabledConfig {}),
        ..Default::default()
    };
    config.filesystem_storage.mode = FilesystemStorageMode::Temporary;
    config.engine.enable_fs_cache = false;
    config.oplog.sweep.enabled = false;
    config.agent_status_flush.enabled = false;
    config.agent_status_checkpoint.enabled = false;
    config.invocation_results.recent_capacity = 1;
    config.invocation_results.bloom_bits = 64;
    config.invocation_results.hydrated_cache_capacity = 0;
    config.set_golem_issuer(TEST_ISSUER.parse().unwrap());
    let shutdown = crate::services::shutdown::Shutdown::new();
    let mut tasks = tokio::task::JoinSet::new();
    let (deps, _epoch, _) = crate::create_worker_executor_impl::<Context, _>(
        config,
        &ReportBootstrap(service.clone(), Default::default()),
        tokio::runtime::Handle::current(),
        &Arc::new(crate::services::worker_activator::LazyWorkerActivator::new()),
        shutdown.clone(),
        &mut tasks,
    )
    .await
    .unwrap();
    deps.shard_service().install_unexpiring(
        1,
        &HashMap::from([(
            golem_common::model::ShardId::new(0),
            golem_common::model::ShardEpoch(0),
        )]),
    );

    let id = OwnedAgentId::new(EnvironmentId::new(), &AgentId::from_parts(7, 42));
    let account = AccountId::new();
    let parsed = ParsedAgentKey::new(
        AgentTypeName("Counter".into()),
        TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(Vec::new())),
            SchemaValue::Record { fields: Vec::new() },
        ),
    );
    let mut metadata = make_agent_metadata(id.agent_id, account, id.environment_id);
    metadata.agent_type_name = Some(parsed.agent_type.clone());
    metadata.constructor_parameters = Some(parsed.canonical_parameters().unwrap());
    let fingerprint = metadata.fingerprint;
    let mut create = create_test_entry(
        id.agent_id,
        AgentMode::Durable,
        ComponentRevision::INITIAL,
        id.environment_id,
        account,
        fingerprint.0,
    );
    let OplogEntry::Create { parameters, .. } = &mut create else {
        unreachable!()
    };
    parameters.agent_type_name = metadata.agent_type_name.clone();
    parameters.constructor_parameters = metadata.constructor_parameters.clone();
    let oplog = service
        .create_fresh(
            &mut service.lock_lifecycle(&id.agent_id).await,
            &id,
            AgentMode::Durable,
            create,
            metadata.clone(),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await
        .unwrap();
    let keys: [InvocationId; 4] = std::array::from_fn(|_| InvocationId(uuid::Uuid::new_v4()));
    futures::stream::iter(keys.into_iter().zip([42, 19, 73, 5]))
        .for_each(|(invocation_id, output)| {
            let oplog = oplog.clone();
            async move {
                let (operation, payload) = oplog
                    .upload_request(AgentInvocationPayload::AgentMethod {
                        method_name: "count".into(),
                        allow_streams: false,
                        input: SchemaValue::Tuple { elements: vec![] }.try_into().unwrap(),
                        caller: golem_common::model::caller::Caller::anonymous(),
                        scope_card: None,
                    })
                    .await
                    .unwrap();
                let context = InvocationContextStack::fresh_rounded();
                oplog
                    .add(OplogEntry::pending_agent_invocation(
                        None,
                        invocation_id,
                        Box::new(operation.clone()),
                        payload.clone(),
                        context.trace_id.clone(),
                        context.trace_states.clone(),
                        context.to_oplog_data(),
                    ))
                    .await
                    .unwrap();
                oplog
                    .add(OplogEntry::AgentInvocationStarted {
                        timestamp: Timestamp::now_utc(),
                        invocation_id,
                        operation: Box::new(operation),
                        payload,
                        trace_id: context.trace_id.clone(),
                        trace_states: context.trace_states.clone(),
                        invocation_context: context.to_oplog_data(),
                        wallet_pin: Box::new(InvocationWalletPin {
                            wallet_token: WalletVersionToken {
                                wallet_id_hash: [0; 32],
                                generation: 0,
                            },
                            pinned_card_ids: Vec::new(),
                            scope_card_id: None,
                        }),
                    })
                    .await
                    .unwrap();
                oplog
                    .add(OplogEntry::AgentInvocationFinished {
                        timestamp: Timestamp::now_utc(),
                        invocation_id,
                        result: OplogPayload::Inline(Box::new(
                            AgentInvocationResult::AgentMethod {
                                output: SchemaValue::U32(output),
                            },
                        )),
                        method_name: Some("count".into()),
                        consumed_fuel: 17,
                        component_revision: ComponentRevision::INITIAL,
                        awaits_settlement: false,
                    })
                    .await
                    .unwrap();
            }
        })
        .await;
    oplog.commit(CommitLevel::Always).await.unwrap();
    let status =
        calculate_last_known_status_for_existing_worker(&deps, &id, AgentMode::Durable, None)
            .await
            .unwrap();
    let old_key = keys[0];
    assert!(!status.invocation_results.is_exact_complete());
    assert!(status.invocation_results.position(&old_key).is_none());
    assert!(status.invocation_results.might_contain(&old_key));
    assert!(status.pending_invocations.is_empty());
    let component = Component {
        id: metadata.component_id,
        revision: ComponentRevision::INITIAL,
        environment_id: id.environment_id,
        component_name: ComponentName("counter".into()),
        hash: Hash::empty(),
        application_id: ApplicationId::new(),
        account_id: account,
        account_email: AccountEmail::new("owner@example.com"),
        application_name: ApplicationName("test".into()),
        environment_name: EnvironmentName::try_from("test").unwrap(),
        component_size: 0,
        metadata: ComponentMetadata::from_parts(
            Default::default(),
            Vec::new(),
            vec![AgentTypeSchema {
                type_name: parsed.agent_type.clone(),
                kind: AgentTypeKind::Regular,
                description: String::new(),
                source_language: String::new(),
                schema: SchemaGraph::empty(),
                constructor: AgentConstructorSchema {
                    name: None,
                    description: String::new(),
                    prompt_hint: None,
                    input_schema: InputSchema::parameters([]),
                },
                methods: Vec::new(),
                dependencies: Vec::new(),
                mode: AgentMode::Durable,
                http_mount: None,
                snapshotting: Snapshotting::Disabled(golem_schema::Empty {}),
                config: Vec::new(),
            }],
            BTreeMap::new(),
        ),
        created_at: chrono::Utc::now(),
        wasm_hash: Hash::empty(),
        package_metadata: None,
        object_store_key: String::new(),
    };
    metadata.last_known_status = status.clone();
    let initial = GetOrCreateWorkerResult {
        initial_worker_metadata: metadata,
        current_status: Arc::new(ArcSwap::from_pointee(status.clone())),
        persisted_status: None,
        execution_status: Arc::new(std::sync::RwLock::new(
            crate::model::ExecutionStatus::Suspended {
                agent_mode: AgentMode::Durable,
                timestamp: Timestamp::now_utc(),
            },
        )),
        owner_context: ResolvedOwnerContext::Agent(Box::new(parsed)),
        snapshot_policy: SnapshotPolicy::Disabled,
        oplog: oplog.clone(),
        initial_component: Arc::new(component),
        reconstructed_ephemeral: false,
        newly_created: false,
    };
    let worker = Worker::<Context>::unresolved(
        &deps,
        Arc::new(crate::services::card_interest::CardInterestIndex::new()),
        id.clone(),
    );
    let constructing = worker.clone();
    let (_, initialized) = worker
        .initialize_with(
            async move {
                constructing
                    .finish_construction(std::time::Instant::now(), Ok(initial))
                    .await
            }
            .boxed(),
        )
        .await;
    initialized.unwrap();
    assert!(matches!(
        deps.worker_service()
            .lookup_invocation_result_index(&id, fingerprint, &status, &old_key)
            .await
            .unwrap(),
        crate::services::worker::InvocationResultIndexLookup::Incomplete
    ));
    let before_lookup = oplog.current_oplog_index().await;

    indexed.set_read_error(Some(IndexedStorageError::Other(
        "unreadable recorded invocation".into(),
    )));
    let result = worker.lookup_invocation_result(&old_key).await;
    let Err(WorkerExecutorError::OplogFailed {
        agent_id, details, ..
    }) = result
    else {
        panic!("expected permanent authoritative failure, got {result:?}");
    };
    assert_eq!(agent_id, id.agent_id);
    assert!(
        details.contains("unreadable recorded invocation"),
        "{details}"
    );

    indexed.set_read_error(Some(IndexedStorageError::Transient(
        "temporary primary read failure".into(),
    )));
    let result = worker.try_lookup_invocation_result(&old_key).await;
    assert!(
        matches!(&result, Err(WorkerExecutorError::RecoveryRequired { details, .. }) if details.contains("temporary primary read failure")),
        "{result:?}"
    );

    deps.shard_service().install_unexpiring(1, &HashMap::new());
    let result = worker.try_lookup_invocation_result(&old_key).await;
    assert!(
        matches!(result, Err(WorkerExecutorError::InvalidShardId { .. })),
        "{result:?}"
    );

    indexed.set_read_error(None);
    let result = worker.try_lookup_invocation_result(&old_key).await;
    let Ok(crate::model::LookupResult::Complete(Ok(output))) = result else {
        panic!("expected the old recorded result, got {result:?}");
    };
    assert_eq!(
        output.result,
        AgentInvocationResult::AgentMethod {
            output: SchemaValue::U32(42),
        }
    );
    assert_eq!(output.oplog_index, Some(OplogIndex::from_u64(4)));
    assert_eq!(oplog.current_oplog_index().await, before_lookup);
    worker
        .state_actor
        .stop_handle()
        .stop_and_wait()
        .await
        .unwrap();
    worker.status_flusher.begin_delete().await;
    oplog.stop_and_wait().await.unwrap();
    shutdown
        .cancel_and_wait_for_tracked(Duration::from_secs(5))
        .await;
}
