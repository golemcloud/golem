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
use crate::services::golem_config::OplogConfig;
use crate::services::oplog::seal::{SealRecord, SealRecordKey, SealRecordKind, seal_namespace};
use crate::services::scheduler::SchedulerService;
use crate::services::shard::ShardServiceDefault;
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use golem_common::model::type_id::TypeId;
use golem_common::model::{ScheduleId, ScheduledAction};
use golem_common::serialization::serialize;
use golem_service_base::storage::blob::BlobStorageLabelledApi;
use test_r::test;

struct Scheduler;

#[async_trait]
impl SchedulerService for Scheduler {
    fn is_durable(&self) -> bool {
        true
    }

    async fn schedule(&self, _time: jiff::Timestamp, _action: ScheduledAction) -> ScheduleId {
        panic!("enumeration must not schedule work")
    }

    async fn schedule_with_id(
        &self,
        _id: ScheduleId,
        _time: jiff::Timestamp,
        _action: ScheduledAction,
    ) -> ScheduleId {
        panic!("enumeration must not schedule work")
    }

    async fn cancel(&self, _id: ScheduleId) {
        panic!("enumeration must not cancel work")
    }
}

pub(super) fn attach(
    oplog: &dyn OplogService,
    indexed: Arc<dyn IndexedStorage + Send + Sync>,
    blobs: Arc<dyn BlobStorage + Send + Sync>,
) -> Arc<dyn SchedulerService> {
    let scheduler: Arc<dyn SchedulerService> = Arc::new(Scheduler);
    oplog.set_seal_service(Arc::new(SealService::new(
        indexed,
        blobs,
        Arc::new(InMemoryKeyValueStorage::new()),
        Arc::new(ShardServiceDefault::new()),
        Arc::downgrade(&scheduler),
        OplogConfig {
            indexed_storage_layers: 2,
            blob_storage_layers: 1,
            ..Default::default()
        },
        Default::default(),
    )));
    scheduler
}

pub(super) async fn seed(
    blobs: &dyn BlobStorage,
    id: &OwnedAgentId,
    mode: AgentMode,
) -> SealRecordKey {
    let mut metadata = make_agent_metadata(id.agent_id, AccountId::new(), id.environment_id());
    metadata.agent_mode = mode;
    metadata.last_known_status.agent_mode = mode;
    metadata.last_known_status.oplog_idx = OplogIndex::INITIAL;
    let key = SealRecordKey {
        agent_id: id.agent_id,
        epoch: ShardEpoch(6),
        seq: 0,
        kind: SealRecordKind::Sealed {
            valid_until_ms: u64::MAX,
        },
        fingerprint: metadata.fingerprint,
    };
    let record = SealRecord {
        writer_nonce: Uuid::new_v4(),
        writer_epoch: key.epoch,
        fingerprint: key.fingerprint,
        agent_mode: mode,
        metadata,
        manifest: Default::default(),
        last_index: OplogIndex::INITIAL,
        completed_promises: Default::default(),
        pending_promises: Default::default(),
        rejected_periodic_snapshots: Default::default(),
    };
    blobs
        .with("test", "seed_sealed_agent")
        .put_raw(
            seal_namespace(id, mode),
            &key.path(),
            &serialize(&record).unwrap(),
        )
        .await
        .unwrap();
    key
}

async fn service(
    blobs: Arc<ReadCountingBlobStorage>,
) -> (MultiLayerOplogService, Arc<dyn SchedulerService>) {
    let indexed = Arc::new(InMemoryIndexedStorage::new());
    let primary = Arc::new(
        PrimaryOplogService::new(
            indexed.clone(),
            blobs.clone(),
            100,
            100,
            1024,
            32,
            Default::default(),
        )
        .await,
    );
    let lower: Arc<dyn OplogArchiveService> = Arc::new(BlobOplogArchiveService::new(
        blobs.clone(),
        indexed.clone(),
        0,
        Default::default(),
    ));
    let oplog = MultiLayerOplogService::new(primary, nev![lower], 100, 100);
    let scheduler = attach(&oplog, indexed, blobs);
    (oplog, scheduler)
}

