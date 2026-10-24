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
use crate::model::ExecutionStatus;
use crate::services::oplog::Oplog;
use crate::services::oplog::UnreadableOplog;
use crate::services::shard::ShardServiceDefault;
use crate::storage::keyvalue::KeyValueStorageError;
use crate::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::model::Timestamp;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::card::{
    Card, CardId, InvocationWalletPin, StoredCard, WalletVersionToken,
};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::TraceId;
use golem_common::model::oplog::{OplogPayload, PayloadId, RawOplogPayload};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{
    AgentInvocationPayload, AgentInvocationResult, AgentMetadata, PendingInvocationRef,
    PendingUpdateKind, PendingUpdateRef, ScanCursor, ShardEpoch,
};
use golem_common::read_only_lock;
use golem_service_base::model::component::Component;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use test_r::test;
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Debug)]
struct IndexTestOplogService {
    entries: BTreeMap<OplogIndex, OplogEntry>,
    stream_index: std::sync::OnceLock<Arc<StreamSessionIndexService>>,
    reads: StdMutex<Vec<(OplogIndex, u64)>>,
    delete_attempts: AtomicUsize,
    pause_next_read: AtomicBool,
    read_started: Notify,
    resume_read: Notify,
}

impl IndexTestOplogService {
    fn new(entries: BTreeMap<OplogIndex, OplogEntry>) -> Self {
        Self {
            entries,
            stream_index: std::sync::OnceLock::new(),
            reads: StdMutex::new(Vec::new()),
            delete_attempts: AtomicUsize::new(0),
            pause_next_read: AtomicBool::new(false),
            read_started: Notify::new(),
            resume_read: Notify::new(),
        }
    }

    fn pause_next_read(&self) {
        self.pause_next_read.store(true, Ordering::Release);
    }

    fn read_starts(&self) -> Vec<OplogIndex> {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .map(|(start, _)| *start)
            .collect()
    }

    fn clear_reads(&self) {
        self.reads.lock().unwrap().clear();
    }
}

#[async_trait]
impl OplogService for IndexTestOplogService {
    async fn staged_exists(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _stage_id: uuid::Uuid,
    ) -> Result<bool, String> {
        unimplemented!()
    }

    async fn lock_lifecycle(&self, _: &AgentId) -> OplogLifecycleGuard {
        unreachable!()
    }

    fn set_stream_session_index(&self, index: Arc<StreamSessionIndexService>) {
        self.stream_index.set(index).unwrap();
    }

    fn stream_session_index(&self) -> Option<Arc<StreamSessionIndexService>> {
        self.stream_index.get().cloned()
    }

    async fn create(
        &self,
        _lifecycle: &mut OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _initial_entry: OplogEntry,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        _shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn crate::services::oplog::Oplog>, WorkerExecutorError> {
        unreachable!()
    }

    async fn create_fresh(
        &self,
        _lifecycle: &mut OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _initial_entry: OplogEntry,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        _shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn crate::services::oplog::Oplog>, WorkerExecutorError> {
        unreachable!()
    }

    async fn open(
        &self,
        _lifecycle: &mut OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _last_oplog_index: Option<OplogIndex>,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        _shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn crate::services::oplog::Oplog>, WorkerExecutorError> {
        unreachable!()
    }

    async fn get_last_index(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
    ) -> Result<OplogIndex, UnreadableOplog> {
        Ok(self
            .entries
            .keys()
            .next_back()
            .copied()
            .unwrap_or(OplogIndex::NONE))
    }

    async fn assert_owning_epoch(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _expected_epoch: golem_common::model::ShardEpoch,
    ) -> Result<(), crate::services::oplog::OplogError> {
        Ok(())
    }

    async fn delete(
        &self,
        _lifecycle: &mut OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _expected_epoch: Option<golem_common::model::ShardEpoch>,
    ) -> Result<(), crate::services::oplog::OplogError> {
        self.delete_attempts.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn read_exact(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.reads.lock().unwrap().push((idx, n));
        if self.pause_next_read.swap(false, Ordering::AcqRel) {
            self.read_started.notify_one();
            self.resume_read.notified().await;
        }
        let end = idx.as_u64().saturating_add(n.saturating_sub(1));
        Ok(self
            .entries
            .range(idx..=OplogIndex::from_u64(end))
            .map(|(index, entry)| (*index, entry.clone()))
            .collect())
    }

    async fn exists(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
    ) -> Result<bool, UnreadableOplog> {
        Ok(true)
    }

    async fn scan_for_component(
        &self,
        _environment_id: &EnvironmentId,
        _component_id: &ComponentId,
        _modes: Option<AgentMode>,
        _cursor: ScanCursor,
        _count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        unreachable!()
    }

    async fn upload_raw_payload(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        unreachable!()
    }

    async fn download_raw_payload(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _payload_id: PayloadId,
        _md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        unreachable!()
    }
}

pub(super) struct IndexTestComponentService;

fn non_agent_component(component_id: ComponentId, environment_id: EnvironmentId) -> Component {
    Component {
        id: component_id,
        revision: ComponentRevision::INITIAL,
        environment_id,
        component_name: golem_common::model::component::ComponentName("test-component".to_string()),
        hash: golem_common::model::diff::Hash::empty(),
        application_id: ApplicationId::new(),
        account_id: AccountId::new(),
        account_email: golem_common::model::account::AccountEmail::new("test@golem"),
        application_name: golem_common::model::application::ApplicationName::try_from(
            "test-app".to_string(),
        )
        .unwrap(),
        environment_name: golem_common::model::environment::EnvironmentName::try_from("test-env")
            .unwrap(),
        component_size: 1,
        metadata: golem_common::model::component_metadata::ComponentMetadata::default(),
        created_at: chrono::Utc::now(),
        wasm_hash: golem_common::model::diff::Hash::empty(),
        object_store_key: "test-object".to_string(),
    }
}

#[async_trait]
impl ComponentService for IndexTestComponentService {
    async fn get(
        &self,
        _engine: &wasmtime::Engine,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
        unreachable!()
    }

    async fn get_metadata(
        &self,
        component_id: ComponentId,
        _forced_revision: Option<ComponentRevision>,
    ) -> Result<Component, WorkerExecutorError> {
        Ok(non_agent_component(component_id, EnvironmentId::new()))
    }

    async fn resolve_component(
        &self,
        _component_reference: String,
        _resolving_environment: EnvironmentId,
        _resolving_application: ApplicationId,
        _resolving_account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        unreachable!()
    }

    async fn all_cached_metadata(&self) -> Vec<Component> {
        Vec::new()
    }

    async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {}
}

fn invocation_pair(
    entries: &mut BTreeMap<OplogIndex, OplogEntry>,
    started_at: u64,
    key: &IdempotencyKey,
) {
    entries.insert(
        OplogIndex::from_u64(started_at),
        OplogEntry::AgentInvocationStarted {
            timestamp: Timestamp::now_utc(),
            idempotency_key: key.clone(),
            payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::ManualUpdate {
                target_revision: ComponentRevision::INITIAL,
            })),
            trace_id: TraceId::generate(),
            trace_states: Vec::new(),
            invocation_context: Vec::new(),
            wallet_pin: Box::new(InvocationWalletPin {
                wallet_token: WalletVersionToken {
                    wallet_id_hash: [0; 32],
                    generation: 0,
                },
                pinned_card_ids: Vec::new(),
                scope_card_id: None,
            }),
        },
    );
    entries.insert(
        OplogIndex::from_u64(started_at + 1),
        OplogEntry::AgentInvocationFinished {
            timestamp: Timestamp::now_utc(),
            result: OplogPayload::Inline(Box::new(AgentInvocationResult::AgentInitialization)),
            method_name: None,
            consumed_fuel: 0,
            component_revision: ComponentRevision::INITIAL,
        },
    );
}

fn invocation_entries(keys: &[IdempotencyKey]) -> BTreeMap<OplogIndex, OplogEntry> {
    let mut entries = BTreeMap::from([(OplogIndex::INITIAL, OplogEntry::recovery_succeeded())]);
    for (offset, key) in keys.iter().enumerate() {
        invocation_pair(&mut entries, 2 + offset as u64 * 2, key);
    }
    entries
}

fn invocation_status(
    oplog_idx: u64,
    capacity: usize,
    results: &[(&IdempotencyKey, u64)],
    revert_generation: u64,
    deleted_regions: DeletedRegions,
) -> AgentStatusRecord {
    let mut invocation_results = InvocationResultMembership::new(capacity, 128, 3);
    for (key, index) in results {
        invocation_results.insert((*key).clone(), OplogIndex::from_u64(*index));
    }
    invocation_results.set_revert_generation(revert_generation);
    AgentStatusRecord {
        oplog_idx: OplogIndex::from_u64(oplog_idx),
        invocation_results,
        deleted_regions,
        ..AgentStatusRecord::default()
    }
}

fn index_test_service(
    entries: BTreeMap<OplogIndex, OplogEntry>,
) -> (
    Arc<DefaultWorkerService>,
    Arc<IndexTestOplogService>,
    OwnedAgentId,
) {
    let oplog = Arc::new(IndexTestOplogService::new(entries));
    let service = index_test_service_with(Arc::new(InMemoryKeyValueStorage::new()), oplog.clone());
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "invocation-index-test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);
    (service, oplog, owned_agent_id)
}

fn index_test_service_with(
    storage: Arc<dyn KeyValueStorage + Send + Sync>,
    oplog: Arc<IndexTestOplogService>,
) -> Arc<DefaultWorkerService> {
    let mut config = GolemConfig::default();
    config.invocation_results.physical_index_catch_up_chunk_size = 2;
    Arc::new(DefaultWorkerService::new(
        storage,
        Arc::new(ShardServiceDefault::new()),
        oplog,
        Arc::new(IndexTestComponentService),
        Arc::new(config),
    ))
}

#[test]
fn rejections_to_store_are_in_index_order_keep_only_candidates_and_nothing_when_unchanged() {
    let index = OplogIndex::from_u64;
    let status = with_candidates(7, Some(3));
    assert_eq!(
        [
            rejections_to_store(vec![index(7)], &status, &HashSet::from([index(3)])),
            rejections_to_store(
                vec![index(3), index(7)],
                &status,
                &HashSet::from([index(7), index(3)])
            ),
            rejections_to_store(Vec::new(), &status, &HashSet::new()),
            rejections_to_store(Vec::new(), &status, &HashSet::from([index(5)])),
            rejections_to_store(vec![index(3), index(5)], &status, &HashSet::new()),
        ],
        [
            Some(vec![index(3), index(7)]),
            None,
            None,
            None,
            Some(vec![index(3)]),
        ]
    );
}

#[test]
async fn get_recovers_uuid_named_non_agent_component_worker() {
    let component_id = ComponentId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id,
        agent_id: Uuid::new_v4().to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let create = OplogEntry::Create {
        timestamp: Timestamp::now_utc(),
        parameters: Box::new(golem_common::model::oplog::CreateParameters {
            agent_id: agent_id.clone(),
            owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
            agent_mode: AgentMode::Durable,
            component_revision: ComponentRevision::INITIAL,
            env: Vec::new(),
            environment_id,
            created_by: AccountId::new(),
            parent: None,
            component_size: 1,
            initial_total_linear_memory_size: 0,
            initial_active_plugins: HashSet::new(),
            local_agent_config: Vec::new(),
            original_phantom_id: None,
            instance_id: Uuid::new_v4(),
        }),
    };
    let shard_service = Arc::new(ShardServiceDefault::new());
    shard_service.install_unexpiring(4, &HashMap::new());
    let service = DefaultWorkerService::new(
        Arc::new(InMemoryKeyValueStorage::new()),
        shard_service,
        Arc::new(IndexTestOplogService::new(BTreeMap::from([(
            OplogIndex::INITIAL,
            create,
        )]))),
        Arc::new(IndexTestComponentService),
        Arc::new(GolemConfig::default()),
    );

    let result = service.get(&owned_agent_id).await.unwrap().unwrap();

    assert_eq!(result.initial_worker_metadata.agent_id, agent_id);
    assert!(result.last_known_status.is_some());
}

/// A status whose two candidates for a start are the automatic snapshot entries `last` and
/// `previous`.
fn with_candidates(last: u64, previous: Option<u64>) -> AgentStatusRecord {
    AgentStatusRecord {
        last_automatic_snapshot: Some(golem_common::model::AutomaticSnapshot {
            index: OplogIndex::from_u64(last),
            timestamp: Timestamp::from(1_000),
            component_revision: ComponentRevision::INITIAL,
            files: golem_common::model::SnapshotFiles::Unnamed,
        }),
        previous_usable_automatic_snapshot: previous.map(|previous| {
            golem_common::model::UsableAutomaticSnapshot {
                index: OplogIndex::from_u64(previous),
                component_revision: ComponentRevision::INITIAL,
                filesystem_snapshot: None,
            }
        }),
        ..AgentStatusRecord::default()
    }
}

#[test]
async fn the_stored_rejections_keep_only_the_candidates_of_the_status_of_the_write() {
    let (service, _, owned_agent_id) = index_test_service(BTreeMap::new());
    let fingerprint = AgentFingerprint::new();
    let stored = || service.get_rejected_periodic_snapshots(&owned_agent_id, fingerprint);

    service
        .reject_periodic_snapshots(
            &owned_agent_id,
            fingerprint,
            &with_candidates(12, Some(7)),
            &HashSet::from([OplogIndex::from_u64(7), OplogIndex::from_u64(3)]),
        )
        .await
        .unwrap();
    let before = stored().await.unwrap();
    service
        .reject_periodic_snapshots(
            &owned_agent_id,
            fingerprint,
            &with_candidates(20, Some(12)),
            &HashSet::from([OplogIndex::from_u64(20)]),
        )
        .await
        .unwrap();
    let after = stored().await.unwrap();

    assert_eq!(before, HashSet::from([OplogIndex::from_u64(7)]));
    assert_eq!(after, HashSet::from([OplogIndex::from_u64(20)]));
}

#[test]
async fn rejected_periodic_snapshots_are_exact_indexes_and_incarnation_scoped() {
    let (service, _, owned_agent_id) = index_test_service(BTreeMap::new());
    let first = AgentFingerprint::new();
    let second = AgentFingerprint::new();
    let expected = HashSet::from([OplogIndex::from_u64(7), OplogIndex::from_u64(12)]);
    let status = with_candidates(12, Some(7));

    service
        .reject_periodic_snapshots(
            &owned_agent_id,
            first,
            &status,
            &HashSet::from([OplogIndex::from_u64(12)]),
        )
        .await
        .unwrap();
    service
        .reject_periodic_snapshots(
            &owned_agent_id,
            first,
            &status,
            &HashSet::from([OplogIndex::from_u64(7)]),
        )
        .await
        .unwrap();

    assert_eq!(
        service
            .get_rejected_periodic_snapshots(&owned_agent_id, first)
            .await
            .unwrap(),
        expected
    );
    assert_eq!(
        service
            .get_rejected_periodic_snapshots(&owned_agent_id, second)
            .await
            .unwrap(),
        HashSet::new()
    );

    service
        .remove_cached_status(&owned_agent_id, invocation_index_fingerprint())
        .await
        .unwrap();
    assert_eq!(
        service
            .get_rejected_periodic_snapshots(&owned_agent_id, first)
            .await
            .unwrap(),
        expected
    );

    service
        .remove_split_status(
            &owned_agent_id,
            DefaultWorkerService::rejected_periodic_snapshots_namespace(&owned_agent_id.agent_id),
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .get_rejected_periodic_snapshots(&owned_agent_id, first)
            .await
            .unwrap(),
        HashSet::new()
    );
}

#[test]
async fn remembered_status_baseline_recovers_complete_hash_after_cache_loss() {
    let storage = Arc::new(InMemoryKeyValueStorage::new());
    let service = DefaultWorkerService::new(
        storage.clone(),
        Arc::new(ShardServiceDefault::new()),
        Arc::new(IndexTestOplogService::new(BTreeMap::new())),
        Arc::new(IndexTestComponentService),
        Arc::new(GolemConfig::default()),
    );
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "status-cache-loss".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);
    let fingerprint = AgentFingerprint::new();
    let namespace = DefaultWorkerService::status_namespace(&agent_id, fingerprint);
    let status = AgentStatusRecord {
        oplog_idx: OplogIndex::from_u64(10),
        skipped_regions: DeletedRegions::from_regions([OplogRegion {
            start: OplogIndex::from_u64(3),
            end: OplogIndex::from_u64(5),
        }]),
        ..AgentStatusRecord::default()
    };

    service
        .write_split_status(&owned_agent_id, namespace.clone(), None, status.clone())
        .await
        .unwrap();
    let fields = storage
        .keys("test", "test", namespace.clone())
        .await
        .unwrap();
    storage
        .del_many("test", "test", namespace.clone(), fields.into())
        .await
        .unwrap();

    service
        .write_split_status(
            &owned_agent_id,
            namespace.clone(),
            Some(&status),
            status.clone(),
        )
        .await
        .unwrap();

    assert_eq!(
        service
            .read_split_status(&owned_agent_id, namespace)
            .await
            .unwrap(),
        Some(status)
    );
}

#[test]
async fn stale_status_baseline_reconciles_without_mixed_auxiliary_fields() {
    let storage = Arc::new(InMemoryKeyValueStorage::new());
    let service = DefaultWorkerService::new(
        storage,
        Arc::new(ShardServiceDefault::new()),
        Arc::new(IndexTestOplogService::new(BTreeMap::new())),
        Arc::new(IndexTestComponentService),
        Arc::new(GolemConfig::default()),
    );
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "status-stale-baseline".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);
    let namespace = DefaultWorkerService::status_namespace(&agent_id, AgentFingerprint::new());
    let persisted = AgentStatusRecord {
        oplog_idx: OplogIndex::from_u64(20),
        skipped_regions: DeletedRegions::from_regions([OplogRegion {
            start: OplogIndex::from_u64(3),
            end: OplogIndex::from_u64(5),
        }]),
        ..AgentStatusRecord::default()
    };
    let remembered = AgentStatusRecord {
        oplog_idx: OplogIndex::from_u64(10),
        ..AgentStatusRecord::default()
    };
    let replacement = AgentStatusRecord {
        oplog_idx: OplogIndex::from_u64(30),
        ..AgentStatusRecord::default()
    };

