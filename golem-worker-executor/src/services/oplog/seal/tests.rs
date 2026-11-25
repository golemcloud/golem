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
use crate::services::oplog::{CompressedOplogChunk, OplogReadError};
use golem_common::model::Timestamp;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogEntry;
use golem_common::serialization::serialize;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use std::time::Duration;
use test_r::test;

fn key(epoch: u64, seq: u64, kind: SealRecordKind) -> SealRecordKey {
    SealRecordKey {
        agent_id: AgentId::from_parts(7, 23),
        epoch: ShardEpoch(epoch),
        seq,
        kind,
    }
}

fn sealed(epoch: u64, seq: u64, deadline: u64) -> SealRecordKey {
    key(
        epoch,
        seq,
        SealRecordKind::Sealed {
            valid_until_ms: deadline,
        },
    )
}

fn listed(key: SealRecordKey, millis: u64, resolution: Duration) -> ListedSealRecord {
    ListedSealRecord {
        key,
        entry: BlobPageEntry {
            path: key.path().into_boxed_path(),
            last_modified_at: Timestamp::from(millis),
            timestamp_resolution: resolution,
        },
    }
}

#[test]
fn seal_keys_have_explicit_order_and_reject_malformed_paths() {
    let low = sealed(9, 10, 2000);
    let high = sealed(9, 10, 3000);
    let deleted = key(9, 10, SealRecordKind::Deleted);
    for key in [low, high, deleted, sealed(u64::MAX, u64::MAX, u64::MAX)] {
        assert_eq!(SealRecordKey::parse(&key.path()), Some(key));
    }
    assert!(low < high && high < deleted);
    assert!(high.path() < deleted.path());
    assert_eq!(low.position(), deleted.position());
    assert!(deleted < sealed(9, 11, 1));
    assert!(deleted < sealed(10, 0, 1));
    let prefix = seal_prefix(&low.agent_id);
    for name in [
        "9-10-2000.sealed",
        "00000000000000000009-00000000000000000010.sealed",
        "00000000000000000009-00000000000000000010-00000000000000002000.deleted",
        "18446744073709551616-00000000000000000000.deleted",
        "00000000000000000009-00000000000000000010.deleted/extra",
        "+0000000000000000009-00000000000000000010.deleted",
        "garbage",
    ] {
        assert_eq!(SealRecordKey::parse(&prefix.join(name)), None, "{name}");
    }
}

#[test]
fn admissibility_uses_the_bucket_end_and_selects_from_listing_only() {
    for (millis, deadline, resolution, expected) in [
        (1000, 999, 1, false),
        (1000, 1000, 1, false),
        (1000, 1001, 1, true),
        (1000, 1500, 1000, false),
        (1000, 1999, 1000, false),
        (1000, 2000, 1000, true),
        (1000, 1000, 1000, false),
        (2000, 2001, 1000, false),
    ] {
        assert_eq!(
            listed(
                sealed(1, 0, deadline),
                millis,
                Duration::from_millis(resolution)
            )
            .admissible(),
            expected
        );
    }
    let mut submillis = listed(sealed(1, 0, 1000), 1000, Duration::from_nanos(1));
    submillis.entry.last_modified_at = prost_types::Timestamp {
        seconds: 1,
        nanos: 1,
    }
    .into();
    assert!(!submillis.admissible());
    submillis.key.kind = SealRecordKind::Sealed {
        valid_until_ms: 1001,
    };
    assert!(submillis.admissible());
    let mut records = vec![
        listed(sealed(1, 0, 2000), 1000, Duration::from_secs(1)),
        listed(sealed(2, 0, 999), 1000, Duration::from_secs(1)),
        listed(sealed(1, 0, 3000), 1000, Duration::from_secs(1)),
    ];
    assert_eq!(valid_record(&records).unwrap().key, sealed(1, 0, 3000));
    let deleted = key(1, 0, SealRecordKind::Deleted);
    records.push(listed(deleted, 9999, Duration::from_secs(1)));
    assert_eq!(valid_record(&records).unwrap().key, deleted);
    // Sequence allocation must still see the inadmissible higher epoch.
    assert_eq!(
        records.iter().map(|r| r.key.position()).max(),
        Some((ShardEpoch(2), 0))
    );
}

