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
use golem_common::model::durable_stream::{StreamSessionKeyV1, StreamSessionRecordV1};
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::{DurableStreamSessionStatus, OwnedAgentId};
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Actor-owned answers complete through the raw tip. Eviction affects only performance: a miss
/// reconstructs from the persisted per-session index and the actor's uncommitted buffer.
#[derive(Default)]
pub(super) struct RawSessionCache {
    entries: HashMap<StreamSessionKeyV1, (Option<DurableStreamSessionStatus>, u64)>,
    clock: u64,
    payload_error: Option<String>,
}

impl RawSessionCache {
    pub fn cached(
        &mut self,
        key: &StreamSessionKeyV1,
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
        key: &StreamSessionKeyV1,
    ) -> Result<Option<DurableStreamSessionStatus>, String> {
        let service = service.ok_or_else(|| "stream session index is not installed".to_string())?;
        let mut status = service
            .lookup_persisted(id, mode, committed, &key.idempotency_key)
            .await?
            .filter(|status| status.session_key.as_ref() == Some(key));
        for (offset, entry) in buffer.iter().enumerate() {
            let index = OplogIndex::from_u64(committed.as_u64() + offset as u64 + 1);
            if let OplogEntry::PendingAgentInvocation {
                idempotency_key, ..
            } = entry
            {
                if status
                    .as_ref()
                    .and_then(|status| status.session_key.as_ref())
                    .is_some_and(|session| &session.idempotency_key == idempotency_key)
                {
                    status
                        .as_mut()
                        .unwrap()
                        .apply_pending_invocation(index, idempotency_key);
                }
                continue;
            }
            if let Some(record) = record(entry)?
                && record_key(&record) == Some(key)
            {
                apply(&mut status, index, &record);
            }
        }
        Ok(status)
    }

    pub fn insert(&mut self, key: StreamSessionKeyV1, status: Option<DurableStreamSessionStatus>) {
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
                    .is_some_and(|key| &key.idempotency_key == idempotency_key)
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
        let Some(key) = record_key(&record) else {
            return;
        };
        // An uncached suffix is not a complete answer. The next lookup recovers its prefix.
        if let Some((status, _)) = self.entries.get_mut(key) {
            apply(status, index, &record);
        }
    }
}

fn apply(
    status: &mut Option<DurableStreamSessionStatus>,
    index: OplogIndex,
    record: &StreamSessionRecordV1,
) {
    if status.is_none() && matches!(record, StreamSessionRecordV1::Prepared(_)) {
        *status = Some(DurableStreamSessionStatus::default());
    }
    if let Some(status) = status {
        status.apply_record(index, record);
    }
}

fn record_key(record: &StreamSessionRecordV1) -> Option<&StreamSessionKeyV1> {
    match record {
        StreamSessionRecordV1::Prepared(v) => Some(&v.attempt.session_key),
        StreamSessionRecordV1::Attached(v) => Some(&v.session_key),
        StreamSessionRecordV1::ResumeAttempt(v) => Some(&v.attempt.session_key),
        StreamSessionRecordV1::Detached(v) => Some(&v.session_key),
        StreamSessionRecordV1::InvocationResult(v) => Some(&v.session_key),
        StreamSessionRecordV1::Finished(v) => Some(&v.session_key),
        _ => None,
    }
}

fn record(entry: &OplogEntry) -> Result<Option<Cow<'_, StreamSessionRecordV1>>, String> {
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
