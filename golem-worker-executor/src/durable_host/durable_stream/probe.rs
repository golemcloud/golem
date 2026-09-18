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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Durable consumer evidence used when reconciling a producer attachment.
pub enum ConsumerAttachmentStatus {
    Prepared,
    Active,
    Deleting,
    Missing,
    IncarnationMismatch,
    EpochMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Consumer terminal and progress facts recovered from its journal.
pub struct ConsumerJournalInspection {
    pub source_offsets: Vec<StreamOffset>,
    pub source_unavailable: Option<StreamOffset>,
}

#[async_trait]
/// Queries consumer-side durable state without relying on its resident session runtime.
pub trait StreamAttachmentConsumerProbe: Send + Sync {
    /// Inspects whether the consumer can still require the attachment's source history.
    async fn status(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError>;

    /// Inspects an attachment while requiring an exact durable mapping when supplied.
    async fn status_exact(
        &self,
        key: &StreamAttachmentKey,
        _mapping: Option<&StreamSessionMappingRecord>,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError> {
        self.status(key).await
    }

    /// Returns committed consumer offsets needed for source-loss reconciliation.
    async fn journal_inspection(
        &self,
        _key: &StreamAttachmentKey,
    ) -> Result<Option<ConsumerJournalInspection>, StreamStoreError> {
        Ok(None)
    }

    /// Returns folded consumer progress without loading journal payloads.
    async fn journal_summary(
        &self,
        _key: &StreamAttachmentKey,
    ) -> Result<Option<ConsumerJournalSummary>, StreamStoreError> {
        Ok(None)
    }

    /// Commits deterministic source loss before producer history is removed.
    async fn commit_source_unavailable(
        &self,
        _key: &StreamAttachmentKey,
        _source_offset: StreamOffset,
        _consumer_read_ordinal: u64,
    ) -> Result<(), StreamStoreError> {
        Err(StreamStoreError::Oplog(
            "consumer probe cannot commit a source-unavailable overlay".to_string(),
        ))
    }
}

/// Consumer probe that reads local metadata or routes to the owning shard.
pub struct DbDirectStreamAttachmentConsumerProbe {
    worker_service: Arc<dyn WorkerService>,
    oplog_service: Arc<dyn OplogService>,
    rpc: Option<Arc<dyn Rpc>>,
}

impl DbDirectStreamAttachmentConsumerProbe {
    /// Creates a probe backed only by the supplied metadata service.
    pub fn new(
        worker_service: Arc<dyn WorkerService>,
        oplog_service: Arc<dyn OplogService>,
    ) -> Self {
        Self {
            worker_service,
            oplog_service,
            rpc: None,
        }
    }

    /// Creates a probe that can route inspection to remote consumer owners.
    pub fn new_routed(
        worker_service: Arc<dyn WorkerService>,
        oplog_service: Arc<dyn OplogService>,
        rpc: Arc<dyn Rpc>,
    ) -> Self {
        Self {
            worker_service,
            oplog_service,
            rpc: Some(rpc),
        }
    }

    /// Returns whether a matching cancellation intent or terminal is durably recorded.
    pub async fn committed_cancellation_status(
        &self,
        key: &StreamAttachmentKey,
        mapping: &StreamSessionMappingRecord,
        intent: &golem_common::model::durable_stream::StreamConsumerCancelIntentRecord,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError> {
        self.inspect_status(key, Some(mapping), Some(intent)).await
    }

    #[tracing::instrument(name = "durable_stream.consumer.status", level = "debug", skip_all)]
    async fn inspect_status(
        &self,
        key: &StreamAttachmentKey,
        expected_mapping: Option<&StreamSessionMappingRecord>,
        expected_cancel: Option<
            &golem_common::model::durable_stream::StreamConsumerCancelIntentRecord,
        >,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError> {
        let session_owner = OwnedAgentId::new(
            key.session_key.callee_environment_id,
            &key.session_key.callee,
        );
        let Some(session_identity) = self
            .worker_service
            .resolve_agent_identity(&session_owner)
            .await
            .map_err(|err| StreamStoreError::Oplog(err.to_string()))?
        else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        if session_identity.fingerprint != key.session_key.callee_fingerprint {
            crate::metrics::workers::record_foreign_stream_fingerprint_mismatch();
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        if session_identity.agent_mode != AgentMode::Durable {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        let session_mode = session_identity.agent_mode;
        let session_index = self.oplog_service.stream_session_index().ok_or_else(|| {
            StreamStoreError::Oplog("stream session index service is unavailable".to_string())
        })?;
        let Some(status) = session_index
            .lookup_latest(
                &session_owner,
                session_mode,
                key.session_key.callee_fingerprint,
                &key.session_key.idempotency_key,
            )
            .await
            .map_err(StreamStoreError::Oplog)?
        else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        if status.session_key.as_ref() != Some(&key.session_key) {
            return Ok(ConsumerAttachmentStatus::Missing);
        }
        if let Some(error) = status.lifecycle_error {
            return Err(StreamStoreError::CorruptHistory(error));
        }
        let Some(prepared_attempt_id) = status.prepared_attempt_id else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        let primary_attachment_id = AttachmentId::primary(
            key.session_key.callee_environment_id,
            &key.session_key.callee,
            &key.session_key.idempotency_key,
        )
        .map_err(|error| StreamStoreError::CorruptHistory(error.to_string()))?;
        if key.attachment_id != primary_attachment_id {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        match (
            status.initial_attachment_epoch,
            status.initial_attachment_attempt_id,
            status.initial_pending_invocation_oplog_index,
        ) {
            (None, None, None) => {}
            (Some(_), Some(initial_attempt_id), Some(pending_index)) => {
                if initial_attempt_id != prepared_attempt_id
                    || status.validated_initial_pending_invocation != Some(pending_index)
                {
                    return Err(StreamStoreError::CorruptHistory(
                        "durable Attached record does not identify its Prepared attempt and pending invocation"
                            .to_string(),
                    ));
                }
            }
            _ => {
                return Err(StreamStoreError::CorruptHistory(
                    "durable attachment lifecycle index is incomplete".to_string(),
                ));
            }
        }
        let attachment_authority = status
            .attachment_epoch
            .zip(status.attachment_attempt_id)
            .zip(status.attachment_attached)
            .map(|((epoch, attempt_id), attached)| (epoch, attempt_id, attached));
        if expected_cancel.is_none()
            && attachment_authority.is_some_and(|(epoch, _, _)| epoch != key.epoch)
        {
            return Ok(ConsumerAttachmentStatus::EpochMismatch);
        }

        let consumer = OwnedAgentId::new(key.consumer_environment_id, &key.consumer);
        let Some(consumer_identity) = self
            .worker_service
            .resolve_agent_identity(&consumer)
            .await
            .map_err(|err| StreamStoreError::Oplog(err.to_string()))?
        else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        if key.consumer_invocation.callee_environment_id != key.consumer_environment_id
            || key.consumer_invocation.callee != key.consumer
            || key.consumer_invocation.callee_fingerprint != key.expected_consumer_fingerprint
        {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        if consumer_identity.fingerprint != key.expected_consumer_fingerprint {
            crate::metrics::workers::record_foreign_stream_fingerprint_mismatch();
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        if consumer_identity.agent_mode != AgentMode::Durable {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        let agent_mode = consumer_identity.agent_mode;
        let metadata = self
            .worker_service
            .lookup_durable_stream_control_metadata(
                &consumer,
                agent_mode,
                key.expected_consumer_fingerprint,
                &key.session_key,
            )
            .await
            .map_err(StreamStoreError::Oplog)?;
        if !metadata.is_loaded() {
            return Ok(ConsumerAttachmentStatus::Missing);
        }
        if let Some(intent) = expected_cancel {
            let mapping = expected_mapping.expect("cancellation inspection requires a mapping");
            return metadata
                .has_committed_cancellation(key, mapping, intent)
                .map(|matches| {
                    if matches {
                        ConsumerAttachmentStatus::Active
                    } else {
                        ConsumerAttachmentStatus::Missing
                    }
                })
                .map_err(StreamStoreError::CorruptHistory);
        }
        let topology = metadata
            .topology_status(key, expected_mapping)
            .map_err(StreamStoreError::CorruptHistory)?;
        if matches!(
            topology,
            ConsumerAttachmentStatus::EpochMismatch | ConsumerAttachmentStatus::IncarnationMismatch
        ) {
            return Ok(topology);
        }
        if metadata.consumer_deleting().is_some_and(|record| {
            record.consumer_environment_id == key.consumer_environment_id
                && record.consumer == key.consumer
                && record.consumer_fingerprint == key.expected_consumer_fingerprint
        }) {
            return Ok(ConsumerAttachmentStatus::Deleting);
        }
        match (attachment_authority, topology) {
            (Some(_), topology) => Ok(topology),
            (None, ConsumerAttachmentStatus::Prepared) => Ok(ConsumerAttachmentStatus::Prepared),
            (None, ConsumerAttachmentStatus::Active) => Err(StreamStoreError::CorruptHistory(
                "durable topology activation precedes session attachment".to_string(),
            )),
            (None, topology) => Ok(topology),
        }
    }
}

#[async_trait]
impl StreamAttachmentConsumerProbe for DbDirectStreamAttachmentConsumerProbe {
    async fn status(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError> {
        self.status_exact(key, None).await
    }

    async fn status_exact(
        &self,
        key: &StreamAttachmentKey,
        expected_mapping: Option<&StreamSessionMappingRecord>,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError> {
        self.inspect_status(key, expected_mapping, None).await
    }

    #[tracing::instrument(
        name = "durable_stream.consumer.journal_inspection",
        level = "debug",
        skip_all
    )]
    async fn journal_inspection(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<Option<ConsumerJournalInspection>, StreamStoreError> {
        let consumer = OwnedAgentId::new(key.consumer_environment_id, &key.consumer);
        let Some(metadata) = self
            .worker_service
            .get(&consumer)
            .await
            .map_err(|err| StreamStoreError::Oplog(err.to_string()))?
        else {
            return Ok(None);
        };
        if metadata.initial_worker_metadata.fingerprint != key.expected_consumer_fingerprint {
            crate::metrics::workers::record_foreign_stream_fingerprint_mismatch();
            return Ok(None);
        }
        if metadata.initial_worker_metadata.agent_mode != AgentMode::Durable {
            return Ok(None);
        }
        let current = self
            .oplog_service
            .get_last_index(&consumer, AgentMode::Durable)
            .await;
        if !current.is_defined() {
            return Ok(None);
        }
        let mut offsets = Vec::new();
        let mut overlay = None;
        for (_, entry) in self
            .oplog_service
            .read_exact(
                &consumer,
                AgentMode::Durable,
                OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self
                .oplog_service
                .download_payload(&consumer, AgentMode::Durable, record)
                .await
                .map_err(StreamStoreError::Oplog)?;
            match record {
                StreamSessionRecord::ConsumerItemValue(record)
                    if record.session_key == key.session_key
                        && record.stream_id == key.stream_id =>
                {
                    if record.consumer_read_ordinal != offsets.len() as u64 {
                        return Err(StreamStoreError::CorruptHistory(
                            "consumer value journal contains a read-ordinal gap".to_string(),
                        ));
                    }
                    for index in 0..record.logical_item_count() {
                        offsets.push(record.source_offset_at(index).ok_or_else(|| {
                            StreamStoreError::CorruptHistory(
                                "packed-u8 consumer journal offset range is invalid".to_string(),
                            )
                        })?);
                    }
                }
                StreamSessionRecord::ConsumerTerminal(record)
                    if record.session_key == key.session_key
                        && record.stream_id == key.stream_id =>
                {
                    if record.consumer_read_ordinal != offsets.len() as u64 {
                        return Err(StreamStoreError::CorruptHistory(
                            "consumer terminal journal contains a read-ordinal gap".to_string(),
                        ));
                    }
                    offsets.push(record.source_offset);
                }
                StreamSessionRecord::SourceUnavailable(record)
                    if record.key.session_key == key.session_key
                        && record.key.stream_id == key.stream_id =>
                {
                    if record.consumer_read_ordinal != offsets.len() as u64 {
                        return Err(StreamStoreError::CorruptHistory(
                            "source-unavailable overlay contains a read-ordinal gap".to_string(),
                        ));
                    }
                    match overlay {
                        Some(existing) if existing != record.source_offset => {
                            return Err(StreamStoreError::CorruptHistory(
                                "conflicting source-unavailable overlays".to_string(),
                            ));
                        }
                        Some(_) => {}
                        None => overlay = Some(record.source_offset),
                    }
                }
                _ => {}
            }
        }
        Ok(Some(ConsumerJournalInspection {
            source_offsets: offsets,
            source_unavailable: overlay,
        }))
    }

    async fn journal_summary(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<Option<ConsumerJournalSummary>, StreamStoreError> {
        let consumer = OwnedAgentId::new(key.consumer_environment_id, &key.consumer);
        let Some(metadata) = self
            .worker_service
            .get(&consumer)
            .await
            .map_err(|err| StreamStoreError::Oplog(err.to_string()))?
        else {
            return Ok(None);
        };
        if metadata.initial_worker_metadata.fingerprint != key.expected_consumer_fingerprint {
            crate::metrics::workers::record_foreign_stream_fingerprint_mismatch();
            return Ok(None);
        }
        if metadata.initial_worker_metadata.agent_mode != AgentMode::Durable {
            return Ok(None);
        }
        let (_, mut rows) = self
            .worker_service
            .lookup_durable_stream_producer_metadata(
                &consumer,
                AgentMode::Durable,
                key.expected_consumer_fingerprint,
                vec![ProducerMetadataKey::ConsumerHead(
                    key.session_key.clone(),
                    key.stream_id,
                )],
            )
            .await
            .map_err(StreamStoreError::Oplog)?;
        let Some(ProducerMetadataRow::ConsumerHead(head)) = rows.pop().flatten() else {
            return Ok(None);
        };
        Ok(Some(ConsumerJournalSummary {
            event_count: head.next_read_ordinal,
            last_offset: head.last_source_offset,
            terminal: head.terminal,
            source_unavailable: head.source_unavailable.is_some(),
        }))
    }

    async fn commit_source_unavailable(
        &self,
        key: &StreamAttachmentKey,
        source_offset: StreamOffset,
        consumer_read_ordinal: u64,
    ) -> Result<(), StreamStoreError> {
        let rpc = self.rpc.as_ref().ok_or_else(|| {
            StreamStoreError::Oplog(
                "consumer probe has no route for a source-unavailable overlay".to_string(),
            )
        })?;
        rpc.control_durable_stream_attachment(
            StreamAttachmentControlRequest {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                mapping: None,
                operation: StreamAttachmentControlOperation::SourceUnavailable {
                    key: key.clone(),
                    source_offset,
                    consumer_read_ordinal,
                },
            },
            &AuthCtx::System,
        )
        .await
        .map_err(|error| StreamStoreError::Oplog(error.to_string()))?;
        Ok(())
    }
}
