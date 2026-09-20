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

use crate::services::stream_session_index::StreamSessionIndexService;
use golem_common::model::agent::AgentMode;
use golem_common::model::durable_stream::{StreamInvocationId, StreamSessionRecord};
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::{DurableStreamSessionStatus, OwnedAgentId};
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Actor-owned answers complete through the raw tip. Eviction affects only performance: a miss
/// reconstructs from the persisted per-session index and the actor's uncommitted buffer.
#[derive(Default)]
pub(super) struct RawSessionCache {
    entries: HashMap<StreamInvocationId, (Option<DurableStreamSessionStatus>, u64)>,
    clock: u64,
    payload_error: Option<String>,
}

impl RawSessionCache {
    pub fn cached(
        &mut self,
        key: &StreamInvocationId,
    ) -> Result<Option<Option<DurableStreamSessionStatus>>, String> {
        if let Some(error) = &self.payload_error {
            return Err(error.clone());
        }
        self.clock += 1;
        Ok(self.entries.get_mut(key).map(|(status, accessed)| {
            *accessed = self.clock;
            status.clone()
        }))
    }

    pub async fn reconstruct(
        service: Option<&Arc<StreamSessionIndexService>>,
        id: &OwnedAgentId,
        mode: AgentMode,
        committed: OplogIndex,
        buffer: &VecDeque<OplogEntry>,
        key: &StreamInvocationId,
    ) -> Result<Option<DurableStreamSessionStatus>, String> {
        if key.callee_environment_id != id.environment_id || key.callee != id.agent_id {
            return Ok(None);
        }
        let service = service.ok_or_else(|| "stream session index is not installed".to_string())?;
        let fingerprint = match buffer.iter().find_map(|entry| match entry {
            OplogEntry::Create { instance_id, .. } => {
                Some(golem_common::model::AgentFingerprint(*instance_id))
            }
            _ => None,
        }) {
            Some(fingerprint) => fingerprint,
            None => {
                service
                    .lookup_producer_identity(id, mode)
                    .await?
                    .producer_fingerprint
            }
        };
        if key.callee_fingerprint != fingerprint {
            return Ok(None);
        }
        let mut status = service
            .lookup_persisted(id, mode, committed, &key.idempotency_key)
            .await?;
        for (offset, entry) in buffer.iter().enumerate() {
            let index = OplogIndex::from_u64(committed.as_u64() + offset as u64 + 1);
            if let OplogEntry::PendingAgentInvocation {
                idempotency_key, ..
            } = entry
            {
                if status
                    .as_ref()
                    .and_then(|status| status.session_key.as_ref())
                    .is_some_and(|session| session == idempotency_key)
                {
                    status
                        .as_mut()
                        .unwrap()
                        .apply_pending_invocation(index, idempotency_key);
                }
                continue;
            }
            if let Some(record) = record(entry)?
                && (matches!(record.as_ref(), StreamSessionRecord::ForkCut(_))
                    || record
                        .local_session_key()
                        .is_some_and(|idempotency_key| idempotency_key == &key.idempotency_key))
            {
                apply(&mut status, index, &record);
            }
        }
        Ok(status.filter(|status| status.session_key.as_ref() == Some(&key.idempotency_key)))
    }

    pub fn insert(&mut self, key: StreamInvocationId, status: Option<DurableStreamSessionStatus>) {
        self.clock += 1;
        if self.entries.len() == 128 {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, accessed))| accessed)
                .unwrap()
                .0
                .clone();
            self.entries.remove(&oldest);
        }
        self.entries.insert(key, (status, self.clock));
    }

    pub fn apply_entry(&mut self, index: OplogIndex, entry: &OplogEntry) {
        if let OplogEntry::PendingAgentInvocation {
            idempotency_key, ..
        } = entry
        {
            for (status, _) in self.entries.values_mut() {
                if status
                    .as_ref()
                    .and_then(|status| status.session_key.as_ref())
                    .is_some_and(|key| key == idempotency_key)
                {
                    status
                        .as_mut()
                        .unwrap()
                        .apply_pending_invocation(index, idempotency_key);
                }
            }
            return;
        }
        let record = match record(entry) {
            Ok(Some(record)) => record,
            Ok(None) => return,
            Err(error) => {
                self.payload_error = Some(error);
                self.entries.clear();
                return;
            }
        };
        if matches!(record.as_ref(), StreamSessionRecord::ForkCut(_)) {
            self.entries.clear();
            return;
        }
        let Some(key) = record.local_session_key() else {
            return;
        };
        if matches!(record.as_ref(), StreamSessionRecord::Prepared(_)) {
            self.entries
                .retain(|session, (status, _)| status.is_some() || &session.idempotency_key != key);
        }
        // An uncached suffix is not a complete answer. The next lookup recovers its prefix.
        for (_, (status, _)) in self
            .entries
            .iter_mut()
            .filter(|(session, _)| &session.idempotency_key == key)
        {
            apply(status, index, &record);
        }
    }
}

fn apply(
    status: &mut Option<DurableStreamSessionStatus>,
    index: OplogIndex,
    record: &StreamSessionRecord,
) {
    if status.is_none() && matches!(record, StreamSessionRecord::Prepared(_)) {
        *status = Some(DurableStreamSessionStatus::default());
    }
    if let Some(status) = status {
        status.apply_record(index, record);
    }
}

fn record(entry: &OplogEntry) -> Result<Option<Cow<'_, StreamSessionRecord>>, String> {
    let OplogEntry::StreamSession { record, .. } = entry else {
        return Ok(None);
    };
    Ok(Some(match record {
        OplogPayload::Inline(record) => Cow::Borrowed(record.as_ref()),
        OplogPayload::SerializedInline {
            cached: Some(record),
            ..
        }
        | OplogPayload::External {
            cached: Some(record),
            ..
        } => Cow::Borrowed(record.as_ref()),
        OplogPayload::SerializedInline {
            bytes,
            cached: None,
        } => {
            Cow::Owned(golem_common::serialization::try_deserialize(bytes)?.ok_or(
                "stream session record has an unsupported or missing serialization version",
            )?)
        }
        OplogPayload::External { cached: None, .. } => {
            return Err("uncached external stream session record cannot be folded".into());
        }
    }))
}