fn record(owned: &OwnedAgentId) -> SealRecord {
    let mut metadata = crate::services::oplog::tests::make_agent_metadata(
        owned.agent_id,
        AccountId::new(),
        owned.environment_id(),
    );
    metadata.created_at = metadata.created_at.rounded();
    metadata.last_known_status.oplog_idx = OplogIndex::from_u64(9);
    SealRecord {
        writer_nonce: Uuid::new_v4(),
        writer_epoch: ShardEpoch(6),
        fingerprint: metadata.fingerprint,
        agent_mode: AgentMode::Durable,
        manifest: BTreeMap::new(),
        metadata,
        last_index: OplogIndex::from_u64(9),
        completed_promises: BTreeMap::from([(OplogIndex::from_u64(3), vec![4, 2, 7])]),
        pending_promises: BTreeSet::new(),
        rejected_periodic_snapshots: BTreeSet::from([
            OplogIndex::from_u64(2),
            OplogIndex::from_u64(8),
        ]),
    }
}

#[test]
async fn record_listing_ignores_malformed_keys_and_decodes_the_selected_body() {
    let storage = InMemoryBlobStorage::new();
    let owned = OwnedAgentId::new(
        EnvironmentId::new(),
        &key(0, 0, SealRecordKind::Deleted).agent_id,
    );
    let body = record(&owned);
    let ns = seal_namespace(&owned);
    let old = sealed(6, 0, u64::MAX);
    storage
        .with("test", "put")
        .put_raw(ns.clone(), &old.path(), &serialize(&body).unwrap())
        .await
        .unwrap();
    let malformed = seal_prefix(&owned.agent_id).join("zz-not-a-record");
    storage
        .with("test", "put")
        .put_raw(ns.clone(), &malformed, b"ignore")
        .await
        .unwrap();
    let records = list_records(&storage, &owned).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        read_record(&storage, &owned, valid_record(&records).unwrap())
            .await
            .unwrap(),
        SealRecordBody::Sealed(Box::new(body.clone()))
    );
    // The newer malformed body must fail rather than falling back to the old record.
    let newer = sealed(6, 1, u64::MAX);
    storage
        .with("test", "put")
        .put_raw(ns.clone(), &newer.path(), b"invalid")
        .await
        .unwrap();
    let records = list_records(&storage, &owned).await.unwrap();
    assert!(matches!(
        read_record(&storage, &owned, valid_record(&records).unwrap()).await,
        Err(OplogArchiveError::Corruption(_))
    ));
    let deleted = key(6, 1, SealRecordKind::Deleted);
    let tombstone = DeletionRecord {
        writer_nonce: Uuid::new_v4(),
        writer_epoch: ShardEpoch(6),
        fingerprint: body.fingerprint,
    };
    storage
        .with("test", "put")
        .put_raw(ns.clone(), &deleted.path(), &serialize(&tombstone).unwrap())
        .await
        .unwrap();
    let records = list_records(&storage, &owned).await.unwrap();
    assert_eq!(
        read_record(&storage, &owned, valid_record(&records).unwrap())
            .await
            .unwrap(),
        SealRecordBody::Deleted(tombstone)
    );
    assert_eq!(
        storage
            .with("test", "get")
            .get_raw(ns, &malformed)
            .await
            .unwrap(),
        Some(b"ignore".to_vec())
    );
}

#[test]
async fn record_body_identity_and_snapshot_must_match_the_key() {
    let storage = InMemoryBlobStorage::new();
    let owned = OwnedAgentId::new(
        EnvironmentId::new(),
        &key(0, 0, SealRecordKind::Deleted).agent_id,
    );
    let original = record(&owned);
    let key = sealed(6, 0, u64::MAX);
    for variant in 0..5 {
        let mut body = original.clone();
        match variant {
            0 => body.writer_epoch = ShardEpoch(5),
            1 => body.fingerprint = AgentFingerprint::new(),
            2 => body.last_index = OplogIndex::from_u64(8),
            3 => body
                .pending_promises
                .insert(OplogIndex::from_u64(4))
                .then_some(())
                .unwrap(),
            _ => body.metadata.agent_mode = AgentMode::Ephemeral,
        }
        storage
            .with("test", "put")
            .put_raw(
                seal_namespace(&owned),
                &key.path(),
                &serialize(&body).unwrap(),
            )
            .await
            .unwrap();
        let records = list_records(&storage, &owned).await.unwrap();
        assert!(matches!(
            read_record(&storage, &owned, valid_record(&records).unwrap()).await,
            Err(OplogArchiveError::Corruption(_))
        ));
    }
}