    service
        .write_split_status(&owned_agent_id, namespace.clone(), None, persisted)
        .await
        .unwrap();
    service
        .write_split_status(
            &owned_agent_id,
            namespace.clone(),
            Some(&remembered),
            replacement.clone(),
        )
        .await
        .unwrap();

    assert_eq!(
        service
            .read_split_status(&owned_agent_id, namespace)
            .await
            .unwrap(),
        Some(replacement)
    );
}

#[test]
async fn cold_status_publication_removes_orphan_transfers_without_a_core() {
    let (service, storage, owned_agent_id, _, _) = assignment_tracking_test_service();
    let fingerprint = invocation_index_fingerprint();
    let mut source = sample_status();
    let (transfer_id, transfer) = source.received_card_transfers.iter().next().unwrap();
    let orphan_field = status_received_card_transfer_field(transfer_id);
    let orphan_value = serialize(transfer).unwrap();
    source.received_card_transfers = ReceivedCardTransferIndex::default();

    for namespace in [
        DefaultWorkerService::status_namespace(&owned_agent_id.agent_id, fingerprint),
        DefaultWorkerService::checkpoint_namespace(&owned_agent_id.agent_id, fingerprint),
    ] {
        storage
            .with_entity("test", "seed_orphan_transfer", "transfer")
            .set_raw(namespace.clone(), &orphan_field, &orphan_value)
            .await
            .unwrap();

        service
            .write_split_status(&owned_agent_id, namespace.clone(), None, source.clone())
            .await
            .unwrap();

        assert_eq!(
            service
                .read_split_status(&owned_agent_id, namespace)
                .await
                .unwrap(),
            Some(source.clone())
        );
    }
}

fn assignment_tracking_test_service() -> (
    DefaultWorkerService,
    Arc<InMemoryKeyValueStorage>,
    OwnedAgentId,
    usize,
    Arc<IndexTestOplogService>,
) {
    let key_value_storage = Arc::new(InMemoryKeyValueStorage::new());
    let shard_service = Arc::new(ShardServiceDefault::new());
    let number_of_shards = 4;
    shard_service.install_unexpiring(number_of_shards, &HashMap::new());
    let oplog_service = Arc::new(IndexTestOplogService::new(BTreeMap::new()));
    let service = DefaultWorkerService::new(
        key_value_storage.clone(),
        shard_service,
        oplog_service.clone(),
        Arc::new(IndexTestComponentService),
        Arc::new(GolemConfig::default()),
    );
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "assignment-tracking-test".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);
    (
        service,
        key_value_storage,
        owned_agent_id,
        number_of_shards,
        oplog_service,
    )
}

async fn assignment_tracking_members(
    key_value_storage: &InMemoryKeyValueStorage,
    owned_agent_id: &OwnedAgentId,
    number_of_shards: usize,
) -> Vec<RunningWorker> {
    let shard_id = ShardId::from_agent_id(&owned_agent_id.agent_id, number_of_shards);
    key_value_storage
        .with_entity("test", "get_assignment_tracking", "agent_id")
        .members_of_set(
            KeyValueStorageNamespace::RunningWorkers,
            &DefaultWorkerService::running_in_shard_key(&shard_id),
        )
        .await
        .unwrap()
}

