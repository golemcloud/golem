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
use crate::services::oplog::primary::read_persisted_oplog_entries;
use crate::services::oplog::{
    BlobOplogArchiveService, CompressedOplogArchiveService, OplogArchiveService,
};
use golem_common::model::oplog::OplogEntry;
use golem_service_base::storage::blob::BlobStorageNamespace;
use std::time::Duration;

fn maintenance(error: impl std::fmt::Display) -> OplogError {
    OplogError::Maintenance(error.to_string())
}

impl SealService {
    fn gc_delay(&self) -> Duration {
        self.config.shard_lease_max_ttl
            + self.config.seal_put_timeout
            + self.config.server_commit_bound
            + self.config.skew_margin * 2
    }

    async fn schedule_collection(
        &self,
        id: &OwnedAgentId,
        time: SystemTime,
    ) -> Result<(), OplogError> {
        let scheduler = self
            .scheduler
            .upgrade()
            .ok_or_else(|| maintenance("Scheduler unavailable"))?;
        if !scheduler.is_durable() {
            return Err(maintenance(
                "Tombstone collection requires durable scheduler storage",
            ));
        }
        scheduler
            .schedule(
                jiff::Timestamp::try_from(time).map_err(maintenance)?,
                ScheduledAction::CollectTombstone {
                    owned_agent_id: id.clone(),
                },
            )
            .await;
        Ok(())
    }

    async fn deletion_records(
        &self,
        id: &OwnedAgentId,
        epoch: ShardEpoch,
    ) -> Result<Vec<ListedSealRecord>, OplogError> {
        let records = list_records(self.blobs.as_ref(), id)
            .await
            .map_err(maintenance)?;
        if let Some(actual) = records.iter().map(|record| record.key.epoch).max()
            && actual > epoch
        {
            return Err(OplogError::Fenced(OplogFence {
                agent_id: id.agent_id,
                expected_epoch: epoch,
                actual_epoch: Some(actual),
            }));
        }
        Ok(records)
    }

    /// Runs before deletion's early ownership assertion, which can itself write an epoch row.
    pub(crate) async fn prepare_deletion(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        id: &OwnedAgentId,
        epoch: ShardEpoch,
    ) -> Result<(), OplogError> {
        lifecycle.assert_agent(&id.agent_id);
        if self.enabled() {
            self.deletion_records(id, epoch).await?;
            self.schedule_collection(id, SystemTime::now() + self.gc_delay())
                .await?;
        }
        Ok(())
    }

