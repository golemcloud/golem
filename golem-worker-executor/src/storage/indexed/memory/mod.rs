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
    AgentDataObservation, AgentIndexedState, IndexedStorage, IndexedStorageError,
    IndexedStorageMetaNamespace, IndexedStorageNamespace, KeyEpochChange, KeyEpochClaim,
    ReleaseAgentResult, ScanResume, UnsealAgentResult,
};
use async_trait::async_trait;
use golem_common::model::{OwnedAgentId, ShardEpoch};
use regex::Regex;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::ops::Bound::Included;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};
use tokio::sync::Mutex;

#[derive(Debug, Default)]
struct MemoryState {
    data: HashMap<String, BTreeMap<u64, Vec<u8>>>,
    key_epochs: HashMap<String, ShardEpoch>,
    stage_reservations: HashMap<String, i64>,
}

#[derive(Debug, Default)]
pub struct InMemoryIndexedStorage {
    /// No operation awaits after acquiring this lock. Whole-agent changes, including their
    /// checks, are indivisible to other operations and to cancellation of the caller.
    state: Mutex<MemoryState>,
    #[cfg(test)]
    read_count: AtomicU64,
}

impl InMemoryIndexedStorage {
    pub fn new() -> Self {
        Self::default()
    }

    fn check_epoch(
        state: &MemoryState,
        composite_key: &str,
        key: &str,
        expected: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        if let Some(expected) = expected {
            let actual = state.key_epochs.get(composite_key).copied();
            if actual != Some(expected) {
                return Err(IndexedStorageError::Fenced {
                    key: key.to_string(),
                    expected,
                    actual,
                });
            }
        }
        Ok(())
    }

    fn check_agent_epochs(
        state: &MemoryState,
        key: &str,
        epoch: ShardEpoch,
    ) -> Result<(), IndexedStorageError> {
        for (stored_key, stored_epoch) in &state.key_epochs {
            if Self::belongs_to_agent(stored_key, key) && *stored_epoch > epoch {
                return Err(IndexedStorageError::Fenced {
                    key: key.to_string(),
                    expected: epoch,
                    actual: Some(*stored_epoch),
                });
            }
        }
        Ok(())
    }

    fn belongs_to_agent(composite_key: &str, key: &str) -> bool {
        let suffix = format!("/{key}");
        composite_key.ends_with(&suffix)
            || (composite_key.starts_with("durable/staged-oplog/")
                || composite_key.starts_with("ephemeral/staged-oplog/"))
                && composite_key
                    .split_once('#')
                    .is_some_and(|(base, _)| base.ends_with(&suffix))
    }

    #[cfg(test)]
    pub(crate) fn read_count(&self) -> u64 {
        self.read_count.load(Ordering::Relaxed)
    }

