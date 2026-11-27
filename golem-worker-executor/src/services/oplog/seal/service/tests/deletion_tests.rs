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
use crate::storage::scheduler::SchedulerStorage;
use crate::storage::scheduler::memory::InMemorySchedulerStorage;
use test_r::test;

fn mature_collections(f: &mut Fixture) {
    f.config.shard_lease_max_ttl = Duration::ZERO;
    f.config.seal_put_timeout = Duration::ZERO;
    f.config.server_commit_bound = Duration::ZERO;
    f.config.skew_margin = Duration::ZERO;
}

async fn seal_fixture(f: &Fixture) -> PendingSeal {
    let seal = f.service();
    let pending = seal
        .prepare(f.metadata.clone(), ShardEpoch(6))
        .await
        .unwrap()
        .unwrap();
    seal.remove_moved_copies(&pending).await.unwrap();
    assert!(seal.release(&pending).await.unwrap());
    pending
}

async fn assert_fence(f: &Fixture, namespace: IndexedStorageNamespace, actual: Option<ShardEpoch>) {
    let result = f
        .indexed
        .append(
            "test",
            "deletion_fence",
            "oplog",
            namespace,
            &f.id().storage_key(),
            1000,
            vec![],
            Some(ShardEpoch(1)),
            None,
        )
        .await;
    assert!(
        matches!(result, Err(IndexedStorageError::Fenced { actual: found, .. }) if found == actual),
        "expected fence {actual:?}, got {result:?}"
    );
}

async fn put_record(f: &Fixture, key: SealRecordKey, bytes: &[u8]) {
    f.blobs
        .with("test", "deletion_record")
        .put_raw(
            seal_namespace(&f.id(), f.metadata.agent_mode),
            &key.path(),
            bytes,
        )
        .await
        .unwrap();
}

async fn record_bytes(f: &Fixture, path: &Path) -> Option<Vec<u8>> {
    f.blobs
        .with("test", "deletion_record_bytes")
        .get_raw(seal_namespace(&f.id(), f.metadata.agent_mode), path)
        .await
        .unwrap()
}

async fn put_tombstone(
    f: &Fixture,
    fingerprint: AgentFingerprint,
    epoch: u64,
    seq: u64,
) -> SealRecordKey {
    let key = SealRecordKey {
        agent_id: f.id().agent_id,
        fingerprint,
        epoch: ShardEpoch(epoch),
        seq,
        kind: SealRecordKind::Deleted,
    };
    put_record(
        f,
        key,
        &serialize(&DeletionRecord {
            writer_nonce: Uuid::new_v4(),
            writer_epoch: key.epoch,
            fingerprint,
        })
        .unwrap(),
    )
    .await;
    key
}

async fn put_seal(
    f: &Fixture,
    template: &SealRecord,
    fingerprint: AgentFingerprint,
    epoch: u64,
    seq: u64,
) -> SealRecordKey {
    let mut record = template.clone();
    record.writer_epoch = ShardEpoch(epoch);
    record.fingerprint = fingerprint;
    record.metadata.fingerprint = fingerprint;
    let key = SealRecordKey {
        agent_id: f.id().agent_id,
        fingerprint,
        epoch: record.writer_epoch,
        seq,
        kind: SealRecordKind::Sealed {
            valid_until_ms: u64::MAX,
        },
    };
    put_record(f, key, &serialize(&record).unwrap()).await;
    key
}

fn chunk_sentinel(f: &Fixture, fingerprint: AgentFingerprint) -> (BlobStorageNamespace, PathBuf) {
    (
        BlobStorageNamespace::CompressedOplog {
            environment_id: f.id().environment_id(),
            type_tag: f.id().agent_id.type_tag(),
            agent_mode: AgentMode::Durable,
            level: 0,
        },
        Path::new("chunks")
            .join(agent_path_segment(&f.id().agent_id))
            .join(fingerprint.to_string())
            .join("late-chunk"),
    )
}

async fn put_chunk_sentinel(f: &Fixture, fingerprint: AgentFingerprint) {
    let (namespace, path) = chunk_sentinel(f, fingerprint);
    f.blobs
        .with("test", "late_chunk")
        .put_raw(namespace, &path, b"late chunk bytes")
        .await
        .unwrap();
}

async fn assert_chunk_sentinel(f: &Fixture, fingerprint: AgentFingerprint, present: bool) {
    let (namespace, path) = chunk_sentinel(f, fingerprint);
    let bytes = f
        .blobs
        .with("test", "late_chunk_after_gc")
        .get_raw(namespace, &path)
        .await
        .unwrap();
    assert_eq!(
        bytes.as_deref(),
        present.then_some(&b"late chunk bytes"[..])
    );
}

fn create_entry(f: &Fixture, fingerprint: AgentFingerprint) -> OplogEntry {
    let mut metadata = f.metadata.clone();
    metadata.fingerprint = fingerprint;
    Fixture::initial_entry(&metadata)
}

async fn write_primary_entry(
    f: &Fixture,
    state: &AgentIndexedState,
    entry: &OplogEntry,
    epoch: ShardEpoch,
) {
    f.indexed
        .set_key_epoch(
            "test",
            "primary_identity_epoch",
            state.primary_namespace(),
            &f.id().storage_key(),
            epoch,
            &[],
            false,
        )
        .await
        .unwrap();
    f.indexed
        .append(
            "test",
            "primary_identity",
            "oplog",
            state.primary_namespace(),
            &f.id().storage_key(),
            OplogIndex::INITIAL.as_u64(),
            serialize(entry).unwrap(),
            Some(epoch),
            None,
        )
        .await
        .unwrap();
}

