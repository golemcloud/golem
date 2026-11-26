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
use crate::services::oplog::sealed_reader::read_sealed;
use crate::services::oplog::tests::ReadCountingBlobStorage;
use crate::services::oplog::{
    BlobOplogArchiveService, CompressedOplogArchiveService, MultiLayerOplogService,
    OplogArchiveService, OplogService, PrimaryOplogService,
};
use crate::services::shard::ShardServiceDefault;
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use crate::storage::indexed::sqlite::SqliteIndexedStorage;
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use golem_common::config::DbSqliteConfig;
use golem_common::model::account::AccountId;
use golem_common::model::oplog::OplogEntry;
use golem_common::model::{
    AutomaticSnapshot, ScheduleId, ShardId, ShardLeaseClock, ShardLeaseRevision, SnapshotFiles,
    Timestamp, UsableAutomaticSnapshot,
};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use test_r::test;

mod deletion_tests;
mod scan_tests;

#[test]
async fn activation_reselects_a_newer_record_after_the_epoch_claim() {
    use crate::services::oplog::tests::{default_execution_status, default_last_known_status};

    let mut f = Fixture::new(Arc::new(InMemoryIndexedStorage::new())).await;
    let stale_seal = f.service();
    let pending = stale_seal
        .prepare(f.metadata.clone(), ShardEpoch(6))
        .await
        .unwrap()
        .unwrap();
    stale_seal.remove_moved_copies(&pending).await.unwrap();
    assert!(stale_seal.release(&pending).await.unwrap());
    let attempt = stale_seal
        .begin_read_attempt(&f.id(), Some(AgentMode::Durable))
        .await
        .unwrap();
    let stale_oplog = f.oplog_service().await;
    let requester_shard = f.shard.clone();

    // A new executor owns epoch 7 while the old executor retains its unexpired local grant.
    f.shard = Arc::new(ShardServiceDefault::new());
    f.register(7, true);
    let current = f.service();
    current.unseal(&f.id(), ShardEpoch(7)).await.unwrap();
    let archive = f.archive(0);
    let handle = archive
        .open(&f.id(), AgentMode::Durable, None, f.metadata.fingerprint)
        .await
        .unwrap();
    let appended = OplogEntry::Exited {
        timestamp: Timestamp::from(10000),
    };
    handle
        .append(&[(OplogIndex::from_u64(10), appended.clone())])
        .await
        .unwrap();
    f.metadata.last_known_status.oplog_idx = OplogIndex::from_u64(10);
    let newer = current
        .prepare(f.metadata.clone(), ShardEpoch(7))
        .await
        .unwrap()
        .unwrap();
    current.remove_moved_copies(&newer).await.unwrap();
    assert!(current.release(&newer).await.unwrap());
    assert_eq!(
        current
            .record_in_mode(&f.id(), AgentMode::Durable)
            .await
            .unwrap()
            .unwrap()
            .last_index,
        OplogIndex::from_u64(10)
    );
    assert!(stale_seal.deadline(&f.id(), ShardEpoch(6)).is_some());
    f.shard = requester_shard;
    f.register(8, true);

    let mut lifecycle = stale_oplog.lock_lifecycle(&f.id().agent_id).await;
    lifecycle.set_read_attempt(Some(Arc::new(attempt)));
    let opened = stale_oplog
        .open(
            &mut lifecycle,
            &f.id(),
            AgentMode::Durable,
            None,
            pending.record.metadata.clone(),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(ShardEpoch(8)),
        )
        .await;
    assert!(
        matches!(opened, Err(ref error) if OplogReadAttempt::is_invalidated_error(error)),
        "the obsolete read selection must be discarded after the claim"
    );
    assert!(
        !f.indexed
            .observe_agent("test", "after_invalidated", &f.id())
            .await
            .unwrap()
            .has_data
    );

    let selected = stale_seal
        .begin_read_attempt(&f.id(), Some(AgentMode::Durable))
        .await
        .unwrap();
    assert_eq!(
        selected.record().unwrap().writer_nonce,
        newer.record.writer_nonce
    );
    let metadata = selected.record().unwrap().metadata.clone();
    lifecycle.set_read_attempt(Some(Arc::new(selected)));
    let opened = stale_oplog
        .open(
            &mut lifecycle,
            &f.id(),
            AgentMode::Durable,
            None,
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            Some(ShardEpoch(8)),
        )
        .await
        .unwrap();
    assert!(opened.fence().is_none());
    assert_eq!(opened.current_oplog_index().await, OplogIndex::from_u64(10));
    let mut expected = f.original.clone();
    expected.insert(OplogIndex::from_u64(10), appended);
    let actual = opened.read_exact(OplogIndex::INITIAL, 10).await.unwrap();
    assert_eq!(actual, expected);
}

#[test]
async fn read_attempt_keeps_one_selected_source_until_authority_changes() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let f = Fixture::with_mode(Arc::new(InMemoryIndexedStorage::new()), mode).await;
        let seal = f.service();
        let service = f.oplog_service().await;
        f.blobs.reset();
        let present = seal.begin_read_attempt(&f.id(), Some(mode)).await.unwrap();
        assert!(present.is_present());
        assert!(present.record().is_none());
        assert_eq!(f.blobs.seal_lists().len(), 1);
        assert_eq!(
            present
                .read_exact(&service, &f.id(), mode, OplogIndex::INITIAL, 9)
                .await
                .unwrap(),
            f.original
        );
        assert!(seal.validate_read_attempt(&f.id(), &present).await.unwrap());
        assert_eq!(f.blobs.seal_lists().len(), 1);

        f.reject(&[5, 8]).await;
        let pending = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        seal.remove_moved_copies(&pending).await.unwrap();
        assert!(seal.release(&pending).await.unwrap());
        assert!(!seal.validate_read_attempt(&f.id(), &present).await.unwrap());

        f.blobs.reset();
        let frozen = seal.begin_read_attempt(&f.id(), Some(mode)).await.unwrap();
        assert!(!frozen.is_present());
        assert!(!frozen.is_absent());
        assert_eq!(
            frozen.record().unwrap().writer_nonce,
            pending.record.writer_nonce
        );
        assert_eq!(f.blobs.seal_lists().len(), 1);
        assert_eq!(
            frozen
                .get_last_index(&service, &f.id(), mode)
                .await
                .unwrap(),
            OplogIndex::from_u64(9)
        );
        assert_eq!(
            frozen
                .read_exact(&service, &f.id(), mode, OplogIndex::INITIAL, 9)
                .await
                .unwrap(),
            f.original
        );
        assert_eq!(
            seal.moved_rejections_for_attempt(&f.id(), f.metadata.fingerprint, &frozen)
                .await
                .unwrap(),
            BTreeSet::from([OplogIndex::from_u64(5), OplogIndex::from_u64(8)])
        );
        assert!(
            seal.moved_rejections_for_attempt(&f.id(), AgentFingerprint(Uuid::new_v4()), &frozen)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(seal.validate_read_attempt(&f.id(), &frozen).await.unwrap());
        assert_eq!(f.blobs.seal_lists().len(), 1);
    }
}

#[test]
async fn absent_read_attempt_validation_does_not_list_again() {
    let f = Fixture::new(Arc::new(InMemoryIndexedStorage::new())).await;
    let seal = f.service();
    let id = OwnedAgentId::new(f.id().environment_id(), &AgentId::from_parts(7, 9999));
    for (mode, lists) in [
        (Some(AgentMode::Durable), 1),
        (Some(AgentMode::Ephemeral), 1),
        (None, 2),
    ] {
        f.blobs.reset();
        let attempt = seal.begin_read_attempt(&id, mode).await.unwrap();
        assert!(attempt.is_absent());
        assert!(!attempt.is_present());
        assert!(attempt.record().is_none());
        assert!(seal.validate_read_attempt(&id, &attempt).await.unwrap());
        assert_eq!(f.blobs.seal_lists().len(), lists);
    }
}

#[test]
async fn mode_addressed_records_list_only_the_requested_namespace() {
    let f = Fixture::new(Arc::new(InMemoryIndexedStorage::new())).await;
    let seal = f.service();
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        f.blobs.reset();
        assert!(seal.record_in_mode(&f.id(), mode).await.unwrap().is_none());
        assert_eq!(f.blobs.reads(), 1, "one LIST for a known mode");
    }
    f.blobs.reset();
    assert!(seal.record(&f.id()).await.unwrap().is_none());
    assert_eq!(f.blobs.reads(), 2, "missing identity probes both modes");

    let missing = OwnedAgentId::new(f.id().environment_id(), &AgentId::from_parts(7, 9999));
    for (mode, lists) in [
        (Some(AgentMode::Durable), 1),
        (Some(AgentMode::Ephemeral), 1),
        (None, 2),
    ] {
        f.blobs.reset();
        assert!(
            seal.read_with_authority(
                &missing,
                mode,
                || async { Ok::<_, OplogArchiveError>(false) },
                |_| async { Ok(false) },
                || Ok(true),
                std::convert::identity,
            )
            .await
            .unwrap()
        );
        assert_eq!(f.blobs.reads(), lists, "authority lookup for {mode:?}");
    }

    let pending = seal
        .prepare(f.metadata.clone(), ShardEpoch(6))
        .await
        .unwrap()
        .unwrap();
    let mut ephemeral = pending.record.clone();
    ephemeral.agent_mode = AgentMode::Ephemeral;
    ephemeral.metadata.agent_mode = AgentMode::Ephemeral;
    ephemeral.writer_nonce = Uuid::new_v4();
    f.blobs
        .with("test", "other_mode")
        .put_raw(
            seal_namespace(&f.id(), AgentMode::Ephemeral),
            &pending.key.path(),
            &serialize(&ephemeral).unwrap(),
        )
        .await
        .unwrap();
    for (mode, nonce) in [
        (AgentMode::Durable, pending.record.writer_nonce),
        (AgentMode::Ephemeral, ephemeral.writer_nonce),
    ] {
        f.blobs.reset();
        assert_eq!(
            seal.record_in_mode(&f.id(), mode)
                .await
                .unwrap()
                .unwrap()
                .writer_nonce,
            nonce
        );
        assert_eq!(f.blobs.reads(), 2, "one LIST and one GET for a known mode");
    }
    f.blobs.reset();
    assert_eq!(
        seal.record(&f.id()).await.unwrap().unwrap().writer_nonce,
        pending.record.writer_nonce
    );
    assert_eq!(f.blobs.reads(), 2, "unknown mode selects Durable first");
    f.blobs
        .with("test", "remove_durable")
        .delete(
            seal_namespace(&f.id(), AgentMode::Durable),
            &pending.key.path(),
        )
        .await
        .unwrap();
    f.blobs.reset();
    assert_eq!(
        seal.record(&f.id()).await.unwrap().unwrap().writer_nonce,
        ephemeral.writer_nonce
    );
    assert_eq!(f.blobs.reads(), 3, "two LISTs and one GET for Ephemeral");
}

