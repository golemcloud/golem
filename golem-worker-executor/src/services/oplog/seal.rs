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

use super::OplogArchiveError;
use super::blob::BlobChunkRef;
use desert_rust::BinaryCodec;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentFingerprint, AgentId, AgentMetadata, OwnedAgentId, ShardEpoch};
use golem_common::serialization::decode;
use golem_service_base::storage::blob::{
    BlobPageEntry, BlobStorage, BlobStorageLabelledApi, BlobStorageNamespace, agent_path_segment,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tracing::warn;
use uuid::Uuid;

mod service;
pub(crate) use service::AgentSealState;
pub use service::SealService;

/// A selected authority for one cold construction attempt. Never retained by a live oplog.
pub struct OplogReadAttempt {
    pub(super) id: OwnedAgentId,
    pub(super) mode: Option<AgentMode>,
    pub(super) observation: Option<crate::storage::indexed::AgentDataObservation>,
    pub(super) source: AgentSealState,
    pub(super) moved: Option<Box<SealRecord>>,
    pub(super) frozen_read_failed: std::sync::atomic::AtomicBool,
}

impl OplogReadAttempt {
    pub(crate) fn frozen_read_failed(&self) -> bool {
        self.frozen_read_failed
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub(crate) fn invalidated() -> golem_service_base::error::worker_executor::WorkerExecutorError {
        golem_service_base::error::worker_executor::WorkerExecutorError::recovery_required(
            "oplog read attempt invalidated",
        )
    }

    pub fn is_invalidated_error(
        error: &golem_service_base::error::worker_executor::WorkerExecutorError,
    ) -> bool {
        matches!(error, golem_service_base::error::worker_executor::WorkerExecutorError::RecoveryRequired { details, .. }
            if details == "oplog read attempt invalidated")
    }
}

impl OplogReadAttempt {
    pub async fn get_last_index(
        &self,
        service: &dyn super::OplogService,
        id: &OwnedAgentId,
        mode: AgentMode,
    ) -> Result<OplogIndex, super::UnreadableOplog> {
        self.assert_id(id);
        if self.is_present() {
            service.get_last_index_present(id, mode).await
        } else {
            Ok(self
                .record()
                .filter(|record| record.agent_mode == mode)
                .map_or(OplogIndex::NONE, |record| record.last_index))
        }
    }

    pub async fn read_source(
        &self,
        service: &dyn super::OplogService,
        id: &OwnedAgentId,
        mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, golem_common::model::oplog::OplogEntry>, super::UnreadableOplog>
    {
        self.assert_id(id);
        if self.is_present() {
            return service.read_source_present(id, mode, idx, n).await;
        }
        let Some(record) = self.record().filter(|record| record.agent_mode == mode) else {
            return Ok(BTreeMap::new());
        };
        let seal = service
            .seal_service()
            .expect("selected sealed reader needs seal service");
        let result =
            super::sealed_reader::read_sealed_source(seal.blobs.as_ref(), id, record, idx, n)
                .await
                .map_err(|error| super::UnreadableOplog::new(&id.agent_id, error));
        if result.is_err() {
            self.frozen_read_failed
                .store(true, std::sync::atomic::Ordering::Release);
        }
        result
    }

    pub async fn read_exact(
        &self,
        service: &dyn super::OplogService,
        id: &OwnedAgentId,
        mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, golem_common::model::oplog::OplogEntry>, super::UnreadableOplog>
    {
        if self.is_present() {
            self.assert_id(id);
            return service.read_exact_present(id, mode, idx, n).await;
        }
        let entries = self.read_source(service, id, mode, idx, n).await?;
        let result = super::reader::exact_from_source(
            super::OplogReadSource::Other("selected sealed"),
            idx,
            n,
            entries,
        )
        .map_err(|error| super::UnreadableOplog::new(&id.agent_id, error));
        if result.is_err() {
            self.frozen_read_failed
                .store(true, std::sync::atomic::Ordering::Release);
        }
        result
    }

    pub fn record(&self) -> Option<&SealRecord> {
        match &self.source {
            AgentSealState::Sealed(record) => Some(record),
            _ => None,
        }
    }

    pub fn is_absent(&self) -> bool {
        matches!(self.source, AgentSealState::Absent)
    }

    pub fn is_present(&self) -> bool {
        matches!(self.source, AgentSealState::Present)
    }

    pub fn mode(&self) -> Option<AgentMode> {
        self.mode
    }

    pub(crate) fn assert_id(&self, id: &OwnedAgentId) {
        assert_eq!(&self.id, id);
    }
}

/// The manifest codec is shared with the PostgreSQL-backed blob archive.
pub(crate) type SealManifest = BTreeMap<usize, BTreeMap<OplogIndex, BlobChunkRef>>;

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
pub struct SealRecord {
    pub writer_nonce: Uuid,
    pub writer_epoch: ShardEpoch,
    pub fingerprint: AgentFingerprint,
    pub agent_mode: AgentMode,
    pub(crate) manifest: SealManifest,
    /// Includes the status folded at `last_index`.
    pub metadata: AgentMetadata,
    pub last_index: OplogIndex,
    pub completed_promises: BTreeMap<OplogIndex, Vec<u8>>,
    /// Only ephemeral agents move pending promises out of PostgreSQL.
    pub pending_promises: BTreeSet<OplogIndex>,
    pub rejected_periodic_snapshots: BTreeSet<OplogIndex>,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
pub(crate) struct DeletionRecord {
    pub writer_nonce: Uuid,
    pub writer_epoch: ShardEpoch,
    pub fingerprint: AgentFingerprint,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SealRecordBody {
    Sealed(Box<SealRecord>),
    Deleted(DeletionRecord),
}

/// Variant order deliberately puts deletion above every deadline at the same epoch and seq.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SealRecordKind {
    Sealed { valid_until_ms: u64 },
    Deleted,
}

/// Within an agent, order by epoch, sequence, kind/deadline, then fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SealRecordKey {
    pub agent_id: AgentId,
    pub epoch: ShardEpoch,
    pub seq: u64,
    pub kind: SealRecordKind,
    pub fingerprint: AgentFingerprint,
}

impl SealRecordKey {
    pub fn path(&self) -> PathBuf {
        let name = match self.kind {
            SealRecordKind::Sealed { valid_until_ms } => format!(
                "{:020}-{:020}-{:020}-{}.sealed",
                self.epoch.0, self.seq, valid_until_ms, self.fingerprint.0
            ),
            SealRecordKind::Deleted => format!(
                "{:020}-{:020}-{}.deleted",
                self.epoch.0, self.seq, self.fingerprint.0
            ),
        };
        seal_prefix(&self.agent_id).join(name)
    }

    pub fn parse(path: &Path) -> Option<Self> {
        let mut parts = path.to_str()?.split('/');
        if parts.next()? != "seals" {
            return None;
        }
        let agent = parts.next()?;
        let agent_id: AgentId = agent.parse().ok()?;
        if agent_path_segment(&agent_id) != agent {
            return None;
        }
        let name = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        let (numbers, sealed) = if let Some(numbers) = name.strip_suffix(".sealed") {
            (numbers, true)
        } else {
            (name.strip_suffix(".deleted")?, false)
        };
        let mut numbers = numbers.splitn(if sealed { 4 } else { 3 }, '-');
        let epoch = ShardEpoch(parse_number(numbers.next()?)?);
        let seq = parse_number(numbers.next()?)?;
        let kind = if sealed {
            SealRecordKind::Sealed {
                valid_until_ms: parse_number(numbers.next()?)?,
            }
        } else {
            SealRecordKind::Deleted
        };
        let fingerprint = numbers.next()?;
        let uuid = Uuid::parse_str(fingerprint).ok()?;
        if uuid.to_string() != fingerprint {
            return None;
        }
        Some(Self {
            agent_id,
            epoch,
            seq,
            kind,
            fingerprint: AgentFingerprint(uuid),
        })
    }

    /// GC boundaries and sequence allocation include inadmissible keys and ignore kind and fingerprint.
    pub fn position(&self) -> (ShardEpoch, u64) {
        (self.epoch, self.seq)
    }
}

fn parse_number(number: &str) -> Option<u64> {
    (number.len() == 20 && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| number.parse().ok())
        .flatten()
}

pub(crate) fn seal_prefix(agent_id: &AgentId) -> PathBuf {
    Path::new("seals").join(agent_path_segment(agent_id))
}

pub(crate) fn seal_namespace(
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
) -> BlobStorageNamespace {
    BlobStorageNamespace::SealRecords {
        environment_id: owned_agent_id.environment_id(),
        type_tag: owned_agent_id.agent_id.type_tag(),
        agent_mode,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ListedSealRecord {
    pub key: SealRecordKey,
    pub entry: BlobPageEntry,
}

impl ListedSealRecord {
    pub fn admissible(&self) -> bool {
        let SealRecordKind::Sealed { valid_until_ms } = self.key.kind else {
            return true;
        };
        let timestamp: prost_types::Timestamp = self.entry.last_modified_at.into();
        let Ok(seconds) = u128::try_from(timestamp.seconds) else {
            return false;
        };
        let nanos = seconds * 1_000_000_000 + timestamp.nanos as u128;
        let resolution = self.entry.timestamp_resolution.as_nanos();
        resolution != 0 && nanos + resolution <= u128::from(valid_until_ms) * 1_000_000
    }
}

pub(crate) fn valid_record(records: &[ListedSealRecord]) -> Option<&ListedSealRecord> {
    records
        .iter()
        .filter(|record| record.admissible())
        .max_by_key(|record| record.key)
}

/// Moved data remains owned by the indexed incarnation while PostgreSQL is authoritative.
pub(crate) fn newest_seal_for_fingerprint(
    records: &[ListedSealRecord],
    fingerprint: AgentFingerprint,
) -> Option<&ListedSealRecord> {
    records
        .iter()
        .filter(|record| {
            record.key.fingerprint == fingerprint
                && matches!(record.key.kind, SealRecordKind::Sealed { .. })
                && record.admissible()
        })
        .max_by_key(|record| record.key)
}

/// All well-formed keys, including inadmissible ones, for sequence allocation and GC.
/// Malformed objects are deliberately not returned to callers that delete records.
pub(crate) async fn list_records(
    storage: &dyn BlobStorage,
    owned_agent_id: &OwnedAgentId,
    mode: AgentMode,
    operation: &'static str,
) -> Result<Vec<ListedSealRecord>, OplogArchiveError> {
    let prefix = seal_prefix(&owned_agent_id.agent_id);
    let mut cursor: Option<PathBuf> = None;
    let mut result = Vec::new();
    loop {
        let page = storage
            .with("oplog_seal", operation)
            .list_page(
                seal_namespace(owned_agent_id, mode),
                &prefix,
                cursor.as_deref(),
                1000,
            )
            .await
            .map_err(|error| {
                OplogArchiveError::of_blob(
                    &error,
                    format!("failed to list seal records of {owned_agent_id}: {error}"),
                )
            })?;
        let full = page.len() == 1000;
        if let Some(last) = page.last() {
            cursor = Some(last.path.to_path_buf());
        }
        for entry in page {
            match SealRecordKey::parse(&entry.path) {
                Some(key) if key.agent_id == owned_agent_id.agent_id => {
                    result.push(ListedSealRecord { key, entry })
                }
                _ => {
                    warn!(agent_id = %owned_agent_id, path = %entry.path.display(), "Ignoring malformed seal record key")
                }
            }
        }
        if !full {
            return Ok(result);
        }
    }
}

/// Fetches only the selected head. A corrupt or missing head is an error, not permission to
/// resurrect an older incarnation. Timestamps always come from the listing, never HEAD.
pub(crate) async fn read_record(
    storage: &dyn BlobStorage,
    owned_agent_id: &OwnedAgentId,
    mode: AgentMode,
    listed: &ListedSealRecord,
) -> Result<SealRecordBody, OplogArchiveError> {
    let corrupt = |detail: String| {
        OplogArchiveError::Corruption(format!(
            "invalid seal record {}: {detail}",
            listed.entry.path.display()
        ))
    };
    if listed.key.agent_id != owned_agent_id.agent_id || !listed.admissible() {
        return Err(corrupt("record is not admissible for this agent".into()));
    }
    let bytes = storage
        .with("oplog_seal", "read_record")
        .get_raw(seal_namespace(owned_agent_id, mode), &listed.entry.path)
        .await
        .map_err(|error| {
            OplogArchiveError::of_blob(
                &error,
                format!("failed to read seal record of {owned_agent_id}: {error}"),
            )
        })?
        .ok_or_else(|| corrupt("listed object is missing".into()))?;
    match listed.key.kind {
        SealRecordKind::Deleted => {
            let body: DeletionRecord =
                decode(&bytes).map_err(|error| corrupt(error.to_string()))?;
            if body.writer_epoch != listed.key.epoch || body.fingerprint != listed.key.fingerprint {
                return Err(corrupt("body identity does not match key".into()));
            }
            Ok(SealRecordBody::Deleted(body))
        }
        SealRecordKind::Sealed { .. } => {
            let mut body: SealRecord =
                decode(&bytes).map_err(|error| corrupt(error.to_string()))?;
            if body.writer_epoch != listed.key.epoch
                || body.fingerprint != listed.key.fingerprint
                || body.agent_mode != mode
                || body.metadata.owned_agent_id() != *owned_agent_id
                || body.metadata.fingerprint != body.fingerprint
                || body.metadata.agent_mode != body.agent_mode
                || body.metadata.last_known_status.oplog_idx != body.last_index
                || (body.agent_mode == AgentMode::Durable && !body.pending_promises.is_empty())
                || body
                    .pending_promises
                    .iter()
                    .any(|idx| body.completed_promises.contains_key(idx))
            {
                return Err(corrupt(
                    "body identity or snapshot does not match key".into(),
                ));
            }
            body.metadata.last_known_status.agent_mode = body.agent_mode;
            Ok(SealRecordBody::Sealed(Box::new(body)))
        }
    }
}

#[cfg(test)]
mod tests;