#[test]
#[test_r::timeout("30s")]
async fn refused_create_and_publication_preserve_equal_epoch_deletion_fence_before_gc() {
    use crate::services::oplog::tests::{default_execution_status, default_last_known_status};
    use crate::services::oplog::{OplogOpenPurpose, StagePublication};

    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for publication in [false, true] {
            let mut f = Fixture::new(indexed.clone()).await;
            let pending = seal_fixture(&f).await;
            // Keep an independently leased old owner and its selected seal across deletion.
            let stale = f.service();
            f.shard = Arc::new(ShardServiceDefault::new());
            f.register(7, true);
            let oplog = f.oplog_service().await;
            oplog
                .delete(
                    &mut oplog.lock_lifecycle(&f.id().agent_id).await,
                    &f.id(),
                    AgentMode::Durable,
                    f.metadata.fingerprint,
                    Some(ShardEpoch(7)),
                )
                .await
                .unwrap();
            let records = list_records(f.blobs.as_ref(), &f.id(), AgentMode::Durable, "test")
                .await
                .unwrap();
            assert_eq!(records.len(), 1);
            let tombstone = records[0].key;
            assert_eq!(tombstone.kind, SealRecordKind::Deleted);
            assert_fence(&f, pending.state.fence_namespace(), Some(ShardEpoch(7))).await;

            let stage_id = Uuid::new_v4();
            let mut cleanup = None;
            let token = if publication {
                let (stage, guard) = oplog
                    .create_staged(&f.id(), AgentMode::Durable, stage_id, f.metadata.clone())
                    .await
                    .unwrap();
                cleanup = Some(guard);
                stage.stop_and_wait().await.unwrap();
                let token = stage.stage_token();
                drop(stage);
                token
            } else {
                None
            };
            // A newer sealed target appears after admission, before publication or create.
            let newer = put_seal(&f, &pending.record, AgentFingerprint::new(), 8, 0).await;
            let before = record_bytes(&f, &newer.path()).await.unwrap();
            {
                let mut lifecycle = oplog.lock_lifecycle(&f.id().agent_id).await;
                if publication {
                    assert!(
                        !oplog
                            .publish_staged(
                                &f.id(),
                                AgentMode::Durable,
                                StagePublication::for_tests(stage_id, token),
                                OplogIndex::INITIAL,
                            )
                            .await
                            .unwrap()
                    );
                } else {
                    assert!(matches!(
                        oplog
                            .create(
                                &mut lifecycle,
                                &f.id(),
                                AgentMode::Durable,
                                create_entry(&f, f.metadata.fingerprint),
                                f.metadata.clone(),
                                default_last_known_status(),
                                default_execution_status(AgentMode::Durable),
                                Some(ShardEpoch(7)),
                            )
                            .await,
                        Err(WorkerExecutorError::AgentAlreadyExists { .. })
                    ));
                }
            }
            if let Some(cleanup) = &mut cleanup {
                cleanup.discard().await.unwrap();
            }
            assert_fence(&f, pending.state.fence_namespace(), Some(ShardEpoch(7))).await;
            assert!(stale.deadline(&f.id(), ShardEpoch(6)).is_some());
            assert!(matches!(
                stale
                    .unseal_record(
                        &f.id(),
                        ShardEpoch(6),
                        &pending.record,
                        OplogOpenPurpose::Activation,
                    )
                    .await,
                Err(WorkerExecutorError::OplogFenced { .. })
            ));
            assert!(
                !f.indexed
                    .observe_agent("test", "refused_target", &f.id())
                    .await
                    .unwrap()
                    .has_data
            );
            assert_fence(&f, pending.state.fence_namespace(), Some(ShardEpoch(7))).await;
            assert!(record_bytes(&f, &tombstone.path()).await.is_some());
            assert_eq!(record_bytes(&f, &newer.path()).await, Some(before));
        }
    }
}

#[test]
async fn stale_deletion_refuses_every_newer_listed_key_before_claim_or_tombstone() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for kind in [
            SealRecordKind::Deleted,
            SealRecordKind::Sealed { valid_until_ms: 0 },
        ] {
            let f = Fixture::new(indexed.clone()).await;
            let pending = seal_fixture(&f).await;
            let newer = SealRecordKey {
                agent_id: f.id().agent_id,
                fingerprint: f.metadata.fingerprint,
                epoch: ShardEpoch(8),
                seq: 0,
                kind,
            };
            // Even an inadmissible or unreadable newer key sets the deletion lower bound.
            put_record(&f, newer, b"unreadable body").await;
            let before = list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
                .await
                .unwrap();
            let oplog = f.oplog_service().await;
            let seal = f.service();
            let mut lifecycle = oplog.lock_lifecycle(&f.id().agent_id).await;
            for result in [
                seal.prepare_deletion(
                    &mut lifecycle,
                    &f.id(),
                    f.metadata.agent_mode,
                    ShardEpoch(7),
                )
                .await,
                oplog
                    .delete(
                        &mut lifecycle,
                        &f.id(),
                        AgentMode::Durable,
                        f.metadata.fingerprint,
                        Some(ShardEpoch(7)),
                    )
                    .await,
            ] {
                assert!(matches!(
                    result,
                    Err(OplogError::Fenced(OplogFence {
                        expected_epoch: ShardEpoch(7),
                        actual_epoch: Some(ShardEpoch(8)),
                        ..
                    }))
                ));
                f.assert_epoch(&pending.state, None).await;
            }
            let after = list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
                .await
                .unwrap();
            assert_eq!(
                after.iter().map(|record| record.key).collect::<Vec<_>>(),
                before.iter().map(|record| record.key).collect::<Vec<_>>()
            );
            assert_eq!(
                record_bytes(&f, &newer.path()).await.unwrap(),
                &b"unreadable body"[..]
            );
            f.assert_history(&pending.record).await;
            assert!(f.scheduler.actions.lock().unwrap().is_empty());
        }
    }
}