#[test]
async fn successful_seal_uses_two_lists_in_both_modes_including_partial_release_retry() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        for partial in [false, true] {
            let f = Fixture::with_mode(Arc::new(InMemoryIndexedStorage::new()), mode).await;
            f.put_promise(3, Some(vec![3, 9])).await;
            f.reject(&[5, 8]).await;
            let seal = f.service();
            if partial {
                let first = seal
                    .prepare(f.metadata.clone(), ShardEpoch(6))
                    .await
                    .unwrap()
                    .unwrap();
                seal.remove_moved_copies(&first).await.unwrap();
            }
            f.blobs.reset();
            let pending = seal
                .prepare(f.metadata.clone(), ShardEpoch(6))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                pending.record.completed_promises,
                BTreeMap::from([(OplogIndex::from_u64(3), vec![3, 9])])
            );
            assert_eq!(
                pending.record.rejected_periodic_snapshots,
                BTreeSet::from([OplogIndex::from_u64(5), OplogIndex::from_u64(8)])
            );
            seal.remove_moved_copies(&pending).await.unwrap();
            assert!(seal.release(&pending).await.unwrap());
            let mut calls = f.blobs.seal_lists();
            calls.sort_by_key(|(_, target, operation)| (*target, *operation));
            assert_eq!(
                calls,
                vec![
                    (mode, "oplog_seal", "prepare"),
                    (mode, "oplog_seal", "verify_record"),
                ],
                "successful {mode:?} seal (partial retry: {partial})"
            );
            let remaining = list_records(f.blobs.as_ref(), &f.id(), mode, "test")
                .await
                .unwrap();
            assert_eq!(remaining.len(), 1);
            assert_eq!(remaining[0].key, pending.key);
            f.assert_history(&pending.record).await;
        }
    }
}

#[derive(Default)]
struct Scheduler {
    actions: Mutex<Vec<ScheduledAction>>,
}

#[async_trait::async_trait]
impl SchedulerService for Scheduler {
    fn is_durable(&self) -> bool {
        true
    }

    async fn schedule(&self, time: jiff::Timestamp, action: ScheduledAction) -> ScheduleId {
        self.schedule_with_id(ScheduleId::fresh(), time, action)
            .await
    }

    async fn schedule_with_id(
        &self,
        id: ScheduleId,
        _time: jiff::Timestamp,
        action: ScheduledAction,
    ) -> ScheduleId {
        self.actions.lock().unwrap().push(action);
        id
    }

    async fn cancel(&self, _id: ScheduleId) {}
}

struct Fixture {
    indexed: Arc<dyn IndexedStorage + Send + Sync>,
    blobs: Arc<ReadCountingBlobStorage>,
    kv: Arc<InMemoryKeyValueStorage>,
    shard: Arc<ShardServiceDefault>,
    scheduler: Arc<Scheduler>,
    metadata: AgentMetadata,
    config: OplogConfig,
    original: BTreeMap<OplogIndex, OplogEntry>,
}

impl Fixture {
    async fn new(indexed: Arc<dyn IndexedStorage + Send + Sync>) -> Self {
        Self::with_mode(indexed, AgentMode::Durable).await
    }

    async fn with_mode(indexed: Arc<dyn IndexedStorage + Send + Sync>, mode: AgentMode) -> Self {
        let mut metadata = crate::services::oplog::tests::make_agent_metadata(
            AgentId::from_parts(7, 2125),
            AccountId::new(),
            EnvironmentId::new(),
        );
        metadata.agent_mode = mode;
        metadata.created_at = metadata.created_at.rounded();
        metadata.last_known_status.oplog_idx = OplogIndex::from_u64(9);
        metadata.last_known_status.last_automatic_snapshot = Some(AutomaticSnapshot {
            index: OplogIndex::from_u64(8),
            timestamp: Timestamp::from(8000),
            component_revision: metadata.last_known_status.component_revision,
            files: SnapshotFiles::Unnamed,
            bootstrap: None,
        });
        metadata
            .last_known_status
            .previous_usable_automatic_snapshot = Some(UsableAutomaticSnapshot {
            index: OplogIndex::from_u64(5),
            component_revision: metadata.last_known_status.component_revision,
            filesystem_snapshot: None,
            bootstrap: None,
        });
        let initial = Self::initial_entry(&metadata);
        let fixture = Self {
            indexed,
            blobs: Arc::new(ReadCountingBlobStorage::new()),
            kv: Arc::new(InMemoryKeyValueStorage::new()),
            shard: Arc::new(ShardServiceDefault::new()),
            scheduler: Arc::new(Scheduler::default()),
            metadata,
            config: OplogConfig {
                indexed_storage_layers: 2,
                blob_storage_layers: 2,
                ..Default::default()
            },
            original: (1..=9)
                .map(|index| {
                    (
                        OplogIndex::from_u64(index),
                        if index == 1 {
                            initial.clone()
                        } else {
                            OplogEntry::Suspend {
                                timestamp: Timestamp::from(index * 1000),
                            }
                        },
                    )
                })
                .collect(),
        };
        fixture.register(6, true);
        if mode == AgentMode::Durable {
            fixture
                .service()
                .claim(&fixture.id(), ShardEpoch(6))
                .await
                .unwrap();
        }
        // Real archive writes create the chunk objects and their indexed manifests.
        for (level, first, last) in [(1, 1, 3), (1, 4, 6), (0, 7, 9)] {
            let archive = fixture.archive(level);
            let handle = archive
                .open(&fixture.id(), mode, None, fixture.metadata.fingerprint)
                .await
                .unwrap();
            let entries: Vec<_> = fixture
                .original
                .range(OplogIndex::from_u64(first)..=OplogIndex::from_u64(last))
                .map(|(index, entry)| (*index, entry.clone()))
                .collect();
            handle.append(&entries).await.unwrap();
        }
        if mode == AgentMode::Ephemeral {
            assert!(
                fixture
                    .service()
                    .claim_ephemeral(&fixture.id(), ShardEpoch(6))
                    .await
                    .unwrap()
            );
        }
        fixture
    }

    fn initial_entry(metadata: &AgentMetadata) -> OplogEntry {
        use golem_common::model::oplog::CreateParameters;
        OplogEntry::Create {
            timestamp: metadata.created_at,
            parameters: Box::new(CreateParameters {
                component_id: metadata.component_id,
                agent_type_name: metadata.agent_type_name.clone(),
                canonical_key: metadata.canonical_key.clone(),
                constructor_parameters: metadata.constructor_parameters.clone(),
                agent_id: metadata.agent_id,
                owner_kind: metadata.owner_kind,
                agent_mode: metadata.agent_mode,
                component_revision: metadata.last_known_status.component_revision,
                env: metadata.env.clone(),
                environment_id: metadata.environment_id,
                created_by: metadata.created_by,
                parent: metadata.parent,
                component_size: 0,
                initial_total_linear_memory_size: 0,
                initial_active_plugins: Default::default(),
                local_agent_config: Vec::new(),
                instance_id: metadata.fingerprint.0,
                create_request: metadata.create_request.clone(),
                never_live: false,
                standalone_tool: metadata.standalone_tool.clone(),
            }),
        }
    }

    fn id(&self) -> OwnedAgentId {
        self.metadata.owned_agent_id()
    }

    fn service(&self) -> SealService {
        let scheduler: Arc<dyn SchedulerService> = self.scheduler.clone();
        SealService::new(
            self.indexed.clone(),
            self.blobs.clone(),
            self.kv.clone(),
            self.shard.clone(),
            Arc::downgrade(&scheduler),
            self.config.clone(),
            Default::default(),
        )
    }

    fn archive(&self, level: usize) -> BlobOplogArchiveService {
        BlobOplogArchiveService::new(
            self.blobs.clone(),
            self.indexed.clone(),
            level,
            Default::default(),
        )
    }

    async fn oplog_service(&self) -> MultiLayerOplogService {
        let primary = Arc::new(
            PrimaryOplogService::new(
                self.indexed.clone(),
                self.blobs.clone(),
                100,
                100,
                1024,
                32,
                Default::default(),
            )
            .await,
        );
        let lower: Vec<Arc<dyn OplogArchiveService>> = vec![
            Arc::new(CompressedOplogArchiveService::new(
                self.indexed.clone(),
                1,
                Default::default(),
            )),
            Arc::new(self.archive(0)),
            Arc::new(self.archive(1)),
        ];
        let service = MultiLayerOplogService::new(
            primary,
            nonempty_collections::NEVec::try_from_vec(lower).unwrap(),
            100,
            100,
        );
        service.set_seal_service(Arc::new(self.service()));
        service
    }

    fn register(&self, epoch: u64, expiring: bool) {
        self.shard.register(
            1,
            &HashMap::from([(ShardId::new(0), ShardEpoch(epoch))]),
            expiring.then(|| ShardLeaseClock {
                sent_at: Instant::now(),
                wall_anchor: SystemTime::now(),
                ttl: Duration::from_secs(60),
            }),
            ShardLeaseRevision {
                incarnation: Uuid::from_u128(2125),
                number: epoch,
            },
            std::num::NonZeroUsize::MIN,
        );
    }

    fn promise(&self, index: u64) -> PromiseId {
        PromiseId {
            agent_id: self.id().agent_id,
            fingerprint: self.metadata.fingerprint,
            oplog_idx: OplogIndex::from_u64(index),
        }
    }