async fn invocation_index_metadata(
    service: &DefaultWorkerService,
    owned_agent_id: &OwnedAgentId,
) -> InvocationResultIndexMetadata {
    let value: Option<Result<InvocationResultIndexMetadata, String>> = service
        .key_value_storage
        .with_entity("test", "read_invocation_result_index", "metadata")
        .get_attempt_deserialize(
            DefaultWorkerService::invocation_result_index_namespace(
                &owned_agent_id.agent_id,
                invocation_index_fingerprint(),
            ),
            INVOCATION_RESULT_INDEX_METADATA_FIELD,
        )
        .await
        .unwrap();
    value.unwrap().unwrap()
}

fn idempotency_key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value.to_string())
}

fn invocation_index_fingerprint() -> AgentFingerprint {
    AgentFingerprint(uuid::Uuid::from_u128(1))
}

fn transfer_id(value: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(value)
}

fn stored_card(card_id: CardId) -> StoredCard {
    StoredCard::Concrete(Card {
        card_id,
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    })
}

fn source_transfer_state() -> SourceCardTransferState {
    let card = stored_card(CardId::new());
    SourceCardTransferState::Recorded(golem_common::model::card::SourceCardTransferRecords {
        derived_card: Some(card.clone()),
        queued: Some(golem_common::base_model::oplog::QueuedCardEventTransfer {
            transfer_id: transfer_id(1),
            card_id: CardId::new(),
            card: Some(card),
            target_holder: golem_common::model::card::CardHolder::Account(
                golem_common::model::card::AccountCardHolder {
                    account_id: uuid::Uuid::from_u128(7),
                },
            ),
        }),
        started: None,
        confirmed: None,
    })
}

fn sample_status() -> AgentStatusRecord {
    let mut status = AgentStatusRecord {
        status: AgentStatus::Running,
        oplog_idx: OplogIndex::from_u64(42),
        component_revision: ComponentRevision::new(3).unwrap(),
        ..AgentStatusRecord::default()
    };
    status
        .invocation_results
        .insert(idempotency_key("k1"), OplogIndex::from_u64(10));
    status
        .invocation_results
        .insert(idempotency_key("k2"), OplogIndex::from_u64(20));
    status.received_card_transfers.insert(
        transfer_id(1),
        ReceivedCardTransferState::Received {
            source_card_id: CardId::new(),
            card: stored_card(CardId::new()),
        },
    );
    status
        .source_card_transfers
        .insert(transfer_id(1), source_transfer_state());
    status.durable_stream_sessions.insert(
        idempotency_key("stream-1"),
        DurableStreamSessionStatus {
            first_prepared: Some(OplogIndex::from_u64(30)),
            prepared: Some(OplogIndex::from_u64(35)),
            invocation_result: Some(OplogIndex::from_u64(40)),
            finished: None,
            ..Default::default()
        },
    );
    status.skipped_regions = DeletedRegions::from_regions([OplogRegion::from_index_range(
        OplogIndex::from_u64(2)..=OplogIndex::from_u64(5),
    )]);
    status.deleted_regions = DeletedRegions::from_regions([OplogRegion::from_index_range(
        OplogIndex::from_u64(7)..=OplogIndex::from_u64(8),
    )]);
    // Use millisecond-precise timestamps so they round-trip exactly through the codec (which
    // serializes `Timestamp` at millisecond resolution).
    status.failed_updates.push(FailedUpdateRecord {
        timestamp: Timestamp::from(1_700_000_000_000u64),
        target_revision: ComponentRevision::new(2).unwrap(),
        details: Some("boom".to_string()),
        pending_update: None,
        snapshot_assisted_details: None,
        snapshot_fault: Some(golem_common::model::oplog::SnapshotFault::Unavailable),
    });
    status.successful_updates.push(SuccessfulUpdateRecord {
        timestamp: Timestamp::from(1_700_000_001_000u64),
        target_revision: ComponentRevision::new(3).unwrap(),
        oplog_index: OplogIndex::from_u64(6),
        filesystem_snapshot: Some(golem_common::model::oplog::FilesystemSnapshotName::update()),
        pending_update: None,
        snapshot_assisted_details: None,
    });
    let assisted_name = golem_common::model::oplog::FilesystemSnapshotName::periodic();
    status
        .pending_updates
        .push_back(golem_common::model::PendingUpdateRef {
            timestamp: Timestamp::from(1_700_000_002_000u64),
            oplog_index: OplogIndex::from_u64(9),
            admission_index: OplogIndex::from_u64(8),
            target_revision: ComponentRevision::new(4).unwrap(),
            kind: golem_common::model::PendingUpdateKind::SnapshotAssistedAutomatic(Box::new(
                golem_common::model::AssistedSelection {
                    source_revision_start_index: OplogIndex::from_u64(6),
                    snapshot: golem_common::model::UsableAutomaticSnapshot {
                        index: OplogIndex::from_u64(7),
                        component_revision: ComponentRevision::new(3).unwrap(),
                        filesystem_snapshot: Some(assisted_name.clone()),
                    },
                },
            )),
        });
    status.authoritative_snapshot = Some(golem_common::model::AuthoritativeSnapshot {
        index: OplogIndex::from_u64(5),
        kind: golem_common::model::AuthoritativeSnapshotKind::SnapshotAssistedAutomatic {
            filesystem_snapshot: Some(assisted_name),
        },
    });
    status
}

/// Applies a computed `(sets, dels)` to a field map simulating the per-worker hash, then
/// reassembles the cached status from it.
fn apply_and_reassemble(
    store: &mut HashMap<String, Vec<u8>>,
    sets: Vec<(String, Vec<u8>)>,
    dels: Vec<String>,
) -> Option<AgentStatusRecord> {
    for (field, bytes) in sets {
        store.insert(field, bytes);
    }
    for field in dels {
        store.remove(&field);
    }
    reassemble_cached_status(
        store
            .iter()
            .map(|(name, bytes)| (name.clone(), Bytes::from(bytes.clone()))),
    )
}

#[test]
fn split_status_round_trips_through_fields() {
    let full = sample_status();

    let mut core = full.clone();
    let parts = split_status(&mut core);
    let (sets, dels) = compute_status_field_writes(None, &[], &core, &parts).unwrap();
    assert!(dels.is_empty());

    let mut store = HashMap::new();
    let reassembled = apply_and_reassemble(&mut store, sets, dels).unwrap();

    // `agent_mode` is transient and defaults to `Durable`; `full` uses the default.
    assert_eq!(reassembled, full);
}

#[test]
fn borrowed_status_core_matches_split_status_encoding() {
    let status = sample_status();
    let mut split = status.clone();
    split_status(&mut split);

    assert_eq!(serialize(&status_core(&status)), serialize(&split));
}

#[test]
fn missing_core_field_is_a_cache_miss() {
    let full = sample_status();
    let mut core = full.clone();
    let parts = split_status(&mut core);
    let (sets, _) = compute_status_field_writes(None, &[], &core, &parts).unwrap();

    // Drop the core field; what remains must not reassemble.
    let without_core = sets
        .into_iter()
        .filter(|(name, _)| name != STATUS_CORE_FIELD)
        .map(|(name, bytes)| (name, Bytes::from(bytes)));
    assert!(reassemble_cached_status(without_core).is_none());
}

#[test]
fn missing_membership_field_is_a_cache_miss() {
    let full = sample_status();
    let mut core = full.clone();
    let parts = split_status(&mut core);
    let (sets, _) = compute_status_field_writes(None, &[], &core, &parts).unwrap();

    let without_membership = sets
        .into_iter()
        .filter(|(name, _)| name != STATUS_MEMBERSHIP_FIELD)
        .map(|(name, bytes)| (name, Bytes::from(bytes)));
    assert!(reassemble_cached_status(without_membership).is_none());
}

#[test]
fn hot_delta_only_writes_changed_fields() {
    let previous = sample_status();

    // Seed the store with the full previous state.
    let mut store = HashMap::new();
    {
        let mut core = previous.clone();
        let parts = split_status(&mut core);
        let (sets, _) = compute_status_field_writes(None, &[], &core, &parts).unwrap();
        for (field, bytes) in sets {
            store.insert(field, bytes);
        }
    }

    // New status: a new bounded invocation result + advanced marker, but identical split
    // regions/updates.
    let mut new = previous.clone();
    new.oplog_idx = OplogIndex::from_u64(50);
    new.invocation_results
        .insert(idempotency_key("k3"), OplogIndex::from_u64(48));

    let mut core = new.clone();
    let parts = split_status(&mut core);
    let (sets, dels) = compute_status_field_writes(Some(&previous), &[], &core, &parts).unwrap();

    let written: HashSet<&str> = sets.iter().map(|(f, _)| f.as_str()).collect();
    assert!(written.contains(STATUS_CORE_FIELD));
    assert!(written.contains(STATUS_MEMBERSHIP_FIELD));
    // Unchanged parts are NOT re-sent.
    assert!(!written.contains(STATUS_REGIONS_FIELD));
    assert!(!written.contains(STATUS_UPDATES_FIELD));
    assert!(!written.contains(status_received_card_transfer_field(&transfer_id(1)).as_str()));
    assert!(!written.contains(status_source_card_transfer_field(&transfer_id(1)).as_str()));
    assert_eq!(written.len(), 2);
    assert!(dels.is_empty());

    let reassembled = apply_and_reassemble(&mut store, sets, dels).unwrap();
    assert_eq!(reassembled, new);
}

#[test]
fn hot_delta_only_writes_changed_transfer_fields() {
    let previous = sample_status();
    let mut new = previous.clone();
    new.oplog_idx = OplogIndex::from_u64(50);
    new.received_card_transfers.insert(
        transfer_id(2),
        ReceivedCardTransferState::Received {
            source_card_id: CardId::new(),
            card: stored_card(CardId::new()),
        },
    );
    new.source_card_transfers
        .insert(transfer_id(1), SourceCardTransferState::Conflict);
    new.source_card_transfers
        .insert(transfer_id(3), source_transfer_state());

    let mut store = HashMap::new();
    {
        let mut core = previous.clone();
        let parts = split_status(&mut core);
        let (sets, _) = compute_status_field_writes(None, &[], &core, &parts).unwrap();
        store.extend(sets);
    }
    let mut core = new.clone();
    let parts = split_status(&mut core);
    let (sets, dels) = compute_status_field_writes(Some(&previous), &[], &core, &parts).unwrap();

    let written: HashSet<&str> = sets.iter().map(|(field, _)| field.as_str()).collect();
    assert_eq!(
        written,
        HashSet::from([
            STATUS_CORE_FIELD,
            status_received_card_transfer_field(&transfer_id(2)).as_str(),
            status_source_card_transfer_field(&transfer_id(1)).as_str(),
            status_source_card_transfer_field(&transfer_id(3)).as_str(),
        ])
    );
    assert!(dels.is_empty());

    let reassembled = apply_and_reassemble(&mut store, sets, dels).unwrap();
    assert_eq!(reassembled, new);
}

