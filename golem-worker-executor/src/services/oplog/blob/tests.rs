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
use futures::StreamExt;
use golem_common::config::DbSqliteConfig;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{LogLevel, OplogEntry, OplogIndex};
use golem_common::model::{AgentId, OwnedAgentId, ScanCursor};
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace, agent_path_segment};
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use test_r::test;

const MODE: AgentMode = AgentMode::Durable;

/// What the service tells about the archive of one agent: whether the archive exists, the entry
/// at the first index, and the last index.
type Observed = (bool, BTreeMap<OplogIndex, OplogEntry>, OplogIndex);

/// The agent names of the tests. None of them can be the name of a directory in blob storage. The
/// name rules refuse a `..` segment. The one form of a blob path removes a `.` segment and an
/// empty segment, a `/` adds a directory, and a name of 500 bytes is longer than a file name.
fn agent_names() -> [String; 5] {
    [
        r#"counter("a/../b")"#.to_string(),
        r#"counter("a/./b")"#.to_string(),
        r#"counter("a//b")"#.to_string(),
        r#"counter("a/b")"#.to_string(),
        format!("counter(\"{}\")", "x".repeat(489)),
    ]
}

fn owned_agent_id(
    environment_id: EnvironmentId,
    component_id: ComponentId,
    agent_name: &str,
) -> OwnedAgentId {
    OwnedAgentId::new(
        environment_id,
        &AgentId {
            component_id,
            agent_id: agent_name.to_string(),
        },
    )
}

fn log_entry(message: &str) -> OplogEntry {
    OplogEntry::log(
        None,
        LogLevel::Debug,
        "test".to_string(),
        message.to_string(),
    )
    .rounded()
}

/// Gives the agents that `scan_for_component` lists. The blob archive gives them in one page.
async fn scan(
    service: &BlobOplogArchiveService,
    environment_id: EnvironmentId,
    component_id: ComponentId,
) -> BTreeSet<OwnedAgentId> {
    let (cursor, agents) = service
        .scan_for_component(
            &environment_id,
            &component_id,
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
        service.exists(agent, MODE).await,
        service
            .read_source(agent, MODE, OplogIndex::INITIAL, 1)
            .await,
        service.get_last_index(agent, MODE).await,
    )
}

/// Each agent gets its own archive, which it can make, read, list and delete. As blob paths,
/// `counter("a/./b")`, `counter("a//b")` and `counter("a/b")` have one form. So the test deletes
/// one of them first and reads the other two. Then it deletes the other four agents.
async fn check_that_each_agent_name_gets_its_own_archive(
    storage: Arc<dyn BlobStorage + Send + Sync>,
) {
    let service = BlobOplogArchiveService::new(storage, 0);
    let environment_id = EnvironmentId::new();
    let component_id = ComponentId::new();
    let archives: Box<[(OwnedAgentId, OplogEntry)]> = agent_names()
        .iter()
        .map(|name| {
            (
                owned_agent_id(environment_id, component_id, name),
                log_entry(name),
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
                    .open(agent, MODE)
                    .await
                    .append(&[(OplogIndex::INITIAL, entry.clone())])
                    .await;
            }
        })
        .await;
    let made = (
        observe_all().await,
        scan(&service, environment_id, component_id).await,
    );

    let first_deleted = &archives[3].0;
    service.delete(first_deleted, MODE).await;
    let after_one_delete = (
        observe_all().await,
        scan(&service, environment_id, component_id).await,
    );

    futures::stream::iter(archives.iter())
        .filter(|(agent, _)| futures::future::ready(agent != first_deleted))
        .for_each(|(agent, _)| service.delete(agent, MODE))
        .await;
    let after_all_deletes = (
        observe_all().await,
        scan(&service, environment_id, component_id).await,
    );

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
async fn each_agent_name_gets_its_own_archive_on_the_in_memory_backend() {
    check_that_each_agent_name_gets_its_own_archive(Arc::new(InMemoryBlobStorage::new())).await;
}

#[test]
async fn each_agent_name_gets_its_own_archive_on_the_filesystem_backend() {
    let root = TempDir::new().unwrap();
    check_that_each_agent_name_gets_its_own_archive(Arc::new(
        FileSystemBlobStorage::new(root.path()).await.unwrap(),
    ))
    .await;
}

#[test]
async fn each_agent_name_gets_its_own_archive_on_the_sqlite_backend() {
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
    check_that_each_agent_name_gets_its_own_archive(Arc::new(
        SqliteBlobStorage::new(pool).await.unwrap(),
    ))
    .await;
}

/// `drop_prefix` deletes the directory of the archive that it empties, with the `agent_id` blob
/// in it. The next append on the same archive makes the directory and the blob again. So the
/// archive exists, and the scan lists the agent again.
#[test]
async fn an_archive_that_drop_prefix_empties_comes_back_on_the_next_append() {
    let service = BlobOplogArchiveService::new(Arc::new(InMemoryBlobStorage::new()), 0);
    let environment_id = EnvironmentId::new();
    let component_id = ComponentId::new();
    let agent = owned_agent_id(environment_id, component_id, r#"counter("a/b")"#);
    let second = log_entry("second");
    let archive = service.open(&agent, MODE).await;

    archive
        .append(&[(OplogIndex::INITIAL, log_entry("first"))])
        .await;
    let dropped = archive.drop_prefix(OplogIndex::INITIAL).await;
    let emptied = (
        dropped,
        service.exists(&agent, MODE).await,
        scan(&service, environment_id, component_id).await,
    );
    archive
        .append(&[(OplogIndex::INITIAL.next(), second.clone())])
        .await;
    let appended = (
        service.exists(&agent, MODE).await,
        scan(&service, environment_id, component_id).await,
        service
            .read_source(&agent, MODE, OplogIndex::INITIAL.next(), 1)
            .await,
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

/// A process that stops after it makes the directory of an archive and before it writes the
/// `agent_id` blob leaves a directory without that blob. The test makes such a directory. The
/// directory holds no archive: `exists` gives false, and the scan skips the directory. The next
/// append writes the blob, although the directory is there.
#[test]
async fn a_directory_without_an_agent_id_holds_no_archive() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let service = BlobOplogArchiveService::new(storage.clone(), 0);
    let environment_id = EnvironmentId::new();
    let component_id = ComponentId::new();
    let agent = owned_agent_id(environment_id, component_id, r#"counter("a/b")"#);
    let entry = log_entry("entry");

    storage
        .create_dir(
            "test",
            "test",
            BlobStorageNamespace::CompressedOplog {
                environment_id,
                component_id,
                agent_mode: MODE,
                level: 0,
            },
            Path::new(&agent_path_segment(&agent.agent_id)),
        )
        .await
        .unwrap();
    let before = (
        service.exists(&agent, MODE).await,
        scan(&service, environment_id, component_id).await,
    );
    service
        .open(&agent, MODE)
        .await
        .append(&[(OplogIndex::INITIAL, entry.clone())])
        .await;
    let after = (
        service.exists(&agent, MODE).await,
        scan(&service, environment_id, component_id).await,
        service
            .read_source(&agent, MODE, OplogIndex::INITIAL, 1)
            .await,
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