    async fn put_promise(&self, index: u64, data: Option<Vec<u8>>) {
        let namespace = promise_namespace_of(&self.id(), self.metadata.fingerprint);
        let key = self.promise(index).storage_key(self.id().environment_id());
        self.kv
            .with_entity("test", "promise", "promise")
            .set(namespace.clone(), &key, &RedisPromiseState::Pending)
            .await
            .unwrap();
        if let Some(data) = data {
            self.kv
                .with_entity("test", "complete", "promise")
                .set(
                    namespace,
                    &format!("{key}:completed"),
                    &RedisPromiseState::Complete(data),
                )
                .await
                .unwrap();
        }
    }

    async fn reject(&self, indexes: &[u64]) {
        let indexes: Vec<_> = indexes.iter().copied().map(OplogIndex::from_u64).collect();
        self.kv
            .with_entity("test", "reject", "snapshot")
            .set(
                KeyValueStorageNamespace::AgentRejectedPeriodicSnapshots {
                    owned_agent_id: self.id(),
                },
                &self.metadata.fingerprint.0.to_string(),
                &indexes,
            )
            .await
            .unwrap();
    }

    async fn assert_history(&self, record: &SealRecord) {
        assert_eq!(
            read_sealed(
                self.blobs.as_ref(),
                &self.id(),
                record,
                OplogIndex::INITIAL,
                9
            )
            .await
            .unwrap(),
            self.original
        );
    }

    async fn assert_epoch(&self, state: &AgentIndexedState, actual: Option<ShardEpoch>) {
        // A refused append observes the epoch without changing the data or the manifest.
        for namespace in state.epoch_namespaces() {
            let error = self
                .indexed
                .append(
                    "test",
                    "epoch",
                    "oplog",
                    namespace,
                    &self.id().storage_key(),
                    1000,
                    vec![],
                    Some(ShardEpoch(1)),
                    None,
                )
                .await
                .unwrap_err();
            assert!(
                matches!(error, IndexedStorageError::Fenced { actual: found, .. } if found == actual),
                "{error:?}"
            );
        }
    }
}

async fn storages(root: &tempfile::TempDir) -> Vec<Arc<dyn IndexedStorage + Send + Sync>> {
    vec![
        Arc::new(InMemoryIndexedStorage::new()),
        Arc::new(
            SqliteIndexedStorage::configured(&DbSqliteConfig {
                database: root.path().join("seal.db").to_string_lossy().into_owned(),
                max_connections: 4,
                foreign_keys: false,
            })
            .await
            .unwrap(),
        ),
    ]
}

#[test]
async fn sealing_preserves_source_ranges_without_weakening_exact_reads() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::new(indexed).await;
        let service = f.oplog_service().await;
        let ranges = [(0, 10), (0, 4), (2, 7), (5, 20), (9, 1), (10, 2), (1, 0)];
        let mut before = Vec::new();
        for (start, count) in ranges {
            let expected = f
                .original
                .iter()
                .filter(|(index, _)| {
                    start + count <= 10 && index.as_u64() >= start && index.as_u64() < start + count
                })
                .map(|(index, entry)| (*index, entry.clone()))
                .collect::<BTreeMap<_, _>>();
            let actual = service
                .read_source(
                    &f.id(),
                    AgentMode::Durable,
                    OplogIndex::from_u64(start),
                    count,
                )
                .await
                .unwrap();
            assert_eq!(actual, expected);
            before.push(actual);
        }
        let seal = f.service();
        let pending = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        seal.remove_moved_copies(&pending).await.unwrap();
        assert!(seal.release(&pending).await.unwrap());
        for ((start, count), expected) in ranges.into_iter().zip(before) {
            let actual = service
                .read_source(
                    &f.id(),
                    AgentMode::Durable,
                    OplogIndex::from_u64(start),
                    count,
                )
                .await
                .unwrap();
            assert_eq!(actual, expected);
        }
        assert!(matches!(
            service
                .read_exact(&f.id(), AgentMode::Durable, OplogIndex::NONE, 10)
                .await,
            Err(crate::services::oplog::reader::UnreadableOplog {
                error: crate::services::oplog::reader::OplogReadError::Gap { .. },
                ..
            })
        ));
        let mut trimmed = pending.record.clone();
        trimmed
            .manifest
            .get_mut(&1)
            .unwrap()
            .remove(&OplogIndex::from_u64(3));
        let actual = crate::services::oplog::sealed_reader::read_sealed_source(
            f.blobs.as_ref(),
            &f.id(),
            &trimmed,
            OplogIndex::NONE,
            10,
        )
        .await
        .unwrap();
        assert_eq!(
            actual,
            f.original
                .range(OplogIndex::from_u64(4)..)
                .map(|(index, entry)| (*index, entry.clone()))
                .collect()
        );
        let mut gapped = pending.record.clone();
        gapped
            .manifest
            .get_mut(&1)
            .unwrap()
            .remove(&OplogIndex::from_u64(6));
        assert!(
            crate::services::oplog::sealed_reader::read_sealed_source(
                f.blobs.as_ref(),
                &f.id(),
                &gapped,
                OplogIndex::NONE,
                10
            )
            .await
            .is_err()
        );
    }
}

#[test]
#[test_r::timeout("60s")]
async fn release_during_indexed_reads_uses_the_frozen_history() {
    use crate::services::oplog::tests::ReadCountingIndexedStorage;

    enum ReadOperation {
        Source,
        Exact,
        Exists,
        TryExists,
        LastIndex,
        TryLastIndex,
    }
    use ReadOperation::*;

    // Existence and last-index reads stop at the first populated layer. History reads also
    // cover release after the newer layer was read but before its older layer was read.
    for (operation, between_layers) in [
        (Source, false),
        (Exact, false),
        (Exists, false),
        (TryExists, false),
        (LastIndex, false),
        (TryLastIndex, false),
        (Source, true),
        (Exact, true),
    ] {
        let storage = Arc::new(ReadCountingIndexedStorage::new());
        let f = Fixture::new(storage.clone()).await;
        let seal = f.service();
        let pending = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        let service = f.oplog_service().await;
        let id = f.id();
        let expected = f.original.clone();
        let namespace = if between_layers {
            pending.state.manifest_namespace(1)
        } else {
            pending.state.primary_namespace()
        };
        let (waiting, resume) = storage.pause_next_read(namespace, false);
        let reader = tokio::spawn(async move {
            match operation {
                Source => assert_eq!(
                    service
                        .read_source(&id, AgentMode::Durable, OplogIndex::NONE, 10)
                        .await
                        .unwrap(),
                    expected
                ),
                Exact => assert_eq!(
                    service
                        .read_exact(&id, AgentMode::Durable, OplogIndex::INITIAL, 9)
                        .await
                        .unwrap(),
                    expected
                ),
                Exists => assert!(service.exists(&id, AgentMode::Durable).await.unwrap()),
                TryExists => assert!(service.try_exists(&id, AgentMode::Durable).await.unwrap()),
                LastIndex => assert_eq!(
                    service
                        .get_last_index(&id, AgentMode::Durable)
                        .await
                        .unwrap(),
                    OplogIndex::from_u64(9)
                ),
                TryLastIndex => assert_eq!(
                    service
                        .try_get_last_index(&id, AgentMode::Durable)
                        .await
                        .unwrap(),
                    OplogIndex::from_u64(9)
                ),
            }
        });
        waiting.await.unwrap();
        seal.remove_moved_copies(&pending).await.unwrap();
        assert!(seal.release(&pending).await.unwrap());
        resume.send(()).unwrap();
        reader.await.unwrap();
        f.assert_epoch(&pending.state, None).await;
    }
}

#[test]
#[test_r::timeout("60s")]
async fn same_epoch_release_and_unseal_retries_gapped_reads_and_bounds_churn() {
    use crate::services::oplog::tests::ReadCountingIndexedStorage;

    for (initial_marker, exhaust_retries) in [(true, false), (false, false), (true, true)] {
        let storage = Arc::new(ReadCountingIndexedStorage::new());
        let f = Fixture::new(storage.clone()).await;
        let seal = f.service();
        let pending = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        if !initial_marker {
            storage
                .with("test", "no_initial_marker")
                .delete(pending.state.marker_namespace(), &f.id().storage_key())
                .await
                .unwrap();
        }
        let service = f.oplog_service().await;
        let id = f.id();
        let (mut waiting, mut resume) =
            storage.pause_next_read(pending.state.primary_namespace(), false);
        let reader = tokio::spawn(async move {
            service
                .read_exact(&id, AgentMode::Durable, OplogIndex::INITIAL, 9)
                .await
        });
        let attempts = if exhaust_retries {
            seal.indexed_retry.max_attempts.max(1)
        } else {
            1
        };
        for attempt in 0..attempts {
            waiting.await.unwrap();
            assert!(seal.release(&pending).await.unwrap());
            let (empty_read, finish_empty_read) =
                storage.pause_next_read(pending.state.manifest_namespace(1), true);
            resume.send(()).unwrap();
            empty_read.await.unwrap();
            // Every layer has returned no rows. Restore the identical history at the SAME
            // epoch before read_exact reports its gap to the authority validator.
            seal.unseal(&f.id(), ShardEpoch(6)).await.unwrap();
            if attempt + 1 == attempts {
                finish_empty_read.send(()).unwrap();
                break;
            }
            (waiting, resume) = storage.pause_next_read(pending.state.primary_namespace(), false);
            finish_empty_read.send(()).unwrap();
        }
        let result = reader.await.unwrap();
        if exhaust_retries {
            assert!(result.unwrap_err().is_transient());
        } else {
            assert_eq!(result.unwrap(), f.original);
        }
    }
}

#[test]
#[test_r::timeout("60s")]
async fn record_lookup_revalidates_absence_before_using_the_frozen_last_index() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::new(indexed).await;
        let seal = f.service();
        let pending = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        assert!(seal.release(&pending).await.unwrap());
        let service = f.oplog_service().await;
        let id = f.id();
        let (waiting, resume) = f.blobs.pause_next_read();
        let reader =
            tokio::spawn(async move { service.get_last_index(&id, AgentMode::Durable).await });
        waiting.await.unwrap();
        seal.unseal(&f.id(), ShardEpoch(6)).await.unwrap();
        f.indexed
            .append(
                "test",
                "new_history",
                "entry",
                pending.state.primary_namespace(),
                &f.id().storage_key(),
                10,
                serialize(&OplogEntry::Suspend {
                    timestamp: Timestamp::from(10000),
                })
                .unwrap(),
                Some(ShardEpoch(6)),
                None,
            )
            .await
            .unwrap();
        resume.send(()).unwrap();
        assert_eq!(reader.await.unwrap().unwrap(), OplogIndex::from_u64(10));
    }
}