#[test]
fn bounded_invocation_result_changes_are_stored_in_membership() {
    let previous = sample_status();

    let mut store = HashMap::new();
    {
        let mut core = previous.clone();
        let parts = split_status(&mut core);
        let (sets, _) = compute_status_field_writes(None, &[], &core, &parts).unwrap();
        for (field, bytes) in sets {
            store.insert(field, bytes);
        }
    }

    // New status with k2 removed (as a revert would do).
    let mut new = previous.clone();
    new.invocation_results.remove(&idempotency_key("k2"));

    let mut core = new.clone();
    let parts = split_status(&mut core);
    let (sets, dels) = compute_status_field_writes(Some(&previous), &[], &core, &parts).unwrap();

    assert!(dels.is_empty());
    let written: HashSet<&str> = sets.iter().map(|(field, _)| field.as_str()).collect();
    assert_eq!(
        written,
        HashSet::from([STATUS_CORE_FIELD, STATUS_MEMBERSHIP_FIELD])
    );

    let reassembled = apply_and_reassemble(&mut store, sets, dels).unwrap();
    assert_eq!(reassembled, new);
}

#[test]
fn cold_reconcile_deletes_stale_transfer_fields() {
    let stale_transfer_id = transfer_id(2);
    let existing_fields = vec![
        STATUS_CORE_FIELD.to_string(),
        status_received_card_transfer_field(&transfer_id(1)),
        status_received_card_transfer_field(&stale_transfer_id),
        status_source_card_transfer_field(&transfer_id(1)),
        status_source_card_transfer_field(&stale_transfer_id),
    ];
    let new = sample_status();

    let mut core = new;
    let parts = split_status(&mut core);
    let (_, dels) = compute_status_field_writes(None, &existing_fields, &core, &parts).unwrap();

    assert_eq!(
        dels,
        vec![
            status_received_card_transfer_field(&stale_transfer_id),
            status_source_card_transfer_field(&stale_transfer_id),
        ]
    );
}

#[test]
async fn invocation_result_index_resumes_across_calls_and_chunks() {
    let first = idempotency_key("first");
    let second = idempotency_key("second");
    let (service, oplog, owned_agent_id) =
        index_test_service(invocation_entries(&[first.clone(), second.clone()]));
    let partial = invocation_status(2, 2, &[], 0, DeletedRegions::new());
    let complete = invocation_status(5, 2, &[(&first, 3), (&second, 5)], 0, DeletedRegions::new());

    service
        .catch_up_invocation_result_index(
            &owned_agent_id,
            AgentMode::Durable,
            invocation_index_fingerprint(),
            &partial,
        )
        .await
        .unwrap();
    service
        .catch_up_invocation_result_index(
            &owned_agent_id,
            AgentMode::Durable,
            invocation_index_fingerprint(),
            &complete,
        )
        .await
        .unwrap();

    assert_eq!(
        oplog.read_starts(),
        vec![
            OplogIndex::INITIAL,
            OplogIndex::from_u64(3),
            OplogIndex::from_u64(5)
        ]
    );
    assert_eq!(
        service
            .lookup_invocation_result_index(
                &owned_agent_id,
                invocation_index_fingerprint(),
                &complete,
                &first,
            )
            .await
            .unwrap(),
        InvocationResultIndexLookup::Found(OplogIndex::from_u64(3))
    );
    assert_eq!(
        service
            .lookup_invocation_result_index(
                &owned_agent_id,
                invocation_index_fingerprint(),
                &complete,
                &second,
            )
            .await
            .unwrap(),
        InvocationResultIndexLookup::Found(OplogIndex::from_u64(5))
    );
}

#[test]
async fn invocation_result_index_restarts_from_beginning_after_hash_loss() {
    let first = idempotency_key("first");
    let second = idempotency_key("second");
    let (service, oplog, owned_agent_id) =
        index_test_service(invocation_entries(&[first.clone(), second.clone()]));
    let partial = invocation_status(3, 2, &[(&first, 3)], 0, DeletedRegions::new());
    let complete = invocation_status(5, 2, &[(&first, 3), (&second, 5)], 0, DeletedRegions::new());

    service
        .catch_up_invocation_result_index(
            &owned_agent_id,
            AgentMode::Durable,
            invocation_index_fingerprint(),
            &partial,
        )
        .await
        .unwrap();
    oplog.pause_next_read();
    let catch_up = tokio::spawn({
        let service = service.clone();
        let owned_agent_id = owned_agent_id.clone();
        async move {
            service
                .catch_up_invocation_result_index(
                    &owned_agent_id,
                    AgentMode::Durable,
                    invocation_index_fingerprint(),
                    &complete,
                )
                .await
        }
    });
    oplog.read_started.notified().await;

    let namespace = DefaultWorkerService::invocation_result_index_namespace(
        &owned_agent_id.agent_id,
        invocation_index_fingerprint(),
    );
    let fields = service
        .key_value_storage
        .with("test", "simulate_hash_loss")
        .keys(namespace.clone())
        .await
        .unwrap();
    service
        .key_value_storage
        .with("test", "simulate_hash_loss")
        .del_many(namespace, fields.into())
        .await
        .unwrap();
    oplog.resume_read.notify_one();
    catch_up.await.unwrap().unwrap();

    assert!(oplog.read_starts().contains(&OplogIndex::INITIAL));
    assert_eq!(
        service
            .lookup_invocation_result_index(
                &owned_agent_id,
                invocation_index_fingerprint(),
                &invocation_status(5, 2, &[(&first, 3), (&second, 5)], 0, DeletedRegions::new()),
                &first,
            )
            .await
            .unwrap(),
        InvocationResultIndexLookup::Found(OplogIndex::from_u64(3))
    );
}

#[test]
async fn invocation_result_index_ahead_of_status_is_not_reset() {
    let key = idempotency_key("completed");
    let (service, oplog, owned_agent_id) =
        index_test_service(invocation_entries(std::slice::from_ref(&key)));
    let complete = invocation_status(3, 2, &[(&key, 3)], 0, DeletedRegions::new());
    service
        .catch_up_invocation_result_index(
            &owned_agent_id,
            AgentMode::Durable,
            invocation_index_fingerprint(),
            &complete,
        )
        .await
        .unwrap();
    oplog.clear_reads();

    let stale = invocation_status(2, 2, &[], 0, DeletedRegions::new());
    service
        .catch_up_invocation_result_index(
            &owned_agent_id,
            AgentMode::Durable,
            invocation_index_fingerprint(),
            &stale,
        )
        .await
        .unwrap();

    assert!(oplog.read_starts().is_empty());
    assert_eq!(
        invocation_index_metadata(&service, &owned_agent_id)
            .await
            .covered_through,
        OplogIndex::from_u64(3)
    );
}

#[test]
async fn invocation_result_index_and_exact_membership_are_jointly_complete() {
    let first = idempotency_key("first");
    let second = idempotency_key("second");
    let third = idempotency_key("third");
    let fourth = idempotency_key("fourth");
    let fifth = idempotency_key("fifth");
    let missing = idempotency_key("missing");
    let (service, _oplog, owned_agent_id) = index_test_service(invocation_entries(&[
        first.clone(),
        second.clone(),
        third.clone(),
        fourth.clone(),
        fifth.clone(),
    ]));
    let indexed = invocation_status(
        9,
        2,
        &[(&first, 3), (&second, 5), (&third, 7), (&fourth, 9)],
        0,
        DeletedRegions::new(),
    );
    service
        .catch_up_invocation_result_index(
            &owned_agent_id,
            AgentMode::Durable,
            invocation_index_fingerprint(),
            &indexed,
        )
        .await
        .unwrap();

    let current = invocation_status(
        11,
        2,
        &[
            (&first, 3),
            (&second, 5),
            (&third, 7),
            (&fourth, 9),
            (&fifth, 11),
        ],
        0,
        DeletedRegions::new(),
    );
    assert_eq!(
        current.invocation_results.oldest_retained_index(),
        Some(OplogIndex::from_u64(9))
    );
    assert_eq!(
        service
            .lookup_invocation_result_index(
                &owned_agent_id,
                invocation_index_fingerprint(),
                &current,
                &missing,
            )
            .await
            .unwrap(),
        InvocationResultIndexLookup::DefinitiveMiss
    );
}

#[test]
async fn incomplete_invocation_result_index_does_not_return_an_obsolete_result() {
    let repeated = idempotency_key("repeated");
    let (service, _oplog, owned_agent_id) =
        index_test_service(invocation_entries(&[repeated.clone(), repeated.clone()]));
    let partial = invocation_status(3, 0, &[(&repeated, 3)], 0, DeletedRegions::new());
    service
        .catch_up_invocation_result_index(
            &owned_agent_id,
            AgentMode::Durable,
            invocation_index_fingerprint(),
            &partial,
        )
        .await
        .unwrap();

    let current = invocation_status(
        5,
        0,
        &[(&repeated, 3), (&repeated, 5)],
        0,
        DeletedRegions::new(),
    );
    assert_eq!(
        service
            .lookup_invocation_result_index(
                &owned_agent_id,
                invocation_index_fingerprint(),
                &current,
                &repeated,
            )
            .await
            .unwrap(),
        InvocationResultIndexLookup::Incomplete
    );

    service
        .catch_up_invocation_result_index(
            &owned_agent_id,
            AgentMode::Durable,
            invocation_index_fingerprint(),
            &current,
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .lookup_invocation_result_index(
                &owned_agent_id,
                invocation_index_fingerprint(),
                &current,
                &repeated,
            )
            .await
            .unwrap(),
        InvocationResultIndexLookup::Found(OplogIndex::from_u64(5))
    );
}