#[test]
async fn sealed_deletion_retains_only_the_owning_fence_and_isolates_chunk_paths() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
            let f = Fixture::with_mode(indexed.clone(), mode).await;
            let pending = seal_fixture(&f).await;
            let other_mode = match mode {
                AgentMode::Durable => AgentMode::Ephemeral,
                AgentMode::Ephemeral => AgentMode::Durable,
            };
            let other_state = f
                .service()
                .indexed_state(&f.id(), other_mode, &SealManifest::new())
                .unwrap();
            f.indexed
                .set_key_epoch(
                    "test",
                    "other_mode_fence",
                    other_state.fence_namespace(),
                    &f.id().storage_key(),
                    ShardEpoch(6),
                    &[],
                    false,
                )
                .await
                .unwrap();
            let other_key = SealRecordKey {
                epoch: ShardEpoch(8),
                ..pending.key
            };
            f.blobs
                .with("test", "other_mode_record")
                .put_raw(
                    seal_namespace(&f.id(), other_mode),
                    &other_key.path(),
                    b"other mode record",
                )
                .await
                .unwrap();
            let other_fingerprint = AgentFingerprint::new();
            let other_agent = AgentId::from_parts(7, 9999);
            let mut sentinels = Vec::new();
            for chunk_mode in [AgentMode::Durable, AgentMode::Ephemeral] {
                for level in 0..2 {
                    let namespace = BlobStorageNamespace::CompressedOplog {
                        environment_id: f.id().environment_id(),
                        type_tag: f.id().agent_id.type_tag(),
                        agent_mode: chunk_mode,
                        level,
                    };
                    for (agent, fingerprint, keep) in [
                        (f.id().agent_id, f.metadata.fingerprint, chunk_mode != mode),
                        (f.id().agent_id, other_fingerprint, true),
                        (other_agent, f.metadata.fingerprint, true),
                    ] {
                        let path = Path::new("chunks")
                            .join(agent_path_segment(&agent))
                            .join(fingerprint.to_string())
                            .join("sentinel");
                        f.blobs
                            .with("test", "chunk_sentinel")
                            .put_raw(namespace.clone(), &path, b"keep exact bytes")
                            .await
                            .unwrap();
                        sentinels.push((namespace.clone(), path, keep));
                    }
                }
            }
            let oplog = f.oplog_service().await;
            oplog
                .delete(
                    &mut oplog.lock_lifecycle(&f.id().agent_id).await,
                    &f.id(),
                    mode,
                    f.metadata.fingerprint,
                    Some(ShardEpoch(6)),
                )
                .await
                .unwrap();
            assert_fence(&f, other_state.fence_namespace(), Some(ShardEpoch(6))).await;
            assert_eq!(
                f.blobs
                    .with("test", "other_mode_after_delete")
                    .get_raw(seal_namespace(&f.id(), other_mode), &other_key.path())
                    .await
                    .unwrap()
                    .unwrap(),
                b"other mode record"
            );
            for namespace in pending.state.epoch_namespaces() {
                let epoch = (namespace == pending.state.fence_namespace()).then_some(ShardEpoch(6));
                assert_fence(&f, namespace, epoch).await;
            }
            assert!(
                !f.indexed
                    .observe_agent("test", "deleted", &f.id())
                    .await
                    .unwrap()
                    .has_data
            );
            assert!(!oplog.exists(&f.id(), mode).await.unwrap());
            let records = list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
                .await
                .unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].key.kind, SealRecordKind::Deleted);
            assert!(matches!(
                read_record(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, &records[0]).await.unwrap(),
                SealRecordBody::Deleted(body)
                    if body.fingerprint == f.metadata.fingerprint && body.writer_epoch == ShardEpoch(6)
            ));
            for (namespace, path, keep) in sentinels {
                let bytes = f
                    .blobs
                    .with("test", "chunk_after_delete")
                    .get_raw(namespace, &path)
                    .await
                    .unwrap();
                assert_eq!(
                    bytes.as_deref(),
                    keep.then_some(&b"keep exact bytes"[..]),
                    "{}",
                    path.display()
                );
            }
        }
    }
}

#[test]
async fn collection_without_tombstone_preserves_seal_chunks_and_preexisting_fence() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::new(indexed).await;
        let pending = seal_fixture(&f).await;
        f.indexed
            .set_key_epoch(
                "test",
                "preexisting_fence",
                pending.state.fence_namespace(),
                &f.id().storage_key(),
                ShardEpoch(6),
                &[],
                false,
            )
            .await
            .unwrap();
        let before = record_bytes(&f, &pending.key.path()).await.unwrap();
        let oplog = f.oplog_service().await;
        oplog
            .collect_tombstone(&f.id(), f.metadata.agent_mode)
            .await
            .unwrap();
        assert_eq!(record_bytes(&f, &pending.key.path()).await, Some(before));
        assert_fence(&f, pending.state.fence_namespace(), Some(ShardEpoch(6))).await;
        f.assert_history(&pending.record).await;
        assert!(f.scheduler.actions.lock().unwrap().is_empty());
    }
}

#[derive(Default)]
struct CollectionScheduler {
    times: Mutex<Vec<jiff::Timestamp>>,
    memory: Option<InMemorySchedulerStorage>,
}

#[async_trait::async_trait]
impl SchedulerService for CollectionScheduler {
    fn is_durable(&self) -> bool {
        self.memory
            .as_ref()
            .is_none_or(|storage| storage.is_durable())
    }

    async fn schedule(&self, time: jiff::Timestamp, action: ScheduledAction) -> ScheduleId {
        self.schedule_with_id(ScheduleId::fresh(), time, action)
            .await
    }

    async fn schedule_with_id(
        &self,
        id: ScheduleId,
        time: jiff::Timestamp,
        action: ScheduledAction,
    ) -> ScheduleId {
        assert!(matches!(action, ScheduledAction::CollectTombstone { .. }));
        self.times.lock().unwrap().push(time);
        id
    }

    async fn cancel(&self, _id: ScheduleId) {}
}

#[test]
async fn collection_stops_at_the_first_immature_tombstone_without_mutation() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::new(indexed).await;
        let pending = seal_fixture(&f).await;
        let recording = Arc::new(CollectionScheduler::default());
        let mut seal = f.service();
        let scheduler: Arc<dyn SchedulerService> = recording.clone();
        seal.scheduler = Arc::downgrade(&scheduler);
        let oplog = f.oplog_service().await;
        let mut lifecycle = oplog.lock_lifecycle(&f.id().agent_id).await;
        for _ in 0..2 {
            // Keep both committed tombstones, as if deletion stopped during chunk cleanup.
            f.blobs.fail_next_delete_dir();
            assert!(matches!(
                seal.delete_with_tombstone(
                    &mut lifecycle,
                    &f.id(),
                    AgentMode::Durable,
                    f.metadata.fingerprint,
                    Some(ShardEpoch(6))
                )
                .await,
                Err(OplogError::Maintenance(_))
            ));
        }
        let records = list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
            .await
            .unwrap();
        let tombstones: Vec<_> = records
            .iter()
            .filter(|record| record.key.kind == SealRecordKind::Deleted)
            .collect();
        assert_eq!(tombstones.len(), 2);
        let earliest = tombstones.iter().min_by_key(|record| record.key).unwrap();
        let timestamp: prost_types::Timestamp = earliest.entry.last_modified_at.into();
        let modified = UNIX_EPOCH
            + Duration::from_secs(timestamp.seconds as u64)
            + Duration::from_nanos(timestamp.nanos as u64);
        let delay = f.config.shard_lease_max_ttl
            + f.config.seal_put_timeout
            + f.config.server_commit_bound
            + f.config.skew_margin * 2;
        recording.times.lock().unwrap().clear();
        seal.collect_tombstone(&mut lifecycle, &f.id(), f.metadata.agent_mode)
            .await
            .unwrap();
        assert_eq!(
            *recording.times.lock().unwrap(),
            vec![jiff::Timestamp::try_from(modified + delay).unwrap()]
        );
        for record in records {
            assert!(record_bytes(&f, &record.entry.path).await.is_some());
        }
        assert_fence(&f, pending.state.fence_namespace(), Some(ShardEpoch(6))).await;
        f.assert_history(&pending.record).await;
    }
}

