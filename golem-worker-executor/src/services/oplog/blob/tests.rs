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

//! The compressed oplog archive through `OplogArchiveService`, on the in-memory, the filesystem
//! and the SQLite blob storage. Each backend runs in the process of the test.

use super::BlobOplogArchiveService;
use crate::services::oplog::OplogArchiveService;
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use futures::StreamExt;
use golem_common::config::DbSqliteConfig;
use golem_common::model::agent::AgentMode;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{LogLevel, OplogEntry, OplogIndex};
use golem_common::model::{AgentFingerprint, AgentId, OwnedAgentId, RetryConfig, ScanCursor};
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace, agent_path_segment};
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use test_r::test;

const MODE: AgentMode = AgentMode::Durable;

/// A blob archive at level 0 whose manifests are in a new in-memory indexed storage. A storage
/// call runs once, without a retry.
fn blob_archive(storage: Arc<dyn BlobStorage + Send + Sync>) -> BlobOplogArchiveService {
    BlobOplogArchiveService::new(
        storage,
        Arc::new(InMemoryIndexedStorage::new()),
        0,
        RetryConfig {
            max_attempts: 1,
            min_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            multiplier: 1.0,
            max_jitter_factor: None,
        },
    )
}

/// What the service tells about the archive of one agent: whether the archive exists, the entry
/// at the first index, and the last index.
type Observed = (bool, BTreeMap<OplogIndex, OplogEntry>, OplogIndex);

fn owned_agent_id(environment_id: EnvironmentId, h: u128) -> OwnedAgentId {
    OwnedAgentId::new(environment_id, &AgentId::from_parts(7, h))
}

fn log_entry(message: &str) -> OplogEntry {
    OplogEntry::log(
        None,
        LogLevel::Debug,
        "test".to_string(),
        message.to_string(),
        None,
    )
    .rounded()
}

/// Gives the agents of the test type that `scan_for_type` lists. The blob archive gives them in
/// one page.
async fn scan(
    service: &BlobOplogArchiveService,
    environment_id: EnvironmentId,
) -> BTreeSet<OwnedAgentId> {
    let (cursor, agents) = service
        .scan_for_type(
            &environment_id,
            golem_common::model::type_id::TypeId::try_from(7u32).unwrap(),
            Some(MODE),
            ScanCursor::default(),
            100,
        )
        .await
        .unwrap();
    assert!(cursor.is_finished());
    agents.into_iter().collect()
}

async fn observe(service: &BlobOplogArchiveService, agent: &OwnedAgentId) -> Observed {
    (
        service.exists(agent, MODE).await.unwrap(),
        service
            .read_source(agent, MODE, OplogIndex::INITIAL, 1)
            .await
            .unwrap(),
        service.get_last_index(agent, MODE).await.unwrap(),
    )
}

/// Each agent gets its own archive, which it can make, read, list and delete. The test deletes
/// one of them first and reads the others. Then it deletes the other four agents.
async fn check_that_each_agent_gets_its_own_archive(storage: Arc<dyn BlobStorage + Send + Sync>) {
    let service = blob_archive(storage);
    let environment_id = EnvironmentId::new();
    let archives: Box<[(OwnedAgentId, OplogEntry)]> = (1..=5)
        .map(|h| {
            (
                owned_agent_id(environment_id, h),
                log_entry(&format!("agent-{h}")),
            )
        })
        .collect();
    let observe_all = || {
        futures::stream::iter(archives.iter())
            .then(|(agent, _)| observe(&service, agent))
            .collect::<Vec<_>>()
    };
    let expected = |deleted: &[&OwnedAgentId]| {
        (
            archives
                .iter()
                .map(|(agent, entry)| {
                    if deleted.contains(&agent) {
                        (false, BTreeMap::new(), OplogIndex::NONE)
                    } else {
                        (
                            true,
                            BTreeMap::from([(OplogIndex::INITIAL, entry.clone())]),
                            OplogIndex::INITIAL,
                        )
                    }
                })
                .collect::<Vec<Observed>>(),
            archives
                .iter()
                .map(|(agent, _)| agent)
                .filter(|agent| !deleted.contains(agent))
                .cloned()
                .collect::<BTreeSet<_>>(),
        )
    };

    futures::stream::iter(archives.iter())
        .for_each(|(agent, entry)| {
            let service = &service;
            async move {
                service
                    .open(agent, MODE, None, AgentFingerprint::new())
                    .await
                    .unwrap()
                    .append(&[(OplogIndex::INITIAL, entry.clone())])
                    .await
                    .unwrap();
            }
        })
        .await;
    let made = (observe_all().await, scan(&service, environment_id).await);

    let first_deleted = &archives[3].0;
    service.delete(first_deleted, MODE).await.unwrap();
    let after_one_delete = (observe_all().await, scan(&service, environment_id).await);

    futures::stream::iter(archives.iter())
        .filter(|(agent, _)| futures::future::ready(agent != first_deleted))
        .for_each(|(agent, _)| {
            let service = &service;
            async move { service.delete(agent, MODE).await.unwrap() }
        })
        .await;
    let after_all_deletes = (observe_all().await, scan(&service, environment_id).await);

    let all_agents = archives
        .iter()
        .map(|(agent, _)| agent)
        .collect::<Box<[_]>>();
    assert_eq!(
        (made, after_one_delete, after_all_deletes),
        (
            expected(&[]),
            expected(&[first_deleted]),
            expected(&all_agents)
        )
    );
}