    /// Returns whether deletion finished and the epoch for any ordinary indexed-only deletion.
    pub(crate) async fn delete_with_tombstone(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(bool, Option<ShardEpoch>), OplogError> {
        lifecycle.assert_agent(&id.agent_id);
        if !self.enabled() {
            return Ok((false, expected_epoch));
        }
        let epoch = match expected_epoch {
            Some(epoch) => epoch,
            None => self.target_epoch(id).map_err(maintenance)?,
        };
        let mut records = self.deletion_records(id, epoch).await?;
        let state = self
            .indexed_state(id, mode, &SealManifest::new())
            .map_err(maintenance)?;
        let key = id.storage_key();
        let present = self.observe(id).await.map_err(maintenance)?.has_data;
        if !present
            && !records.is_empty()
            && valid_record(&records).is_none_or(|head| head.key.fingerprint != fingerprint)
        {
            return Ok((true, expected_epoch));
        }
        let mut needs_tombstone = !present || !records.is_empty();
        if !needs_tombstone {
            for namespace in std::iter::once(state.marker_namespace()).chain(
                (0..self.config.blob_storage_layers).map(|level| state.manifest_namespace(level)),
            ) {
                if self
                    .indexed
                    .exists("oplog_seal", "deletion_presence", namespace, &key)
                    .await
                    .map_err(maintenance)?
                {
                    needs_tombstone = true;
                    break;
                }
            }
        }
        // Schedule before the first fence mutation. A lost PUT reply or process crash must
        // never leave a tombstone whose collection cannot be rediscovered after a restart.
        if needs_tombstone {
            self.schedule_collection(id, SystemTime::now() + self.gc_delay())
                .await?;
        }
        if !present || expected_epoch.is_none() {
            self.indexed
                .set_key_epoch(
                    "oplog_seal",
                    "delete_claim",
                    state.fence_namespace(),
                    &key,
                    epoch,
                    &self.presence_namespaces(id),
                    false,
                )
                .await
                .map_err(|error| match error {
                    error @ IndexedStorageError::Fenced { .. } => {
                        OplogError::Fenced(OplogFence::refused(id.agent_id, error))
                    }
                    error => maintenance(error),
                })?;
        }
        if !needs_tombstone {
            return Ok((false, Some(epoch)));
        }
        let body = DeletionRecord {
            writer_nonce: Uuid::new_v4(),
            writer_epoch: epoch,
            fingerprint,
        };
        let tombstone = loop {
            let seq = records
                .iter()
                .map(|record| record.key.seq)
                .max()
                .map(|seq| {
                    seq.checked_add(1)
                        .ok_or_else(|| maintenance("Deletion sequence exhausted"))
                })
                .transpose()?
                .unwrap_or(0);
            let tombstone = SealRecordKey {
                agent_id: id.agent_id,
                epoch,
                seq,
                kind: SealRecordKind::Deleted,
                fingerprint,
            };
            let bytes = serialize(&body).map_err(maintenance)?;
            let outcome = self
                .blobs
                .put_raw_if_absent(
                    "oplog_seal",
                    "write_tombstone",
                    seal_namespace(id),
                    &tombstone.path(),
                    &bytes,
                )
                .await
                .map_err(maintenance)?;
            records = self.deletion_records(id, epoch).await?;
            let listed = records
                .iter()
                .find(|record| record.key == tombstone)
                .ok_or_else(|| maintenance("Written tombstone is not listed"))?;
            if outcome == PutIfAbsent::Written
                || matches!(
                    read_record(self.blobs.as_ref(), id, listed).await.map_err(maintenance)?,
                    SealRecordBody::Deleted(ref stored) if stored.writer_nonce == body.writer_nonce
                )
            {
                break tombstone;
            }
        };

        self.indexed
            .delete_with_epoch(
                "oplog_seal",
                "delete_fenced",
                state.fence_namespace(),
                &key,
                Some(epoch),
                true,
            )
            .await
            .map_err(|error| match error {
                error @ IndexedStorageError::Fenced { .. } => {
                    OplogError::Fenced(OplogFence::refused(id.agent_id, error))
                }
                error => maintenance(error),
            })?;
        for namespace in self.presence_namespaces(id) {
            if namespace != state.fence_namespace() {
                self.indexed
                    .delete_with_epoch("oplog_seal", "delete_layer", namespace, &key, None, false)
                    .await
                    .map_err(maintenance)?;
            }
        }
        self.delete_incarnation_chunks(id, fingerprint).await?;
        for record in records.iter().filter(|record| {
            matches!(record.key.kind, SealRecordKind::Sealed { .. })
                && record.key.position() < tombstone.position()
        }) {
            self.blobs
                .with("oplog_seal", "delete_old_record")
                .delete(seal_namespace(id), &record.entry.path)
                .await
                .map_err(maintenance)?;
        }
        Ok((true, Some(epoch)))
    }

    async fn delete_incarnation_chunks(
        &self,
        id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), OplogError> {
        let path = Path::new("chunks")
            .join(agent_path_segment(&id.agent_id))
            .join(fingerprint.to_string());
        for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
            for level in 0..self.config.blob_storage_layers {
                self.blobs
                    .with("oplog_seal", "delete_incarnation_chunks")
                    .delete_dir(
                        BlobStorageNamespace::CompressedOplog {
                            environment_id: id.environment_id(),
                            type_tag: id.agent_id.type_tag(),
                            agent_mode: mode,
                            level,
                        },
                        &path,
                    )
                    .await
                    .map_err(maintenance)?;
            }
        }
        Ok(())
    }