#[test]
async fn matured_collection_removes_only_its_region_and_preserves_all_newer_keys() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let mut f = Fixture::new(indexed).await;
        let pending = seal_fixture(&f).await;
        mature_collections(&mut f);
        let oplog = f.oplog_service().await;
        oplog
            .delete(
                &mut oplog.lock_lifecycle(&f.id().agent_id).await,
                &f.id(),
                AgentMode::Durable,
                f.metadata.fingerprint,
                Some(ShardEpoch(6)),
            )
            .await
            .unwrap();
        let mut newer = pending.record.clone();
        newer.writer_epoch = ShardEpoch(8);
        newer.fingerprint = AgentFingerprint::new();
        newer.metadata.fingerprint = newer.fingerprint;
        let admitted = SealRecordKey {
            agent_id: f.id().agent_id,
            fingerprint: newer.fingerprint,
            epoch: ShardEpoch(8),
            seq: 0,
            kind: SealRecordKind::Sealed {
                valid_until_ms: u64::MAX,
            },
        };
        put_record(&f, admitted, &serialize(&newer).unwrap()).await;
        let older = SealRecordKey {
            fingerprint: f.metadata.fingerprint,
            epoch: ShardEpoch(5),
            ..admitted
        };
        put_record(&f, older, b"older body need not be readable").await;
        let inadmissible = SealRecordKey {
            epoch: ShardEpoch(9),
            kind: SealRecordKind::Sealed { valid_until_ms: 0 },
            ..admitted
        };
        put_record(&f, inadmissible, b"late upload").await;
        let malformed = seal_prefix(&f.id().agent_id).join("not-a-record.sealed");
        f.blobs
            .with("test", "malformed_record")
            .put_raw(
                seal_namespace(&f.id(), f.metadata.agent_mode),
                &malformed,
                b"malformed sentinel",
            )
            .await
            .unwrap();
        // A newer owner may already have established its own empty fence.
        f.indexed
            .set_key_epoch(
                "test",
                "newer_fence",
                pending.state.fence_namespace(),
                &f.id().storage_key(),
                ShardEpoch(8),
                &[],
                false,
            )
            .await
            .unwrap();
        let before = record_bytes(&f, &admitted.path()).await.unwrap();
        oplog
            .collect_tombstone(&f.id(), f.metadata.agent_mode)
            .await
            .unwrap();
        let records = list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
            .await
            .unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.key)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([admitted, inadmissible])
        );
        assert_eq!(record_bytes(&f, &admitted.path()).await, Some(before));
        assert_eq!(
            record_bytes(&f, &inadmissible.path()).await.unwrap(),
            &b"late upload"[..]
        );
        assert_eq!(
            record_bytes(&f, &malformed).await.unwrap(),
            &b"malformed sentinel"[..]
        );
        assert_fence(&f, pending.state.fence_namespace(), Some(ShardEpoch(8))).await;
        assert!(
            matches!(f.service().state(&f.id()).await.unwrap(), AgentSealState::Sealed(record)
            if record.fingerprint == newer.fingerprint)
        );
        // A duplicate scheduler delivery, now without a tombstone, is a complete no-op.
        oplog
            .collect_tombstone(&f.id(), f.metadata.agent_mode)
            .await
            .unwrap();
        assert_fence(&f, pending.state.fence_namespace(), Some(ShardEpoch(8))).await;
    }
}

#[test]
async fn collection_of_late_equal_position_seals_does_not_resurrect_deleted_agent() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let mut f = Fixture::new(indexed).await;
        let pending = seal_fixture(&f).await;
        assert_eq!(pending.key.seq, 0);
        // Construct the persisted crash prefix of an unknown seal PUT outcome. This is not
        // a held-PUT test: the explicit late writes below model the delayed server commit.
        f.blobs
            .with("test", "pending_put_not_yet_visible")
            .delete(
                seal_namespace(&f.id(), f.metadata.agent_mode),
                &pending.key.path(),
            )
            .await
            .unwrap();
        mature_collections(&mut f);
        let oplog = f.oplog_service().await;
        oplog
            .delete(
                &mut oplog.lock_lifecycle(&f.id().agent_id).await,
                &f.id(),
                AgentMode::Durable,
                f.metadata.fingerprint,
                Some(ShardEpoch(6)),
            )
            .await
            .unwrap();
        let records = list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
            .await
            .unwrap();
        assert_eq!(records.len(), 1);
        let tombstone = records[0].key;
        assert_eq!(tombstone.position(), pending.key.position());
        assert_eq!(tombstone.kind, SealRecordKind::Deleted);
        let mut late_keys = Vec::new();
        for valid_until_ms in [0, u64::MAX] {
            let key = SealRecordKey {
                kind: SealRecordKind::Sealed { valid_until_ms },
                ..pending.key
            };
            put_record(&f, key, &serialize(&pending.record).unwrap()).await;
            late_keys.push(key);
        }
        let records = list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
            .await
            .unwrap();
        assert!(
            records
                .iter()
                .any(|record| record.key == late_keys[1] && record.admissible())
        );
        assert_eq!(valid_record(&records).unwrap().key, tombstone);
        assert!(!oplog.exists(&f.id(), AgentMode::Durable).await.unwrap());
        oplog
            .collect_tombstone(&f.id(), f.metadata.agent_mode)
            .await
            .unwrap();
        for key in late_keys.into_iter().chain([tombstone]) {
            assert!(record_bytes(&f, &key.path()).await.is_none());
        }
        assert!(
            list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
                .await
                .unwrap()
                .is_empty()
        );
        f.assert_epoch(&pending.state, None).await;
        assert!(matches!(
            f.service().state(&f.id()).await.unwrap(),
            AgentSealState::Absent
        ));
        assert!(!oplog.exists(&f.id(), AgentMode::Durable).await.unwrap());
    }
}

#[test]
async fn final_tombstone_delete_failure_or_lost_reply_never_leaves_a_fence() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for after_commit in [false, true] {
            let mut f = Fixture::new(indexed.clone()).await;
            let pending = seal_fixture(&f).await;
            mature_collections(&mut f);
            let oplog = f.oplog_service().await;
            oplog
                .delete(
                    &mut oplog.lock_lifecycle(&f.id().agent_id).await,
                    &f.id(),
                    AgentMode::Durable,
                    f.metadata.fingerprint,
                    Some(ShardEpoch(6)),
                )
                .await
                .unwrap();
            let records = list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
                .await
                .unwrap();
            assert_eq!(records.len(), 1);
            let tombstone = records[0].key;
            if after_commit {
                f.blobs.fail_delete_after_commit(0);
            } else {
                f.blobs.fail_next_delete();
            }
            assert!(matches!(
                oplog
                    .collect_tombstone(&f.id(), f.metadata.agent_mode)
                    .await,
                Err(OplogError::Maintenance(_))
            ));
            // No ordinary record remains, so the injected delete is the final tombstone mutation.
            assert_eq!(
                record_bytes(&f, &tombstone.path()).await.is_none(),
                after_commit
            );
            f.assert_epoch(&pending.state, None).await;
            // Reconstruct the service after the failed/indeterminate final mutation.
            let restarted = f.oplog_service().await;
            restarted
                .collect_tombstone(&f.id(), f.metadata.agent_mode)
                .await
                .unwrap();
            assert!(
                list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
                    .await
                    .unwrap()
                    .is_empty()
            );
            f.assert_epoch(&pending.state, None).await;
            assert!(!restarted.exists(&f.id(), AgentMode::Durable).await.unwrap());
        }
    }
}

