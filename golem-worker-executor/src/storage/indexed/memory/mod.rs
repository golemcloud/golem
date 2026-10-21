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
    ScanResume,
};
use async_trait::async_trait;
use golem_common::model::ShardEpoch;
use regex::Regex;
use std::collections::{BTreeMap, BinaryHeap};
use std::ops::Bound::Included;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug)]
pub struct InMemoryIndexedStorage {
    data: Arc<scc::HashMap<String, BTreeMap<u64, Vec<u8>>>>,
    /// The writer generation recorded per key. An append that asserts an epoch holds this entry
    /// while it writes `data`, which is what makes the check and the insert one step.
    key_epochs: Arc<scc::HashMap<String, ShardEpoch>>,
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
            #[cfg(test)]
            read_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Refuses unless `record` holds exactly `expected`; an absent record refuses too. The same
    /// terms as the SQL backends' check.
    fn check_record(
        &self,
        key: &str,
        expected: ShardEpoch,
        record: &scc::hash_map::Entry<'_, String, ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        let stored = match record {
            scc::hash_map::Entry::Occupied(occupied) => Some(*occupied.get()),
            scc::hash_map::Entry::Vacant(_) => None,
        };
        match stored {
            Some(epoch) if epoch == expected => Ok(()),
            actual => Err(IndexedStorageError::Fenced {
                key: key.to_string(),
                expected,
                actual,
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
        conflict_on_held_index: bool,
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
            return Err(if conflict_on_held_index {
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

    /// Gives the key of the map for `key` in `namespace`. The key holds no agent id, because
    /// `key` already names the agent. The other indexed storage backends do the same. An agent
    /// name can hold `/`, so with a separate part for the name, a scan cannot find where the name
    /// stops.
    fn composite_key(namespace: IndexedStorageNamespace, key: &str) -> String {
        match namespace {
            IndexedStorageNamespace::OpLog { agent_mode, .. } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}/oplog/{key}")
            }
            IndexedStorageNamespace::StagedOpLog { agent_mode, .. } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}/staged-oplog/{key}")
            }
            IndexedStorageNamespace::CompressedOpLog {
                agent_mode, level, ..
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}/compressed-oplog/{level}/{key}")
            }
            IndexedStorageNamespace::BlobOplogManifest {
                agent_mode, level, ..
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}/blob-oplog/{level}/{key}")
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
                let pattern: String = format!(r"^{mode}/oplog/({}.*)$", regex::escape(prefix));
                let regex = Regex::new(&pattern).unwrap();

                Box::new(move |key| {
                    regex
                        .captures(key)
                        .map(|caps| caps.get(1).unwrap().as_str().to_string())
                })
            }
            IndexedStorageMetaNamespace::CompressedOplog { agent_mode, level } => {
                let mode = super::agent_mode_prefix(agent_mode);
                let pattern: String = format!(
                    r"^{mode}/compressed-oplog/{level}/({}.*)$",
                    regex::escape(prefix)
                );
                let regex = Regex::new(&pattern).unwrap();

                Box::new(move |key| {
                    regex
                        .captures(key)
                        .map(|caps| caps.get(1).unwrap().as_str().to_string())
                })
            }
            IndexedStorageMetaNamespace::BlobOplogManifest { agent_mode, level } => {
                let mode = super::agent_mode_prefix(agent_mode);
                let pattern: String =
                    format!(r"^{mode}/blob-oplog/{level}/({}.*)$", regex::escape(prefix));
                let regex = Regex::new(&pattern).unwrap();

                Box::new(move |key| {
                    regex
                        .captures(key)
                        .map(|caps| caps.get(1).unwrap().as_str().to_string())
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
        let conflict_on_held_index = matches!(
            &namespace,
            IndexedStorageNamespace::OpLog { .. }
                | IndexedStorageNamespace::StagedOpLog { .. }
                | IndexedStorageNamespace::BlobOplogManifest { .. }
        );
        let composite_key = Self::composite_key(namespace, key);
        self.append_checked(
            composite_key,
            key,
            &[(id, value)],
            expected_epoch,
            conflict_on_held_index,
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
        let conflict_on_held_index = matches!(
            namespace,
            IndexedStorageNamespace::OpLog { .. }
                | IndexedStorageNamespace::StagedOpLog { .. }
                | IndexedStorageNamespace::BlobOplogManifest { .. }
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
            conflict_on_held_index,
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
                vacant.insert_entry(epoch);
                Ok(())
            }
            scc::hash_map::Entry::Occupied(mut occupied) => {
                let stored = *occupied.get();
                if epoch >= stored {
                    *occupied.get_mut() = epoch;
                    Ok(())
                } else {
                    Err(IndexedStorageError::Fenced {
                        key: key.to_string(),
                        expected: epoch,
                        actual: Some(stored),
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
            // Neither a record nor entries: already gone, most often by an earlier attempt of
            // this same deletion (see the trait).
            if matches!(record, scc::hash_map::Entry::Vacant(_))
                && !self.data.contains_async(&composite_key).await
            {
                return Ok(());
            }
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
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        let delete_if_empty = matches!(
            &namespace,
            IndexedStorageNamespace::CompressedOpLog { .. }
                | IndexedStorageNamespace::BlobOplogManifest { .. }
        );
        let composite_key = Self::composite_key(namespace, key);
        // The record's guard is held across the trim, in the order an append takes them, so
        // nobody can record a new generation between the check and the trim.
        let record = match expected_epoch {
            Some(expected) => {
                let record = self.key_epochs.entry_async(composite_key.clone()).await;
                self.check_record(key, expected, &record)?;
                Some(record)
            }
            None => None,
        };
        self.data
            .update_async(&composite_key, |_, entry| {
                entry.retain(|k, _| *k > last_dropped_id);
            })
            .await;
        if delete_if_empty {
            self.data
                .remove_if_async(&composite_key, |entry| entry.is_empty())
                .await;
        }
        drop(record);
        Ok(())
    }

    async fn delete_empty_with_epoch(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<bool, IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        // The record's guard and then the key's entry are held across the emptiness test and the
        // removal, in the order a fenced append takes them, so no writer can add an entry in
        // between: a fenced one waits on the record, an unfenced one on the entry.
        let record = self.key_epochs.entry_async(composite_key.clone()).await;
        if let Some(expected) = expected_epoch {
            self.check_record(key, expected, &record)?;
        }
        let empty = match self.data.entry_async(composite_key).await {
            scc::hash_map::Entry::Occupied(entries) if entries.get().is_empty() => {
                let _ = entries.remove_entry();
                true
            }
            scc::hash_map::Entry::Occupied(_) => false,
            scc::hash_map::Entry::Vacant(_) => true,
        };
        drop(record);
        Ok(empty)
    }
}

#[cfg(test)]
mod tests;
