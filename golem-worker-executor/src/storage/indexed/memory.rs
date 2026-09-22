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

use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageMetaNamespace, IndexedStorageNamespace,
    ScanCursor, ScanResume, WriterId,
};
use async_trait::async_trait;
use golem_common::model::AgentId;
use golem_common::model::ShardEpoch;
use regex::Regex;
use std::collections::{BTreeMap, BinaryHeap};
use std::ops::Bound::Included;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The maps are shared, so [`Self::for_writer`] can hand out a second handle onto the same store
/// that writes as somebody else.
#[derive(Debug)]
pub struct InMemoryIndexedStorage {
    data: Arc<scc::HashMap<String, BTreeMap<u64, Vec<u8>>>>,
    /// The writer generation recorded per key. An append that asserts an epoch holds this entry
    /// while it writes `data`, which is what makes the check and the insert one step.
    key_epochs: Arc<scc::HashMap<String, (ShardEpoch, WriterId)>>,
    writer_id: WriterId,
    #[cfg(test)]
    read_count: Arc<AtomicU64>,
}

impl Default for InMemoryIndexedStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryIndexedStorage {
    pub fn new() -> Self {
        Self {
            data: Arc::new(scc::HashMap::new()),
            key_epochs: Arc::new(scc::HashMap::new()),
            writer_id: WriterId::process(),
            #[cfg(test)]
            read_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A second handle onto this same store that writes as `writer_id`: how two processes racing
    /// over one key are played out inside a single one.
    pub fn for_writer(&self, writer_id: WriterId) -> Self {
        Self {
            data: self.data.clone(),
            key_epochs: self.key_epochs.clone(),
            writer_id,
            #[cfg(test)]
            read_count: self.read_count.clone(),
        }
    }

    /// Refuses unless `record` holds exactly `expected`, recorded by this writer; an absent record
    /// refuses too. The same terms as the SQL backends' check.
    fn check_record(
        &self,
        key: &str,
        expected: ShardEpoch,
        record: &scc::hash_map::Entry<'_, String, (ShardEpoch, WriterId)>,
    ) -> Result<(), IndexedStorageError> {
        let stored = match record {
            scc::hash_map::Entry::Occupied(occupied) => Some(*occupied.get()),
            scc::hash_map::Entry::Vacant(_) => None,
        };
        match stored {
            Some((epoch, writer)) if epoch == expected && writer == self.writer_id => Ok(()),
            other => Err(IndexedStorageError::Fenced {
                key: key.to_string(),
                expected,
                actual: other.map(|(epoch, _)| epoch),
                writer_conflict: other
                    .is_some_and(|(epoch, writer)| epoch == expected && writer != self.writer_id),
            }),
        }
    }

    /// Inserts `pairs` under `composite_key`, all or nothing, after checking `expected_epoch`
    /// against the key's record. The record's entry is held until the insert is done.
    async fn append_checked(
        &self,
        composite_key: String,
        key: &str,
        pairs: &[(u64, Vec<u8>)],
        expected_epoch: Option<ShardEpoch>,
        primary_oplog_insert: bool,
    ) -> Result<(), IndexedStorageError> {
        let _record = match expected_epoch {
            None => None,
            Some(expected) => {
                let record = self.key_epochs.entry_async(composite_key.clone()).await;
                self.check_record(key, expected, &record)?;
                Some(record)
            }
        };

        let mut entry = self.data.entry_async(composite_key).await.or_default();
        if pairs.iter().any(|(id, _)| entry.contains_key(id)) {
            return Err(if primary_oplog_insert {
                IndexedStorageError::Conflict("Key already exists".to_string())
            } else {
                IndexedStorageError::Other("Key already exists".to_string())
            });
        }
        for (id, value) in pairs {
            entry.get_mut().insert(*id, value.clone());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn read_count(&self) -> u64 {
        self.read_count.load(Ordering::Relaxed)
    }

    fn composite_key(namespace: IndexedStorageNamespace, key: &str) -> String {
        match namespace {
            IndexedStorageNamespace::OpLog {
                agent_id:
                    AgentId {
                        component_id,
                        agent_id: agent_name,
                    },
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}/oplog/{component_id}/{agent_name}/{key}")
            }
            IndexedStorageNamespace::StagedOpLog {
                agent_id:
                    AgentId {
                        component_id,
                        agent_id: agent_name,
                    },
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}/staged-oplog/{component_id}/{agent_name}/{key}")
            }
            IndexedStorageNamespace::CompressedOpLog {
                agent_id:
                    AgentId {
                        component_id,
                        agent_id: agent_name,
                    },
                agent_mode,
                level,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}/compressed-oplog/{level}/{component_id}/{agent_name}/{key}")
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn match_key(
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
    ) -> Box<dyn Fn(&str) -> Option<String> + Send + Sync> {
        let prefix = prefix.unwrap_or("");
        match namespace {
            IndexedStorageMetaNamespace::Oplog { agent_mode } => {
                let mode = super::agent_mode_prefix(agent_mode);
                let pattern: String = format!(
                    r"^{mode}/oplog/([^/]+)/([^/]+)/({}.*)$",
                    regex::escape(prefix)
                );
                let regex = Regex::new(&pattern).unwrap();

                Box::new(move |key| {
                    regex
                        .captures(key)
                        .map(|caps| caps.get(3).unwrap().as_str().to_string())
                })
            }
            IndexedStorageMetaNamespace::CompressedOplog { agent_mode, level } => {
                let mode = super::agent_mode_prefix(agent_mode);
                let pattern: String = format!(
                    r"^{mode}/compressed-oplog/{level}/([^/]+)/([^/]+)/({}.*)$",
                    regex::escape(prefix)
                );
                let regex = Regex::new(&pattern).unwrap();

                Box::new(move |key| {
                    regex
                        .captures(key)
                        .map(|caps| caps.get(3).unwrap().as_str().to_string())
                })
            }
        }
    }
}

#[async_trait]
impl IndexedStorage for InMemoryIndexedStorage {
    async fn number_of_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
    ) -> Result<u8, IndexedStorageError> {
        Ok(0)
    }

    async fn wait_for_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _replicas: u8,
        _timeout: Duration,
    ) -> Result<u8, IndexedStorageError> {
        Ok(0)
    }

    async fn exists(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self.data.contains_async(&composite_key).await)
    }

    async fn scan(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), IndexedStorageError> {
        let mut result = Vec::new();
        let matcher = Self::match_key(namespace, prefix);
        let mut idx = 0;
        let mut has_more = false;

        self.data
            .iter_async(|key, _| {
                idx += 1;
                if idx > cursor {
                    if let Some(matched) = matcher(key) {
                        result.push(matched);

                        if (result.len() as u64) == count {
                            has_more = true;
                            false
                        } else {
                            true
                        }
                    } else {
                        true
                    }
                } else {
                    true
                }
            })
            .await;

        if has_more {
            Ok((idx, result))
        } else {
            Ok((0, result))
        }
    }

    async fn scan_stable(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        resume: Option<ScanResume>,
        count: u64,
    ) -> Result<(Option<ScanResume>, Vec<String>), IndexedStorageError> {
        let after = resume
            .map(|resume| resume.into_marker("In-memory"))
            .transpose()?;
        let matcher = Self::match_key(namespace, prefix);

        // The map is unordered, so keep the `count` smallest matching keys in a capped max-heap
        // rather than sorting every key in the namespace.
        let limit = count as usize;
        let mut page: BinaryHeap<String> = BinaryHeap::new();
        self.data
            .iter_async(|key, _| {
                if let Some(key) = matcher(key)
                    && after.as_deref().is_none_or(|after| key.as_str() > after)
                {
                    if page.len() < limit {
                        page.push(key);
                    } else if let Some(highest) = page.peek()
                        && key < *highest
                    {
                        page.pop();
                        page.push(key);
                    }
                }
                true
            })
            .await;
        let matched = page.into_sorted_vec();

        Ok((super::last_key_resume(&matched, count), matched))
    }

    async fn append(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        value: Vec<u8>,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        let primary_oplog_insert = matches!(
            &namespace,
            IndexedStorageNamespace::OpLog { .. } | IndexedStorageNamespace::StagedOpLog { .. }
        );
        let composite_key = Self::composite_key(namespace, key);
        self.append_checked(
            composite_key,
            key,
            &[(id, value)],
            expected_epoch,
            primary_oplog_insert,
        )
        .await
    }

    async fn append_many(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: &IndexedStorageNamespace,
        key: &str,
        pairs: Arc<[(u64, bytes::Bytes)]>,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        // Nothing to write is nothing to fence, as on every other backend.
        if pairs.is_empty() {
            return Ok(());
        }
        let primary_oplog_insert = matches!(
            namespace,
            IndexedStorageNamespace::OpLog { .. } | IndexedStorageNamespace::StagedOpLog { .. }
        );
        let composite_key = Self::composite_key(namespace.clone(), key);
        let pairs: Vec<(u64, Vec<u8>)> = pairs
            .iter()
            .map(|(id, value)| (*id, value.to_vec()))
            .collect();
        self.append_checked(
            composite_key,
            key,
            &pairs,
            expected_epoch,
            primary_oplog_insert,
        )
        .await
    }

    async fn set_key_epoch(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        epoch: ShardEpoch,
    ) -> Result<(), IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        match self.key_epochs.entry_async(composite_key).await {
            scc::hash_map::Entry::Vacant(vacant) => {
                vacant.insert_entry((epoch, self.writer_id));
                Ok(())
            }
            scc::hash_map::Entry::Occupied(mut occupied) => {
                let (stored, writer) = *occupied.get();
                if epoch > stored || (epoch == stored && writer == self.writer_id) {
                    *occupied.get_mut() = (epoch, self.writer_id);
                    Ok(())
                } else {
                    Err(IndexedStorageError::Fenced {
                        key: key.to_string(),
                        expected: epoch,
                        actual: Some(stored),
                        writer_conflict: epoch == stored && writer != self.writer_id,
                    })
                }
            }
        }
    }