#[test]
async fn lost_deletion_with_primary_create_preserves_same_incarnation_and_fence() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let mut f = Fixture::new(indexed).await;
        let pending = seal_fixture(&f).await;
        let tombstone = put_tombstone(&f, f.metadata.fingerprint, 7, 0).await;
        let entry = create_entry(&f, f.metadata.fingerprint);
        write_primary_entry(&f, &pending.state, &entry, ShardEpoch(7)).await;
        put_chunk_sentinel(&f, f.metadata.fingerprint).await;
        let before = record_bytes(&f, &pending.key.path()).await.unwrap();
        mature_collections(&mut f);
        let oplog = f.oplog_service().await;
        oplog
            .collect_tombstone(&f.id(), f.metadata.agent_mode)
            .await
            .unwrap();
        assert!(record_bytes(&f, &tombstone.path()).await.is_none());
        assert_eq!(record_bytes(&f, &pending.key.path()).await, Some(before));
        assert_chunk_sentinel(&f, f.metadata.fingerprint, true).await;
        f.assert_history(&pending.record).await;
        assert_fence(&f, pending.state.fence_namespace(), Some(ShardEpoch(7))).await;
        assert_eq!(
            oplog
                .read_source(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 1)
                .await
                .unwrap(),
            BTreeMap::from([(OplogIndex::INITIAL, entry)])
        );
    }
}

#[test]
#[test_r::timeout("30s")]
async fn lost_deletion_with_newer_admissible_seal_preserves_all_seals_chunks_and_fence() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let mut f = Fixture::new(indexed).await;
        let pending = seal_fixture(&f).await;
        let tombstone = put_tombstone(&f, f.metadata.fingerprint, 7, 0).await;
        f.indexed
            .set_key_epoch(
                "test",
                "lost_delete_fence",
                pending.state.fence_namespace(),
                &f.id().storage_key(),
                ShardEpoch(7),
                &[],
                false,
            )
            .await
            .unwrap();
        put_chunk_sentinel(&f, f.metadata.fingerprint).await;
        mature_collections(&mut f);
        let oplog = Arc::new(f.oplog_service().await);
        // The first listing discovers tombstones; the second must be fresh after
        // indexed identity resolution. Publish the seal only at that second boundary.
        let (listing, resume) = f.blobs.pause_seal_list_after(1);
        let collecting = {
            let oplog = oplog.clone();
            let id = f.id();
            tokio::spawn(async move { oplog.collect_tombstone(&id, AgentMode::Durable).await })
        };
        listing.await.unwrap();
        let newer = put_seal(&f, &pending.record, f.metadata.fingerprint, 8, 0).await;
        let before = [
            (
                pending.key,
                record_bytes(&f, &pending.key.path()).await.unwrap(),
            ),
            (newer, record_bytes(&f, &newer.path()).await.unwrap()),
        ];
        resume.send(()).unwrap();
        collecting.await.unwrap().unwrap();
        assert!(record_bytes(&f, &tombstone.path()).await.is_none());
        for (key, bytes) in before {
            assert_eq!(record_bytes(&f, &key.path()).await, Some(bytes));
        }
        assert_chunk_sentinel(&f, f.metadata.fingerprint, true).await;
        f.assert_history(&pending.record).await;
        assert_fence(&f, pending.state.fence_namespace(), Some(ShardEpoch(7))).await;
        assert!(oplog.exists(&f.id(), AgentMode::Durable).await.unwrap());
    }
}

#[test]
async fn first_trimmed_seal_without_marker_does_not_merge_unidentified_moved_data() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::new(indexed).await;
        f.archive(1)
            .open(&f.id(), AgentMode::Durable, None, f.metadata.fingerprint)
            .await
            .unwrap()
            .drop_prefix(OplogIndex::from_u64(3))
            .await
            .unwrap();
        let record = SealRecord {
            writer_nonce: Uuid::new_v4(),
            writer_epoch: ShardEpoch(5),
            fingerprint: f.metadata.fingerprint,
            agent_mode: AgentMode::Durable,
            manifest: BTreeMap::from([
                (
                    0,
                    f.archive(0)
                        .manifest_snapshot(&f.id(), AgentMode::Durable)
                        .await
                        .unwrap(),
                ),
                (
                    1,
                    f.archive(1)
                        .manifest_snapshot(&f.id(), AgentMode::Durable)
                        .await
                        .unwrap(),
                ),
            ]),
            metadata: f.metadata.clone(),
            last_index: OplogIndex::from_u64(9),
            completed_promises: BTreeMap::from([(OplogIndex::from_u64(3), vec![99])]),
            pending_promises: BTreeSet::new(),
            rejected_periodic_snapshots: BTreeSet::from([OplogIndex::from_u64(8)]),
        };
        put_seal(&f, &record, f.metadata.fingerprint, 5, 0).await;
        f.put_promise(4, Some(vec![4])).await;
        f.reject(&[5]).await;
        let seal = f.service();
        assert_eq!(seal.indexed_identity(&f.id()).await.unwrap(), None);
        assert_eq!(
            seal.completed_promise(f.id().environment_id(), &f.promise(3))
                .await
                .unwrap(),
            None
        );
        assert!(
            seal.rejected_snapshots(&f.id(), f.metadata.fingerprint)
                .await
                .unwrap()
                .is_empty()
        );
        let pending = seal
            .prepare(f.metadata.clone(), ShardEpoch(6))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.record.fingerprint, f.metadata.fingerprint);
        assert_eq!(
            pending.record.completed_promises,
            BTreeMap::from([(OplogIndex::from_u64(4), vec![4])])
        );
        assert_eq!(
            pending.record.rejected_periodic_snapshots,
            BTreeSet::from([OplogIndex::from_u64(5)])
        );
        assert!(seal.release(&pending).await.unwrap());
    }
}