    fn composite_key(namespace: IndexedStorageNamespace, key: &str) -> String {
        match namespace {
            IndexedStorageNamespace::SealedPresent { agent_mode, .. } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}/sealed-present/{key}")
            }
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
        let prefix = regex::escape(prefix.unwrap_or(""));
        let (mode, family) = match namespace {
            IndexedStorageMetaNamespace::Oplog { agent_mode } => (agent_mode, "oplog".to_string()),
            IndexedStorageMetaNamespace::CompressedOplog { agent_mode, level } => {
                (agent_mode, format!("compressed-oplog/{level}"))
            }
            IndexedStorageMetaNamespace::BlobOplogManifest { agent_mode, level } => {
                (agent_mode, format!("blob-oplog/{level}"))
            }
        };
        let mode = super::agent_mode_prefix(mode);
        let regex = Regex::new(&format!(r"^{mode}/{family}/({prefix}.*)$")).unwrap();
        Box::new(move |key| {
            regex
                .captures(key)
                .map(|caps| caps.get(1).unwrap().as_str().to_string())
        })
    }

    fn append_checked(
        state: &mut MemoryState,
        namespace: IndexedStorageNamespace,
        key: &str,
        pairs: &[(u64, Vec<u8>)],
        expected_epoch: Option<ShardEpoch>,
        stage_token: Option<i64>,
    ) -> Result<(), IndexedStorageError> {
        let conflict_on_held_index = matches!(
            &namespace,
            IndexedStorageNamespace::OpLog { .. }
                | IndexedStorageNamespace::StagedOpLog { .. }
                | IndexedStorageNamespace::BlobOplogManifest { .. }
                | IndexedStorageNamespace::SealedPresent { .. }
        );
        let staged = matches!(&namespace, IndexedStorageNamespace::StagedOpLog { .. });
        let initial = matches!(&namespace, IndexedStorageNamespace::OpLog { .. })
            && pairs.iter().any(|(id, _)| *id == 1);
        let composite_key = Self::composite_key(namespace, key);
        if staged
            && (stage_token.is_none()
                || state
                    .stage_reservations
                    .get(key.split('#').next().unwrap_or(key))
                    .copied()
                    != stage_token)
        {
            return Err(IndexedStorageError::StageInvalidated {
                key: key.to_string(),
            });
        }
        Self::check_epoch(state, &composite_key, key, expected_epoch)?;
        if pairs.is_empty() {
            return Ok(());
        }
        let entry = state.data.entry(composite_key).or_default();
        if pairs.iter().any(|(id, _)| entry.contains_key(id)) {
            return Err(if conflict_on_held_index {
                IndexedStorageError::Conflict("Key already exists".to_string())
            } else {
                IndexedStorageError::Other("Key already exists".to_string())
            });
        }
        entry.extend(pairs.iter().cloned());
        if initial {
            state.stage_reservations.remove(key);
        }
        Ok(())
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl IndexedStorage for InMemoryIndexedStorage {
    fn supports_release(&self) -> bool {
        true
    }

    async fn reserve_stage(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        owned_agent_id: &OwnedAgentId,
        token: i64,
    ) -> Result<Option<i64>, IndexedStorageError> {
        let mut state = self.state.lock().await;
        let key = owned_agent_id.storage_key();
        if state
            .data
            .keys()
            .any(|stored| Self::belongs_to_agent(stored, &key))
        {
            return Ok(None);
        }
        state.stage_reservations.insert(key, token);
        Ok(Some(token))
    }

    async fn remove_stage_reservation(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        owned_agent_id: &OwnedAgentId,
        token: Option<i64>,
    ) -> Result<bool, IndexedStorageError> {
        let mut state = self.state.lock().await;
        let key = owned_agent_id.storage_key();
        if token.is_some() && state.stage_reservations.get(&key).copied() != token {
            return Ok(false);
        }
        Ok(state.stage_reservations.remove(&key).is_some())
    }

    async fn observe_agent(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<AgentDataObservation, IndexedStorageError> {
        let state = self.state.lock().await;
        let key = owned_agent_id.storage_key();
        let has_data = state
            .data
            .keys()
            .any(|stored_key| Self::belongs_to_agent(stored_key, &key));
        let marker = |agent_mode| {
            state
                .data
                .get(&Self::composite_key(
                    IndexedStorageNamespace::SealedPresent {
                        owned_agent_id: owned_agent_id.clone(),
                        agent_mode,
                    },
                    &key,
                ))
                .and_then(|entries| entries.get(&0))
                .cloned()
        };
        AgentDataObservation::from_raw(
            has_data,
            marker(golem_common::model::agent::AgentMode::Durable),
            marker(golem_common::model::agent::AgentMode::Ephemeral),
        )
    }

    async fn mark_sealed(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        agent: &AgentIndexedState,
        epoch: ShardEpoch,
        fingerprint: golem_common::model::AgentFingerprint,
        superseded_marker: Option<super::SealMarker>,
    ) -> Result<(), IndexedStorageError> {
        let mut state = self.state.lock().await;
        let key = agent.owned_agent_id.storage_key();
        Self::check_epoch(
            &state,
            &Self::composite_key(agent.fence_namespace(), &key),
            &key,
            Some(epoch),
        )?;
        let marker = Self::composite_key(agent.marker_namespace(), &key);
        if let Some(entries) = state.data.get(&marker) {
            let value = entries.get(&0).ok_or_else(|| {
                IndexedStorageError::Other("Missing indexed seal marker generation".into())
            })?;
            let existing = super::SealMarker::decode(value).map_err(IndexedStorageError::Other)?;
            if let Some(replacement) = existing
                .replacement(fingerprint, superseded_marker)
                .map_err(IndexedStorageError::Other)?
            {
                state.data.insert(
                    marker,
                    BTreeMap::from([(
                        0,
                        replacement.encode().map_err(IndexedStorageError::Other)?,
                    )]),
                );
            }
        } else {
            state.data.insert(
                marker,
                BTreeMap::from([(
                    0,
                    super::SealMarker::fresh(fingerprint)
                        .encode()
                        .map_err(IndexedStorageError::Other)?,
                )]),
            );
        }
        Ok(())
    }

    async fn release_agent(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        agent: &AgentIndexedState,
        epoch: ShardEpoch,
    ) -> Result<ReleaseAgentResult, IndexedStorageError> {
        let mut state = self.state.lock().await;
        let key = agent.owned_agent_id.storage_key();
        if !state
            .data
            .keys()
            .chain(state.key_epochs.keys())
            .any(|stored_key| Self::belongs_to_agent(stored_key, &key))
        {
            state.stage_reservations.remove(&key);
            return Ok(ReleaseAgentResult::Released);
        }
        Self::check_epoch(
            &state,
            &Self::composite_key(agent.fence_namespace(), &key),
            &key,
            Some(epoch),
        )?;
        Self::check_agent_epochs(&state, &key, epoch)?;
        let expected: HashMap<_, BTreeMap<_, _>> = agent
            .manifests
            .iter()
            .map(|(level, entries)| {
                (
                    Self::composite_key(agent.manifest_namespace(*level), &key),
                    entries
                        .iter()
                        .map(|(id, bytes)| (*id, bytes.to_vec()))
                        .collect(),
                )
            })
            .collect();
        for (stored_key, entries) in &state.data {
            if !Self::belongs_to_agent(stored_key, &key) || entries.is_empty() {
                continue;
            }
            let family = stored_key.split('/').nth(1);
            if family == Some("sealed-present") {
                for value in entries.values() {
                    super::SealMarker::decode(value).map_err(IndexedStorageError::Other)?;
                }
            }
            if matches!(family, Some("oplog" | "compressed-oplog"))
                || family == Some("blob-oplog") && expected.get(stored_key) != Some(entries)
            {
                return Ok(ReleaseAgentResult::NotReleased);
            }
        }
        for (manifest_key, entries) in &expected {
            if state
                .data
                .get(manifest_key)
                .is_none_or(|stored| stored != entries)
                && !entries.is_empty()
            {
                return Ok(ReleaseAgentResult::NotReleased);
            }
        }
        state
            .data
            .retain(|stored_key, _| !Self::belongs_to_agent(stored_key, &key));
        state
            .key_epochs
            .retain(|stored_key, _| !Self::belongs_to_agent(stored_key, &key));
        state.stage_reservations.remove(&key);
        Ok(ReleaseAgentResult::Released)
    }

    async fn unseal_agent(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        agent: &AgentIndexedState,
        epoch: ShardEpoch,
        record_epoch: ShardEpoch,
        valid_until: SystemTime,
        fingerprint: golem_common::model::AgentFingerprint,
    ) -> Result<UnsealAgentResult, IndexedStorageError> {
        let mut state = self.state.lock().await;
        let key = agent.owned_agent_id.storage_key();
        if epoch < record_epoch {
            return Err(IndexedStorageError::Fenced {
                key,
                expected: epoch,
                actual: Some(record_epoch),
            });
        }
        Self::check_agent_epochs(&state, &key, epoch)?;
        let mut restored: HashMap<_, BTreeMap<_, _>> = agent
            .manifests
            .iter()
            .filter(|(_, entries)| !entries.is_empty())
            .map(|(level, entries)| {
                (
                    Self::composite_key(agent.manifest_namespace(*level), &key),
                    entries
                        .iter()
                        .map(|(id, bytes)| (*id, bytes.to_vec()))
                        .collect(),
                )
            })
            .collect();
        restored.insert(
            Self::composite_key(agent.primary_namespace(), &key),
            BTreeMap::new(),
        );
        let marker = Self::composite_key(agent.marker_namespace(), &key);
        let seal_marker = match state.data.get(&marker) {
            Some(entries) => {
                let value = entries.get(&0).ok_or_else(|| {
                    IndexedStorageError::Other("Missing indexed seal marker generation".into())
                })?;
                let existing =
                    super::SealMarker::decode(value).map_err(IndexedStorageError::Other)?;
                if existing.fingerprint != fingerprint {
                    return Ok(UnsealAgentResult::AlreadyPresent);
                }
                existing
            }
            None => super::SealMarker::fresh(fingerprint),
        };
        restored.insert(
            marker,
            BTreeMap::from([(0, seal_marker.encode().map_err(IndexedStorageError::Other)?)]),
        );
        if state
            .data
            .keys()
            .any(|stored_key| Self::belongs_to_agent(stored_key, &key))
        {
            let own_rows = state
                .data
                .iter()
                .filter(|(stored_key, _)| Self::belongs_to_agent(stored_key, &key))
                .count()
                == restored.len()
                && restored
                    .iter()
                    .all(|(stored_key, entries)| state.data.get(stored_key) == Some(entries));
            let own_epochs = state
                .key_epochs
                .iter()
                .filter(|(stored_key, _)| Self::belongs_to_agent(stored_key, &key))
                .all(|(_, stored_epoch)| *stored_epoch == epoch)
                && agent.epoch_namespaces().into_iter().all(|namespace| {
                    state.key_epochs.get(&Self::composite_key(namespace, &key)) == Some(&epoch)
                });
            if own_rows && own_epochs {
                state.stage_reservations.remove(&key);
                return Ok(UnsealAgentResult::Unsealed);
            }
            return Ok(UnsealAgentResult::AlreadyPresent);
        }
        if SystemTime::now() > valid_until {
            return Ok(UnsealAgentResult::Expired);
        }
        for (stored_key, stored_epoch) in &mut state.key_epochs {
            if Self::belongs_to_agent(stored_key, &key) {
                *stored_epoch = epoch;
            }
        }
        for namespace in agent.epoch_namespaces() {
            state
                .key_epochs
                .insert(Self::composite_key(namespace, &key), epoch);
        }
        state.data.extend(restored);
        state.stage_reservations.remove(&key);
        Ok(UnsealAgentResult::Unsealed)
    }

    async fn remove_key_epoch_if_empty(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        epoch: ShardEpoch,
    ) -> Result<bool, IndexedStorageError> {
        let mut state = self.state.lock().await;
        let agent_key = if matches!(&namespace, IndexedStorageNamespace::StagedOpLog { .. }) {
            key.split('#').next().unwrap_or(key)
        } else {
            key
        };
        let composite_key = Self::composite_key(namespace, key);
        if state.key_epochs.get(&composite_key) != Some(&epoch)
            || state
                .data
                .keys()
                .any(|stored_key| Self::belongs_to_agent(stored_key, agent_key))
        {
            return Ok(false);
        }
        state.key_epochs.remove(&composite_key);
        Ok(true)
    }

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
        Ok(self
            .state
            .lock()
            .await
            .data
            .contains_key(&Self::composite_key(namespace, key)))
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
        let state = self.state.lock().await;
        let limit = count as usize;
        let mut page: BinaryHeap<String> = BinaryHeap::new();
        for stored_key in state.data.keys() {
            if let Some(key) = matcher(stored_key)
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
        }
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
        stage_token: Option<i64>,
    ) -> Result<(), IndexedStorageError> {
        let mut state = self.state.lock().await;
        Self::append_checked(
            &mut state,
            namespace,
            key,
            &[(id, value)],
            expected_epoch,
            stage_token,
        )
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
        stage_token: Option<i64>,
    ) -> Result<(), IndexedStorageError> {
        if pairs.is_empty() && !matches!(namespace, IndexedStorageNamespace::StagedOpLog { .. }) {
            return Ok(());
        }
        let pairs: Vec<_> = pairs
            .iter()
            .map(|(id, bytes)| (*id, bytes.to_vec()))
            .collect();
        let mut state = self.state.lock().await;
        Self::append_checked(
            &mut state,
            namespace.clone(),
            key,
            &pairs,
            expected_epoch,
            stage_token,
        )
    }

    async fn set_key_epoch(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        epoch: ShardEpoch,
        presence_namespaces: &[IndexedStorageNamespace],
        remove_if_absent: bool,
    ) -> Result<KeyEpochClaim, IndexedStorageError> {
        let mut state = self.state.lock().await;
        let composite_key = Self::composite_key(namespace.clone(), key);
        let stored = state.key_epochs.get(&composite_key).copied();
        if stored.is_some_and(|stored| stored > epoch) {
            return Err(IndexedStorageError::Fenced {
                key: key.to_string(),
                expected: epoch,
                actual: stored,
            });
        }
        let present = std::iter::once(&namespace)
            .chain(presence_namespaces)
            .any(|namespace| {
                state
                    .data
                    .contains_key(&Self::composite_key(namespace.clone(), key))
            });
        let change = match stored {
            None => KeyEpochChange::Inserted,
            Some(stored) if stored < epoch => KeyEpochChange::Raised,
            Some(_) => KeyEpochChange::Unchanged,
        };
        if present || !remove_if_absent || change != KeyEpochChange::Inserted {
            state.key_epochs.insert(composite_key, epoch);
        }
        Ok(KeyEpochClaim {
            has_data: present,
            change,
        })
    }

    async fn delete_with_epoch(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        expected_epoch: Option<ShardEpoch>,
        retain_epoch: bool,
    ) -> Result<(), IndexedStorageError> {
        if retain_epoch && expected_epoch.is_none() {
            return Err(IndexedStorageError::Other(
                "retaining a deletion fence requires an asserted epoch".into(),
            ));
        }
        let mode = match &namespace {
            IndexedStorageNamespace::OpLog { agent_mode, .. }
            | IndexedStorageNamespace::CompressedOpLog { agent_mode, .. }
            | IndexedStorageNamespace::BlobOplogManifest { agent_mode, .. }
            | IndexedStorageNamespace::StagedOpLog { agent_mode, .. }
            | IndexedStorageNamespace::SealedPresent { agent_mode, .. } => *agent_mode,
        };
        let mut state = self.state.lock().await;
        let agent_key = if matches!(&namespace, IndexedStorageNamespace::StagedOpLog { .. }) {
            key.split('#').next().unwrap_or(key)
        } else {
            key
        };
        let composite_key = Self::composite_key(namespace, key);
        let missing = !state.key_epochs.contains_key(&composite_key)
            && !state.data.contains_key(&composite_key);
        if missing && retain_epoch {
            Self::check_agent_epochs(&state, agent_key, expected_epoch.unwrap())?;
        }
        if !missing {
            Self::check_epoch(&state, &composite_key, key, expected_epoch)?;
        }
        state.data.remove(&composite_key);
        if retain_epoch {
            state
                .key_epochs
                .insert(composite_key, expected_epoch.unwrap());
            let mode = super::agent_mode_prefix(mode);
            state
                .data
                .remove(&format!("{mode}/sealed-present/{agent_key}"));
        } else {
            state.key_epochs.remove(&composite_key);
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
        stage_token: Option<i64>,
    ) -> Result<bool, IndexedStorageError> {
        let staged = matches!(
            &source_namespace,
            IndexedStorageNamespace::StagedOpLog { .. }
        );
        let source = Self::composite_key(source_namespace, source_key);
        let target = Self::composite_key(target_namespace, target_key);
        let mut state = self.state.lock().await;
        let agent_key = source_key.split('#').next().unwrap_or(source_key);
        if staged
            && (stage_token.is_none()
                || state.stage_reservations.get(agent_key).copied() != stage_token)
        {
            return Err(IndexedStorageError::StageInvalidated {
                key: source_key.to_string(),
            });
        }
        if state.data.contains_key(&target) {
            return Ok(false);
        }
        let source_entries = state
            .data
            .get(&source)
            .ok_or_else(|| IndexedStorageError::Other("source index is missing".into()))?;
        if expected_last_id == 0
            || source_entries.len() as u64 != expected_last_id
            || source_entries.keys().copied().ne(1..=expected_last_id)
        {
            return Err(IndexedStorageError::Other(
                "source index is empty, gapped, or has an unexpected tip".into(),
            ));
        }
        let entries = state.data.remove(&source).unwrap();
        if staged {
            state.stage_reservations.remove(agent_key);
        }
        state.data.insert(target, entries);
        Ok(true)
    }

    async fn length(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, IndexedStorageError> {
        Ok(self
            .state
            .lock()
            .await
            .data
            .get(&Self::composite_key(namespace, key))
            .map_or(0, |entries| entries.len() as u64))
    }

    async fn discard_stage(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        stage_token: Option<i64>,
    ) -> Result<(), IndexedStorageError> {
        let mut state = self.state.lock().await;
        let agent_key = key.split('#').next().unwrap_or(key);
        if stage_token.is_some() && state.stage_reservations.get(agent_key).copied() == stage_token
        {
            state.data.remove(&Self::composite_key(namespace, key));
            state.stage_reservations.remove(agent_key);
        }
        Ok(())
    }

    async fn delete(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError> {
        let mut state = self.state.lock().await;
        state.data.remove(&Self::composite_key(namespace, key));
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
        Ok(self
            .state
            .lock()
            .await
            .data
            .get(&Self::composite_key(namespace, key))
            .map(|entries| {
                entries
                    .range((Included(start_id), Included(end_id)))
                    .map(|(id, bytes)| (*id, bytes.clone()))
                    .collect()
            })
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
        Ok(self
            .state
            .lock()
            .await
            .data
            .get(&Self::composite_key(namespace, key))
            .and_then(|entries| {
                entries
                    .first_key_value()
                    .map(|(id, bytes)| (*id, bytes.clone()))
            }))
    }

    async fn last(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        Ok(self
            .state
            .lock()
            .await
            .data
            .get(&Self::composite_key(namespace, key))
            .and_then(|entries| {
                entries
                    .last_key_value()
                    .map(|(id, bytes)| (*id, bytes.clone()))
            }))
    }

    async fn last_id(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<u64>, IndexedStorageError> {
        Ok(self
            .state
            .lock()
            .await
            .data
            .get(&Self::composite_key(namespace, key))
            .and_then(|entries| entries.last_key_value().map(|(id, _)| *id)))
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
        Ok(self
            .state
            .lock()
            .await
            .data
            .get(&Self::composite_key(namespace, key))
            .and_then(|entries| {
                entries
                    .range(id..)
                    .next()
                    .map(|(id, bytes)| (*id, bytes.clone()))
            }))
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
        let mut state = self.state.lock().await;
        Self::check_epoch(&state, &composite_key, key, expected_epoch)?;
        if let Some(entries) = state.data.get_mut(&composite_key) {
            entries.retain(|id, _| *id > last_dropped_id);
            if delete_if_empty && entries.is_empty() {
                state.data.remove(&composite_key);
            }
        }
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
        let mut state = self.state.lock().await;
        Self::check_epoch(&state, &composite_key, key, expected_epoch)?;
        if state
            .data
            .get(&composite_key)
            .is_some_and(|entries| !entries.is_empty())
        {
            return Ok(false);
        }
        state.data.remove(&composite_key);
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