#[test]
async fn independent_catch_up_cannot_publish_over_newer_generation() {
    let reverted = idempotency_key("independent-reverted");
    let current = idempotency_key("independent-current");
    let mut entries = invocation_entries(std::slice::from_ref(&reverted));
    entries.insert(
        OplogIndex::from_u64(4),
        OplogEntry::revert(OplogRegion::from_index_range(
            OplogIndex::from_u64(2)..=OplogIndex::from_u64(3),
        )),
    );
    invocation_pair(&mut entries, 5, &current);
    let oplog = Arc::new(IndexTestOplogService::new(entries));
    let faults = KeyValueStorageFaults::default();
    let storage: Arc<dyn KeyValueStorage + Send + Sync> =
        Arc::new(FaultInjectingKeyValueStorage::new(
            Arc::new(InMemoryKeyValueStorage::new()),
            faults.clone(),
        ));
    let old_service = index_test_service_with(storage.clone(), oplog.clone());
    let new_service = index_test_service_with(storage, oplog);
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "independent-invocation-index".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);
    let old_status = invocation_status(3, 2, &[(&reverted, 3)], 0, DeletedRegions::new());
    let deleted_regions = DeletedRegions::from_regions([OplogRegion::from_index_range(
        OplogIndex::from_u64(2)..=OplogIndex::from_u64(3),
    )]);
    let new_status = invocation_status(6, 2, &[(&current, 6)], 1, deleted_regions);

    let gate = faults.gate_next_pass("advance_invocation_result_index");
    let old_task = tokio::spawn({
        let service = old_service.clone();
        let owned_agent_id = owned_agent_id.clone();
        async move {
            service
                .catch_up_invocation_result_index(
                    &owned_agent_id,
                    AgentMode::Durable,
                    invocation_index_fingerprint(),
                    &old_status,
                )
                .await
        }
    });
    gate.entered().await;
    new_service
        .catch_up_invocation_result_index(
            &owned_agent_id,
            AgentMode::Durable,
            invocation_index_fingerprint(),
            &new_status,
        )
        .await
        .unwrap();
    gate.release();
    old_task.await.unwrap().unwrap();

    let metadata = invocation_index_metadata(&new_service, &owned_agent_id).await;
    assert_eq!(metadata.revert_generation, 1);
    assert_eq!(metadata.covered_through, OplogIndex::from_u64(6));
    assert_eq!(
        new_service
            .lookup_invocation_result_index(
                &owned_agent_id,
                invocation_index_fingerprint(),
                &new_status,
                &current,
            )
            .await
            .unwrap(),
        InvocationResultIndexLookup::Found(OplogIndex::from_u64(6))
    );
    assert_eq!(
        new_service
            .lookup_invocation_result_index(
                &owned_agent_id,
                invocation_index_fingerprint(),
                &new_status,
                &reverted,
            )
            .await
            .unwrap(),
        InvocationResultIndexLookup::DefinitiveMiss
    );
}

#[test]
async fn stale_reset_cannot_delete_independent_newer_progress() {
    let current = idempotency_key("reset-current");
    let oplog = Arc::new(IndexTestOplogService::new(invocation_entries(
        std::slice::from_ref(&current),
    )));
    let inner = Arc::new(InMemoryKeyValueStorage::new());
    let faults = KeyValueStorageFaults::default();
    let storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(
        FaultInjectingKeyValueStorage::new(inner.clone(), faults.clone()),
    );
    let stale_service = index_test_service_with(storage.clone(), oplog.clone());
    let current_service = index_test_service_with(storage, oplog);
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "independent-reset-index".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent_id);
    let namespace = DefaultWorkerService::invocation_result_index_namespace(
        &agent_id,
        invocation_index_fingerprint(),
    );
    let mut malformed_metadata = serialize(&InvocationResultIndexMetadata {
        covered_through: OplogIndex::from_u64(2),
        revert_generation: 0,
        current_idempotency_key: None,
        cancelled_idempotency_key: None,
    })
    .unwrap();
    malformed_metadata.pop();
    assert!(deserialize::<InvocationResultIndexMetadata>(&malformed_metadata).is_err());
    inner
        .set_many(
            "test",
            "seed_malformed_index",
            "invocation_result",
            namespace.clone(),
            &[
                (
                    INVOCATION_RESULT_INDEX_METADATA_FIELD,
                    malformed_metadata.as_slice(),
                ),
                ("orphaned-mapping", b"stale"),
            ],
        )
        .await
        .unwrap();
    let stale_status = invocation_status(3, 1, &[(&current, 3)], 0, DeletedRegions::new());
    let current_status = invocation_status(3, 1, &[(&current, 3)], 1, DeletedRegions::new());

    let gate = faults.gate_next_pass("reset_invocation_result_index");
    let stale_task = tokio::spawn({
        let service = stale_service.clone();
        let owned_agent_id = owned_agent_id.clone();
        async move {
            service
                .catch_up_invocation_result_index(
                    &owned_agent_id,
                    AgentMode::Durable,
                    invocation_index_fingerprint(),
                    &stale_status,
                )
                .await
        }
    });
    gate.entered().await;
    current_service
        .catch_up_invocation_result_index(
            &owned_agent_id,
            AgentMode::Durable,
            invocation_index_fingerprint(),
            &current_status,
        )
        .await
        .unwrap();
    gate.release();
    stale_task.await.unwrap().unwrap();

    let metadata = invocation_index_metadata(&current_service, &owned_agent_id).await;
    assert_eq!(metadata.revert_generation, 1);
    assert_eq!(metadata.covered_through, OplogIndex::from_u64(3));
    assert_eq!(
        inner
            .get(
                "test",
                "verify_reset",
                "invocation_result",
                namespace,
                "orphaned-mapping",
            )
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        current_service
            .lookup_invocation_result_index(
                &owned_agent_id,
                invocation_index_fingerprint(),
                &current_status,
                &current,
            )
            .await
            .unwrap(),
        InvocationResultIndexLookup::Found(OplogIndex::from_u64(3))
    );
}

#[test]
async fn concurrent_generation_catch_up_clears_reverted_results() {
    let reverted = idempotency_key("reverted");
    let current = idempotency_key("current");
    let mut entries = invocation_entries(std::slice::from_ref(&reverted));
    entries.insert(
        OplogIndex::from_u64(4),
        OplogEntry::revert(OplogRegion::from_index_range(
            OplogIndex::from_u64(2)..=OplogIndex::from_u64(3),
        )),
    );
    invocation_pair(&mut entries, 5, &current);
    let (service, oplog, owned_agent_id) = index_test_service(entries);
    let old_status = invocation_status(3, 2, &[(&reverted, 3)], 0, DeletedRegions::new());
    let deleted_regions = DeletedRegions::from_regions([OplogRegion::from_index_range(
        OplogIndex::from_u64(2)..=OplogIndex::from_u64(3),
    )]);
    let new_status = invocation_status(6, 2, &[(&current, 6)], 1, deleted_regions);

    oplog.pause_next_read();
    let old_task = tokio::spawn({
        let service = service.clone();
        let owned_agent_id = owned_agent_id.clone();
        async move {
            service
                .catch_up_invocation_result_index(
                    &owned_agent_id,
                    AgentMode::Durable,
                    invocation_index_fingerprint(),
                    &old_status,
                )
                .await
        }
    });
    oplog.read_started.notified().await;
    let new_task = tokio::spawn({
        let service = service.clone();
        let owned_agent_id = owned_agent_id.clone();
        let new_status = new_status.clone();
        async move {
            service
                .catch_up_invocation_result_index(
                    &owned_agent_id,
                    AgentMode::Durable,
                    invocation_index_fingerprint(),
                    &new_status,
                )
                .await
        }
    });
    tokio::task::yield_now().await;
    assert!(!new_task.is_finished());
    oplog.resume_read.notify_one();
    old_task.await.unwrap().unwrap();
    new_task.await.unwrap().unwrap();

    let metadata = invocation_index_metadata(&service, &owned_agent_id).await;
    assert_eq!(metadata.revert_generation, 1);
    assert_eq!(metadata.covered_through, OplogIndex::from_u64(6));
    assert_eq!(
        service
            .lookup_invocation_result_index(
                &owned_agent_id,
                invocation_index_fingerprint(),
                &new_status,
                &reverted,
            )
            .await
            .unwrap(),
        InvocationResultIndexLookup::DefinitiveMiss
    );
    assert_eq!(
        service
            .lookup_invocation_result_index(
                &owned_agent_id,
                invocation_index_fingerprint(),
                &new_status,
                &current,
            )
            .await
            .unwrap(),
        InvocationResultIndexLookup::Found(OplogIndex::from_u64(6))
    );
    let fields = service
        .key_value_storage
        .with("test", "list_invocation_result_index")
        .keys(DefaultWorkerService::invocation_result_index_namespace(
            &owned_agent_id.agent_id,
            invocation_index_fingerprint(),
        ))
        .await
        .unwrap();
    assert!(!fields.contains(&invocation_result_index_field(&reverted)));
}

#[test]
async fn cancelled_catch_up_releases_invocation_result_index_lock_registration() {
    let key = idempotency_key("completed");
    let (service, oplog, owned_agent_id) =
        index_test_service(invocation_entries(std::slice::from_ref(&key)));
    let status = invocation_status(3, 1, &[(&key, 3)], 0, DeletedRegions::new());

    oplog.pause_next_read();
    let catch_up = tokio::spawn({
        let service = service.clone();
        let owned_agent_id = owned_agent_id.clone();
        async move {
            service
                .catch_up_invocation_result_index(
                    &owned_agent_id,
                    AgentMode::Durable,
                    invocation_index_fingerprint(),
                    &status,
                )
                .await
        }
    });
    oplog.read_started.notified().await;

    catch_up.abort();
    assert!(catch_up.await.unwrap_err().is_cancelled());

    assert!(
        !service
            .invocation_result_index_locks
            .lock()
            .unwrap()
            .contains_key(&owned_agent_id),
        "a cancelled catch-up must not retain one dead lock registration per agent"
    );
}

#[test]
async fn set_assignment_tracking_writes_durable_worker_to_recovery_index() {
    let (service, key_value_storage, owned_agent_id, number_of_shards, _) =
        assignment_tracking_test_service();
    let status = AgentStatusRecord {
        status: AgentStatus::Running,
        agent_mode: AgentMode::Durable,
        ..AgentStatusRecord::default()
    };

    service
        .set_assignment_tracking(&owned_agent_id, invocation_index_fingerprint(), &status)
        .await
        .unwrap();

    assert_eq!(
        assignment_tracking_members(&key_value_storage, &owned_agent_id, number_of_shards).await,
        vec![RunningWorker {
            owned_agent_id,
            fingerprint: invocation_index_fingerprint(),
        }]
    );
}

#[test]
async fn set_assignment_tracking_does_not_write_ephemeral_worker_to_recovery_index() {
    let (service, key_value_storage, owned_agent_id, number_of_shards, _) =
        assignment_tracking_test_service();
    let status = AgentStatusRecord {
        status: AgentStatus::Running,
        agent_mode: AgentMode::Ephemeral,
        ..AgentStatusRecord::default()
    };

    service
        .set_assignment_tracking(&owned_agent_id, invocation_index_fingerprint(), &status)
        .await
        .unwrap();

    assert!(
        assignment_tracking_members(&key_value_storage, &owned_agent_id, number_of_shards)
            .await
            .is_empty()
    );
}