    async fn delete_with_epoch(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        // The record's guard first and held across the data removal, in the order an append takes
        // them, so nobody can record a new generation between the check and the deletes.
        let record = self.key_epochs.entry_async(composite_key.clone()).await;
        if let Some(expected) = expected_epoch {
            self.check_record(key, expected, &record)?;
        }
        self.data.remove_async(&composite_key).await;
        if let scc::hash_map::Entry::Occupied(occupied) = record {
            let _ = occupied.remove();
        }
        Ok(())
    }

    async fn move_if_absent(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        source_namespace: IndexedStorageNamespace,
        source_key: &str,
        target_namespace: IndexedStorageNamespace,
        target_key: &str,
        expected_last_id: u64,
    ) -> Result<bool, IndexedStorageError> {
        let source = Self::composite_key(source_namespace, source_key);
        let target = Self::composite_key(target_namespace, target_key);
        if self.data.contains_async(&target).await {
            return Ok(false);
        }
        let source_entries = self
            .data
            .read_async(&source, |_, entries| entries.clone())
            .await
            .ok_or_else(|| IndexedStorageError::Other("source index is missing".to_string()))?;
        if expected_last_id == 0
            || source_entries.len() as u64 != expected_last_id
            || source_entries.keys().copied().ne(1..=expected_last_id)
        {
            return Err(IndexedStorageError::Other(
                "source index is empty, gapped, or has an unexpected tip".to_string(),
            ));
        }
        if self
            .data
            .insert_async(target, source_entries)
            .await
            .is_err()
        {
            return Ok(false);
        }
        self.data.remove_async(&source).await;
        Ok(true)
    }