#[test]
async fn unidentified_never_sealed_primary_gc_removes_only_the_tombstone() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let mut f = Fixture::new(indexed).await;
        let state = f
            .service()
            .indexed_state(&f.id(), AgentMode::Durable, &SealManifest::new())
            .unwrap();
        f.archive(1)
            .open(&f.id(), AgentMode::Durable, None, f.metadata.fingerprint)
            .await
            .unwrap()
            .drop_prefix(OplogIndex::from_u64(3))
            .await
            .unwrap();
        let tombstone = put_tombstone(&f, f.metadata.fingerprint, 7, 0).await;
        let entry = OplogEntry::Suspend {
            timestamp: Timestamp::now_utc().rounded(),
        };
        write_primary_entry(&f, &state, &entry, ShardEpoch(7)).await;
        put_chunk_sentinel(&f, f.metadata.fingerprint).await;
        let manifests = [
            f.archive(0)
                .manifest_snapshot(&f.id(), AgentMode::Durable)
                .await
                .unwrap(),
            f.archive(1)
                .manifest_snapshot(&f.id(), AgentMode::Durable)
                .await
                .unwrap(),
        ];
        mature_collections(&mut f);
        let oplog = f.oplog_service().await;
        let observation = f
            .indexed
            .observe_agent("test", "unidentified", &f.id())
            .await
            .unwrap();
        assert!(observation.has_data);
        assert!(observation.durable_marker.is_none());
        assert!(observation.ephemeral_marker.is_none());
        oplog
            .collect_tombstone(&f.id(), f.metadata.agent_mode)
            .await
            .unwrap();
        oplog
            .collect_tombstone(&f.id(), f.metadata.agent_mode)
            .await
            .unwrap();
        assert!(record_bytes(&f, &tombstone.path()).await.is_none());
        assert_chunk_sentinel(&f, f.metadata.fingerprint, true).await;
        for (level, manifest) in manifests.into_iter().enumerate() {
            assert_eq!(
                f.archive(level)
                    .manifest_snapshot(&f.id(), AgentMode::Durable)
                    .await
                    .unwrap(),
                manifest
            );
        }
        assert_fence(&f, state.fence_namespace(), Some(ShardEpoch(7))).await;
        assert_eq!(
            oplog
                .read_source(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 1)
                .await
                .unwrap(),
            BTreeMap::from([(OplogIndex::INITIAL, entry)])
        );
        assert!(f.scheduler.actions.lock().unwrap().is_empty());
    }
}

#[test]
async fn absent_primary_retry_for_a_does_not_shadow_recreated_sealed_b() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::new(indexed).await;
        let pending = seal_fixture(&f).await;
        let old = f.metadata.fingerprint;
        let oplog = f.oplog_service().await;
        oplog
            .delete(
                &mut oplog.lock_lifecycle(&f.id().agent_id).await,
                &f.id(),
                AgentMode::Durable,
                old,
                Some(ShardEpoch(6)),
            )
            .await
            .unwrap();
        let replacement = AgentFingerprint::new();
        let newer = put_seal(&f, &pending.record, replacement, 7, 0).await;
        put_chunk_sentinel(&f, replacement).await;
        let records = list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
            .await
            .unwrap();
        let before: BTreeSet<_> = records.iter().map(|record| record.key).collect();
        let newer_bytes = record_bytes(&f, &newer.path()).await.unwrap();
        f.register(8, true);
        oplog
            .delete(
                &mut oplog.lock_lifecycle(&f.id().agent_id).await,
                &f.id(),
                AgentMode::Durable,
                old,
                Some(ShardEpoch(8)),
            )
            .await
            .unwrap();
        assert_eq!(
            list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
                .await
                .unwrap()
                .iter()
                .map(|record| record.key)
                .collect::<BTreeSet<_>>(),
            before
        );
        assert_eq!(record_bytes(&f, &newer.path()).await, Some(newer_bytes));
        assert_chunk_sentinel(&f, replacement, true).await;
        assert_fence(&f, pending.state.fence_namespace(), Some(ShardEpoch(6))).await;
        assert!(
            matches!(f.service().state(&f.id()).await.unwrap(), AgentSealState::Sealed(record)
            if record.fingerprint == replacement)
        );
    }
}

#[test]
async fn a_and_b_tombstones_survive_reseal_and_deletion_until_gc_collects_both_chunk_sets() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let mut f = Fixture::new(indexed).await;
        let pending_a = seal_fixture(&f).await;
        let a = f.metadata.fingerprint;
        let oplog = f.oplog_service().await;
        oplog
            .delete(
                &mut oplog.lock_lifecycle(&f.id().agent_id).await,
                &f.id(),
                AgentMode::Durable,
                a,
                Some(ShardEpoch(6)),
            )
            .await
            .unwrap();
        let records = list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
            .await
            .unwrap();
        assert_eq!(records.len(), 1);
        let tombstone_a = records[0].key;
        put_chunk_sentinel(&f, a).await;
        let b = AgentFingerprint::new();
        f.metadata.fingerprint = b;
        let initial = create_entry(&f, b);
        f.original.insert(OplogIndex::INITIAL, initial);
        f.register(7, true);
        let seal = f.service();
        seal.claim(&f.id(), ShardEpoch(7)).await.unwrap();
        for (level, first, last) in [(1, 1, 3), (1, 4, 6), (0, 7, 9)] {
            let archive = f
                .archive(level)
                .open(&f.id(), AgentMode::Durable, None, b)
                .await
                .unwrap();
            let entries: Vec<_> = f
                .original
                .range(OplogIndex::from_u64(first)..=OplogIndex::from_u64(last))
                .map(|(index, entry)| (*index, entry.clone()))
                .collect();
            archive.append(&entries).await.unwrap();
        }
        let pending_b = seal
            .prepare(f.metadata.clone(), ShardEpoch(7))
            .await
            .unwrap()
            .unwrap();
        seal.remove_moved_copies(&pending_b).await.unwrap();
        assert!(seal.release(&pending_b).await.unwrap());
        assert!(record_bytes(&f, &tombstone_a.path()).await.is_some());
        assert_chunk_sentinel(&f, a, true).await;
        mature_collections(&mut f);
        let oplog = f.oplog_service().await;
        oplog
            .delete(
                &mut oplog.lock_lifecycle(&f.id().agent_id).await,
                &f.id(),
                AgentMode::Durable,
                b,
                Some(ShardEpoch(7)),
            )
            .await
            .unwrap();
        let records = list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
            .await
            .unwrap();
        assert_eq!(records.len(), 2);
        assert!(
            records
                .iter()
                .all(|record| record.key.kind == SealRecordKind::Deleted)
        );
        assert_eq!(
            records
                .iter()
                .map(|record| record.key.fingerprint)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([a, b])
        );
        assert_chunk_sentinel(&f, a, true).await;
        put_chunk_sentinel(&f, b).await;
        let tombstone_b = records
            .iter()
            .find(|record| record.key.fingerprint == b)
            .unwrap()
            .key;
        let late_a = put_seal(
            &f,
            &pending_a.record,
            a,
            tombstone_a.epoch.0,
            tombstone_a.seq,
        )
        .await;
        let late_b = put_seal(
            &f,
            &pending_b.record,
            b,
            tombstone_b.epoch.0,
            tombstone_b.seq,
        )
        .await;
        // Crash after the first regional seal deletion. Ascending GC must touch A's
        // region first and leave B's seal, chunks, tombstone, and owning fence intact.
        f.blobs.fail_delete_after_commit(0);
        assert!(matches!(
            oplog
                .collect_tombstone(&f.id(), f.metadata.agent_mode)
                .await,
            Err(OplogError::Maintenance(_))
        ));
        assert!(record_bytes(&f, &late_a.path()).await.is_none());
        assert!(record_bytes(&f, &late_b.path()).await.is_some());
        assert!(record_bytes(&f, &tombstone_a.path()).await.is_some());
        assert!(record_bytes(&f, &tombstone_b.path()).await.is_some());
        assert_chunk_sentinel(&f, a, true).await;
        assert_chunk_sentinel(&f, b, true).await;
        assert_fence(&f, pending_b.state.fence_namespace(), Some(ShardEpoch(7))).await;
        oplog
            .collect_tombstone(&f.id(), f.metadata.agent_mode)
            .await
            .unwrap();
        assert!(
            list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
                .await
                .unwrap()
                .is_empty()
        );
        assert_chunk_sentinel(&f, a, false).await;
        assert_chunk_sentinel(&f, b, false).await;
        f.assert_epoch(&pending_b.state, None).await;
        assert!(!oplog.exists(&f.id(), AgentMode::Durable).await.unwrap());
    }
}