#[test]
async fn each_agent_gets_its_own_archive_on_the_in_memory_backend() {
    check_that_each_agent_gets_its_own_archive(Arc::new(InMemoryBlobStorage::new())).await;
}

#[test]
async fn each_agent_gets_its_own_archive_on_the_filesystem_backend() {
    let root = TempDir::new().unwrap();
    check_that_each_agent_gets_its_own_archive(Arc::new(
        FileSystemBlobStorage::new(root.path()).await.unwrap(),
    ))
    .await;
}

#[test]
async fn each_agent_gets_its_own_archive_on_the_sqlite_backend() {
    let root = TempDir::new().unwrap();
    let pool = SqlitePool::configured(&DbSqliteConfig {
        database: root
            .path()
            .join("blob_storage.db")
            .to_string_lossy()
            .into_owned(),
        max_connections: 4,
        foreign_keys: false,
    })
    .await
    .unwrap();
    check_that_each_agent_gets_its_own_archive(Arc::new(
        SqliteBlobStorage::new(pool).await.unwrap(),
    ))
    .await;
}

/// `drop_prefix` deletes the manifest of the archive that it empties. The next append on the same
/// archive lists its chunk in the manifest again. So the archive exists, and the scan lists the
/// agent again.
#[test]
async fn an_archive_that_drop_prefix_empties_comes_back_on_the_next_append() {
    let service = blob_archive(Arc::new(InMemoryBlobStorage::new()));
    let environment_id = EnvironmentId::new();
    let agent = owned_agent_id(environment_id, 1);
    let second = log_entry("second");
    let archive = service
        .open(&agent, MODE, None, AgentFingerprint::new())
        .await
        .unwrap();

    archive
        .append(&[(OplogIndex::INITIAL, log_entry("first"))])
        .await
        .unwrap();
    let dropped = archive.drop_prefix(OplogIndex::INITIAL).await.unwrap();
    let emptied = (
        dropped,
        service.exists(&agent, MODE).await.unwrap(),
        scan(&service, environment_id).await,
    );
    archive
        .append(&[(OplogIndex::INITIAL.next(), second.clone())])
        .await
        .unwrap();
    let appended = (
        service.exists(&agent, MODE).await.unwrap(),
        scan(&service, environment_id).await,
        service
            .read_source(&agent, MODE, OplogIndex::INITIAL.next(), 1)
            .await
            .unwrap(),
    );

    assert_eq!(
        (emptied, appended),
        (
            (1, false, BTreeSet::new()),
            (
                true,
                BTreeSet::from([agent]),
                BTreeMap::from([(OplogIndex::INITIAL.next(), second)])
            )
        )
    );
}