    async fn length(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| entry.len() as u64)
            .await
            .unwrap_or_default())
    }

    async fn delete(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        self.data.remove_async(&composite_key).await;
        Ok(())
    }

    async fn read(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<(u64, Vec<u8>)>, IndexedStorageError> {
        #[cfg(test)]
        self.read_count.fetch_add(1, Ordering::Relaxed);

        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                let mut result = Vec::new();
                for (id, value) in entry.range((Included(start_id), Included(end_id))) {
                    result.push((*id, value.clone()));
                }
                result
            })
            .await
            .unwrap_or_default())
    }

    async fn first(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                let first = entry.first_key_value();
                first.map(|(id, value)| (*id, value.clone()))
            })
            .await
            .flatten())
    }

    async fn last(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                let last = entry.last_key_value();
                last.map(|(id, value)| (*id, value.clone()))
            })
            .await
            .flatten())
    }

    async fn last_id(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                entry.last_key_value().map(|(id, _)| *id)
            })
            .await
            .flatten())
    }

    async fn closest(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                entry
                    .keys()
                    .find(|k| **k >= id)
                    .map(|key| (*key, entry[key].clone()))
            })
            .await
            .flatten())
    }

    async fn drop_prefix(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        self.data
            .update_async(&composite_key, |_, entry| {
                entry.retain(|k, _| *k > last_dropped_id);
            })
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::storage::indexed::{
        IndexedStorage, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
        IndexedStorageNamespace,
    };
    use assert2::check;
    use golem_common::model::AgentId;
    use golem_common::model::component::ComponentId;

    fn test_agent_id() -> AgentId {
        use std::sync::OnceLock;
        static WORKER_ID: OnceLock<AgentId> = OnceLock::new();

        WORKER_ID
            .get_or_init(|| AgentId {
                component_id: ComponentId::new(),
                agent_id: "worker".to_string(),
            })
            .clone()
    }

    fn staged_namespace() -> IndexedStorageNamespace {
        IndexedStorageNamespace::StagedOpLog {
            agent_id: test_agent_id(),
            agent_mode: golem_common::model::agent::AgentMode::Durable,
        }
    }

    fn primary_namespace() -> IndexedStorageNamespace {
        IndexedStorageNamespace::OpLog {
            agent_id: test_agent_id(),
            agent_mode: golem_common::model::agent::AgentMode::Durable,
        }
    }

    #[test]
    async fn staged_publication_is_atomic_validated_and_hidden() {
        let storage = super::InMemoryIndexedStorage::new();
        for (id, value) in [(1, b"one"), (2, b"two")] {
            storage
                .append(
                    "test",
                    "append",
                    "entry",
                    staged_namespace(),
                    "stage",
                    id,
                    value.to_vec(),
                    None,
                )
                .await
                .unwrap();
        }
        assert_eq!(
            storage
                .scan(
                    "test",
                    "scan",
                    IndexedStorageMetaNamespace::Oplog {
                        agent_mode: golem_common::model::agent::AgentMode::Durable,
                    },
                    None,
                    0,
                    100,
                )
                .await
                .unwrap()
                .1,
            Vec::<String>::new()
        );
        assert!(
            storage
                .move_if_absent(
                    "test",
                    "publish",
                    staged_namespace(),
                    "stage",
                    primary_namespace(),
                    "target",
                    2
                )
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .read(
                    "test",
                    "read",
                    "entry",
                    IndexedStorageNamespace::OpLog {
                        agent_id: test_agent_id(),
                        agent_mode: golem_common::model::agent::AgentMode::Durable
                    },
                    "target",
                    1,
                    2
                )
                .await
                .unwrap(),
            vec![(1, b"one".to_vec()), (2, b"two".to_vec())]
        );
        assert!(
            !storage
                .exists("test", "exists", staged_namespace(), "stage")
                .await
                .unwrap()
        );

        for (key, pairs, tip) in [
            ("empty", vec![], 1),
            ("gap", vec![(1, b"one".to_vec()), (3, b"three".to_vec())], 3),
            ("tip", vec![(1, b"one".to_vec())], 2),
        ] {
            for (id, value) in pairs {
                storage
                    .append(
                        "test",
                        "append",
                        "entry",
                        staged_namespace(),
                        key,
                        id,
                        value,
                        None,
                    )
                    .await
                    .unwrap();
            }
            assert!(
                storage
                    .move_if_absent(
                        "test",
                        "publish",
                        staged_namespace(),
                        key,
                        primary_namespace(),
                        key,
                        tip
                    )
                    .await
                    .is_err()
            );
            assert!(
                !storage
                    .exists(
                        "test",
                        "exists",
                        IndexedStorageNamespace::OpLog {
                            agent_id: test_agent_id(),
                            agent_mode: golem_common::model::agent::AgentMode::Durable
                        },
                        key
                    )
                    .await
                    .unwrap()
            );
        }
    }

    #[test]
    async fn staged_publication_never_overwrites_and_has_one_concurrent_winner() {
        let storage = std::sync::Arc::new(super::InMemoryIndexedStorage::new());
        for stage in ["first", "second"] {
            storage
                .append(
                    "test",
                    "append",
                    "entry",
                    staged_namespace(),
                    stage,
                    1,
                    stage.as_bytes().to_vec(),
                    None,
                )
                .await
                .unwrap();
        }
        let a = {
            let storage = storage.clone();
            tokio::spawn(async move {
                storage
                    .move_if_absent(
                        "test",
                        "publish",
                        staged_namespace(),
                        "first",
                        primary_namespace(),
                        "target",
                        1,
                    )
                    .await
                    .unwrap()
            })
        };
        let b = {
            let storage = storage.clone();
            tokio::spawn(async move {
                storage
                    .move_if_absent(
                        "test",
                        "publish",
                        staged_namespace(),
                        "second",
                        primary_namespace(),
                        "target",
                        1,
                    )
                    .await
                    .unwrap()
            })
        };
        assert_ne!(a.await.unwrap(), b.await.unwrap());
        let before = storage
            .read(
                "test",
                "read",
                "entry",
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                "target",
                1,
                1,
            )
            .await
            .unwrap();
        assert!(
            !storage
                .move_if_absent(
                    "test",
                    "publish",
                    staged_namespace(),
                    if before[0].1 == b"first" {
                        "second"
                    } else {
                        "first"
                    },
                    primary_namespace(),
                    "target",
                    1
                )
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .read(
                    "test",
                    "read",
                    "entry",
                    IndexedStorageNamespace::OpLog {
                        agent_id: test_agent_id(),
                        agent_mode: golem_common::model::agent::AgentMode::Durable
                    },
                    "target",
                    1,
                    1
                )
                .await
                .unwrap(),
            before
        );

        storage
            .append(
                "test",
                "append",
                "entry",
                staged_namespace(),
                "third",
                1,
                b"staged".to_vec(),
                None,
            )
            .await
            .unwrap();
        let publish = {
            let storage = storage.clone();
            tokio::spawn(async move {
                storage
                    .move_if_absent(
                        "test",
                        "publish",
                        staged_namespace(),
                        "third",
                        primary_namespace(),
                        "ordinary-race",
                        1,
                    )
                    .await
                    .unwrap()
            })
        };
        let append = {
            let storage = storage.clone();
            tokio::spawn(async move {
                storage
                    .append(
                        "test",
                        "append",
                        "entry",
                        IndexedStorageNamespace::OpLog {
                            agent_id: test_agent_id(),
                            agent_mode: golem_common::model::agent::AgentMode::Durable,
                        },
                        "ordinary-race",
                        1,
                        b"ordinary".to_vec(),
                        None,
                    )
                    .await
                    .is_ok()
            })
        };
        assert_ne!(publish.await.unwrap(), append.await.unwrap());
    }

    #[test]
    async fn scan_stable_pages_in_order_and_hands_a_key_back_once() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let agent_mode = golem_common::model::agent::AgentMode::Durable;

        for key in ["k1", "k2", "k3", "k4", "k5"] {
            api.append(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode,
                },
                key,
                1,
                &100,
                None,
            )
            .await
            .unwrap();
        }

        let mut pages: Vec<Vec<String>> = Vec::new();
        let mut resume = None;
        for _ in 0..8 {
            let (next, page) = storage
                .with("test", "test")
                .scan_stable(
                    IndexedStorageMetaNamespace::Oplog { agent_mode },
                    None,
                    resume,
                    2,
                )
                .await
                .unwrap();
            pages.push(page);
            match next {
                Some(next) => resume = Some(next),
                None => break,
            }
        }

        // The short page is what ends the walk, so five keys cost three pages and not a fourth,
        // and no key appears in two of them.
        let expected: Vec<Vec<String>> = vec![
            vec!["k1".to_string(), "k2".to_string()],
            vec!["k3".to_string(), "k4".to_string()],
            vec!["k5".to_string()],
        ];
        check!(pages == expected);
    }

    #[test]
    async fn closest_exact_match() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            1,
            &100,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            2,
            &200,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            3,
            &300,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            4,
            &400,
            None,
        )
        .await
        .unwrap();

        let result = api
            .closest(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                3,
            )
            .await
            .unwrap();

        check!(result == Some((3, 300)));
    }

    #[test]
    async fn closest_no_match() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            1,
            &100,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            2,
            &200,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            3,
            &300,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            4,
            &400,
            None,
        )
        .await
        .unwrap();

        let result: Option<(u64, i32)> = api
            .closest(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                5,
            )
            .await
            .unwrap();

        check!(result == None);
    }

    #[test]
    async fn closest_match() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            10,
            &100,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            20,
            &200,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            30,
            &300,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            40,
            &400,
            None,
        )
        .await
        .unwrap();

        let result = api
            .closest(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                33,
            ) // 40 is the closest that is <= 33
            .await
            .unwrap();

        check!(result == Some((40, 400)));
    }

    #[test]
    async fn read() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            10,
            &100,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            20,
            &200,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            30,
            &300,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            40,
            &400,
            None,
        )
        .await
        .unwrap();

        let result = api
            .read(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                20,
                40,
            )
            .await
            .unwrap();

        check!(result == vec![(20, 200), (30, 300), (40, 400)]);
    }

    #[test]
    async fn read_wider() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            10,
            &100,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            20,
            &200,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            30,
            &300,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            40,
            &400,
            None,
        )
        .await
        .unwrap();

        let result = api
            .read(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                1,
                100,
            )
            .await
            .unwrap();

        check!(result == vec![(10, 100), (20, 200), (30, 300), (40, 400)]);
    }

    #[test]
    async fn first() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            10,
            &100,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            20,
            &200,
            None,
        )
        .await
        .unwrap();

        let result = api
            .first(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
            )
            .await
            .unwrap();

        check!(result == Some((10, 100)));
    }

    #[test]
    async fn last() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            10,
            &100,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            20,
            &200,
            None,
        )
        .await
        .unwrap();

        let result = api
            .last(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
            )
            .await
            .unwrap();

        check!(result == Some((20, 200)));
    }

    #[test]
    async fn drop_prefix() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            1,
            &100,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            2,
            &200,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            3,
            &300,
            None,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            4,
            &400,
            None,
        )
        .await
        .unwrap();

        storage
            .with("test", "test")
            .drop_prefix(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                2,
            )
            .await
            .unwrap();

        let result = api
            .read(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                1,
                4,
            )
            .await
            .unwrap();

        check!(result == vec![(3, 300), (4, 400)]);
    }
}