#[test]
async fn lost_deletion_does_not_hide_or_erase_moved_data_from_present_or_resealed_incarnation() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for trim in [0, 3] {
            let mut f = Fixture::new(indexed.clone()).await;
            let payload = vec![3, 1, 4];
            let rejected = BTreeSet::from([OplogIndex::from_u64(5), OplogIndex::from_u64(8)]);
            f.put_promise(3, Some(payload.clone())).await;
            f.reject(&[5, 8]).await;
            if trim != 0 {
                f.archive(1)
                    .open(&f.id(), AgentMode::Durable, None, f.metadata.fingerprint)
                    .await
                    .unwrap()
                    .drop_prefix(OplogIndex::from_u64(trim))
                    .await
                    .unwrap();
            }
            let first = seal_fixture(&f).await;
            assert_eq!(
                first.record.completed_promises,
                BTreeMap::from([(OplogIndex::from_u64(3), payload.clone())])
            );
            assert_eq!(first.record.rejected_periodic_snapshots, rejected);
            assert!(
                f.kv.with_entity("test", "moved_promise_absent", "promise")
                    .get_all_raw(promise_namespace_of(&f.id(), f.metadata.fingerprint))
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(
                f.kv.with_entity("test", "moved_rejections_absent", "snapshot")
                    .get_raw(
                        KeyValueStorageNamespace::AgentRejectedPeriodicSnapshots {
                            owned_agent_id: f.id()
                        },
                        &f.metadata.fingerprint.0.to_string()
                    )
                    .await
                    .unwrap()
                    .is_none()
            );
            f.register(7, true);
            f.service().unseal(&f.id(), ShardEpoch(7)).await.unwrap();
            // Persist the deletion's crash prefix: its tombstone committed, but indexed
            // deletion never ran. The marker identifies the trimmed incarnation.
            let tombstone = put_tombstone(&f, f.metadata.fingerprint, 7, 0).await;
            let mut foreign = first.record.clone();
            foreign.writer_epoch = ShardEpoch(8);
            foreign.fingerprint = AgentFingerprint::new();
            foreign.metadata.fingerprint = foreign.fingerprint;
            foreign.completed_promises = BTreeMap::from([(OplogIndex::from_u64(3), vec![99])]);
            foreign.rejected_periodic_snapshots = BTreeSet::from([OplogIndex::from_u64(2)]);
            let foreign_key = SealRecordKey {
                agent_id: f.id().agent_id,
                fingerprint: foreign.fingerprint,
                epoch: foreign.writer_epoch,
                seq: 0,
                kind: SealRecordKind::Sealed {
                    valid_until_ms: u64::MAX,
                },
            };
            put_record(&f, foreign_key, &serialize(&foreign).unwrap()).await;
            put_chunk_sentinel(&f, f.metadata.fingerprint).await;
            let seal = f.service();
            assert!(matches!(
                seal.state(&f.id()).await.unwrap(),
                AgentSealState::Present
            ));
            assert_eq!(
                seal.completed_promise(f.id().environment_id(), &f.promise(3))
                    .await
                    .unwrap(),
                Some(payload.clone())
            );
            assert_eq!(
                seal.rejected_snapshots(&f.id(), f.metadata.fingerprint)
                    .await
                    .unwrap(),
                rejected
            );
            assert_eq!(
                seal.completed_promise(
                    f.id().environment_id(),
                    &PromiseId {
                        fingerprint: foreign.fingerprint,
                        ..f.promise(3)
                    }
                )
                .await
                .unwrap(),
                None
            );
            mature_collections(&mut f);
            let present = f.oplog_service().await;
            present
                .collect_tombstone(&f.id(), f.metadata.agent_mode)
                .await
                .unwrap();
            assert!(record_bytes(&f, &tombstone.path()).await.is_none());
            assert!(record_bytes(&f, &first.key.path()).await.is_some());
            assert_chunk_sentinel(&f, f.metadata.fingerprint, true).await;
            assert_fence(&f, first.state.fence_namespace(), Some(ShardEpoch(7))).await;
            assert_eq!(
                present
                    .read_source(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 9)
                    .await
                    .unwrap(),
                f.original
                    .range(OplogIndex::from_u64(trim + 1)..)
                    .map(|(index, entry)| (*index, entry.clone()))
                    .collect()
            );
            // Retry the same lost deletion after a new seal as well.
            assert_eq!(
                put_tombstone(&f, f.metadata.fingerprint, 7, 0).await,
                tombstone
            );
            f.register(9, true);
            seal.claim(&f.id(), ShardEpoch(9)).await.unwrap();
            let second = seal
                .prepare(f.metadata.clone(), ShardEpoch(9))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                second.record.completed_promises,
                first.record.completed_promises
            );
            assert_eq!(second.record.rejected_periodic_snapshots, rejected);
            seal.remove_moved_copies(&second).await.unwrap();
            assert!(seal.release(&second).await.unwrap());
            assert!(record_bytes(&f, &tombstone.path()).await.is_some());
            mature_collections(&mut f);
            let oplog = f.oplog_service().await;
            oplog
                .collect_tombstone(&f.id(), f.metadata.agent_mode)
                .await
                .unwrap();
            assert_chunk_sentinel(&f, f.metadata.fingerprint, true).await;
            assert!(record_bytes(&f, &tombstone.path()).await.is_none());
            assert!(record_bytes(&f, &second.key.path()).await.is_some());
            let seal = f.service();
            assert_eq!(
                seal.completed_promise(f.id().environment_id(), &f.promise(3))
                    .await
                    .unwrap(),
                Some(payload)
            );
            assert_eq!(
                seal.rejected_snapshots(&f.id(), f.metadata.fingerprint)
                    .await
                    .unwrap(),
                rejected
            );
            assert_eq!(
                read_sealed(
                    f.blobs.as_ref(),
                    &f.id(),
                    &second.record,
                    OplogIndex::from_u64(trim + 1),
                    9 - trim
                )
                .await
                .unwrap(),
                f.original
                    .range(OplogIndex::from_u64(trim + 1)..)
                    .map(|(index, entry)| (*index, entry.clone()))
                    .collect()
            );
            f.assert_epoch(&second.state, None).await;
        }
    }
}