async fn put_chunk(
    storage: &dyn BlobStorage,
    owned: &OwnedAgentId,
    body: &mut SealRecord,
    level: usize,
    first: u64,
    last: u64,
) -> Vec<(OplogIndex, OplogEntry)> {
    let entries: Vec<_> = (first..=last)
        .map(|i| {
            (
                OplogIndex::from_u64(i),
                OplogEntry::Suspend {
                    timestamp: Timestamp::from(i * 1000),
                },
            )
        })
        .collect();
    let object = format!(
        "chunks/{}/{}/{last}-{}",
        owned.agent_id,
        body.fingerprint.0,
        Uuid::new_v4()
    );
    let chunk =
        CompressedOplogChunk::compress(entries.iter().map(|(_, entry)| entry.clone()).collect())
            .unwrap();
    storage
        .with("test", "put")
        .put_raw(
            BlobStorageNamespace::CompressedOplog {
                environment_id: owned.environment_id(),
                type_tag: owned.agent_id.type_tag(),
                agent_mode: body.agent_mode,
                level,
            },
            Path::new(&object),
            &serialize(&chunk).unwrap(),
        )
        .await
        .unwrap();
    body.manifest.entry(level).or_default().insert(
        OplogIndex::from_u64(last),
        BlobChunkRef {
            object,
            count: last - first + 1,
        },
    );
    entries
}

#[test]
async fn record_listing_continues_after_a_full_page() {
    let storage = InMemoryBlobStorage::new();
    let owned = OwnedAgentId::new(EnvironmentId::new(), &sealed(0, 0, 0).agent_id);
    for seq in 0..1001 {
        storage
            .with("test", "put")
            .put_raw(
                seal_namespace(&owned),
                &sealed(4, seq, u64::MAX).path(),
                b"listing does not decode bodies",
            )
            .await
            .unwrap();
    }
    let records = list_records(&storage, &owned).await.unwrap();
    assert_eq!(records.len(), 1001);
    assert_eq!(valid_record(&records).unwrap().key.seq, 1000);
}

#[test]
async fn ephemeral_record_restores_status_mode_and_pending_promises() {
    let storage = InMemoryBlobStorage::new();
    let owned = OwnedAgentId::new(EnvironmentId::new(), &sealed(0, 0, 0).agent_id);
    let mut body = record(&owned);
    body.agent_mode = AgentMode::Ephemeral;
    body.metadata.agent_mode = AgentMode::Ephemeral;
    body.metadata.last_known_status.agent_mode = AgentMode::Ephemeral;
    body.pending_promises.insert(OplogIndex::from_u64(5));
    put_chunk(&storage, &owned, &mut body, 2, 1, 9).await;
    storage
        .with("test", "put")
        .put_raw(
            seal_namespace(&owned),
            &sealed(6, 0, u64::MAX).path(),
            &serialize(&body).unwrap(),
        )
        .await
        .unwrap();
    let records = list_records(&storage, &owned).await.unwrap();
    assert_eq!(
        read_record(&storage, &owned, valid_record(&records).unwrap())
            .await
            .unwrap(),
        SealRecordBody::Sealed(Box::new(body))
    );
}