#[test]
async fn deletion_without_current_oplog_cleans_only_the_requested_incarnation() {
    let (service, storage, owned_agent_id, number_of_shards, oplog_service) =
        assignment_tracking_test_service();
    let first = AgentFingerprint(Uuid::from_u128(1));
    let second = AgentFingerprint(Uuid::from_u128(2));
    let first_status = AgentStatusRecord {
        status: AgentStatus::Running,
        agent_mode: AgentMode::Durable,
        oplog_idx: OplogIndex::INITIAL,
        ..AgentStatusRecord::default()
    };
    let second_status = AgentStatusRecord {
        component_size: 7,
        ..first_status.clone()
    };
    for (fingerprint, status) in [(first, &first_status), (second, &second_status)] {
        service
            .write_split_status(
                &owned_agent_id,
                DefaultWorkerService::status_namespace(&owned_agent_id.agent_id, fingerprint),
                None,
                status.clone(),
            )
            .await
            .unwrap();
        service
            .set_assignment_tracking(&owned_agent_id, fingerprint, status)
            .await
            .unwrap();
        service
            .reject_periodic_snapshots(
                &owned_agent_id,
                fingerprint,
                &with_candidates(status.oplog_idx.into(), None),
                &HashSet::from([status.oplog_idx]),
            )
            .await
            .unwrap();
        storage
            .with_entity("test", "seed_invocation_index", "entry")
            .set_raw(
                DefaultWorkerService::invocation_result_index_namespace(
                    &owned_agent_id.agent_id,
                    fingerprint,
                ),
                "entry",
                b"value",
            )
            .await
            .unwrap();
    }
    service
        .write_cached_agent_mode(
            &owned_agent_id,
            &CachedAgentMode {
                agent_mode: AgentMode::Durable,
                fingerprint: second,
            },
        )
        .await
        .unwrap();

    // The second call is an in-process retry of the delete: it requests the delete of all
    // snapshots of the deleted incarnation again.
    let requested = std::sync::Mutex::new(Vec::new());
    let request = |fingerprint| requested.lock().unwrap().push(fingerprint);
    futures::StreamExt::for_each(futures::stream::iter(0..2), |_| async {
        service
            .remove(
                &mut crate::services::oplog::OpenOplogs::new("delete-test")
                    .lock_lifecycle(&owned_agent_id.agent_id)
                    .await,
                &owned_agent_id,
                AgentMode::Durable,
                first,
                None,
                &request,
            )
            .await
            .unwrap();
    })
    .await;
    assert_eq!(*requested.lock().unwrap(), vec![first, first]);

    // The retry finds no identity and deletes the oplog again: the first attempt may have
    // left archive storage behind.
    assert_eq!(oplog_service.delete_attempts.load(Ordering::Relaxed), 2);

    assert_eq!(
        service
            .read_cached_status(&owned_agent_id, first)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        service
            .read_cached_status(&owned_agent_id, second)
            .await
            .unwrap(),
        Some(second_status.clone())
    );
    assert!(
        storage
            .with_entity("test", "read_invocation_index", "entry")
            .get_raw(
                DefaultWorkerService::invocation_result_index_namespace(
                    &owned_agent_id.agent_id,
                    first,
                ),
                "entry",
            )
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        service
            .get_rejected_periodic_snapshots(&owned_agent_id, first)
            .await
            .unwrap(),
        HashSet::new()
    );
    assert_eq!(
        service
            .get_rejected_periodic_snapshots(&owned_agent_id, second)
            .await
            .unwrap(),
        HashSet::from([second_status.oplog_idx])
    );
    assert_eq!(
        service
            .read_cached_agent_mode(&owned_agent_id)
            .await
            .unwrap(),
        Some(CachedAgentMode {
            agent_mode: AgentMode::Durable,
            fingerprint: second,
        })
    );
    assert_eq!(
        assignment_tracking_members(&storage, &owned_agent_id, number_of_shards).await,
        vec![RunningWorker {
            owned_agent_id: owned_agent_id.clone(),
            fingerprint: second,
        }]
    );
    service
        .remove_cached_agent_mode_if_fingerprint(&owned_agent_id, second)
        .await
        .unwrap();
    assert_eq!(
        service
            .read_cached_agent_mode(&owned_agent_id)
            .await
            .unwrap(),
        None
    );
}

#[test]
fn tracks_workers_with_pending_invocations_for_assignment_recovery() {
    let mut status = AgentStatusRecord {
        status: AgentStatus::Idle,
        ..AgentStatusRecord::default()
    };
    status.pending_invocations.push(PendingInvocationRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::INITIAL,
        idempotency_key: None,
        manual_update_target_revision: Some(
            golem_common::model::component::ComponentRevision::INITIAL,
        ),
    });

    assert!(DefaultWorkerService::should_track_for_assignment_recovery(
        &status
    ));
}

#[test]
fn tracks_workers_with_pending_updates_for_assignment_recovery() {
    let status = AgentStatusRecord {
        status: AgentStatus::Idle,
        pending_updates: VecDeque::from([PendingUpdateRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::INITIAL,
            admission_index: OplogIndex::INITIAL,
            target_revision: golem_common::model::component::ComponentRevision::INITIAL,
            kind: PendingUpdateKind::Automatic,
        }]),
        ..AgentStatusRecord::default()
    };

    assert!(DefaultWorkerService::should_track_for_assignment_recovery(
        &status
    ));
}

#[test]
fn tracks_idle_worker_with_pending_caller_side_stream_cancellation() {
    use golem_common::model::durable_stream::{
        LocalStreamId, StreamCancelReason, StreamCancelRole, StreamConsumerCancelIntentRecord,
        StreamInvocationId, StreamRecordReference,
    };

    let mut status = AgentStatusRecord::default();
    status
        .pending_durable_stream_cancellations
        .insert(StreamConsumerCancelIntentRecord {
            format_version: 1,
            session_key: golem_common::model::durable_stream::StreamRegistrationInvocation::Remote(
                StreamInvocationId {
                    callee_environment_id: EnvironmentId::new(),
                    callee: AgentId {
                        component_id: ComponentId::new(),
                        agent_id: "remote".into(),
                    },
                    callee_fingerprint: AgentFingerprint(uuid::Uuid::new_v4()),
                    idempotency_key: IdempotencyKey::new("caller-side".into()),
                },
            ),
            consumer_invocation: IdempotencyKey::new("consumer".into()),
            source: StreamRecordReference::Local(LocalStreamId(OplogIndex::from_u64(2))),
            epoch: 1,
            role: StreamCancelRole::OutputConsumer,
            reason: StreamCancelReason::Cancelled,
            details: None,
        });

    assert!(status.durable_stream_sessions.iter().next().is_none());
    assert!(DefaultWorkerService::should_track_for_assignment_recovery(
        &status
    ));
    status.pending_durable_stream_cancellations.clear();
    assert!(!DefaultWorkerService::should_track_for_assignment_recovery(
        &status
    ));
}

#[test]
fn does_not_track_idle_workers_without_pending_work() {
    let status = AgentStatusRecord {
        status: AgentStatus::Idle,
        ..AgentStatusRecord::default()
    };

    assert!(!DefaultWorkerService::should_track_for_assignment_recovery(
        &status
    ));
}

/// Minimal oplog service for recovery and identity tests.
#[derive(Debug, Default)]
struct FakeOplogService {
    existing: Vec<OwnedAgentId>,
    /// Agents whose oplog does not read.
    unreadable: Vec<OwnedAgentId>,
    /// Agents whose oplog reads once, and then does not read.
    unreadable_after_first_read: Vec<OwnedAgentId>,
    reads: AtomicUsize,
    initial_entries: HashMap<(OwnedAgentId, AgentMode), OplogEntry>,
    fail_delete_once: AtomicBool,
    hide_identity_after_delete_attempt: bool,
    delete_attempts: AtomicUsize,
}

#[async_trait]
impl OplogService for FakeOplogService {
    async fn staged_exists(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _stage_id: uuid::Uuid,
    ) -> Result<bool, String> {
        unimplemented!()
    }

    async fn lock_lifecycle(&self, _: &AgentId) -> OplogLifecycleGuard {
        unreachable!()
    }

    fn set_stream_session_index(&self, _index: Arc<StreamSessionIndexService>) {}

    fn stream_session_index(&self) -> Option<Arc<StreamSessionIndexService>> {
        None
    }

    async fn create_fresh(
        &self,
        _lifecycle: &mut OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _initial_entry: OplogEntry,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        _shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog + 'static>, WorkerExecutorError> {
        unreachable!()
    }

    async fn create(
        &self,
        _lifecycle: &mut OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _initial_entry: OplogEntry,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        _shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog + 'static>, WorkerExecutorError> {
        unreachable!()
    }

    async fn open(
        &self,
        _lifecycle: &mut OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _last_oplog_index: Option<OplogIndex>,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        _shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog + 'static>, WorkerExecutorError> {
        unreachable!()
    }

    async fn get_last_index(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
    ) -> Result<OplogIndex, UnreadableOplog> {
        unreachable!()
    }

    async fn assert_owning_epoch(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _expected_epoch: golem_common::model::ShardEpoch,
    ) -> Result<(), crate::services::oplog::OplogError> {
        Ok(())
    }