#[test]
async fn memory_scheduler_storage_disables_sealing_without_lifecycle_writes() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        let f = Fixture::new(indexed).await;
        let memory = Arc::new(CollectionScheduler {
            memory: Some(InMemorySchedulerStorage::new()),
            ..Default::default()
        });
        let scheduler: Arc<dyn SchedulerService> = memory.clone();
        let mut seal = f.service();
        seal.scheduler = Arc::downgrade(&scheduler);
        assert!(!seal.enabled());
        assert!(
            seal.prepare(f.metadata.clone(), ShardEpoch(6))
                .await
                .unwrap()
                .is_none()
        );
        let oplog = f.oplog_service().await;
        assert!(
            !seal
                .delete_with_tombstone(
                    &mut oplog.lock_lifecycle(&f.id().agent_id).await,
                    &f.id(),
                    AgentMode::Durable,
                    f.metadata.fingerprint,
                    Some(ShardEpoch(6)),
                )
                .await
                .unwrap()
                .0
        );
        assert!(
            list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode, "test")
                .await
                .unwrap()
                .is_empty()
        );
        assert!(memory.times.lock().unwrap().is_empty());
        assert_eq!(
            oplog
                .read_source(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 9)
                .await
                .unwrap(),
            f.original
        );
    }
}

#[test]
async fn collection_is_mode_addressed_even_for_identical_keys_fingerprints_and_epochs() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
            let other_mode = match mode {
                AgentMode::Durable => AgentMode::Ephemeral,
                AgentMode::Ephemeral => AgentMode::Durable,
            };
            let mut f = Fixture::with_mode(indexed.clone(), mode).await;
            let pending = seal_fixture(&f).await;
            mature_collections(&mut f);
            let oplog = f.oplog_service().await;
            oplog
                .delete(
                    &mut oplog.lock_lifecycle(&f.id().agent_id).await,
                    &f.id(),
                    mode,
                    f.metadata.fingerprint,
                    Some(ShardEpoch(6)),
                )
                .await
                .unwrap();
            assert!(
                f.scheduler
                    .actions
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|action| matches!(action,
                ScheduledAction::CollectTombstone { owned_agent_id, agent_mode }
                    if *owned_agent_id == f.id() && *agent_mode == mode))
            );
            let records = list_records(f.blobs.as_ref(), &f.id(), mode, "test")
                .await
                .unwrap();
            assert_eq!(records.len(), 1);
            let tombstone = records[0].key;
            let tombstone_bytes = record_bytes(&f, &tombstone.path()).await.unwrap();
            let other_namespace = seal_namespace(&f.id(), other_mode);
            let mut other_record = pending.record.clone();
            other_record.agent_mode = other_mode;
            other_record.metadata.agent_mode = other_mode;
            let other_bytes = serialize(&other_record).unwrap();
            for (key, bytes) in [(pending.key, &other_bytes), (tombstone, &tombstone_bytes)] {
                f.blobs
                    .with("test", "other_mode_records")
                    .put_raw(other_namespace.clone(), &key.path(), bytes)
                    .await
                    .unwrap();
            }
            let other_state = f
                .service()
                .indexed_state(&f.id(), other_mode, &SealManifest::new())
                .unwrap();
            f.indexed
                .set_key_epoch(
                    "test",
                    "other_mode_fence",
                    other_state.fence_namespace(),
                    &f.id().storage_key(),
                    ShardEpoch(6),
                    &[],
                    false,
                )
                .await
                .unwrap();
            let path = Path::new("chunks")
                .join(agent_path_segment(&f.id().agent_id))
                .join(f.metadata.fingerprint.to_string())
                .join("late_chunk");
            for chunk_mode in [mode, other_mode] {
                f.blobs
                    .with("test", "mode_chunk")
                    .put_raw(
                        BlobStorageNamespace::CompressedOplog {
                            environment_id: f.id().environment_id(),
                            type_tag: f.id().agent_id.type_tag(),
                            agent_mode: chunk_mode,
                            level: 0,
                        },
                        &path,
                        b"late chunk",
                    )
                    .await
                    .unwrap();
            }
            oplog.collect_tombstone(&f.id(), mode).await.unwrap();
            assert!(
                list_records(f.blobs.as_ref(), &f.id(), mode, "test")
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                list_records(f.blobs.as_ref(), &f.id(), other_mode, "test")
                    .await
                    .unwrap()
                    .len(),
                2
            );
            for (key, bytes) in [(pending.key, &other_bytes), (tombstone, &tombstone_bytes)] {
                assert_eq!(
                    f.blobs
                        .with("test", "other_mode_after_gc")
                        .get_raw(other_namespace.clone(), &key.path())
                        .await
                        .unwrap()
                        .as_ref(),
                    Some(bytes)
                );
            }
            assert_fence(&f, pending.state.fence_namespace(), None).await;
            assert_fence(&f, other_state.fence_namespace(), Some(ShardEpoch(6))).await;
            for (chunk_mode, present) in [(mode, false), (other_mode, true)] {
                assert_eq!(
                    f.blobs
                        .with("test", "chunk_after_gc")
                        .get_raw(
                            BlobStorageNamespace::CompressedOplog {
                                environment_id: f.id().environment_id(),
                                type_tag: f.id().agent_id.type_tag(),
                                agent_mode: chunk_mode,
                                level: 0,
                            },
                            &path
                        )
                        .await
                        .unwrap()
                        .is_some(),
                    present
                );
            }
            oplog.collect_tombstone(&f.id(), other_mode).await.unwrap();
            assert!(
                list_records(f.blobs.as_ref(), &f.id(), other_mode, "test")
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_fence(&f, other_state.fence_namespace(), None).await;
        }
    }
}

#[path = "gc_retry.rs"]
mod gc_retry;
