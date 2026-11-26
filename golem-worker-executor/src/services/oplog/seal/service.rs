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
use crate::services::oplog::epoch::retry_storage_op_fenceable;
use crate::services::oplog::{
    CreateStagedError, OplogError, OplogFence, OplogLifecycleGuard, OplogOpenPurpose,
};
use crate::services::promise::{RedisPromiseState, promise_namespace_of};
use crate::services::scheduler::SchedulerService;
use crate::services::shard::ShardService;
use crate::storage::indexed::{
    AgentDataObservation, AgentIndexedState, IndexedStorage, IndexedStorageError,
    IndexedStorageLabelledApi, IndexedStorageNamespace, MAX_INDEXED_ID, ReleaseAgentResult,
    UnsealAgentResult,
};
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use crate::worker::snapshot_selection::kept_rejections;
use bytes::Bytes;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{PromiseId, RetryConfig, ScheduledAction};
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::PutIfAbsent;
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

mod deletion;
mod scan;

#[cfg(test)]
mod tests;

pub(crate) enum AgentSealState {
    Present,
    Sealed(Box<SealRecord>),
    Absent,
}

pub(crate) struct PendingSeal {
    pub record: SealRecord,
    state: AgentIndexedState,
    key: SealRecordKey,
    listed_records: Vec<ListedSealRecord>,
    rejected_snapshot_copy: Option<Bytes>,
}

/// Shared storage boundary for sealed oplogs and their moved data. The scheduler is weak:
/// it already owns the outer oplog service that holds this service.
pub struct SealService {
    indexed: Arc<dyn IndexedStorage + Send + Sync>,
    pub(crate) blobs: Arc<dyn BlobStorage + Send + Sync>,
    kv: Arc<dyn KeyValueStorage + Send + Sync>,
    shard: Arc<dyn ShardService>,
    scheduler: Weak<dyn SchedulerService>,
    config: OplogConfig,
    indexed_retry: RetryConfig,
}