    async fn delete(
        &self,
        _lifecycle: &mut OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _expected_epoch: Option<golem_common::model::ShardEpoch>,
    ) -> Result<(), crate::services::oplog::OplogError> {
        self.delete_attempts.fetch_add(1, Ordering::Relaxed);
        if self.fail_delete_once.swap(false, Ordering::Relaxed) {
            Err(crate::services::oplog::OplogError::Maintenance(
                "injected oplog delete failure".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    async fn read_exact(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _idx: OplogIndex,
        _n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        Ok(BTreeMap::new())
    }

    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        _idx: OplogIndex,
        _n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        if self.unreadable_after_first_read.contains(owned_agent_id)
            && self.reads.fetch_add(1, Ordering::Relaxed) > 0
        {
            return Err(UnreadableOplog {
                agent_id: owned_agent_id.agent_id(),
                error: crate::services::oplog::OplogReadError::corruption(
                    crate::services::oplog::OplogReadSource::Archive(1),
                    "failed to decode compressed oplog chunk ending at 2",
                ),
            });
        }
        if self.unreadable.contains(owned_agent_id) {
            return Err(UnreadableOplog {
                agent_id: owned_agent_id.agent_id(),
                error: crate::services::oplog::OplogReadError::corruption(
                    crate::services::oplog::OplogReadSource::Archive(1),
                    "failed to decode compressed oplog chunk ending at 2",
                ),
            });
        }
        if self.hide_identity_after_delete_attempt
            && self.delete_attempts.load(Ordering::Relaxed) > 0
        {
            return Ok(BTreeMap::new());
        }
        Ok(self
            .initial_entries
            .get(&(owned_agent_id.clone(), agent_mode))
            .cloned()
            .map(|entry| BTreeMap::from([(OplogIndex::INITIAL, entry)]))
            .unwrap_or_default())
    }

    async fn exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
    ) -> Result<bool, UnreadableOplog> {
        Ok(self.existing.contains(owned_agent_id))
    }

    async fn scan_for_component(
        &self,
        _environment_id: &EnvironmentId,
        _component_id: &ComponentId,
        _modes: Option<AgentMode>,
        _cursor: ScanCursor,
        _count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        unreachable!()
    }

    async fn upload_raw_payload(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        unreachable!()
    }

    async fn download_raw_payload(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _payload_id: PayloadId,
        _md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        unreachable!()
    }
}

#[derive(Debug)]
struct UnusedComponentService;

#[async_trait]
impl ComponentService for UnusedComponentService {
    async fn get(
        &self,
        _engine: &wasmtime::Engine,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
        unreachable!()
    }

    async fn get_metadata(
        &self,
        _component_id: ComponentId,
        _forced_revision: Option<ComponentRevision>,
    ) -> Result<Component, WorkerExecutorError> {
        unreachable!()
    }

    async fn resolve_component(
        &self,
        _component_reference: String,
        _resolving_environment: EnvironmentId,
        _resolving_application: ApplicationId,
        _resolving_account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        unreachable!()
    }

    async fn all_cached_metadata(&self) -> Vec<Component> {
        unreachable!()
    }

    async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {
        unreachable!()
    }
}

/// The error an unreachable cluster answers every operation with.
fn unreachable() -> KeyValueStorageError {
    KeyValueStorageError::NotAttempted("pool acquire timed out".to_string())
}

/// Key-value storage whose every operation fails, standing in for an unreachable cluster.
fn unreachable_storage() -> Arc<FaultInjectingKeyValueStorage> {
    let faults = KeyValueStorageFaults::default();
    faults.fail_all(usize::MAX, unreachable());
    Arc::new(FaultInjectingKeyValueStorage::new(
        Arc::new(InMemoryKeyValueStorage::new()),
        faults,
    ))
}

/// The snapshot deletes that a scan requests through `on_stale`.
#[derive(Default)]
struct StaleRequests(std::sync::Mutex<Vec<(OwnedAgentId, AgentFingerprint)>>);

impl StaleRequests {
    fn on_stale(&self) -> impl Fn(&OwnedAgentId, AgentFingerprint) + Send + Sync + '_ {
        |agent, fingerprint| {
            self.0.lock().unwrap().push((agent.clone(), fingerprint));
        }
    }

    fn take(&self) -> Vec<(OwnedAgentId, AgentFingerprint)> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

async fn running_members(storage: &InMemoryKeyValueStorage, shard_key: &str) -> Vec<RunningWorker> {
    storage
        .with_entity("worker", "enum", "agent_id")
        .members_of_set(KeyValueStorageNamespace::RunningWorkers, shard_key)
        .await
        .unwrap()
}

#[test]
fn a_member_is_stale_when_its_agent_has_no_oplog_or_another_incarnation() {
    let agent = test_owned_agent_id("classified");
    let (own, other) = (
        AgentFingerprint(Uuid::from_u128(1)),
        AgentFingerprint(Uuid::from_u128(2)),
    );
    let identity = |fingerprint| ResolvedAgentIdentity {
        agent_mode: AgentMode::Durable,
        fingerprint,
        create_entry: test_create_entry(&agent, AgentMode::Durable, fingerprint),
    };

    assert_eq!(
        [
            stale_reason(None, own),
            stale_reason(Some(&identity(own)), own),
            stale_reason(Some(&identity(other)), own),
        ],
        [
            Some(StaleReason::Absent),
            None,
            Some(StaleReason::FingerprintMismatch),
        ]
    );
}

/// A scan that fails after it removed a stale member has requested the delete of that
/// member's snapshots: no later scan can find the member again.
#[test]
async fn a_recovery_scan_that_fails_after_a_removal_has_requested_the_delete_of_the_removed_member()
{
    let storage = Arc::new(InMemoryKeyValueStorage::new());
    let shard_key = DefaultWorkerService::running_in_shard_key(&ShardId::new(0));
    let members = ["first-stale", "second-stale"].map(|name| RunningWorker {
        owned_agent_id: test_owned_agent_id(name),
        fingerprint: AgentFingerprint(Uuid::new_v4()),
    });
    futures::future::try_join_all(members.iter().map(|member| async {
        storage
            .with_entity("worker", "add", "agent_id")
            .add_to_set(KeyValueStorageNamespace::RunningWorkers, &shard_key, member)
            .await
    }))
    .await
    .unwrap();
    let faults = KeyValueStorageFaults::default();
    faults.fail_after("remove_stale", 1, 1, unreachable());
    let service = test_worker_service(
        Arc::new(FaultInjectingKeyValueStorage::new(storage.clone(), faults)),
        Arc::new(FakeOplogService::default()),
    );
    let requested = StaleRequests::default();

    let result = service
        .enum_workers_at_key(&shard_key, &requested.on_stale())
        .await;
    let remaining = running_members(&storage, &shard_key).await;
    let stored = members
        .iter()
        .map(|member| (member.owned_agent_id.clone(), member.fingerprint))
        .collect::<Vec<_>>();
    let removed = members
        .iter()
        .filter(|member| !remaining.contains(member))
        .map(|member| (member.owned_agent_id.clone(), member.fingerprint))
        .collect::<Vec<_>>();
    let requested = requested.take();

    assert!(
        result.is_err(),
        "the second removal fails the scan: {result:?}"
    );
    assert_eq!(removed.len(), 1);
    assert!(
        removed.iter().all(|member| requested.contains(member)),
        "a removed member was not requested: removed {removed:?}, requested {requested:?}"
    );
    assert!(
        requested.iter().all(|member| stored.contains(member)),
        "the scan requested an incarnation that no member names: {requested:?}"
    );
}

/// The restart path removes a stale member only after it requested the delete of its
/// snapshots, so a removal that fails leaves the member, already requested.
#[test]
async fn a_stale_member_whose_removal_fails_has_requested_its_snapshot_delete() {
    let storage = Arc::new(InMemoryKeyValueStorage::new());
    let shard_service = Arc::new(ShardServiceDefault::new());
    shard_service.install_unexpiring(1, &HashMap::new());
    let shard_key = DefaultWorkerService::running_in_shard_key(&ShardId::new(0));
    let member = RunningWorker {
        owned_agent_id: test_owned_agent_id("gone-before-the-restart"),
        fingerprint: AgentFingerprint(Uuid::new_v4()),
    };
    storage
        .with_entity("worker", "add", "agent_id")
        .add_to_set(
            KeyValueStorageNamespace::RunningWorkers,
            &shard_key,
            &member,
        )
        .await
        .unwrap();
    let faults = KeyValueStorageFaults::default();
    faults.fail("remove_stale", 1, unreachable());
    let service = DefaultWorkerService::new(
        Arc::new(FaultInjectingKeyValueStorage::new(storage.clone(), faults)),
        shard_service,
        Arc::new(FakeOplogService::default()),
        Arc::new(UnusedComponentService),
        Arc::new(GolemConfig::default()),
    );
    let requested = StaleRequests::default();

    let removed = service
        .remove_if_stale(
            &member.owned_agent_id,
            member.fingerprint,
            &requested.on_stale(),
        )
        .await;

    assert!(removed.is_err(), "the removal fails: {removed:?}");
    assert_eq!(
        (
            requested.take(),
            running_members(&storage, &shard_key).await
        ),
        (
            vec![(member.owned_agent_id.clone(), member.fingerprint)],
            vec![member]
        )
    );
}

#[test]
async fn a_member_of_the_stored_incarnation_is_not_stale() {
    let storage = Arc::new(InMemoryKeyValueStorage::new());
    let shard_service = Arc::new(ShardServiceDefault::new());
    shard_service.install_unexpiring(1, &HashMap::new());
    let shard_key = DefaultWorkerService::running_in_shard_key(&ShardId::new(0));
    let agent = test_owned_agent_id("still-here");
    let fingerprint = AgentFingerprint(Uuid::new_v4());
    let member = RunningWorker {
        owned_agent_id: agent.clone(),
        fingerprint,
    };
    storage
        .with_entity("worker", "add", "agent_id")
        .add_to_set(
            KeyValueStorageNamespace::RunningWorkers,
            &shard_key,
            &member,
        )
        .await
        .unwrap();
    let oplog = FakeOplogService {
        initial_entries: HashMap::from([(
            (agent.clone(), AgentMode::Durable),
            test_create_entry(&agent, AgentMode::Durable, fingerprint),
        )]),
        ..FakeOplogService::default()
    };
    let service = DefaultWorkerService::new(
        storage.clone(),
        shard_service,
        Arc::new(oplog),
        Arc::new(UnusedComponentService),
        Arc::new(GolemConfig::default()),
    );
    let requested = StaleRequests::default();

    let removed = service
        .remove_if_stale(&agent, fingerprint, &requested.on_stale())
        .await
        .unwrap();

    assert_eq!(
        (
            removed,
            requested.take(),
            running_members(&storage, &shard_key).await
        ),
        (false, Vec::new(), vec![member])
    );
}

fn test_owned_agent_id(name: &str) -> OwnedAgentId {
    OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: name.to_string(),
        },
    )
}

fn test_create_entry(
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    fingerprint: AgentFingerprint,
) -> OplogEntry {
    OplogEntry::create(Box::new(golem_common::model::oplog::CreateParameters {
        agent_id: owned_agent_id.agent_id.clone(),
        owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
        agent_mode,
        component_revision: ComponentRevision::INITIAL,
        env: Vec::new(),
        environment_id: owned_agent_id.environment_id,
        created_by: AccountId::new(),
        parent: None,
        component_size: 0,
        initial_total_linear_memory_size: 0,
        initial_active_plugins: HashSet::new(),
        local_agent_config: Vec::new(),
        original_phantom_id: None,
        instance_id: fingerprint.0,
    }))
}

fn test_worker_service(
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    oplog_service: Arc<dyn OplogService>,
) -> DefaultWorkerService {
    let shard_service = Arc::new(ShardServiceDefault::new());
    shard_service.install_unexpiring(1, &HashMap::from([(ShardId::new(0), ShardEpoch(0))]));
    DefaultWorkerService::new(
        key_value_storage,
        shard_service,
        oplog_service,
        Arc::new(UnusedComponentService),
        Arc::new(GolemConfig::default()),
    )
}

#[test]
async fn recovery_scan_skips_workers_whose_oplog_is_gone_and_names_them_as_stale() {
    let storage = Arc::new(InMemoryKeyValueStorage::new());
    let shard_key = DefaultWorkerService::running_in_shard_key(&ShardId::new(0));
    let deleted = test_owned_agent_id("deleted-by-a-racing-delete");
    let deleted_fingerprint = AgentFingerprint(Uuid::new_v4());
    let deleted_member = RunningWorker {
        owned_agent_id: deleted.clone(),
        fingerprint: deleted_fingerprint,
    };
    storage
        .with_entity("worker", "add", "agent_id")
        .add_to_set(
            KeyValueStorageNamespace::RunningWorkers,
            &shard_key,
            &deleted_member,
        )
        .await
        .unwrap();

    let service = test_worker_service(storage.clone(), Arc::new(FakeOplogService::default()));

    // The index entry outlived the worker; recovery isolates that instead of aborting.
    let requested = StaleRequests::default();
    let workers = service
        .enum_workers_at_key(&shard_key, &requested.on_stale())
        .await
        .unwrap();
    assert!(workers.is_empty());
    assert_eq!(requested.take(), vec![(deleted, deleted_fingerprint)]);
    let remaining: Vec<RunningWorker> = storage
        .with_entity("worker", "enum", "agent_id")
        .members_of_set(KeyValueStorageNamespace::RunningWorkers, &shard_key)
        .await
        .unwrap();
    assert!(remaining.is_empty());
}