#[test]
async fn frozen_manifest_reads_exact_ranges_across_chunks_and_levels() {
    let storage = InMemoryBlobStorage::new();
    let owned = OwnedAgentId::new(
        EnvironmentId::new(),
        &key(0, 0, SealRecordKind::Deleted).agent_id,
    );
    let mut body = record(&owned);
    let mut expected = BTreeMap::new();
    for (level, first, last) in [(1, 7, 8), (1, 9, 9), (3, 1, 3), (3, 4, 6)] {
        expected.extend(put_chunk(&storage, &owned, &mut body, level, first, last).await);
    }
    assert_eq!(
        read_sealed(&storage, &owned, &body, OplogIndex::INITIAL, 9)
            .await
            .unwrap(),
        expected
    );
    assert_eq!(
        read_sealed(&storage, &owned, &body, OplogIndex::from_u64(2), 7)
            .await
            .unwrap(),
        expected
            .range(OplogIndex::from_u64(2)..=OplogIndex::from_u64(8))
            .map(|(k, v)| (*k, v.clone()))
            .collect()
    );
    assert!(
        read_sealed(&storage, &owned, &body, OplogIndex::INITIAL, 0)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        read_sealed(&storage, &owned, &body, OplogIndex::from_u64(u64::MAX), 2).await,
        Err(OplogReadError::InvalidRange { .. })
    ));
    let mut broken = body.clone();
    broken
        .manifest
        .get_mut(&3)
        .unwrap()
        .remove(&OplogIndex::from_u64(3));
    assert!(matches!(
        read_sealed(&storage, &owned, &broken, OplogIndex::INITIAL, 9).await,
        Err(OplogReadError::Gap { .. })
    ));
    let mut broken = body.clone();
    broken
        .manifest
        .get_mut(&1)
        .unwrap()
        .get_mut(&OplogIndex::from_u64(8))
        .unwrap()
        .count += 1;
    assert!(matches!(
        read_sealed(&storage, &owned, &broken, OplogIndex::INITIAL, 9).await,
        Err(OplogReadError::Corruption { .. })
    ));
    let mut broken = body.clone();
    put_chunk(&storage, &owned, &mut broken, 3, 3, 5).await;
    assert_eq!(
        read_sealed(&storage, &owned, &broken, OplogIndex::INITIAL, 9)
            .await
            .unwrap(),
        expected
    );
    let object = &body.manifest[&1][&OplogIndex::from_u64(8)].object;
    storage
        .with("test", "delete")
        .delete(
            BlobStorageNamespace::CompressedOplog {
                environment_id: owned.environment_id(),
                type_tag: owned.agent_id.type_tag(),
                agent_mode: body.agent_mode,
                level: 1,
            },
            Path::new(object),
        )
        .await
        .unwrap();
    assert!(matches!(
        read_sealed(&storage, &owned, &body, OplogIndex::INITIAL, 9).await,
        Err(OplogReadError::Corruption { .. })
    ));
}

#[test]
async fn frozen_manifest_preserves_resident_reads_of_retained_overlapping_chunks() {
    use crate::services::oplog::{BlobOplogArchiveService, OplogArchiveService};
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use std::sync::Arc;

    let storage = Arc::new(InMemoryBlobStorage::new());
    let indexed = Arc::new(InMemoryIndexedStorage::new());
    let service = BlobOplogArchiveService::new(storage.clone(), indexed, 2, Default::default());
    let owned = OwnedAgentId::new(EnvironmentId::new(), &sealed(0, 0, 0).agent_id);
    let mut body = record(&owned);
    let archive = service
        .open(&owned, body.agent_mode, None, body.fingerprint)
        .await
        .unwrap();
    let entries: Vec<_> = (1..=9)
        .map(|i| {
            (
                OplogIndex::from_u64(i),
                OplogEntry::Suspend {
                    timestamp: Timestamp::from(i * 1000),
                },
            )
        })
        .collect();
    archive.append(&entries[..3]).await.unwrap();
    archive.append(&entries).await.unwrap();
    body.manifest.insert(
        2,
        service
            .manifest_snapshot(&owned, body.agent_mode)
            .await
            .unwrap(),
    );
    for start in 1..=9 {
        for end in start..=9 {
            assert_eq!(
                read_sealed(
                    storage.as_ref(),
                    &owned,
                    &body,
                    OplogIndex::from_u64(start),
                    end - start + 1
                )
                .await
                .unwrap(),
                service
                    .read_source(
                        &owned,
                        body.agent_mode,
                        OplogIndex::from_u64(start),
                        end - start + 1
                    )
                    .await
                    .unwrap()
            );
        }
    }
    let old = &body.manifest[&2][&OplogIndex::from_u64(3)].object;
    storage
        .with("test", "delete")
        .delete(
            BlobStorageNamespace::CompressedOplog {
                environment_id: owned.environment_id(),
                type_tag: owned.agent_id.type_tag(),
                agent_mode: body.agent_mode,
                level: 2,
            },
            Path::new(old),
        )
        .await
        .unwrap();
    assert_eq!(
        read_sealed(storage.as_ref(), &owned, &body, OplogIndex::INITIAL, 9)
            .await
            .unwrap(),
        entries.into_iter().collect()
    );
}