pub(super) fn assert_listing_only(blobs: &ReadCountingBlobStorage, mode: Option<AgentMode>) {
    let lists = blobs.seal_lists.lock().unwrap();
    assert_eq!(
        blobs.reads(),
        lists.len(),
        "scan must use LIST only, never GET or HEAD"
    );
    assert!(!lists.is_empty());
    if let Some(mode) = mode {
        assert!(lists.iter().all(|(listed, _, _)| *listed == mode));
    } else {
        assert!(lists.iter().any(|(mode, _, _)| *mode == AgentMode::Durable));
        assert!(
            lists
                .iter()
                .any(|(mode, _, _)| *mode == AgentMode::Ephemeral)
        );
    }
}

async fn drain(
    oplog: &dyn OplogService,
    environment: EnvironmentId,
    type_id: TypeId,
    mode: Option<AgentMode>,
    count: u64,
) -> Vec<OwnedAgentId> {
    let mut cursor = ScanCursor::default();
    let mut result = Vec::new();
    for _ in 0..5000 {
        let page = oplog
            .scan_for_type(&environment, type_id, mode, cursor.clone(), count)
            .await
            .unwrap();
        assert!(page.examined_keys <= count.clamp(1, 1000));
        result.extend(page.agent_ids);
        if page.cursor.is_finished() {
            result.sort_by_key(|id| id.agent_id);
            return result;
        }
        assert_ne!(page.cursor, cursor, "each page must advance its cursor");
        cursor = page.cursor;
    }
    panic!("scan did not terminate");
}

#[test]
async fn sealed_scan_selects_complete_heads_across_pages() {
    let blobs = Arc::new(ReadCountingBlobStorage::new());
    let (oplog, _scheduler) = service(blobs.clone()).await;
    let env = EnvironmentId::new();
    let id = |n| OwnedAgentId::new(env, &AgentId::from_parts(7, n));
    let mode = AgentMode::Durable;
    let namespace = seal_namespace(&id(10), mode);
    let storage = blobs.with("test", "seed_scan_keys");
    for n in [10, 20, 30] {
        let key = seed(blobs.as_ref(), &id(n), mode).await;
        // One agent spans many pages; another is deleted; the last has a late,
        // inadmissible record above an admissible seal.
        for seq in 1..=6 {
            let kind = match (n, seq) {
                (10, 5) | (20, 6) => SealRecordKind::Deleted,
                (30, 6) => SealRecordKind::Sealed { valid_until_ms: 0 },
                _ => key.kind,
            };
            let key = SealRecordKey { seq, kind, ..key };
            if n == 20 && seq == 6 {
                storage
                    .put_raw(
                        namespace.clone(),
                        &SealRecordKey {
                            kind: SealRecordKind::Sealed {
                                valid_until_ms: u64::MAX,
                            },
                            ..key
                        }
                        .path(),
                        b"same-position seal loses to deletion",
                    )
                    .await
                    .unwrap();
            }
            storage
                .put_raw(
                    namespace.clone(),
                    &key.path(),
                    b"LIST never reads this body",
                )
                .await
                .unwrap();
        }
    }
    let other = seed(blobs.as_ref(), &id(40), AgentMode::Ephemeral).await;
    for path in [
        PathBuf::from("seals/malformed"),
        SealRecordKey {
            agent_id: AgentId::from_parts(8, 50),
            ..other
        }
        .path(),
        SealRecordKey {
            agent_id: AgentId::from_parts(0, 50),
            ..other
        }
        .path(),
    ] {
        storage
            .put_raw(namespace.clone(), &path, b"ignored")
            .await
            .unwrap();
    }
    let writes = blobs.puts.load(Ordering::Relaxed);
    for count in [0, 1, 2, 3, 7, 1000, 2000] {
        for mode in [Some(AgentMode::Durable), Some(AgentMode::Ephemeral), None] {
            blobs.reset();
            let actual = drain(&oplog, env, TypeId::try_from(7u32).unwrap(), mode, count).await;
            let expected = match mode {
                Some(AgentMode::Durable) => vec![id(10), id(30)],
                Some(AgentMode::Ephemeral) => vec![id(40)],
                None => vec![id(10), id(30), id(40)],
            };
            assert_eq!(actual, expected, "count={count}, mode={mode:?}");
            assert_listing_only(&blobs, mode);
            assert_eq!(blobs.puts.load(Ordering::Relaxed), writes);
        }
    }
}