#[test]
#[test_r::timeout("60s")]
async fn frozen_read_failure_retries_only_when_authority_changed() {
    for restore in [false, true] {
        let f = Fixture::new(Arc::new(InMemoryIndexedStorage::new())).await;
        let seal = f.service();
        let pending = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        assert!(seal.release(&pending).await.unwrap());
        let service = f.oplog_service().await;
        let id = f.id();
        let (waiting, resume) = f
            .blobs
            .pause_next_read_in(BlobStorageNamespace::CompressedOplog {
                environment_id: id.environment_id(),
                type_tag: id.agent_id.type_tag(),
                agent_mode: AgentMode::Durable,
                level: 0,
            });
        let reader = tokio::spawn(async move {
            service
                .read_exact(&id, AgentMode::Durable, OplogIndex::INITIAL, 9)
                .await
        });
        waiting.await.unwrap();
        if restore {
            seal.unseal(&f.id(), ShardEpoch(6)).await.unwrap();
        }
        f.blobs.fail_next_read();
        resume.send(()).unwrap();
        let result = reader.await.unwrap();
        if restore {
            assert_eq!(result.unwrap(), f.original);
        } else {
            assert!(result.is_err());
        }
    }
}

#[test]
async fn prepare_and_crash_before_release_keep_presence_and_moved_data_readable() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::new(indexed).await;
        f.put_promise(3, Some(vec![42, 7])).await;
        f.put_promise(4, None).await;
        f.reject(&[2, 5, 8]).await;
        let service = f.service();
        let pending = service
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        for level in 0..2 {
            assert_eq!(
                pending.record.manifest[&level],
                f.archive(level)
                    .manifest_snapshot(&f.id(), AgentMode::Durable)
                    .await
                    .unwrap()
            );
        }
        assert!(pending.record.pending_promises.is_empty());
        assert!(
            f.indexed
                .with("test", "marker")
                .exists(pending.state.marker_namespace(), &f.id().storage_key())
                .await
                .unwrap()
        );
        assert!(matches!(
            service.state(&f.id()).await.unwrap(),
            AgentSealState::Present
        ));
        assert_eq!(
            f.kv.with_entity("test", "prepared", "promise")
                .get_all_raw(promise_namespace_of(&f.id(), f.metadata.fingerprint))
                .await
                .unwrap()
                .len(),
            3
        );
        f.assert_history(&pending.record).await;
        service.remove_moved_copies(&pending).await.unwrap();
        drop(service);
        drop(pending);
        // No resident PendingSeal or service cache participates in recovery.
        let recovered = f.service();
        assert!(matches!(
            recovered.state(&f.id()).await.unwrap(),
            AgentSealState::Present
        ));
        assert_eq!(
            recovered
                .completed_promise(f.id().environment_id(), &f.promise(3))
                .await
                .unwrap(),
            Some(vec![42, 7])
        );
        assert_eq!(
            recovered
                .rejected_snapshots(&f.id(), f.metadata.fingerprint)
                .await
                .unwrap(),
            BTreeSet::from([OplogIndex::from_u64(5), OplogIndex::from_u64(8)])
        );
        let fields =
            f.kv.with_entity("test", "remaining", "promise")
                .get_all_raw(promise_namespace_of(&f.id(), f.metadata.fingerprint))
                .await
                .unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(
            fields[0].0,
            f.promise(4).storage_key(f.id().environment_id())
        );
        assert!(matches!(
            decode::<RedisPromiseState>(&fields[0].1).unwrap(),
            RedisPromiseState::Pending
        ));
        let record = recovered.record(&f.id()).await.unwrap().unwrap();
        f.assert_history(&record).await;
    }
}

#[test]
async fn release_and_new_epoch_unseal_preserve_frozen_history_and_restore_authority() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let mut f = Fixture::new(indexed).await;
        let service = f.service();
        let pending = service
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        service.remove_moved_copies(&pending).await.unwrap();
        assert!(service.release(&pending).await.unwrap());
        assert!(service.release(&pending).await.unwrap());
        assert!(
            !f.indexed
                .observe_agent("test", "released", &f.id())
                .await
                .unwrap()
                .has_data
        );
        f.assert_epoch(&pending.state, None).await;
        assert!(matches!(
            f.service().state(&f.id()).await.unwrap(),
            AgentSealState::Sealed(_)
        ));
        f.assert_history(&pending.record).await;
        // Creation and fork publication must not overwrite a released incarnation.
        assert!(matches!(
            service.check_target(&f.id()).await,
            Err(CreateStagedError::TargetSealed)
        ));
        f.assert_epoch(&pending.state, None).await;
        // A different executor really holds the stale epoch; lease validation alone must not
        // explain the refusal. The record's writer epoch is the lower bound on restoration.
        f.shard = Arc::new(ShardServiceDefault::new());
        f.register(5, true);
        let service = f.service();
        assert!(service.deadline(&f.id(), ShardEpoch(5)).is_some());
        assert!(service.unseal(&f.id(), ShardEpoch(5)).await.is_err());
        assert!(
            !f.indexed
                .observe_agent("test", "stale", &f.id())
                .await
                .unwrap()
                .has_data
        );
        f.register(7, true);
        service.unseal(&f.id(), ShardEpoch(7)).await.unwrap();
        // Lost reply: a new service repeats the restoration against already committed storage.
        f.service().unseal(&f.id(), ShardEpoch(7)).await.unwrap();
        assert!(matches!(
            service.state(&f.id()).await.unwrap(),
            AgentSealState::Present
        ));
        f.assert_epoch(&pending.state, Some(ShardEpoch(7))).await;
        assert!(
            f.indexed
                .with("test", "primary_presence")
                .exists(pending.state.primary_namespace(), &f.id().storage_key())
                .await
                .unwrap()
        );
        assert!(
            f.indexed
                .with("test", "marker")
                .exists(pending.state.marker_namespace(), &f.id().storage_key())
                .await
                .unwrap()
        );
        for level in 0..2 {
            assert_eq!(
                f.archive(level)
                    .manifest_snapshot(&f.id(), AgentMode::Durable)
                    .await
                    .unwrap(),
                pending.record.manifest[&level]
            );
        }
        f.assert_history(&service.record(&f.id()).await.unwrap().unwrap())
            .await;
        assert!(f.scheduler.actions.lock().unwrap().iter().any(|action| matches!(action,
            ScheduledAction::ArchiveOplog { owned_agent_id, last_oplog_index, agent_mode: AgentMode::Durable, .. }
                if *owned_agent_id == f.id() && *last_oplog_index == OplogIndex::from_u64(9))));
    }
}

#[test]
async fn later_seal_merges_moved_completions_and_prunes_the_rejected_union() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let mut f = Fixture::new(indexed).await;
        f.put_promise(3, Some(vec![1, 2, 3])).await;
        f.put_promise(4, None).await;
        f.reject(&[5]).await;
        let service = f.service();
        let first = service
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        service.remove_moved_copies(&first).await.unwrap();
        assert!(service.release(&first).await.unwrap());
        assert_eq!(
            service
                .completed_promise(EnvironmentId::new(), &f.promise(3))
                .await
                .unwrap(),
            None
        );
        let mut wrong = f.promise(3);
        wrong.fingerprint = AgentFingerprint::new();
        assert_eq!(
            service
                .completed_promise(f.id().environment_id(), &wrong)
                .await
                .unwrap(),
            None
        );
        assert!(
            service
                .rejected_snapshots(&f.id(), wrong.fingerprint)
                .await
                .unwrap()
                .is_empty()
        );
        f.register(7, true);
        service.unseal(&f.id(), ShardEpoch(7)).await.unwrap();
        // A duplicate completion must not replace the payload moved by the first seal.
        f.put_promise(3, Some(vec![99])).await;
        f.put_promise(4, Some(vec![4, 4])).await;
        f.put_promise(6, None).await;
        f.reject(&[2, 8]).await;
        let second = service
            .prepare(f.metadata.clone(), ShardEpoch(7))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            second.record.completed_promises,
            BTreeMap::from([
                (OplogIndex::from_u64(3), vec![1, 2, 3]),
                (OplogIndex::from_u64(4), vec![4, 4]),
            ])
        );
        assert_eq!(
            second.record.rejected_periodic_snapshots,
            BTreeSet::from([OplogIndex::from_u64(5), OplogIndex::from_u64(8)])
        );
        service.remove_moved_copies(&second).await.unwrap();
        assert!(service.release(&second).await.unwrap());
        assert_eq!(
            service
                .completed_promise(f.id().environment_id(), &f.promise(3))
                .await
                .unwrap(),
            Some(vec![1, 2, 3])
        );
        f.register(8, true);
        service.unseal(&f.id(), ShardEpoch(8)).await.unwrap();
        f.metadata
            .last_known_status
            .previous_usable_automatic_snapshot = None;
        let third = service
            .prepare(f.metadata.clone(), ShardEpoch(8))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            third.record.rejected_periodic_snapshots,
            BTreeSet::from([OplogIndex::from_u64(8)])
        );
        f.assert_history(&third.record).await;
        let fields =
            f.kv.with_entity("test", "pending", "promise")
                .get_all_raw(promise_namespace_of(&f.id(), f.metadata.fingerprint))
                .await
                .unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(
            fields[0].0,
            f.promise(6).storage_key(f.id().environment_id())
        );
    }
}