// Exercise tier handoffs at every possible request boundary, including duplicated transfer data.
#[test]
async fn frozen_manifest_all_subranges_across_overlapping_tiers() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let storage = InMemoryBlobStorage::new();
        let owned = OwnedAgentId::new(EnvironmentId::new(), &sealed(0, 0, 0).agent_id);
        let mut body = record(&owned);
        body.agent_mode = mode;
        body.metadata.agent_mode = mode;
        let mut expected = BTreeMap::new();
        for (level, first, last) in [
            (0, 10, 11),
            (0, 12, 12),
            (2, 5, 7),
            (2, 8, 10),
            (5, 1, 2),
            (5, 3, 6),
        ] {
            expected.extend(put_chunk(&storage, &owned, &mut body, level, first, last).await);
        }
        body.manifest.insert(1, BTreeMap::new());
        for first in 1..=12 {
            for last in first..=12 {
                let start = OplogIndex::from_u64(first);
                let end = OplogIndex::from_u64(last);
                let actual = read_sealed(&storage, &owned, &body, start, last - first + 1)
                    .await
                    .unwrap_or_else(|error| panic!("{mode:?} [{first}..={last}]: {error}"));
                let wanted: BTreeMap<_, _> = expected
                    .range(start..=end)
                    .map(|(index, entry)| (*index, entry.clone()))
                    .collect();
                assert_eq!(actual, wanted, "{mode:?} [{first}..={last}]");
            }
        }
    }
}

#[test]
async fn ephemeral_pending_completed_promise_collision_is_rejected() {
    let storage = InMemoryBlobStorage::new();
    let owned = OwnedAgentId::new(EnvironmentId::new(), &sealed(0, 0, 0).agent_id);
    let mut body = record(&owned);
    body.agent_mode = AgentMode::Ephemeral;
    body.metadata.agent_mode = AgentMode::Ephemeral;
    body.pending_promises.insert(OplogIndex::from_u64(3));
    storage
        .with("test", "put")
        .put_raw(
            seal_namespace(&owned),
            &sealed(6, 0, u64::MAX).path(),
            &serialize(&body).unwrap(),
        )
        .await
        .unwrap();
    let records = list_records(&storage, &owned).await.unwrap();
    assert!(matches!(
        read_record(&storage, &owned, valid_record(&records).unwrap()).await,
        Err(OplogArchiveError::Corruption(_))
    ));
}

// Adversarial retained manifests include nested chunks, partial overlaps, and real gaps.
#[test]
async fn frozen_manifest_resident_parity_for_nested_chunks_and_gaps() {
    use crate::services::oplog::{BlobOplogArchiveService, OplogArchiveService};
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use std::sync::Arc;

    for ranges in [
        vec![(1, 3), (1, 9), (2, 5), (4, 7)],
        vec![(1, 3), (5, 7), (9, 11)],
        vec![(3, 7), (1, 5), (6, 9)],
    ] {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let service = BlobOplogArchiveService::new(
            storage.clone(),
            Arc::new(InMemoryIndexedStorage::new()),
            2,
            Default::default(),
        );
        let owned = OwnedAgentId::new(EnvironmentId::new(), &sealed(0, 0, 0).agent_id);
        let mut body = record(&owned);
        let archive = service
            .open(&owned, body.agent_mode, None, body.fingerprint)
            .await
            .unwrap();
        for &(first, last) in &ranges {
            let entries: Vec<_> = (first..=last)
                .map(|i| {
                    (
                        OplogIndex::from_u64(i),
                        OplogEntry::Suspend {
                            timestamp: Timestamp::from(i * 1000),
                        },
                    )
                })
                .collect();
            archive.append(&entries).await.unwrap();
        }
        body.manifest.insert(
            2,
            service
                .manifest_snapshot(&owned, body.agent_mode)
                .await
                .unwrap(),
        );
        for first in 1..=12 {
            for last in first..=12 {
                let start = OplogIndex::from_u64(first);
                let count = last - first + 1;
                let sealed = read_sealed(storage.as_ref(), &owned, &body, start, count).await;
                let resident = service
                    .read_source(&owned, body.agent_mode, start, count)
                    .await;
                // A resident source may return an incomplete suffix; compare exact logical reads.
                let resident = resident.and_then(|entries| {
                    crate::services::oplog::reader::exact_from_source(
                        crate::services::oplog::reader::OplogReadSource::Archive(2),
                        start,
                        count,
                        entries,
                    )
                });
                match (sealed, resident) {
                    (Ok(a), Ok(b)) => assert_eq!(a, b, "{ranges:?} [{first}..={last}]"),
                    (Err(_), Err(_)) => {}
                    (a, b) => panic!(
                        "accept/reject mismatch: {ranges:?} [{first}..={last}]: sealed={a:?}, resident={b:?}"
                    ),
                }
            }
        }
    }
}