/// A process that stops after it stores the object of a chunk and before the manifest lists it
/// leaves an object in the directory of the archive that no manifest entry refers to. The test
/// makes such an object. The directory holds no archive: `exists` gives false, and the scan skips
/// the agent. The next append lists its own chunk, although the object is there.
#[test]
async fn a_directory_without_a_manifest_holds_no_archive() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let service = blob_archive(storage.clone());
    let environment_id = EnvironmentId::new();
    let agent = owned_agent_id(environment_id, 1);
    let entry = log_entry("entry");
    let fingerprint = AgentFingerprint::new();

    storage
        .put_raw(
            "test",
            "test",
            BlobStorageNamespace::CompressedOplog {
                environment_id,
                type_tag: agent.agent_id.type_tag(),
                agent_mode: MODE,
                level: 0,
            },
            &Path::new("chunks")
                .join(agent_path_segment(&agent.agent_id))
                .join(fingerprint.to_string())
                .join("0-unlisted"),
            b"not-an-oplog-chunk",
        )
        .await
        .unwrap();
    let before = (
        service.exists(&agent, MODE).await.unwrap(),
        scan(&service, environment_id).await,
    );
    service
        .open(&agent, MODE, None, fingerprint)
        .await
        .unwrap()
        .append(&[(OplogIndex::INITIAL, entry.clone())])
        .await
        .unwrap();
    let after = (
        service.exists(&agent, MODE).await.unwrap(),
        scan(&service, environment_id).await,
        service
            .read_source(&agent, MODE, OplogIndex::INITIAL, 1)
            .await
            .unwrap(),
    );

    assert_eq!(
        (before, after),
        (
            (false, BTreeSet::new()),
            (
                true,
                BTreeSet::from([agent]),
                BTreeMap::from([(OplogIndex::INITIAL, entry)])
            )
        )
    );
}

#[test]
async fn chunk_paths_isolate_incarnations_and_manifest_reads_need_no_fingerprint() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let service = blob_archive(storage.clone());
        let agent = owned_agent_id(EnvironmentId::new(), 1);
        let old_fingerprint = AgentFingerprint::new();
        let new_fingerprint = AgentFingerprint::new();
        let old = service
            .open(&agent, mode, None, old_fingerprint)
            .await
            .unwrap();
        old.append(&[(OplogIndex::INITIAL, log_entry("old"))])
            .await
            .unwrap();

        // Retire the old manifest while leaving its objects for delayed cleanup.
        service.manifest(&agent, mode).delete().await.unwrap();
        let new = service
            .open_fresh(&agent, mode, None, new_fingerprint)
            .await
            .unwrap();
        let entry = log_entry("new");
        new.append(&[(OplogIndex::INITIAL, entry.clone())])
            .await
            .unwrap();

        let namespace = super::BlobOplogArchive::blob_namespace(&agent, mode, 0);
        let chunks = Path::new("chunks").join(agent_path_segment(&agent.agent_id));
        for fingerprint in [old_fingerprint, new_fingerprint] {
            let prefix = chunks.join(fingerprint.to_string());
            let objects = storage
                .list_dir("test", "test", namespace.clone(), &prefix)
                .await
                .unwrap();
            assert_eq!(objects.len(), 1);
            assert_eq!(objects[0].parent(), Some(prefix.as_path()));
            let filename = objects[0].file_name().unwrap().to_str().unwrap();
            let uuid = filename
                .strip_prefix(&format!("{}-", OplogIndex::INITIAL))
                .unwrap();
            uuid::Uuid::parse_str(uuid).unwrap();
        }

        storage
            .delete_dir(
                "test",
                "test",
                namespace.clone(),
                &chunks.join(old_fingerprint.to_string()),
            )
            .await
            .unwrap();
        assert_eq!(
            service
                .read_source(&agent, mode, OplogIndex::INITIAL, 1)
                .await
                .unwrap(),
            BTreeMap::from([(OplogIndex::INITIAL, entry)])
        );
        service.delete(&agent, mode).await.unwrap();
        assert!(
            storage
                .list_blobs_below("test", "test", namespace, &chunks)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

// Persisted verification must read storage even after the writing handle cached the entries.
#[test]
async fn verify_persisted_reopens_and_detects_missing_incarnation_chunk() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let service = blob_archive(storage.clone());
        let agent = owned_agent_id(EnvironmentId::new(), 812);
        let archive = service
            .open(&agent, mode, None, AgentFingerprint::new())
            .await
            .unwrap();
        let entries = vec![(OplogIndex::INITIAL, log_entry("persisted"))];
        archive.append(&entries).await.unwrap();
        archive.verify_persisted(&entries).await.unwrap();
        assert_eq!(
            archive.read_source(OplogIndex::INITIAL, 1).await.unwrap(),
            entries.iter().cloned().collect()
        );
        let namespace = super::BlobOplogArchive::blob_namespace(&agent, mode, 0);
        let prefix = Path::new("chunks").join(agent_path_segment(&agent.agent_id));
        storage
            .delete_dir("test", "test", namespace, &prefix)
            .await
            .unwrap();
        assert!(archive.verify_persisted(&entries).await.is_err());
    }
}