#[test]
async fn unbounded_lease_and_changed_manifest_refuse_lifecycle_writes() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::new(indexed).await;
        let service = f.service();
        f.register(7, false);
        assert!(
            service
                .prepare(f.metadata.clone(), ShardEpoch(7))
                .await
                .unwrap()
                .is_none()
        );
        assert!(service.record(&f.id()).await.unwrap().is_none());
        f.register(8, true);
        service.claim(&f.id(), ShardEpoch(8)).await.unwrap();
        let pending = service
            .prepare(f.metadata.clone(), ShardEpoch(8))
            .await
            .unwrap()
            .unwrap();
        let archive = f.archive(0);
        let handle = archive
            .open(&f.id(), AgentMode::Durable, None, f.metadata.fingerprint)
            .await
            .unwrap();
        handle
            .append(&[(
                OplogIndex::from_u64(10),
                OplogEntry::Suspend {
                    timestamp: Timestamp::from(10000),
                },
            )])
            .await
            .unwrap();
        assert!(!service.release(&pending).await.unwrap());
        assert!(matches!(
            service.state(&f.id()).await.unwrap(),
            AgentSealState::Present
        ));
        assert_eq!(
            f.archive(0)
                .manifest_snapshot(&f.id(), AgentMode::Durable)
                .await
                .unwrap()
                .len(),
            pending.record.manifest[&0].len() + 1
        );
        f.assert_history(&pending.record).await;
        // The record alone must also reserve the name before a release completes.
        assert!(matches!(
            service.check_target(&f.id()).await,
            Err(CreateStagedError::TargetSealed)
        ));
        f.register(9, false);
        assert!(matches!(
            service.unseal(&f.id(), ShardEpoch(9)).await,
            Err(WorkerExecutorError::ShardingNotReady)
        ));
    }
}

#[test]
async fn sealed_oplog_reads_write_nothing_and_open_restores_before_appending() {
    use crate::services::oplog::CommitLevel;
    use crate::services::oplog::tests::{default_execution_status, default_last_known_status};

    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::new(indexed).await;
        let seal = f.service();
        let pending = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        seal.remove_moved_copies(&pending).await.unwrap();
        assert!(seal.release(&pending).await.unwrap());
        let service = f.oplog_service().await;
        assert!(service.exists(&f.id(), AgentMode::Durable).await.unwrap());
        assert!(
            !service
                .try_exists(&f.id(), AgentMode::Ephemeral)
                .await
                .unwrap()
        );
        assert_eq!(
            service
                .get_last_index(&f.id(), AgentMode::Durable)
                .await
                .unwrap(),
            OplogIndex::from_u64(9)
        );
        assert_eq!(
            service
                .read_source(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 9)
                .await
                .unwrap(),
            f.original
        );
        f.assert_epoch(&pending.state, None).await;
        let create = service
            .create(
                &mut service.lock_lifecycle(&f.id().agent_id).await,
                &f.id(),
                AgentMode::Durable,
                OplogEntry::suspend(),
                f.metadata.clone(),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                Some(ShardEpoch(6)),
            )
            .await;
        assert!(matches!(
            create,
            Err(WorkerExecutorError::AgentAlreadyExists { .. })
        ));
        f.assert_epoch(&pending.state, None).await;
        let no_owner = service
            .open(
                &mut service.lock_lifecycle(&f.id().agent_id).await,
                &f.id(),
                AgentMode::Durable,
                Some(OplogIndex::from_u64(9)),
                f.metadata.clone(),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
            )
            .await;
        assert!(matches!(
            no_owner,
            Err(WorkerExecutorError::ShardingNotReady)
        ));
        f.assert_epoch(&pending.state, None).await;
        f.register(7, true);
        {
            let mut lifecycle = service.lock_lifecycle(&f.id().agent_id).await;
            lifecycle.set_open_purpose(crate::services::oplog::OplogOpenPurpose::Archive);
            let archive = service
                .open(
                    &mut lifecycle,
                    &f.id(),
                    AgentMode::Durable,
                    Some(OplogIndex::from_u64(9)),
                    f.metadata.clone(),
                    default_last_known_status(),
                    default_execution_status(AgentMode::Durable),
                    Some(ShardEpoch(7)),
                )
                .await;
            assert!(matches!(
                archive,
                Err(WorkerExecutorError::AgentNotFound { .. })
            ));
        }
        f.assert_epoch(&pending.state, None).await;
        assert!(f.scheduler.actions.lock().unwrap().is_empty());
        let opened = service
            .open(
                &mut service.lock_lifecycle(&f.id().agent_id).await,
                &f.id(),
                AgentMode::Durable,
                Some(OplogIndex::from_u64(9)),
                f.metadata.clone(),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                Some(ShardEpoch(7)),
            )
            .await
            .unwrap();
        assert!(opened.fence().is_none());
        let next = OplogEntry::Suspend {
            timestamp: Timestamp::from(10000),
        };
        opened.add(next.clone()).await.unwrap();
        opened.commit(CommitLevel::Always).await.unwrap();
        let mut expected = f.original.clone();
        expected.insert(OplogIndex::from_u64(10), next);
        assert_eq!(
            service
                .read_source(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 20)
                .await
                .unwrap(),
            expected
        );
        assert!(matches!(
            seal.state(&f.id()).await.unwrap(),
            AgentSealState::Present
        ));
        opened.stop_and_wait().await.unwrap();
    }
}

#[test]
async fn trimmed_source_ranges_preserve_resident_suffix_semantics_after_sealing() {
    use crate::services::promise::{DefaultPromiseService, PromiseService};

    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for trim in [0, 3, 6] {
            let f = Fixture::new(indexed.clone()).await;
            f.put_promise(3, Some(vec![3, 1, 4])).await;
            f.reject(&[5, 8]).await;
            if trim != 0 {
                let archive = f
                    .archive(1)
                    .open(&f.id(), AgentMode::Durable, None, f.metadata.fingerprint)
                    .await
                    .unwrap();
                archive
                    .drop_prefix(OplogIndex::from_u64(trim))
                    .await
                    .unwrap();
            }
            let service = f.oplog_service().await;
            let ranges: Vec<_> = (0..=11)
                .flat_map(|start| {
                    [0, 1, 2, 3, 6, 9, 15]
                        .into_iter()
                        .map(move |count| (start, count))
                })
                .collect();
            let mut before = Vec::new();
            for &(start, count) in &ranges {
                let actual = service
                    .read_source(
                        &f.id(),
                        AgentMode::Durable,
                        OplogIndex::from_u64(start),
                        count,
                    )
                    .await
                    .unwrap();
                let expected = f
                    .original
                    .iter()
                    .filter(|(index, _)| {
                        start + count <= 10
                            && index.as_u64() > trim
                            && index.as_u64() >= start
                            && index.as_u64() < start + count
                    })
                    .map(|(index, entry)| (*index, entry.clone()))
                    .collect::<BTreeMap<_, _>>();
                assert_eq!(
                    actual, expected,
                    "resident trim={trim} start={start} count={count}"
                );
                before.push(actual);
            }
            let seal = f.service();
            let pending = seal
                .prepare(f.metadata.clone(), ShardEpoch(6))
                .await
                .unwrap()
                .unwrap();
            seal.remove_moved_copies(&pending).await.unwrap();
            assert!(seal.release(&pending).await.unwrap());
            for (&(start, count), expected) in ranges.iter().zip(&before) {
                let actual = service
                    .read_source(
                        &f.id(),
                        AgentMode::Durable,
                        OplogIndex::from_u64(start),
                        count,
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    &actual, expected,
                    "sealed trim={trim} start={start} count={count}"
                );
            }
            seal.unseal(&f.id(), ShardEpoch(6)).await.unwrap();
            for (&(start, count), expected) in ranges.iter().zip(&before) {
                assert_eq!(
                    &service
                        .read_source(
                            &f.id(),
                            AgentMode::Durable,
                            OplogIndex::from_u64(start),
                            count
                        )
                        .await
                        .unwrap(),
                    expected,
                    "unsealed trim={trim} start={start} count={count}"
                );
            }
            assert!(
                f.kv.with_entity("test", "moved_promise_absent", "promise")
                    .get_all_raw(promise_namespace_of(&f.id(), f.metadata.fingerprint))
                    .await
                    .unwrap()
                    .is_empty()
            );
            // Neither the promise service nor its seal access shares resident state with
            // the service that moved these fields out of KV storage.
            let cold_seal = Arc::new(f.service());
            let promises = DefaultPromiseService::new(
                f.kv.clone(),
                Arc::new(SealPromiseAccess(cold_seal.clone())),
            );
            assert!(
                !promises
                    .complete_without_wakeup(f.id().environment_id(), f.promise(3), vec![99])
                    .await
                    .unwrap()
            );
            assert_eq!(
                promises
                    .poll(f.id().environment_id(), f.promise(3))
                    .await
                    .unwrap()
                    .get()
                    .await,
                Some(vec![3, 1, 4])
            );
            assert_eq!(
                cold_seal
                    .rejected_snapshots(&f.id(), f.metadata.fingerprint)
                    .await
                    .unwrap(),
                BTreeSet::from([OplogIndex::from_u64(5), OplogIndex::from_u64(8)])
            );
            let second = cold_seal
                .prepare(f.metadata.clone(), ShardEpoch(6))
                .await
                .expect("trimmed history must remain sealable after unseal")
                .unwrap();
            assert_eq!(
                second.record.completed_promises,
                pending.record.completed_promises
            );
            assert_eq!(
                second.record.rejected_periodic_snapshots,
                pending.record.rejected_periodic_snapshots
            );
            cold_seal.remove_moved_copies(&second).await.unwrap();
            assert!(cold_seal.release(&second).await.unwrap());
            for (&(start, count), expected) in ranges.iter().zip(&before) {
                assert_eq!(
                    &service
                        .read_source(
                            &f.id(),
                            AgentMode::Durable,
                            OplogIndex::from_u64(start),
                            count
                        )
                        .await
                        .unwrap(),
                    expected,
                    "resealed trim={trim} start={start} count={count}"
                );
            }
            // Restore before reusing the backend for the next independently populated fixture.
            cold_seal.unseal(&f.id(), ShardEpoch(6)).await.unwrap();
            for level in 0..2 {
                f.archive(level)
                    .delete(&f.id(), AgentMode::Durable, None)
                    .await
                    .unwrap();
            }
        }
    }
}