#[test]
async fn sealed_scan_isolates_environment_local_type_ids() {
    let blobs = Arc::new(ReadCountingBlobStorage::new());
    let (oplog, _scheduler) = service(blobs.clone()).await;
    let environments = [EnvironmentId::new(), EnvironmentId::new()];
    let mut expected = Vec::new();
    for env in environments {
        for tag in [7, 8] {
            let id = OwnedAgentId::new(env, &AgentId::from_parts(tag, 42));
            seed(blobs.as_ref(), &id, AgentMode::Durable).await;
            expected.push(id);
        }
    }
    for id in expected {
        blobs.reset();
        assert_eq!(
            drain(
                &oplog,
                id.environment_id(),
                id.agent_id.type_id().unwrap(),
                Some(AgentMode::Durable),
                1
            )
            .await,
            vec![id]
        );
        assert_listing_only(&blobs, Some(AgentMode::Durable));
    }
}

#[test]
async fn sealed_scan_caps_listing_budget_for_one_large_agent() {
    let blobs = Arc::new(ReadCountingBlobStorage::new());
    let (oplog, _scheduler) = service(blobs.clone()).await;
    let env = EnvironmentId::new();
    let id = OwnedAgentId::new(env, &AgentId::from_parts(7, 2125));
    let key = seed(blobs.as_ref(), &id, AgentMode::Durable).await;
    for seq in 1..=1000 {
        blobs
            .with("test", "seed_scan_keys")
            .put_raw(
                seal_namespace(&id, AgentMode::Durable),
                &SealRecordKey { seq, ..key }.path(),
                b"LIST-only body",
            )
            .await
            .unwrap();
    }
    blobs.reset();
    assert_eq!(
        drain(
            &oplog,
            env,
            TypeId::try_from(7u32).unwrap(),
            Some(AgentMode::Durable),
            2000
        )
        .await,
        vec![id]
    );
    assert_listing_only(&blobs, Some(AgentMode::Durable));
}

#[test]
async fn sealed_scan_variable_budgets_preserve_complete_heads() {
    let blobs = Arc::new(ReadCountingBlobStorage::new());
    let (oplog, _scheduler) = service(blobs.clone()).await;
    let env = EnvironmentId::new();
    let mut expected = Vec::new();
    for n in 1..=31 {
        let id = OwnedAgentId::new(env, &AgentId::from_parts(7, n));
        let key = seed(blobs.as_ref(), &id, AgentMode::Durable).await;
        let namespace = seal_namespace(&id, AgentMode::Durable);
        // Every third agent ends in deletion. An inadmissible seal follows every head.
        let last_seq = (n % 9 + 1) as u64;
        for seq in 1..=last_seq + 1 {
            let kind = if seq == last_seq + 1 {
                SealRecordKind::Sealed { valid_until_ms: 0 }
            } else if seq == last_seq && n % 3 == 0 {
                SealRecordKind::Deleted
            } else {
                key.kind
            };
            blobs
                .with("test", "variable_budget_seed")
                .put_raw(
                    namespace.clone(),
                    &SealRecordKey { seq, kind, ..key }.path(),
                    b"LIST-only",
                )
                .await
                .unwrap();
        }
        for suffix in ["000-invalid", "999-invalid"] {
            blobs
                .with("test", "variable_budget_malformed")
                .put_raw(
                    namespace.clone(),
                    &key.path().parent().unwrap().join(suffix),
                    b"ignored",
                )
                .await
                .unwrap();
        }
        if n % 3 != 0 {
            expected.push(id);
        }
    }
    expected.sort_by_key(|id| id.agent_id);
    for offset in 0..7 {
        blobs.reset();
        let budgets = [1, 2, 3, 5, 13, 0, 17];
        let mut cursor = ScanCursor::default();
        let mut actual = Vec::new();
        for step in 0..5000 {
            let count = budgets[(step + offset) % budgets.len()];
            let page = oplog
                .scan_for_type(
                    &env,
                    TypeId::try_from(7u32).unwrap(),
                    Some(AgentMode::Durable),
                    cursor.clone(),
                    count,
                )
                .await
                .unwrap();
            assert!(page.examined_keys <= count.clamp(1, 1000));
            actual.extend(page.agent_ids);
            if page.cursor.is_finished() {
                actual.sort_by_key(|id| id.agent_id);
                assert_eq!(actual, expected, "budget offset {offset}");
                break;
            }
            assert_ne!(cursor, page.cursor);
            cursor = page.cursor;
            assert!(step < 4999, "scan failed to terminate");
        }
        assert_listing_only(&blobs, Some(AgentMode::Durable));
    }
}