    /// Reads only indexed authority, including its blob manifests, never a seal snapshot.
    pub(super) async fn indexed_identity(
        &self,
        id: &OwnedAgentId,
    ) -> Result<Option<AgentFingerprint>, OplogError> {
        for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
            let mut entry = read_persisted_oplog_entries(
                self.indexed.clone(),
                IndexedStorageNamespace::OpLog {
                    owned_agent_id: id.clone(),
                    agent_mode: mode,
                },
                id.storage_key(),
                1,
                1,
            )
            .await
            .map_err(maintenance)?
            .pop()
            .map(|(_, entry)| entry);
            for level in 1..self.config.indexed_storage_layers {
                if entry.is_some() {
                    break;
                }
                entry = CompressedOplogArchiveService::new(
                    self.indexed.clone(),
                    level,
                    self.indexed_retry.clone(),
                )
                .read_source(id, mode, OplogIndex::INITIAL, 1)
                .await
                .map_err(maintenance)?
                .remove(&OplogIndex::INITIAL);
            }
            for level in 0..self.config.blob_storage_layers {
                if entry.is_some() {
                    break;
                }
                entry = BlobOplogArchiveService::new(
                    self.blobs.clone(),
                    self.indexed.clone(),
                    level,
                    self.indexed_retry.clone(),
                )
                .read_source(id, mode, OplogIndex::INITIAL, 1)
                .await
                .map_err(maintenance)?
                .remove(&OplogIndex::INITIAL);
            }
            match entry {
                Some(OplogEntry::Create { parameters, .. }) if parameters.agent_mode == mode => {
                    return Ok(Some(AgentFingerprint(parameters.instance_id)));
                }
                _ => {}
            }
        }
        Ok(None)
    }

    pub(crate) async fn collect_tombstone(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        id: &OwnedAgentId,
    ) -> Result<(), OplogError> {
        lifecycle.assert_agent(&id.agent_id);
        let mut tombstones = list_records(self.blobs.as_ref(), id)
            .await
            .map_err(maintenance)?;
        tombstones.retain(|record| record.key.kind == SealRecordKind::Deleted);
        tombstones.sort_by_key(|record| record.key);
        let mut previous = None;
        for tombstone in tombstones {
            let timestamp: prost_types::Timestamp = tombstone.entry.last_modified_at.into();
            let modified = UNIX_EPOCH
                + Duration::from_secs(u64::try_from(timestamp.seconds).map_err(maintenance)?)
                + Duration::from_nanos(u64::try_from(timestamp.nanos).map_err(maintenance)?);
            let deadline = modified + self.gc_delay();
            if SystemTime::now() < deadline {
                return self.schedule_collection(id, deadline).await;
            }
            // Identity must precede the fresh listing: if release races this read, its
            // replacement admissible record must already be visible in that listing.
            let identity = self.indexed_identity(id).await?;
            let observation = self.observe(id).await.map_err(maintenance)?;
            let pinned = Self::pinned_fingerprint(identity, &observation).map_err(maintenance)?;
            let records = list_records(self.blobs.as_ref(), id)
                .await
                .map_err(maintenance)?;
            if !records.iter().any(|record| record.key == tombstone.key) {
                previous = Some(tombstone.key.position());
                continue;
            }
            let fingerprint = tombstone.key.fingerprint;
            let unidentified = observation.has_data && pinned.is_none();
            if unidentified {
                crate::metrics::oplog::record_archive_maintenance_failure(
                    "gc_identity_unavailable",
                );
            }
            let lost = unidentified
                || pinned == Some(fingerprint)
                || records.iter().any(|record| {
                    matches!(record.key.kind, SealRecordKind::Sealed { .. })
                        && record.key.fingerprint == fingerprint
                        && record.key > tombstone.key
                        && record.admissible()
                });
            if !lost {
                for record in records.iter().filter(|record| {
                    matches!(record.key.kind, SealRecordKind::Sealed { .. })
                        && previous.is_none_or(|lower| record.key.position() > lower)
                        && record.key.position() <= tombstone.key.position()
                }) {
                    self.blobs
                        .with("oplog_seal", "gc_record")
                        .delete(seal_namespace(id), &record.entry.path)
                        .await
                        .map_err(maintenance)?;
                }
                self.delete_incarnation_chunks(id, fingerprint).await?;
                // A crash here must leave the tombstone as the exact-epoch recovery anchor.
                for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
                    let state = self
                        .indexed_state(id, mode, &SealManifest::new())
                        .map_err(maintenance)?;
                    self.indexed
                        .remove_key_epoch_if_empty(
                            "oplog_seal",
                            "gc_fence",
                            state.fence_namespace(),
                            &id.storage_key(),
                            tombstone.key.epoch,
                        )
                        .await
                        .map_err(maintenance)?;
                }
            }
            self.blobs
                .with("oplog_seal", "gc_tombstone")
                .delete(seal_namespace(id), &tombstone.entry.path)
                .await
                .map_err(maintenance)?;
            previous = Some(tombstone.key.position());
        }
        Ok(())
    }
}