#[test]
async fn staging_refuses_sealed_targets_and_accepts_absent_targets() {
    use crate::services::oplog::CommitLevel;
    use crate::services::oplog::tests::{default_execution_status, default_last_known_status};

    let root = tempfile::TempDir::new().unwrap();
    let mut failures = Vec::new();
    for (backend, indexed) in storages(&root).await.into_iter().enumerate() {
        for sealed in [false, true] {
            let f = Fixture::new(indexed.clone()).await;
            let service = f.oplog_service().await;
            if sealed {
                let seal = f.service();
                let pending = seal
                    .prepare(f.metadata.clone(), ShardEpoch(6))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(seal.release(&pending).await.unwrap());
            }
            assert_eq!(
                service
                    .read_exact(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 9)
                    .await
                    .unwrap(),
                f.original
            );

            let stage_id = Uuid::new_v4();
            let refusal = service
                .create_staged(&f.id(), AgentMode::Durable, stage_id, f.metadata.clone())
                .await;
            assert!(matches!(
                (&refusal, sealed),
                (Err(CreateStagedError::TargetSealed), true)
                    | (Err(CreateStagedError::TargetPresent), false)
            ));
            if sealed {
                assert!(
                    !service
                        .staged_exists(&f.id(), AgentMode::Durable, stage_id)
                        .await
                        .unwrap()
                );
                assert!(
                    !f.indexed
                        .observe_agent("test", "refused_stage", &f.id())
                        .await
                        .unwrap()
                        .has_data
                );
                assert!(
                    !f.indexed
                        .remove_stage_reservation("test", "refused_reservation", &f.id(), None)
                        .await
                        .unwrap()
                );
                let state = f
                    .service()
                    .indexed_state(
                        &f.id(),
                        AgentMode::Durable,
                        &BTreeMap::from([(0, BTreeMap::new()), (1, BTreeMap::new())]),
                    )
                    .unwrap();
                f.assert_epoch(&state, None).await;
                assert_eq!(
                    service
                        .read_exact(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 9)
                        .await
                        .unwrap(),
                    f.original
                );
                continue;
            }
            let mut metadata = f.metadata.clone();
            metadata.agent_id = AgentId::from_parts(7, 2126);
            let target = metadata.owned_agent_id();
            let stage = service
                .create_staged(&target, AgentMode::Durable, stage_id, metadata.clone())
                .await
                .unwrap();
            stage.add(OplogEntry::suspend()).await.unwrap();
            stage.commit(CommitLevel::Always).await.unwrap();
            stage.stop_and_wait().await.unwrap();
            assert!(
                service
                    .staged_exists(&target, AgentMode::Durable, stage_id)
                    .await
                    .unwrap()
            );
            let create = service
                .create(
                    &mut service.lock_lifecycle(&target.agent_id).await,
                    &target,
                    AgentMode::Durable,
                    OplogEntry::suspend(),
                    metadata,
                    default_last_known_status(),
                    default_execution_status(AgentMode::Durable),
                    Some(ShardEpoch(6)),
                )
                .await;
            assert!(matches!(
                create,
                Err(WorkerExecutorError::AgentAlreadyExists { .. })
            ));

            // A stage is hidden until publication; it cannot change the visible target history.
            let actual = service
                .read_exact(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 9)
                .await;
            service
                .discard_staged(&target, AgentMode::Durable, stage_id, stage.stage_token())
                .await
                .unwrap();
            assert_eq!(
                service
                    .read_exact(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 9)
                    .await
                    .unwrap(),
                f.original,
                "history must remain intact after discarding the stage"
            );
            if actual.as_ref().ok() != Some(&f.original) {
                failures.push(format!("backend={backend} sealed={sealed}: {actual:?}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "Hidden stage changed target reads: {failures:#?}"
    );
}

#[test]
#[test_r::timeout("30s")]
async fn plain_initial_create_invalidates_an_admitted_stages_first_write_before_release() {
    use crate::services::oplog::CommitLevel;
    use crate::services::oplog::tests::{default_execution_status, default_last_known_status};

    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let mut f = Fixture::new(indexed).await;
        f.metadata.agent_id = AgentId::from_parts(7, 2127);
        let target = f.id();
        let service = Arc::new(f.oplog_service().await);
        let stage_id = Uuid::new_v4();
        let stage = service
            .create_staged(&target, AgentMode::Durable, stage_id, f.metadata.clone())
            .await
            .unwrap();
        let creating = {
            let service = service.clone();
            let metadata = f.metadata.clone();
            let target = target.clone();
            let initial = f.original[&OplogIndex::INITIAL].clone();
            tokio::spawn(async move {
                service
                    .create(
                        &mut service.lock_lifecycle(&target.agent_id).await,
                        &target,
                        AgentMode::Durable,
                        initial,
                        metadata,
                        default_last_known_status(),
                        default_execution_status(AgentMode::Durable),
                        Some(ShardEpoch(6)),
                    )
                    .await
            })
        };
        let oplog = creating.await.unwrap().unwrap();
        oplog.stop_and_wait().await.unwrap();
        // The competing create invalidates the token before the stage's first stored entry.
        stage.add(OplogEntry::suspend()).await.unwrap();
        assert!(matches!(
            stage.commit(CommitLevel::Always).await,
            Err(OplogError::StageInvalidated { .. })
        ));
        // Use real chunk and manifest writes, then trim the primary as archive transfer does.
        for (level, first, last) in [(1, 1, 3), (1, 4, 6), (0, 7, 9)] {
            let archive = f.archive(level);
            let handle = archive
                .open(&target, AgentMode::Durable, None, f.metadata.fingerprint)
                .await
                .unwrap();
            let entries: Vec<_> = f
                .original
                .range(OplogIndex::from_u64(first)..=OplogIndex::from_u64(last))
                .map(|(index, entry)| (*index, entry.clone()))
                .collect();
            handle.append(&entries).await.unwrap();
        }
        f.indexed
            .drop_prefix(
                "test",
                "archive_primary",
                IndexedStorageNamespace::OpLog {
                    owned_agent_id: target.clone(),
                    agent_mode: AgentMode::Durable,
                },
                &target.storage_key(),
                1,
                Some(ShardEpoch(6)),
            )
            .await
            .unwrap();
        let seal = f.service();
        let pending = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        assert!(seal.release(&pending).await.unwrap());
        assert_eq!(
            service
                .read_exact(&target, AgentMode::Durable, OplogIndex::INITIAL, 9)
                .await
                .unwrap(),
            f.original
        );
        assert!(matches!(
            stage.commit(CommitLevel::Always).await,
            Err(OplogError::StageInvalidated { .. })
        ));
        assert!(matches!(
            stage.wait_for_replicas(1, Duration::from_secs(1)).await,
            Err(OplogError::StageInvalidated { .. })
        ));
        stage.stop_and_wait().await.unwrap();
        assert!(
            !f.indexed
                .observe_agent("test", "after_stale_stage", &target)
                .await
                .unwrap()
                .has_data
        );
        f.assert_epoch(&pending.state, None).await;
        assert_eq!(
            service
                .read_exact(&target, AgentMode::Durable, OplogIndex::INITIAL, 9)
                .await
                .unwrap(),
            f.original
        );
    }
}

#[test]
#[test_r::timeout("30s")]
async fn staging_reservation_keeps_sealed_history_readable_during_admission() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::new(indexed).await;
        let seal = f.service();
        let pending = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        assert!(seal.release(&pending).await.unwrap());
        let service = Arc::new(f.oplog_service().await);
        let (reserved, resume) = f.blobs.pause_seal_list_after(0);
        let admission = {
            let service = service.clone();
            let metadata = f.metadata.clone();
            tokio::spawn(async move {
                service
                    .create_staged(
                        &metadata.owned_agent_id(),
                        AgentMode::Durable,
                        Uuid::new_v4(),
                        metadata.clone(),
                    )
                    .await
            })
        };
        reserved.await.unwrap();
        let during_admission = service
            .read_exact(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 9)
            .await;
        resume.send(()).unwrap();
        assert!(matches!(
            admission.await.unwrap(),
            Err(CreateStagedError::TargetSealed)
        ));
        assert_eq!(
            service
                .read_exact(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 9)
                .await
                .unwrap(),
            f.original
        );
        assert_eq!(during_admission.unwrap(), f.original);
    }
}

#[test]
#[test_r::timeout("30s")]
async fn failed_admission_cleans_its_claim_but_cancellation_leaves_only_fences() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for cancel in [false, true] {
            let f = Fixture::new(indexed.clone()).await;
            let seal = f.service();
            let pending = seal
                .prepare(f.metadata.clone(), ShardEpoch(6))
                .await
                .unwrap()
                .unwrap();
            assert!(seal.release(&pending).await.unwrap());
            let service = Arc::new(f.oplog_service().await);
            if cancel {
                let (reserved, resume) = f.blobs.pause_seal_list_after(0);
                let admission = {
                    let service = service.clone();
                    let metadata = f.metadata.clone();
                    tokio::spawn(async move {
                        service
                            .create_staged(
                                &metadata.owned_agent_id(),
                                AgentMode::Durable,
                                Uuid::new_v4(),
                                metadata.clone(),
                            )
                            .await
                    })
                };
                reserved.await.unwrap();
                admission.abort();
                assert!(admission.await.unwrap_err().is_cancelled());
                assert!(resume.is_closed());
            } else {
                f.blobs.fail_next_seal_list();
                assert!(matches!(
                    service
                        .create_staged(
                            &f.id(),
                            AgentMode::Durable,
                            Uuid::new_v4(),
                            f.metadata.clone()
                        )
                        .await,
                    Err(CreateStagedError::Retryable(_))
                ));
            }
            assert!(
                !f.indexed
                    .observe_agent("test", "admission_absent", &f.id())
                    .await
                    .unwrap()
                    .has_data
            );
            if cancel {
                assert!(matches!(
                    f.indexed
                        .append(
                            "test",
                            "cancelled_claim",
                            "entry",
                            pending.state.primary_namespace(),
                            &f.id().storage_key(),
                            1000,
                            vec![],
                            Some(ShardEpoch(1)),
                            None,
                        )
                        .await,
                    Err(IndexedStorageError::Fenced {
                        actual: Some(ShardEpoch(6)),
                        ..
                    })
                ));
            } else {
                assert!(
                    !f.indexed
                        .remove_stage_reservation("test", "cleanup_complete", &f.id(), None)
                        .await
                        .unwrap()
                );
                f.assert_epoch(&pending.state, None).await;
            }
            assert_eq!(
                service
                    .read_exact(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 9)
                    .await
                    .unwrap(),
                f.original
            );
        }
    }
}