impl std::fmt::Debug for SealService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealService")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl SealService {
    pub fn new(
        indexed: Arc<dyn IndexedStorage + Send + Sync>,
        blobs: Arc<dyn BlobStorage + Send + Sync>,
        kv: Arc<dyn KeyValueStorage + Send + Sync>,
        shard: Arc<dyn ShardService>,
        scheduler: Weak<dyn SchedulerService>,
        config: OplogConfig,
        indexed_retry: RetryConfig,
    ) -> Self {
        Self {
            indexed,
            blobs,
            kv,
            shard,
            scheduler,
            config,
            indexed_retry,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.indexed.supports_release()
            && self.config.blob_storage_layers > 0
            && self
                .scheduler
                .upgrade()
                .is_some_and(|scheduler| scheduler.is_durable())
    }

    pub(crate) fn can_seal_mode(&self, mode: AgentMode) -> bool {
        self.enabled()
            && (mode == AgentMode::Durable
                || self.config.indexed_storage_layers.saturating_sub(1)
                    + self.config.blob_storage_layers
                    > 1)
    }

    pub(crate) fn deadline(&self, id: &OwnedAgentId, epoch: ShardEpoch) -> Option<SystemTime> {
        self.shard
            .lease_valid_until(&id.agent_id, epoch, self.config.skew_margin)
    }

    pub(crate) async fn admit_stage(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        id: &OwnedAgentId,
        stage: Uuid,
    ) -> Result<i64, CreateStagedError> {
        lifecycle.assert_agent(&id.agent_id);
        let epoch = self.target_epoch(id)?;
        let token = self
            .indexed
            .reserve_stage("oplog_seal", "reserve_stage", id, stage, Some(epoch))
            .await
            .map_err(|error| Self::indexed_error(id, error))?
            .ok_or(CreateStagedError::TargetPresent)?;
        let refusal = match self.record(id).await {
            Ok(None) => return Ok(token),
            Ok(Some(_)) => CreateStagedError::TargetSealed,
            Err(error) => CreateStagedError::Retryable(error.to_string()),
        };
        // Claim removal stays inside the lifecycle critical section. Cancellation leaves a
        // bare fence and possibly a reservation; it must never start cleanup after releasing it.
        if self
            .indexed
            .remove_stage_reservation("oplog_seal", "refuse_stage", id, Some(token))
            .await
            .map_err(|error| CreateStagedError::Retryable(error.to_string()))?
        {
            self.remove_claim(id, epoch)
                .await
                .map_err(|error| CreateStagedError::Retryable(error.to_string()))?;
        }
        Err(refusal)
    }

    fn target_epoch(&self, id: &OwnedAgentId) -> Result<ShardEpoch, CreateStagedError> {
        let assignment = self.shard.current_assignment()?;
        let shard =
            golem_common::model::ShardId::from_agent_id(&id.agent_id, assignment.number_of_shards);
        Ok(assignment
            .epoch_of(&shard)
            .ok_or(WorkerExecutorError::ShardingNotReady)?)
    }

    pub(crate) async fn check_target(&self, id: &OwnedAgentId) -> Result<(), CreateStagedError> {
        let epoch = self.target_epoch(id)?;
        self.claim(id, epoch).await?;
        if self
            .record(id)
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
            .is_some()
        {
            self.remove_claim(id, epoch).await?;
            return Err(CreateStagedError::TargetSealed);
        }
        Ok(())
    }

    pub(crate) async fn record(
        &self,
        id: &OwnedAgentId,
    ) -> Result<Option<Box<SealRecord>>, OplogArchiveError> {
        for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
            if let Some(record) = self.record_in_mode(id, mode).await? {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }

    pub(crate) async fn record_in_mode(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
    ) -> Result<Option<Box<SealRecord>>, OplogArchiveError> {
        let records = list_records(self.blobs.as_ref(), id, mode, "read_record").await?;
        match valid_record(&records) {
            Some(head) if matches!(head.key.kind, SealRecordKind::Sealed { .. }) => {
                match read_record(self.blobs.as_ref(), id, mode, head).await? {
                    SealRecordBody::Sealed(record) => Ok(Some(record)),
                    SealRecordBody::Deleted(_) => unreachable!(),
                }
            }
            _ => Ok(None),
        }
    }

    fn pinned_fingerprint(
        identity: Option<AgentFingerprint>,
        observation: &AgentDataObservation,
    ) -> Result<Option<AgentFingerprint>, OplogArchiveError> {
        if identity.is_some() {
            return Ok(identity);
        }
        if let (Some(durable), Some(ephemeral)) =
            (observation.durable_marker, observation.ephemeral_marker)
            && durable.fingerprint != ephemeral.fingerprint
        {
            return Err(OplogArchiveError::Corruption(
                "Indexed seal markers disagree on the agent incarnation".into(),
            ));
        }
        Ok(observation
            .durable_marker
            .or(observation.ephemeral_marker)
            .map(|marker| marker.fingerprint))
    }

    async fn record_for_fingerprint(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        records: &[ListedSealRecord],
    ) -> Result<Option<Box<SealRecord>>, OplogArchiveError> {
        let Some(listed) = newest_seal_for_fingerprint(records, fingerprint) else {
            return Ok(None);
        };
        match read_record(self.blobs.as_ref(), id, mode, listed).await? {
            SealRecordBody::Sealed(record) => Ok(Some(record)),
            SealRecordBody::Deleted(_) => unreachable!(),
        }
    }

    async fn record_for_moved_data(
        &self,
        id: &OwnedAgentId,
        mode: Option<AgentMode>,
    ) -> Result<Option<Box<SealRecord>>, WorkerExecutorError> {
        self.read_with_authority(
            id,
            mode,
            || async {
                let identity = self.indexed_identity(id).await?;
                let observation = self
                    .observe(id)
                    .await
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                let Some(fingerprint) =
                    Self::pinned_fingerprint(identity, &observation).map_err(|error| {
                        WorkerExecutorError::oplog_failed(id.agent_id, error.to_string())
                    })?
                else {
                    return Ok(None);
                };
                for candidate_mode in [AgentMode::Durable, AgentMode::Ephemeral]
                    .into_iter()
                    .filter(|candidate| mode.is_none_or(|mode| mode == *candidate))
                {
                    let records =
                        list_records(self.blobs.as_ref(), id, candidate_mode, "moved_data")
                            .await
                            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                    if let Some(record) = self
                        .record_for_fingerprint(id, candidate_mode, fingerprint, &records)
                        .await
                        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
                    {
                        return Ok(Some(record));
                    }
                }
                Ok(None)
            },
            |record| async move { Ok(Some(record)) },
            || Ok(None),
            |error| WorkerExecutorError::runtime(error.to_string()),
        )
        .await
    }

    pub(crate) async fn state(
        &self,
        id: &OwnedAgentId,
    ) -> Result<AgentSealState, OplogArchiveError> {
        self.read_with_authority(
            id,
            None,
            || async { Ok(AgentSealState::Present) },
            |record| async move { Ok(AgentSealState::Sealed(record)) },
            || Ok(AgentSealState::Absent),
            std::convert::identity,
        )
        .await
    }

    async fn observe(&self, id: &OwnedAgentId) -> Result<AgentDataObservation, OplogArchiveError> {
        self.indexed
            .observe_agent("oplog_seal", "observe", id)
            .await
            .map_err(|error| OplogArchiveError::of_indexed(&error, error.to_string()))
    }

    /// Validates a whole resident read against release and same-epoch restoration. An absent
    /// observation also brackets record selection; a record never overrides present indexed data.
    pub(crate) async fn read_with_authority<T, E, R, RF, S, SF, A>(
        &self,
        id: &OwnedAgentId,
        mode: Option<AgentMode>,
        mut present: R,
        mut sealed: S,
        mut absent: A,
        map_error: impl Fn(OplogArchiveError) -> E + Send,
    ) -> Result<T, E>
    where
        T: Send,
        E: Send,
        R: FnMut() -> RF + Send,
        RF: std::future::Future<Output = Result<T, E>> + Send,
        S: FnMut(Box<SealRecord>) -> SF + Send,
        SF: std::future::Future<Output = Result<T, E>> + Send,
        A: FnMut() -> Result<T, E> + Send,
    {
        if !self.enabled() {
            return present().await;
        }
        for _ in 0..self.indexed_retry.max_attempts.max(1) {
            let before = self.observe(id).await.map_err(&map_error)?;
            if before.has_data {
                let result = present().await;
                if self.observe(id).await.map_err(&map_error)? == before {
                    return result;
                }
                continue;
            }
            let record = match mode {
                Some(mode) => self.record_in_mode(id, mode).await,
                None => self.record(id).await,
            };
            if self.observe(id).await.map_err(&map_error)? != before {
                continue;
            }
            let Some(record) = record.map_err(&map_error)? else {
                return absent();
            };
            let nonce = record.writer_nonce;
            let result = sealed(record).await;
            if result.is_err() {
                // Deletion or a new owner can retire the selected record and its chunks while
                // a frozen reader is in flight. A failure belongs to that read only if the
                // same record remains authoritative.
                if self.observe(id).await.map_err(&map_error)? != before {
                    continue;
                }
                let current = match mode {
                    Some(mode) => self.record_in_mode(id, mode).await,
                    None => self.record(id).await,
                };
                if self.observe(id).await.map_err(&map_error)? != before {
                    continue;
                }
                if current
                    .map_err(&map_error)?
                    .is_none_or(|record| record.writer_nonce != nonce)
                {
                    continue;
                }
            }
            return result;
        }
        Err(map_error(OplogArchiveError::Unavailable(format!(
            "Oplog authority changed repeatedly while reading {id}"
        ))))
    }

    pub(crate) async fn pending_promise(
        &self,
        environment_id: EnvironmentId,
        promise_id: &PromiseId,
    ) -> Result<bool, WorkerExecutorError> {
        if !self.enabled() {
            return Ok(false);
        }
        Ok(self
            .record_for_moved_data(
                &OwnedAgentId::new(environment_id, &promise_id.agent_id),
                Some(AgentMode::Ephemeral),
            )
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
            .is_some_and(|record| {
                record.agent_mode == AgentMode::Ephemeral
                    && record.fingerprint == promise_id.fingerprint
                    && record.pending_promises.contains(&promise_id.oplog_idx)
            }))
    }

    pub(crate) async fn completed_promise(
        &self,
        environment_id: EnvironmentId,
        promise_id: &PromiseId,
    ) -> Result<Option<Vec<u8>>, WorkerExecutorError> {
        if !self.enabled() {
            return Ok(None);
        }
        let id = OwnedAgentId::new(environment_id, &promise_id.agent_id);
        let record = self
            .record_for_moved_data(&id, None)
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        Ok(record
            .filter(|record| record.fingerprint == promise_id.fingerprint)
            .and_then(|record| {
                record
                    .completed_promises
                    .get(&promise_id.oplog_idx)
                    .cloned()
            }))
    }

    pub(crate) async fn read_initial(
        &self,
        id: &OwnedAgentId,
        record: &SealRecord,
    ) -> Result<golem_common::model::oplog::OplogEntry, WorkerExecutorError> {
        super::super::sealed_reader::read_sealed(
            self.blobs.as_ref(),
            id,
            record,
            OplogIndex::INITIAL,
            1,
        )
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
        .remove(&OplogIndex::INITIAL)
        .ok_or_else(|| WorkerExecutorError::runtime("Sealed oplog has no initial entry"))
    }

    pub(crate) async fn rejected_snapshots(
        &self,
        id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<BTreeSet<OplogIndex>, WorkerExecutorError> {
        if !self.enabled() {
            return Ok(BTreeSet::new());
        }
        Ok(self
            .record_for_moved_data(id, None)
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
            .filter(|record| record.fingerprint == fingerprint)
            .map_or_else(BTreeSet::new, |record| record.rejected_periodic_snapshots))
    }

    fn indexed_state(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        manifest: &SealManifest,
    ) -> Result<AgentIndexedState, WorkerExecutorError> {
        let manifests = manifest
            .iter()
            .map(|(level, entries)| {
                let rows = entries
                    .iter()
                    .map(|(index, entry)| {
                        Ok((
                            index.as_u64(),
                            Bytes::from(serialize(entry).map_err(WorkerExecutorError::runtime)?),
                        ))
                    })
                    .collect::<Result<Vec<_>, WorkerExecutorError>>()?;
                Ok((*level, Arc::from(rows)))
            })
            .collect::<Result<_, WorkerExecutorError>>()?;
        Ok(AgentIndexedState {
            owned_agent_id: id.clone(),
            agent_mode: mode,
            compressed_layers: self.config.indexed_storage_layers.saturating_sub(1),
            manifests,
        })
    }

    fn presence_namespaces(&self, id: &OwnedAgentId) -> Vec<IndexedStorageNamespace> {
        [AgentMode::Durable, AgentMode::Ephemeral]
            .into_iter()
            .flat_map(|mode| {
                let manifests = (0..self.config.blob_storage_layers)
                    .map(|level| (level, BTreeMap::new()))
                    .collect();
                let state = self
                    .indexed_state(id, mode, &manifests)
                    .expect("empty manifests serialize");
                let mut namespaces = state.epoch_namespaces();
                namespaces.push(state.marker_namespace());
                namespaces
            })
            .collect()
    }

    fn indexed_error(id: &OwnedAgentId, error: IndexedStorageError) -> WorkerExecutorError {
        match error {
            error @ IndexedStorageError::Fenced { .. } => {
                OplogError::Fenced(OplogFence::refused(id.agent_id, error)).into()
            }
            error => WorkerExecutorError::runtime(error.to_string()),
        }
    }

    /// Claims the first archive layer and removes an absent agent's claim in the same transaction.
    /// The caller holds the agent's lifecycle guard across the operation and its record lookup.
    pub(crate) async fn claim_ephemeral(
        &self,
        id: &OwnedAgentId,
        epoch: ShardEpoch,
    ) -> Result<bool, WorkerExecutorError> {
        let namespaces = self.presence_namespaces(id);
        let state = self.indexed_state(id, AgentMode::Ephemeral, &SealManifest::new())?;
        let key = id.storage_key();
        retry_storage_op_fenceable(&self.indexed_retry, "ephemeral_claim", &key, || {
            self.indexed.set_key_epoch(
                "oplog_seal",
                "claim_ephemeral",
                state.fence_namespace(),
                &key,
                epoch,
                &namespaces,
                true,
            )
        })
        .await
        .map_err(|error| Self::indexed_error(id, error))
    }

    /// The claim and presence answer share the backend transaction. A release that wins first
    /// therefore sends this open to unseal instead of letting it recreate an empty primary.
    pub(crate) async fn claim(
        &self,
        id: &OwnedAgentId,
        epoch: ShardEpoch,
    ) -> Result<bool, WorkerExecutorError> {
        let namespaces = self.presence_namespaces(id);
        let key = id.storage_key();
        retry_storage_op_fenceable(&self.indexed_retry, "seal_claim", &key, || {
            self.indexed.set_key_epoch(
                "oplog_seal",
                "claim",
                IndexedStorageNamespace::OpLog {
                    owned_agent_id: id.clone(),
                    agent_mode: AgentMode::Durable,
                },
                &key,
                epoch,
                &namespaces,
                false,
            )
        })
        .await
        .map_err(|error| Self::indexed_error(id, error))
    }

    pub(crate) async fn remove_claim(
        &self,
        id: &OwnedAgentId,
        epoch: ShardEpoch,
    ) -> Result<(), WorkerExecutorError> {
        let key = id.storage_key();
        retry_storage_op_fenceable(&self.indexed_retry, "seal_remove_claim", &key, || {
            self.indexed.remove_key_epoch_if_empty(
                "oplog_seal",
                "remove_claim",
                IndexedStorageNamespace::OpLog {
                    owned_agent_id: id.clone(),
                    agent_mode: AgentMode::Durable,
                },
                &key,
                epoch,
            )
        })
        .await
        .map_err(|error| Self::indexed_error(id, error))?;
        Ok(())
    }

    pub(crate) async fn unseal(
        &self,
        id: &OwnedAgentId,
        epoch: ShardEpoch,
    ) -> Result<(), WorkerExecutorError> {
        let Some(record) = self
            .record_in_mode(id, AgentMode::Durable)
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
        else {
            return Err(WorkerExecutorError::worker_not_found(id.agent_id));
        };
        self.unseal_record(id, epoch, &record, OplogOpenPurpose::Activation)
            .await
    }

    pub(crate) async fn unseal_record(
        &self,
        id: &OwnedAgentId,
        epoch: ShardEpoch,
        record: &SealRecord,
        purpose: OplogOpenPurpose,
    ) -> Result<(), WorkerExecutorError> {
        if record.agent_mode != AgentMode::Durable && purpose != OplogOpenPurpose::Delete {
            return Err(WorkerExecutorError::invalid_request(
                "An ephemeral agent cannot unseal",
            ));
        }
        let deadline = self
            .deadline(id, epoch)
            .ok_or(WorkerExecutorError::ShardingNotReady)?;
        let scheduler = self
            .scheduler
            .upgrade()
            .ok_or(WorkerExecutorError::ShardingNotReady)?;
        scheduler
            .schedule(
                jiff::Timestamp::now() + self.config.archive_interval,
                ScheduledAction::ArchiveOplog {
                    account_id: record.metadata.created_by,
                    owned_agent_id: id.clone(),
                    agent_mode: record.agent_mode,
                    last_oplog_index: record.last_index,
                    next_after: self.config.archive_interval,
                },
            )
            .await;
        let state = self.indexed_state(id, record.agent_mode, &record.manifest)?;
        match retry_storage_op_fenceable(&self.indexed_retry, "unseal", &id.storage_key(), || {
            self.indexed.unseal_agent(
                "oplog_seal",
                "unseal",
                &state,
                epoch,
                record.writer_epoch,
                deadline,
                record.fingerprint,
            )
        })
        .await
        .map_err(|error| Self::indexed_error(id, error))?
        {
            UnsealAgentResult::Unsealed | UnsealAgentResult::AlreadyPresent => Ok(()),
            UnsealAgentResult::Expired => Err(WorkerExecutorError::ShardingNotReady),
        }
    }

    pub(crate) async fn prepare(
        &self,
        metadata: AgentMetadata,
        epoch: ShardEpoch,
    ) -> Result<Option<PendingSeal>, WorkerExecutorError> {
        if !self.can_seal_mode(metadata.agent_mode) {
            return Ok(None);
        }
        let id = metadata.owned_agent_id();
        let Some(deadline) = self.deadline(&id, epoch) else {
            return Ok(None);
        };
        let valid_until_ms = u64::try_from(
            deadline
                .duration_since(UNIX_EPOCH)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
                .as_millis(),
        )
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        let mut manifest = SealManifest::new();
        for level in 0..self.config.blob_storage_layers {
            let rows = self
                .indexed
                .with_entity("oplog_seal", "snapshot_manifest", "manifest")
                .read_raw(
                    IndexedStorageNamespace::BlobOplogManifest {
                        owned_agent_id: id.clone(),
                        agent_mode: metadata.agent_mode,
                        level,
                    },
                    &id.storage_key(),
                    0,
                    MAX_INDEXED_ID,
                )
                .await
                .map_err(|error| Self::indexed_error(&id, error))?;
            let rows = rows
                .into_iter()
                .map(|(index, bytes)| {
                    Ok((
                        OplogIndex::from_u64(index),
                        decode(&bytes)
                            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?,
                    ))
                })
                .collect::<Result<_, WorkerExecutorError>>()?;
            manifest.insert(level, rows);
        }
        let state = self.indexed_state(&id, metadata.agent_mode, &manifest)?;
        for namespace in std::iter::once(state.primary_namespace()).chain(
            (1..=state.compressed_layers).map(|level| IndexedStorageNamespace::CompressedOpLog {
                owned_agent_id: id.clone(),
                agent_mode: metadata.agent_mode,
                level,
            }),
        ) {
            if self
                .indexed
                .with("oplog_seal", "check_empty")
                .length(namespace, &id.storage_key())
                .await
                .map_err(|error| Self::indexed_error(&id, error))?
                != 0
            {
                return Ok(None);
            }
        }
        let identity = self.indexed_identity(&id).await?;
        let observation = self
            .observe(&id)
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        let pinned = Self::pinned_fingerprint(identity, &observation)
            .map_err(|error| WorkerExecutorError::oplog_failed(id.agent_id, error.to_string()))?;
        if pinned.is_some_and(|fingerprint| fingerprint != metadata.fingerprint) {
            return Err(WorkerExecutorError::oplog_failed(
                id.agent_id,
                "Retired worker fingerprint differs from indexed authority",
            ));
        }
        let mut records = list_records(self.blobs.as_ref(), &id, metadata.agent_mode, "prepare")
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        let previous = match pinned {
            Some(fingerprint) => self
                .record_for_fingerprint(&id, metadata.agent_mode, fingerprint, &records)
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?,
            None => None,
        };
        let superseded_marker = match metadata.agent_mode {
            AgentMode::Durable => observation.durable_marker,
            AgentMode::Ephemeral => observation.ephemeral_marker,
        }
        .filter(|marker| identity.is_some_and(|fingerprint| fingerprint != marker.fingerprint));
        let completed_promises = previous
            .as_ref()
            .map_or_else(BTreeMap::new, |record| record.completed_promises.clone());
        let mut record = SealRecord {
            writer_nonce: Uuid::new_v4(),
            writer_epoch: epoch,
            fingerprint: metadata.fingerprint,
            agent_mode: metadata.agent_mode,
            last_index: metadata.last_known_status.oplog_idx,
            metadata,
            manifest,
            completed_promises,
            pending_promises: previous
                .as_ref()
                .map_or_else(BTreeSet::new, |record| record.pending_promises.clone()),
            rejected_periodic_snapshots: previous
                .map_or_else(BTreeSet::new, |record| record.rejected_periodic_snapshots),
        };
        let fields = self
            .kv
            .with_entity("oplog_seal", "snapshot_promises", "promise")
            .get_all_raw(promise_namespace_of(&id, record.fingerprint))
            .await
            .map_err(WorkerExecutorError::runtime)?;
        let keys: BTreeSet<_> = fields.iter().map(|(key, _)| key.as_str()).collect();
        for (key, bytes) in &fields {
            let Some(base) = key.strip_suffix(":completed") else {
                continue;
            };
            let Some(index) = base
                .rsplit(':')
                .next()
                .and_then(|index| index.parse::<u64>().ok())
            else {
                continue;
            };
            if !keys.contains(base)
                && !record
                    .pending_promises
                    .contains(&OplogIndex::from_u64(index))
            {
                continue;
            }
            if let RedisPromiseState::Complete(payload) =
                decode(bytes).map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
            {
                record
                    .completed_promises
                    .entry(OplogIndex::from_u64(index))
                    .or_insert(payload);
            }
        }
        if record.agent_mode == AgentMode::Ephemeral {
            for (key, _) in &fields {
                if key.ends_with(":completed") {
                    continue;
                }
                if let Some(index) = key
                    .rsplit(':')
                    .next()
                    .and_then(|index| index.parse::<u64>().ok())
                {
                    let index = OplogIndex::from_u64(index);
                    if !record.completed_promises.contains_key(&index) {
                        record.pending_promises.insert(index);
                    }
                }
            }
            record
                .pending_promises
                .retain(|index| !record.completed_promises.contains_key(index));
        }
        let rejected_snapshot_copy = self
            .kv
            .with_entity("oplog_seal", "snapshot_rejections", "rejected_snapshots")
            .get_raw(
                KeyValueStorageNamespace::AgentRejectedPeriodicSnapshots {
                    owned_agent_id: id.clone(),
                },
                &record.fingerprint.0.to_string(),
            )
            .await
            .map_err(WorkerExecutorError::runtime)?;
        let rejected = rejected_snapshot_copy
            .as_deref()
            .map(deserialize::<Vec<OplogIndex>>)
            .transpose()
            .map_err(WorkerExecutorError::runtime)?;
        record
            .rejected_periodic_snapshots
            .extend(rejected.into_iter().flatten());
        record.rejected_periodic_snapshots = kept_rejections(
            &record.metadata.last_known_status,
            record.rejected_periodic_snapshots.iter().copied(),
        );
        let mut seq = records
            .iter()
            .filter(|record| record.key.epoch == epoch)
            .map(|record| record.key.seq)
            .max()
            .map(|seq| {
                seq.checked_add(1)
                    .ok_or_else(|| WorkerExecutorError::runtime("Seal sequence exhausted"))
            })
            .transpose()?
            .unwrap_or(0);
        // A competing writer can only add a finite prefix under this lease. Every collision
        // consumes a sequence; an expired lease stops retries rather than extending the deadline.
        loop {
            if SystemTime::now() >= deadline {
                return Ok(None);
            }
            let key = SealRecordKey {
                agent_id: id.agent_id,
                epoch,
                seq,
                kind: SealRecordKind::Sealed { valid_until_ms },
                fingerprint: record.fingerprint,
            };
            let bytes = serialize(&record).map_err(WorkerExecutorError::runtime)?;
            let outcome = self
                .blobs
                .with("oplog_seal", "write_record")
                .put_raw_if_absent_once(
                    seal_namespace(&id, record.agent_mode),
                    &key.path(),
                    &bytes,
                    self.config.seal_put_timeout,
                )
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            records = list_records(self.blobs.as_ref(), &id, record.agent_mode, "verify_record")
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            let Some(listed) = records.iter().find(|listed| listed.key == key) else {
                return Err(WorkerExecutorError::runtime(
                    "Written seal record is not listed",
                ));
            };
            if !listed.admissible() {
                return Ok(None);
            }
            if outcome == PutIfAbsent::AlreadyExists {
                let stored = read_record(self.blobs.as_ref(), &id, record.agent_mode, listed)
                    .await
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                if !matches!(stored, SealRecordBody::Sealed(ref stored) if stored.writer_nonce == record.writer_nonce)
                {
                    seq = records
                        .iter()
                        .filter(|record| record.key.epoch == epoch)
                        .map(|record| record.key.seq)
                        .max()
                        .unwrap_or(seq)
                        .checked_add(1)
                        .ok_or_else(|| WorkerExecutorError::runtime("Seal sequence exhausted"))?;
                    continue;
                }
            }
            retry_storage_op_fenceable(
                &self.indexed_retry,
                "mark_sealed",
                &id.storage_key(),
                || {
                    self.indexed.mark_sealed(
                        "oplog_seal",
                        "mark",
                        &state,
                        epoch,
                        record.fingerprint,
                        superseded_marker,
                    )
                },
            )
            .await
            .map_err(|error| Self::indexed_error(&id, error))?;
            return Ok(Some(PendingSeal {
                record,
                state,
                key,
                listed_records: records,
                rejected_snapshot_copy,
            }));
        }
    }

    pub(crate) async fn remove_moved_copies(
        &self,
        pending: &PendingSeal,
    ) -> Result<(), WorkerExecutorError> {
        let record = &pending.record;
        let id = record.metadata.owned_agent_id();
        let namespace = promise_namespace_of(&id, record.fingerprint);
        for index in record
            .completed_promises
            .keys()
            .chain(record.pending_promises.iter())
        {
            let key = PromiseId {
                agent_id: id.agent_id,
                fingerprint: record.fingerprint,
                oplog_idx: *index,
            }
            .storage_key(id.environment_id());
            self.kv
                .with("oplog_seal", "remove_promise")
                .del(namespace.clone(), &key)
                .await
                .map_err(WorkerExecutorError::runtime)?;
            if record.completed_promises.contains_key(index) {
                self.kv
                    .with("oplog_seal", "remove_promise_result")
                    .del(namespace.clone(), &format!("{key}:completed"))
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
            }
        }
        let fields = self
            .kv
            .with_entity("oplog_seal", "find_orphan_promises", "promise")
            .get_all_raw(namespace.clone())
            .await
            .map_err(WorkerExecutorError::runtime)?;
        let keys: BTreeSet<_> = fields.iter().map(|(key, _)| key.as_str()).collect();
        for (key, _) in &fields {
            if let Some(base) = key.strip_suffix(":completed")
                && !keys.contains(base)
                && !base
                    .rsplit(':')
                    .next()
                    .and_then(|index| index.parse::<u64>().ok())
                    .is_some_and(|index| {
                        record
                            .pending_promises
                            .contains(&OplogIndex::from_u64(index))
                    })
            {
                self.kv
                    .with("oplog_seal", "remove_orphan_promise")
                    .del(namespace.clone(), key)
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
            }
        }
        if let Some(expected) = &pending.rejected_snapshot_copy {
            let field = record.fingerprint.0.to_string();
            // A newer owner may have persisted additional rejections since this snapshot.
            // Only the exact copy now held by the immutable record can be removed.
            self.kv
                .with_entity("oplog_seal", "remove_rejections", "rejected_snapshots")
                .compare_and_set_many_raw(
                    KeyValueStorageNamespace::AgentRejectedPeriodicSnapshots { owned_agent_id: id },
                    &field,
                    Some(expected),
                    &[&field],
                    &[],
                )
                .await
                .map_err(WorkerExecutorError::runtime)?;
        }
        Ok(())
    }

    pub(crate) async fn release(&self, pending: &PendingSeal) -> Result<bool, WorkerExecutorError> {
        let id = pending.record.metadata.owned_agent_id();
        let released = retry_storage_op_fenceable(
            &self.indexed_retry,
            "release_agent",
            &id.storage_key(),
            || {
                self.indexed.release_agent(
                    "oplog_seal",
                    "release",
                    &pending.state,
                    pending.record.writer_epoch,
                )
            },
        )
        .await
        .map_err(|error| Self::indexed_error(&id, error))?
            == ReleaseAgentResult::Released;
        if released {
            for old in pending.listed_records.iter().filter(|listed| {
                matches!(listed.key.kind, SealRecordKind::Sealed { .. }) && listed.key < pending.key
            }) {
                if let Err(error) = self
                    .blobs
                    .with("oplog_seal", "remove_old_record")
                    .delete(
                        seal_namespace(&id, pending.record.agent_mode),
                        &old.entry.path,
                    )
                    .await
                {
                    warn!(agent_id = %id, %error, "Failed to remove an older seal record");
                }
            }
        }
        Ok(released)
    }
}