#[test]
async fn deletion_identity_classification_does_not_mutate_an_unrelated_hint() {
    let storage = Arc::new(InMemoryKeyValueStorage::new());
    let owned_agent_id = test_owned_agent_id("ephemeral-replacement");
    let current = AgentFingerprint(Uuid::from_u128(2));
    let unrelated_hint = AgentFingerprint(Uuid::from_u128(3));
    let oplog = FakeOplogService {
        initial_entries: HashMap::from([(
            (owned_agent_id.clone(), AgentMode::Ephemeral),
            test_create_entry(&owned_agent_id, AgentMode::Ephemeral, current),
        )]),
        ..FakeOplogService::default()
    };
    let service = test_worker_service(storage, Arc::new(oplog));
    let hint = CachedAgentMode {
        agent_mode: AgentMode::Durable,
        fingerprint: unrelated_hint,
    };
    service
        .write_cached_agent_mode(&owned_agent_id, &hint)
        .await
        .unwrap();

    let identity = service
        .read_agent_identity_for_deletion(&owned_agent_id, AgentMode::Durable)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(identity.agent_mode, AgentMode::Ephemeral);
    assert_eq!(identity.fingerprint, current);
    assert_eq!(
        service
            .read_cached_agent_mode(&owned_agent_id)
            .await
            .unwrap(),
        Some(hint)
    );
}

#[test]
async fn stale_recovery_cleanup_removes_only_the_observed_incarnation() {
    let storage = Arc::new(InMemoryKeyValueStorage::new());
    let shard_key = DefaultWorkerService::running_in_shard_key(&ShardId::new(0));
    let owned_agent_id = test_owned_agent_id("recreated");
    let stale = RunningWorker {
        owned_agent_id: owned_agent_id.clone(),
        fingerprint: AgentFingerprint(Uuid::from_u128(1)),
    };
    let current = RunningWorker {
        owned_agent_id,
        fingerprint: AgentFingerprint(Uuid::from_u128(2)),
    };
    for member in [&stale, &current] {
        storage
            .with_entity("worker", "add", "agent_id")
            .add_to_set(KeyValueStorageNamespace::RunningWorkers, &shard_key, member)
            .await
            .unwrap();
    }
    let service = test_worker_service(storage.clone(), Arc::new(FakeOplogService::default()));

    service
        .remove_running_worker_member(&shard_key, &stale)
        .await
        .unwrap();

    let remaining: Vec<RunningWorker> = storage
        .with_entity("worker", "enum", "agent_id")
        .members_of_set(KeyValueStorageNamespace::RunningWorkers, &shard_key)
        .await
        .unwrap();
    assert_eq!(remaining, vec![current]);
}

/// An agent whose oplog does not read is left as it is, in the index and in storage, and the
/// scan goes on to the other members: no retry of the scan would read that oplog.
#[test]
async fn recovery_scan_leaves_an_agent_whose_oplog_does_not_read_and_goes_on() {
    let storage = Arc::new(InMemoryKeyValueStorage::new());
    let shard_key = DefaultWorkerService::running_in_shard_key(&ShardId::new(0));
    let unreadable = RunningWorker {
        owned_agent_id: test_owned_agent_id("unreadable-archive"),
        fingerprint: AgentFingerprint(Uuid::new_v4()),
    };
    let deleted = RunningWorker {
        owned_agent_id: test_owned_agent_id("deleted-by-a-racing-delete"),
        fingerprint: AgentFingerprint(Uuid::new_v4()),
    };
    for member in [&unreadable, &deleted] {
        storage
            .with_entity("worker", "add", "agent_id")
            .add_to_set(KeyValueStorageNamespace::RunningWorkers, &shard_key, member)
            .await
            .unwrap();
    }
    let service = test_worker_service(
        storage.clone(),
        Arc::new(FakeOplogService {
            unreadable: vec![unreadable.owned_agent_id.clone()],
            ..FakeOplogService::default()
        }),
    );

    let requested = StaleRequests::default();
    let workers = service
        .enum_workers_at_key(&shard_key, &requested.on_stale())
        .await
        .unwrap();
    assert!(workers.is_empty());
    assert_eq!(
        requested.take(),
        vec![(deleted.owned_agent_id, deleted.fingerprint)]
    );
    let remaining: Vec<RunningWorker> = storage
        .with_entity("worker", "enum", "agent_id")
        .members_of_set(KeyValueStorageNamespace::RunningWorkers, &shard_key)
        .await
        .unwrap();
    assert_eq!(remaining, vec![unreadable]);
}

#[test]
async fn recovery_scan_leaves_an_agent_whose_metadata_read_finds_the_oplog_failed() {
    let storage = Arc::new(InMemoryKeyValueStorage::new());
    let shard_key = DefaultWorkerService::running_in_shard_key(&ShardId::new(0));
    let agent = test_owned_agent_id("unreadable-after-identity");
    let fingerprint = AgentFingerprint(Uuid::new_v4());
    let member = RunningWorker {
        owned_agent_id: agent.clone(),
        fingerprint,
    };
    storage
        .with_entity("worker", "add", "agent_id")
        .add_to_set(
            KeyValueStorageNamespace::RunningWorkers,
            &shard_key,
            &member,
        )
        .await
        .unwrap();
    let service = test_worker_service(
        storage.clone(),
        Arc::new(FakeOplogService {
            initial_entries: HashMap::from([(
                (agent.clone(), AgentMode::Durable),
                test_create_entry(&agent, AgentMode::Durable, fingerprint),
            )]),
            unreadable_after_first_read: vec![agent.clone()],
            ..FakeOplogService::default()
        }),
    );

    let requested = StaleRequests::default();
    let workers = service
        .enum_workers_at_key(&shard_key, &requested.on_stale())
        .await
        .unwrap();

    assert!(workers.is_empty());
    assert_eq!(requested.take(), Vec::new());
    assert_eq!(running_members(&storage, &shard_key).await, vec![member]);
}

/// The distinction that matters more than the skip: an entry we could not *read* is not an
/// entry that is gone. Skipping it would leave a running agent suspended for as long as this
/// executor owns the shard, and would do it silently.
#[test]
async fn recovery_scan_fails_rather_than_stranding_a_worker_it_cannot_read() {
    let shard_key = DefaultWorkerService::running_in_shard_key(&ShardId::new(0));
    let stranded = test_owned_agent_id("running-but-unreadable");
    let stranded_member = RunningWorker {
        owned_agent_id: stranded,
        fingerprint: AgentFingerprint(Uuid::new_v4()),
    };
    // The index itself still answers; every read of the worker behind it does not.
    let storage = Arc::new(InMemoryKeyValueStorage::new());
    storage
        .with_entity("worker", "add", "agent_id")
        .add_to_set(
            KeyValueStorageNamespace::RunningWorkers,
            &shard_key,
            &stranded_member,
        )
        .await
        .unwrap();
    let faults = KeyValueStorageFaults::default();
    faults.fail_all_except(&["enum"], unreachable());

    let service = test_worker_service(
        Arc::new(FaultInjectingKeyValueStorage::new(storage, faults)),
        Arc::new(FakeOplogService::default()),
    );

    let requested = StaleRequests::default();
    let result = service
        .enum_workers_at_key(&shard_key, &requested.on_stale())
        .await;

    assert!(
        result.is_err(),
        "an unreadable worker must fail the scan, not be skipped: {result:?}"
    );
    assert_eq!(requested.take(), Vec::new());
}

#[test]
async fn delete_reports_an_unreachable_storage() {
    let service = test_worker_service(unreachable_storage(), Arc::new(FakeOplogService::default()));
    let owned_agent_id = test_owned_agent_id("doomed");

    assert!(
        service
            .remove_cached_status(&owned_agent_id, AgentFingerprint(Uuid::new_v4()))
            .await
            .is_err(),
        "expected the cached status delete failure to surface"
    );
    let requested = std::sync::atomic::AtomicBool::new(false);
    assert!(
        service
            .remove(
                &mut crate::services::oplog::OpenOplogs::new("delete-test")
                    .lock_lifecycle(&owned_agent_id.agent_id)
                    .await,
                &owned_agent_id,
                AgentMode::Durable,
                AgentFingerprint(Uuid::new_v4()),
                None,
                &|_| requested.store(true, std::sync::atomic::Ordering::SeqCst),
            )
            .await
            .is_err(),
        "expected the delete failure to surface"
    );
    // A delete that fails before the oplog delete requests no delete of snapshots.
    assert!(!requested.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
async fn oplog_delete_failure_is_returned_and_explicit_remove_retry_completes() {
    let owned_agent_id = test_owned_agent_id("retry-oplog-delete");
    let fingerprint = AgentFingerprint(Uuid::new_v4());
    let oplog_service = Arc::new(FakeOplogService {
        initial_entries: HashMap::from([(
            (owned_agent_id.clone(), AgentMode::Durable),
            test_create_entry(&owned_agent_id, AgentMode::Durable, fingerprint),
        )]),
        fail_delete_once: AtomicBool::new(true),
        hide_identity_after_delete_attempt: true,
        ..FakeOplogService::default()
    });
    let service = test_worker_service(
        Arc::new(InMemoryKeyValueStorage::new()),
        oplog_service.clone(),
    );
    let lifecycle_locks = crate::services::oplog::OpenOplogs::new("delete-test");
    let mut lifecycle = lifecycle_locks
        .lock_lifecycle(&owned_agent_id.agent_id)
        .await;

    let error = service
        .remove(
            &mut lifecycle,
            &owned_agent_id,
            AgentMode::Durable,
            fingerprint,
            None,
            &|_| {},
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("injected oplog delete failure"));

    service
        .remove(
            &mut lifecycle,
            &owned_agent_id,
            AgentMode::Durable,
            fingerprint,
            None,
            &|_| {},
        )
        .await
        .unwrap();
    assert_eq!(oplog_service.delete_attempts.load(Ordering::Relaxed), 2);
}

#[test]
async fn recovery_scan_reports_an_unreadable_index() {
    let service = test_worker_service(unreachable_storage(), Arc::new(FakeOplogService::default()));

    let result = service
        .enum_workers_at_key(
            &DefaultWorkerService::running_in_shard_key(&ShardId::new(0)),
            &|_, _| {},
        )
        .await;

    assert!(
        result.is_err(),
        "expected the index read failure to surface"
    );
}