#[test]
#[test_r::timeout("30s")]
async fn discard_superseded_empty_stage_preserves_current_stage_writer() {
    use crate::services::oplog::CommitLevel;

    let root = tempfile::TempDir::new().unwrap();
    let mut failures = Vec::new();
    for (backend, indexed) in storages(&root).await.into_iter().enumerate() {
        let f = Fixture::new(indexed).await;
        let service = f.oplog_service().await;
        let mut metadata = f.metadata.clone();
        metadata.agent_id = AgentId::from_parts(7, 2199);
        let target = metadata.owned_agent_id();
        let old_id = Uuid::new_v4();
        let old = service
            .create_staged(&target, AgentMode::Durable, old_id, metadata.clone())
            .await
            .unwrap();
        let current = service
            .create_staged(&target, AgentMode::Durable, Uuid::new_v4(), metadata)
            .await
            .unwrap();
        // An empty reservation may be superseded. Cleanup of its old attempt must
        // not withdraw the independent, successfully admitted replacement.
        service
            .discard_staged(&target, AgentMode::Durable, old_id, old.stage_token())
            .await
            .unwrap();
        current.add(OplogEntry::suspend()).await.unwrap();
        let result = current.commit(CommitLevel::Always).await;
        if let Err(error) = result {
            failures.push(format!("backend={backend}: {error:?}"));
        }
        drop(old);
    }
    assert!(
        failures.is_empty(),
        "discarding a superseded stage invalidated the current writer: {failures:?}"
    );
}

#[test]
async fn stale_seal_cleanup_preserves_rejections_persisted_by_a_new_owner() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for initial in [None, Some(vec![5])] {
            let f = Fixture::new(indexed.clone()).await;
            if let Some(initial) = initial {
                f.reject(&initial).await;
            }
            let service = f.service();
            let pending = service
                .prepare(f.metadata.clone(), ShardEpoch(6))
                .await
                .unwrap()
                .unwrap();
            // The old owner pauses after snapshotting. A new owner extends the rejection field
            // through the same compare-and-set boundary as the worker service.
            f.register(7, true);
            assert!(service.claim(&f.id(), ShardEpoch(7)).await.unwrap());
            let namespace = KeyValueStorageNamespace::AgentRejectedPeriodicSnapshots {
                owned_agent_id: f.id(),
            };
            let field = f.metadata.fingerprint.0.to_string();
            let kv = f.kv.with_entity("test", "new_owner_rejections", "snapshot");
            let current = kv.get_raw(namespace.clone(), &field).await.unwrap();
            let indexes = vec![OplogIndex::from_u64(5), OplogIndex::from_u64(8)];
            let encoded = serialize(&indexes).unwrap();
            assert!(
                kv.compare_and_set_many_raw(
                    namespace.clone(),
                    &field,
                    current.as_deref(),
                    &[],
                    &[(&field, &encoded)]
                )
                .await
                .unwrap()
            );
            service.remove_moved_copies(&pending).await.unwrap();
            assert!(matches!(
                service.release(&pending).await,
                Err(WorkerExecutorError::OplogFenced { .. })
            ));
            let preserved = kv
                .get::<Vec<OplogIndex>>(namespace, &field)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(preserved, indexes);
            let mut rejections = f
                .service()
                .rejected_snapshots(&f.id(), f.metadata.fingerprint)
                .await
                .unwrap();
            rejections.extend(preserved);
            assert_eq!(
                rejections,
                BTreeSet::from([OplogIndex::from_u64(5), OplogIndex::from_u64(8)])
            );
            let later = service
                .prepare(f.metadata.clone(), ShardEpoch(7))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                later.record.rejected_periodic_snapshots,
                BTreeSet::from([OplogIndex::from_u64(5), OplogIndex::from_u64(8)])
            );
            service.remove_moved_copies(&later).await.unwrap();
            assert!(service.release(&later).await.unwrap());
        }
    }
}

#[test]
#[test_r::timeout("30s")]
async fn failed_stage_admission_does_not_fence_a_competing_create() {
    use crate::services::oplog::CommitLevel;
    use crate::services::oplog::tests::{default_execution_status, default_last_known_status};

    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for create_first in [false, true] {
            let mut f = Fixture::new(indexed.clone()).await;
            f.metadata.agent_id = AgentId::from_parts(7, 2267);
            let target = f.id();
            let service = Arc::new(f.oplog_service().await);
            let (probed, resume) = f
                .blobs
                .pause_seal_list_after(if create_first { 1 } else { 0 });
            let creating = async {
                service
                    .create(
                        &mut service.lock_lifecycle(&target.agent_id).await,
                        &target,
                        AgentMode::Durable,
                        OplogEntry::suspend(),
                        f.metadata.clone(),
                        default_last_known_status(),
                        default_execution_status(AgentMode::Durable),
                        Some(ShardEpoch(6)),
                    )
                    .await
            };
            let admission = service.create_staged(
                &target,
                AgentMode::Durable,
                Uuid::new_v4(),
                f.metadata.clone(),
            );
            tokio::pin!(creating, admission);
            if create_first {
                tokio::select! {
                    result = &mut creating => panic!("create did not pause: {result:?}"),
                    _ = probed => {}
                }
                assert!(futures::poll!(&mut admission).is_pending());
                resume.send(()).unwrap();
            } else {
                tokio::select! {
                    result = &mut admission => panic!("admission did not pause: {result:?}"),
                    _ = probed => {}
                }
                assert!(futures::poll!(&mut creating).is_pending());
                f.blobs.fail_next_seal_list();
                resume.send(()).unwrap();
                assert!(matches!(
                    (&mut admission).await,
                    Err(CreateStagedError::Retryable(_))
                ));
            }
            let oplog = creating.await.unwrap();
            if create_first {
                assert!(matches!(
                    admission.await,
                    Err(CreateStagedError::TargetPresent)
                ));
            }
            oplog.add(OplogEntry::exited()).await.unwrap();
            let committed = oplog.commit(CommitLevel::Always).await;
            assert!(
                committed.is_ok(),
                "failed stage admission fenced the concurrent create although the shard stayed at epoch 6: {committed:?}"
            );
            assert_eq!(
                service
                    .read_exact(&target, AgentMode::Durable, OplogIndex::INITIAL, 2)
                    .await
                    .unwrap()
                    .len(),
                2
            );
        }
    }
}

#[test]
async fn sealed_ephemeral_open_is_frozen_and_leaves_no_indexed_rows() {
    use crate::services::oplog::tests::{default_execution_status, default_last_known_status};
    use crate::services::oplog::{CommitLevel, EphemeralOplog, OplogError};

    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::with_mode(indexed, AgentMode::Ephemeral).await;
        let seal = f.service();
        let pending = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        seal.remove_moved_copies(&pending).await.unwrap();
        assert!(seal.release(&pending).await.unwrap());
        f.assert_epoch(&pending.state, None).await;
        let service = f.oplog_service().await;
        for epoch in [None, Some(ShardEpoch(6)), Some(ShardEpoch(7))] {
            let oplog = service
                .open(
                    &mut service.lock_lifecycle(&f.id().agent_id).await,
                    &f.id(),
                    AgentMode::Ephemeral,
                    None,
                    f.metadata.clone(),
                    default_last_known_status(),
                    default_execution_status(AgentMode::Ephemeral),
                    epoch,
                )
                .await
                .unwrap();
            let fence = oplog
                .fence()
                .expect("sealed ephemeral handle must refuse writes");
            assert_eq!(oplog.shard_epoch(), None);
            assert_eq!(oplog.current_oplog_index().await, OplogIndex::from_u64(9));
            assert_eq!(
                oplog.read_exact(OplogIndex::INITIAL, 9).await.unwrap(),
                f.original
            );
            let raw = oplog
                .raw_durable_stream_session_status(
                    &golem_common::model::durable_stream::StreamSessionKey {
                        callee_environment_id: f.id().environment_id(),
                        callee: f.id().agent_id,
                        callee_fingerprint: f.metadata.fingerprint,
                        idempotency_key: golem_common::model::InvocationId::constructor(
                            f.metadata.fingerprint,
                        ),
                    },
                )
                .await;
            assert_eq!(raw.watermark, pending.record.last_index);
            assert!(raw.status.unwrap().is_none());
            assert_eq!(oplog.length().await.unwrap(), 9);
            assert_eq!(
                oplog.add(OplogEntry::suspend()).await,
                Err(OplogError::Fenced(fence.clone()))
            );
            assert_eq!(
                oplog.commit(CommitLevel::Always).await,
                Err(OplogError::Fenced(fence.clone()))
            );
            assert!(oplog.upload_raw_payload(vec![1; 2048]).await.is_err());
            assert_eq!(
                oplog
                    .enqueue_add_pair(
                        OplogEntry::suspend(),
                        Box::new(|_| panic!("frozen pair builder ran"))
                    )
                    .await,
                Err(OplogError::Fenced(fence.clone()))
            );
            assert!(matches!(
                oplog
                    .add_start_with_indexed_reserved_raw_payload(Box::new(|_| panic!(
                        "frozen reserved builder ran"
                    )))
                    .await,
                Err(OplogError::Fenced(actual)) if actual == fence
            ));
            assert_eq!(
                EphemeralOplog::try_archive_blocking(&oplog).await.unwrap(),
                None
            );
            oplog.stop_and_wait().await.unwrap();
            assert!(
                !f.indexed
                    .observe_agent("test", "frozen", &f.id())
                    .await
                    .unwrap()
                    .has_data
            );
            f.assert_epoch(&pending.state, None).await;
            assert!(f.scheduler.actions.lock().unwrap().is_empty());
        }
        assert!(seal.unseal(&f.id(), ShardEpoch(7)).await.is_err());
    }
}

#[test]
async fn prepare_refuses_worker_identity_that_disagrees_with_create_or_trimmed_marker() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for trimmed in [false, true] {
            let f = Fixture::new(indexed.clone()).await;
            let seal = f.service();
            if trimmed {
                f.archive(1)
                    .open(&f.id(), AgentMode::Durable, None, f.metadata.fingerprint)
                    .await
                    .unwrap()
                    .drop_prefix(OplogIndex::from_u64(3))
                    .await
                    .unwrap();
                let pending = seal
                    .prepare(f.metadata.clone(), ShardEpoch(6))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(seal.release(&pending).await.unwrap());
                seal.unseal(&f.id(), ShardEpoch(6)).await.unwrap();
            }
            let mut wrong = f.metadata.clone();
            wrong.fingerprint = AgentFingerprint::new();
            assert!(seal.prepare(wrong, ShardEpoch(6)).await.is_err());
            let pending = seal
                .prepare(f.metadata.clone(), ShardEpoch(6))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(pending.record.fingerprint, f.metadata.fingerprint);
        }
    }
}

#[test]
async fn fresh_ephemeral_create_supersedes_stale_marker_identity_when_sealed() {
    use crate::services::oplog::tests::{default_execution_status, default_last_known_status};
    use crate::services::oplog::{CommitLevel, EphemeralOplog};

    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::with_mode(indexed, AgentMode::Ephemeral).await;
        f.put_promise(3, Some(vec![1])).await;
        f.reject(&[8]).await;
        let seal = f.service();
        let old = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        seal.remove_moved_copies(&old).await.unwrap();
        let stale_marker = f
            .indexed
            .observe_agent("test", "old_marker", &f.id())
            .await
            .unwrap()
            .ephemeral_marker
            .unwrap();
        assert_eq!(stale_marker.fingerprint, f.metadata.fingerprint);
        assert!(seal.release(&old).await.unwrap());
        let mut replacement = f.metadata.clone();
        replacement.fingerprint = AgentFingerprint::new();
        replacement.last_known_status.oplog_idx = OplogIndex::INITIAL;
        let service = f.oplog_service().await;
        let oplog = service
            .create_fresh(
                &mut service.lock_lifecycle(&f.id().agent_id).await,
                &f.id(),
                AgentMode::Ephemeral,
                Fixture::initial_entry(&replacement),
                replacement.clone(),
                default_last_known_status(),
                default_execution_status(AgentMode::Ephemeral),
                Some(ShardEpoch(6)),
            )
            .await
            .unwrap();
        oplog.commit(CommitLevel::Always).await.unwrap();
        // Drain the compressed layer into blob storage before attempting a seal.
        for _ in 0..3 {
            EphemeralOplog::try_archive_blocking(&oplog).await.unwrap();
        }
        oplog.stop_and_wait().await.unwrap();
        // Reproduce a stale marker under the replacement's readable indexed Create.
        f.indexed
            .append(
                "test",
                "stale_marker",
                "marker",
                old.state.marker_namespace(),
                &f.id().storage_key(),
                0,
                serialize(&stale_marker).unwrap(),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            f.indexed
                .observe_agent("test", "stale_marker", &f.id())
                .await
                .unwrap()
                .ephemeral_marker,
            Some(stale_marker)
        );
        let cold = f.service();
        assert_eq!(
            cold.indexed_identity(&f.id()).await.unwrap(),
            Some(replacement.fingerprint)
        );
        assert_eq!(
            cold.completed_promise(f.id().environment_id(), &f.promise(3))
                .await
                .unwrap(),
            None
        );
        assert!(
            cold.rejected_snapshots(&f.id(), f.metadata.fingerprint)
                .await
                .unwrap()
                .is_empty()
        );
        let pending = cold
            .prepare(replacement.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.record.fingerprint, replacement.fingerprint);
        assert!(pending.record.completed_promises.is_empty());
        assert!(pending.record.rejected_periodic_snapshots.is_empty());
        assert_eq!(
            f.indexed
                .observe_agent("test", "replacement_marker", &f.id())
                .await
                .unwrap()
                .ephemeral_marker
                .unwrap()
                .fingerprint,
            replacement.fingerprint
        );
        assert!(cold.release(&pending).await.unwrap());
        assert!(
            matches!(f.service().state(&f.id()).await.unwrap(), AgentSealState::Sealed(record)
            if record.fingerprint == replacement.fingerprint)
        );
    }
}

struct SealPromiseAccess(Arc<SealService>);

// A promise that crosses the KV-to-seal transition must keep the first completion,
// including when two independent callers complete it and cleanup is retried.
#[test]
async fn ephemeral_promise_competing_completions_across_seal_cleanup() {
    use crate::services::promise::{DefaultPromiseService, PromiseService};

    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for completion_phase in 0..3 {
            let f = Fixture::with_mode(indexed.clone(), AgentMode::Ephemeral).await;
            f.put_promise(2, None).await;
            let seal = Arc::new(f.service());
            let access = Arc::new(SealPromiseAccess(seal.clone()));
            let first = DefaultPromiseService::new(f.kv.clone(), access.clone());
            let second = DefaultPromiseService::new(f.kv.clone(), access.clone());
            let pending_handle = first
                .poll(f.id().environment_id(), f.promise(2))
                .await
                .unwrap();
            let pending = seal
                .prepare(f.metadata.clone(), ShardEpoch(6))
                .await
                .unwrap()
                .unwrap();
            if completion_phase >= 1 {
                seal.remove_moved_copies(&pending).await.unwrap();
            }
            if completion_phase >= 2 {
                assert!(seal.release(&pending).await.unwrap());
            }
            let (a, b) = tokio::join!(
                first.complete_without_wakeup(f.id().environment_id(), f.promise(2), vec![11]),
                second.complete_without_wakeup(f.id().environment_id(), f.promise(2), vec![22]),
            );
            let (a, b) = (a.unwrap(), b.unwrap());
            assert_ne!(a, b, "exactly one completion must win");
            let expected = if a { vec![11] } else { vec![22] };
            seal.remove_moved_copies(&pending).await.unwrap();
            if completion_phase < 2 {
                assert!(seal.release(&pending).await.unwrap());
            }
            seal.remove_moved_copies(&pending).await.unwrap();
            let remote = DefaultPromiseService::new(f.kv.clone(), access);
            for service in [&first, &second, &remote] {
                assert_eq!(
                    service
                        .poll(f.id().environment_id(), f.promise(2))
                        .await
                        .unwrap()
                        .get()
                        .await,
                    Some(expected.clone()),
                );
                assert!(
                    !service
                        .complete_without_wakeup(f.id().environment_id(), f.promise(2), vec![99],)
                        .await
                        .unwrap()
                );
            }
            assert_eq!(pending_handle.get().await, Some(expected));
        }
    }
}

#[async_trait::async_trait]
impl crate::services::promise::PromiseWorkerAccess for SealPromiseAccess {
    async fn pending_promise(
        &self,
        env: EnvironmentId,
        id: &PromiseId,
    ) -> Result<bool, WorkerExecutorError> {
        self.0.pending_promise(env, id).await
    }
    async fn completed_promise(
        &self,
        env: EnvironmentId,
        id: &PromiseId,
    ) -> Result<Option<Vec<u8>>, WorkerExecutorError> {
        self.0.completed_promise(env, id).await
    }
    async fn activate_worker_if_needed(
        &self,
        _env: EnvironmentId,
        _id: &PromiseId,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }
    async fn run_excluding_removal<'a>(
        &self,
        _id: &OwnedAgentId,
        operation: futures::future::BoxFuture<'a, Result<bool, WorkerExecutorError>>,
    ) -> Result<bool, WorkerExecutorError> {
        operation.await
    }
}

#[test]
async fn sealed_ephemeral_pending_promise_completes_once_and_cross_agent_poll_sees_payload() {
    use crate::services::promise::{DefaultPromiseService, PromiseService};
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::with_mode(indexed, AgentMode::Ephemeral).await;
        f.put_promise(2, None).await;
        f.put_promise(4, Some(vec![4, 1])).await;
        let seal = Arc::new(f.service());
        let pending = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            pending.record.pending_promises,
            BTreeSet::from([OplogIndex::from_u64(2)])
        );
        let access = Arc::new(SealPromiseAccess(seal.clone()));
        let promises = DefaultPromiseService::new(f.kv.clone(), access.clone());
        let local = promises
            .poll(f.id().environment_id(), f.promise(2))
            .await
            .unwrap();
        seal.remove_moved_copies(&pending).await.unwrap();
        assert!(seal.release(&pending).await.unwrap());
        assert!(
            promises
                .complete_without_wakeup(f.id().environment_id(), f.promise(2), vec![2, 7, 9])
                .await
                .unwrap()
        );
        // A delayed cleanup of this same snapshot must preserve the external completion.
        seal.remove_moved_copies(&pending).await.unwrap();
        assert_eq!(local.get().await, Some(vec![2, 7, 9]));
        assert!(
            !promises
                .complete_without_wakeup(f.id().environment_id(), f.promise(2), vec![0])
                .await
                .unwrap()
        );
        let remote = DefaultPromiseService::new(f.kv.clone(), access);
        assert_eq!(
            remote
                .poll(f.id().environment_id(), f.promise(2))
                .await
                .unwrap()
                .get()
                .await,
            Some(vec![2, 7, 9])
        );
        assert_eq!(
            remote
                .poll(f.id().environment_id(), f.promise(4))
                .await
                .unwrap()
                .get()
                .await,
            Some(vec![4, 1])
        );
        assert!(
            !remote
                .complete_without_wakeup(f.id().environment_id(), f.promise(4), vec![0])
                .await
                .unwrap()
        );
        let fields =
            f.kv.with_entity("test", "remaining", "promise")
                .get_all_raw(promise_namespace_of(&f.id(), f.metadata.fingerprint))
                .await
                .unwrap();
        assert_eq!(
            fields.len(),
            1,
            "only the external completion survives seal"
        );
        assert_eq!(
            fields[0].0,
            format!(
                "{}:completed",
                f.promise(2).storage_key(f.id().environment_id())
            )
        );
        f.assert_epoch(&pending.state, None).await;
        assert!(
            !f.indexed
                .observe_agent("test", "promise", &f.id())
                .await
                .unwrap()
                .has_data
        );
    }
}
