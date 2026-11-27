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
use crate::services::oplog::{OplogReadError, OplogReadSource};

/// The stream error of a failed read of the metadata or the identity of `agent_id`. A read that
/// failed for a time ([`InvocationObservationFailure::stored_read`]), also in the reason of a
/// failed resume, stays unreadable until a later read passes. Any other error, also a read that
/// failed for good or a recovery for another cause, is the text of the error.
fn agent_read_error(agent_id: &AgentId, error: WorkerExecutorError) -> StreamStoreError {
    use crate::worker::InvocationObservationFailure;
    match InvocationObservationFailure::stored_read(&error) {
        Some((_, WorkerExecutorError::RecoveryRequired { details, .. })) => {
            StreamStoreError::Unreadable(UnreadableOplog::new(
                agent_id,
                OplogReadError::source_unavailable(
                    OplogReadSource::Other("agent identity"),
                    details.clone(),
                ),
            ))
        }
        _ => StreamStoreError::Oplog(error.to_string()),
    }
}

/// Reads the stream session record of the entry at `index` in the oplog of `consumer`. A record
/// that does not read for a time is an unreadable oplog of `consumer` at `index`. A record that
/// does not read for good is the text of its read error, which names `consumer` and `index`.
async fn consumer_record(
    oplog_service: &dyn OplogService,
    consumer: &OwnedAgentId,
    agent_mode: AgentMode,
    index: OplogIndex,
    record: OplogPayload<StreamSessionRecord>,
) -> Result<StreamSessionRecord, StreamStoreError> {
    oplog_service
        .download_payload(consumer, agent_mode, record)
        .await
        .map_err(|error| {
            let unreadable = UnreadableOplog::of_payload(
                &consumer.agent_id,
                "consumer journal record",
                index,
                error,
            );
            if unreadable.is_transient() {
                StreamStoreError::Unreadable(unreadable)
            } else {
                StreamStoreError::Oplog(unreadable.to_string())
            }
        })
}

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
    pub reader_id: LocalStreamReaderId,
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
    ) -> Result<Option<Vec<ConsumerJournalInspection>>, StreamStoreError> {
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
        _reader_id: LocalStreamReaderId,
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
            .map_err(|error| agent_read_error(&session_owner.agent_id, error))?
        else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        if session_identity.fingerprint != key.session_key.callee_fingerprint {
            crate::metrics::workers::record_foreign_stream_fingerprint_mismatch();
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
            .map_err(StreamStoreError::from)?
        else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        if status.session_key.as_ref() != Some(&key.session_key.idempotency_key) {
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
            && key.consumer_environment_id == key.session_key.callee_environment_id
            && key.consumer == key.session_key.callee
            && key.expected_consumer_fingerprint == key.session_key.callee_fingerprint
            && attachment_authority.is_some_and(|(epoch, _, _)| epoch != key.epoch)
        {
            return Ok(ConsumerAttachmentStatus::EpochMismatch);
        }

        let consumer = OwnedAgentId::new(key.consumer_environment_id, &key.consumer);
        let Some(consumer_identity) = self
            .worker_service
            .resolve_agent_identity(&consumer)
            .await
            .map_err(|error| agent_read_error(&consumer.agent_id, error))?
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
            .map_err(StreamStoreError::from)?;
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
#[golem_async_trait::region_simple]
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
    ) -> Result<Option<Vec<ConsumerJournalInspection>>, StreamStoreError> {
        let consumer = OwnedAgentId::new(key.consumer_environment_id, &key.consumer);
        let Some(metadata) = self
            .worker_service
            .get(&consumer)
            .await
            .map_err(|error| agent_read_error(&consumer.agent_id, error))?
        else {
            return Ok(None);
        };
        if metadata.initial_worker_metadata.fingerprint != key.expected_consumer_fingerprint {
            crate::metrics::workers::record_foreign_stream_fingerprint_mismatch();
            return Ok(None);
        }
        let agent_mode = metadata.initial_worker_metadata.agent_mode;
        let control = self
            .worker_service
            .lookup_durable_stream_control_metadata(
                &consumer,
                agent_mode,
                key.expected_consumer_fingerprint,
                &key.session_key,
            )
            .await
            .map_err(StreamStoreError::from)?;
        let readers = control.readers_for_attachment(key);
        if readers.is_empty() {
            return Ok(None);
        }
        let current = self
            .oplog_service
            .get_last_index(&consumer, agent_mode)
            .await?;
        if !current.is_defined() {
            return Ok(None);
        }
        let lineage = StreamForkLineage::load_from_service(
            self.oplog_service.as_ref(),
            &consumer,
            agent_mode,
            key.expected_consumer_fingerprint,
            current,
        )
        .await
        .map_err(StreamStoreError::from)?;
        let mut inspections = readers
            .into_iter()
            .map(|reader_id| {
                (
                    reader_id,
                    ConsumerJournalInspection {
                        reader_id,
                        source_offsets: Vec::new(),
                        source_unavailable: None,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        for (index, entry) in self
            .oplog_service
            .read_exact(&consumer, agent_mode, OplogIndex::INITIAL, current.as_u64())
            .await?
        {
            if lineage.deleted_regions().is_in_deleted_region(index) {
                continue;
            }
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = consumer_record(
                self.oplog_service.as_ref(),
                &consumer,
                agent_mode,
                index,
                record,
            )
            .await?;
            if let Some(original_index) = record.reobserves() {
                let original = self
                    .oplog_service
                    .read_exact(&consumer, agent_mode, original_index, 1)
                    .await?;
                let Some(OplogEntry::StreamSession {
                    record: original, ..
                }) = original.get(&original_index)
                else {
                    return Err(StreamStoreError::CorruptHistory(
                        "consumer re-observation has no original record".into(),
                    ));
                };
                let original = consumer_record(
                    self.oplog_service.as_ref(),
                    &consumer,
                    agent_mode,
                    original_index,
                    original.clone(),
                )
                .await?;
                if original_index >= index
                    || !record.has_supported_format()
                    || !record.matches_reobserved(&original)
                {
                    return Err(StreamStoreError::CorruptHistory(
                        "consumer re-observation differs from original input".into(),
                    ));
                }
                continue;
            }
            let reader_id = match &record {
                StreamSessionRecord::ConsumerItemValue(record)
                    if record.session_key.qualify(
                        key.consumer_environment_id,
                        &key.consumer,
                        key.expected_consumer_fingerprint,
                    ) == key.session_key =>
                {
                    record.reader_id
                }
                StreamSessionRecord::ConsumerTerminal(record)
                    if record.session_key.qualify(
                        key.consumer_environment_id,
                        &key.consumer,
                        key.expected_consumer_fingerprint,
                    ) == key.session_key =>
                {
                    record.reader_id
                }
                StreamSessionRecord::SourceUnavailable(record)
                    if record.session_key.qualify(
                        key.consumer_environment_id,
                        &key.consumer,
                        key.expected_consumer_fingerprint,
                    ) == key.session_key =>
                {
                    record.reader_id
                }
                _ => continue,
            };
            let Some(inspection) = inspections.get_mut(&reader_id) else {
                continue;
            };
            let offsets = &mut inspection.source_offsets;
            let overlay = &mut inspection.source_unavailable;
            match record {
                StreamSessionRecord::ConsumerItemValue(record)
                    if record.session_key.qualify(
                        key.consumer_environment_id,
                        &key.consumer,
                        key.expected_consumer_fingerprint,
                    ) == key.session_key
                        && record.reader_id == reader_id =>
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
                    if record.session_key.qualify(
                        key.consumer_environment_id,
                        &key.consumer,
                        key.expected_consumer_fingerprint,
                    ) == key.session_key
                        && record.reader_id == reader_id =>
                {
                    if record.consumer_read_ordinal != offsets.len() as u64 {
                        return Err(StreamStoreError::CorruptHistory(
                            "consumer terminal journal contains a read-ordinal gap".to_string(),
                        ));
                    }
                    if let Some(source_unavailable) = *overlay {
                        if record.source_offset != source_unavailable
                            || record.authored_by != StreamTerminalAuthor::Protocol
                            || record.terminal
                                != StreamTerminal::Failed(StreamError::Interrupted(
                                    "durable stream source unavailable".into(),
                                ))
                        {
                            return Err(StreamStoreError::CorruptHistory(
                                "consumer EOF differs from its source-unavailable overlay".into(),
                            ));
                        }
                    } else {
                        offsets.push(record.source_offset);
                    }
                }
                StreamSessionRecord::SourceUnavailable(record)
                    if record.session_key.qualify(
                        key.consumer_environment_id,
                        &key.consumer,
                        key.expected_consumer_fingerprint,
                    ) == key.session_key
                        && record.reader_id == reader_id =>
                {
                    if record.consumer_read_ordinal != offsets.len() as u64 {
                        return Err(StreamStoreError::CorruptHistory(
                            "source-unavailable overlay contains a read-ordinal gap".to_string(),
                        ));
                    }
                    match *overlay {
                        Some(existing) if existing != record.source_offset => {
                            return Err(StreamStoreError::CorruptHistory(
                                "conflicting source-unavailable overlays".to_string(),
                            ));
                        }
                        Some(_) => {}
                        None => *overlay = Some(record.source_offset),
                    }
                }
                _ => {}
            }
        }
        let mut inspections = inspections.into_values().collect::<Vec<_>>();
        inspections.sort_by_key(|inspection| {
            (
                inspection.reader_id.introducing_oplog_index,
                inspection.reader_id.binding_slot,
            )
        });
        Ok(Some(inspections))
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
            .map_err(|error| agent_read_error(&consumer.agent_id, error))?
        else {
            return Ok(None);
        };
        if metadata.initial_worker_metadata.fingerprint != key.expected_consumer_fingerprint {
            crate::metrics::workers::record_foreign_stream_fingerprint_mismatch();
            return Ok(None);
        }
        let agent_mode = metadata.initial_worker_metadata.agent_mode;
        let control = self
            .worker_service
            .lookup_durable_stream_control_metadata(
                &consumer,
                agent_mode,
                key.expected_consumer_fingerprint,
                &key.session_key,
            )
            .await
            .map_err(StreamStoreError::from)?;
        let readers = control.readers_for_attachment(key);
        if readers.is_empty() {
            return Ok(None);
        }
        let (_, rows) = self
            .worker_service
            .lookup_durable_stream_producer_metadata(
                &consumer,
                agent_mode,
                key.expected_consumer_fingerprint,
                readers
                    .iter()
                    .map(|reader| {
                        ProducerMetadataKey::ConsumerHead(key.session_key.clone(), *reader)
                    })
                    .collect(),
            )
            .await
            .map_err(StreamStoreError::from)?;
        let heads = rows
            .into_iter()
            .map(|row| match row {
                Some(ProducerMetadataRow::ConsumerHead(head)) => Ok(head),
                None => Ok(IndexedConsumerJournal::default()),
                _ => Err(StreamStoreError::CorruptHistory(
                    "consumer head has an unexpected metadata row".into(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if heads.len() != readers.len() {
            return Ok(None);
        }
        let terminal = heads.iter().all(|head| head.terminal);
        let source_unavailable = !terminal
            && heads
                .iter()
                .all(|head| head.terminal || head.source_unavailable.is_some());
        let head = heads
            .iter()
            .min_by_key(|head| head.next_read_ordinal)
            .unwrap();
        Ok(Some(ConsumerJournalSummary {
            event_count: head.next_read_ordinal,
            last_offset: head.last_source_offset,
            terminal,
            source_unavailable,
        }))
    }

    async fn commit_source_unavailable(
        &self,
        key: &StreamAttachmentKey,
        reader_id: LocalStreamReaderId,
        source_offset: StreamOffset,
        consumer_read_ordinal: u64,
    ) -> Result<(), StreamStoreError> {
        let rpc = self.rpc.as_ref().ok_or_else(|| {
            StreamStoreError::Oplog(
                "consumer probe has no route for a source-unavailable overlay".to_string(),
            )
        })?;
        DurableStreamRemoteError::retry_while_unavailable(|| {
            rpc.control_durable_stream_attachment(
                StreamAttachmentControlRequest {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    mapping: None,
                    operation: StreamAttachmentControlOperation::SourceUnavailable {
                        key: key.clone(),
                        reader_id,
                        source_offset,
                        consumer_read_ordinal,
                    },
                },
                &AuthCtx::System,
            )
        })
        .await
        .map_err(|error| StreamStoreError::Oplog(error.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod read_error_tests {
    use super::*;
    use crate::services::oplog::PrimaryOplogService;
    use crate::services::oplog::tests::{
        default_execution_status, default_last_known_status, make_agent_metadata,
    };
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use golem_common::model::account::AccountId;
    use golem_common::model::{RetryConfig, Timestamp};
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use test_r::test;

    #[test]
    async fn an_undecodable_consumer_record_is_an_error_that_names_its_entry() {
        let service = PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            100,
            32,
            RetryConfig::default(),
        )
        .await;
        let agent_id = crate::test_agent_id("probe-journal-unreadable");
        let environment_id = EnvironmentId::new();
        let consumer = OwnedAgentId::new(environment_id, &agent_id);
        let oplog = service
            .open(
                &mut service.lock_lifecycle(&agent_id).await,
                &consumer,
                AgentMode::Durable,
                None,
                make_agent_metadata(agent_id, AccountId::new(), environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
            )
            .await
            .unwrap();
        use futures::{StreamExt, TryStreamExt};
        futures::stream::iter(1..=5)
            .then(|size| oplog.add(OplogEntry::grow_memory(size)))
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        oplog
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::SerializedInline {
                    bytes: vec![255, 7, 7],
                    cached: None,
                },
                summary: None,
            })
            .await
            .unwrap();
        oplog.commit(CommitLevel::Always).await.unwrap();
        let index = OplogIndex::from_u64(6);
        let Some(OplogEntry::StreamSession { record, .. }) = service
            .read_exact(&consumer, AgentMode::Durable, index, 1)
            .await
            .unwrap()
            .remove(&index)
        else {
            panic!("entry 6 is the stream session record");
        };

        let error = consumer_record(&service, &consumer, AgentMode::Durable, index, record)
            .await
            .expect_err("an undecodable record must fail the read");

        let StreamStoreError::Oplog(text) = &error else {
            panic!("a record that does not decode is no temporary read failure, got {error:?}");
        };
        assert!(text.contains(&agent_id.to_string()), "{text}");
        assert!(text.contains("at oplog index 6:"), "{text}");
        assert!(
            text.contains("unknown serialization version: 255"),
            "{text}"
        );
    }

    #[test]
    fn only_a_temporary_read_failure_of_an_agent_is_unreadable_in_the_stream_error() {
        let agent_id = crate::test_agent_id("probe-identity-unreadable");
        let failed = agent_read_error(
            &agent_id,
            WorkerExecutorError::oplog_failed(
                agent_id,
                "failed to decode oplog entry 6: unknown serialization version: 255",
            ),
        );
        assert!(
            matches!(&failed, StreamStoreError::Oplog(details)
                if details.contains("failed to decode oplog entry 6: unknown serialization version: 255")),
            "{failed:?}"
        );

        let temporary = agent_read_error(
            &agent_id,
            WorkerExecutorError::failed_to_resume_worker(
                agent_id,
                WorkerExecutorError::oplog_read_unavailable(
                    "oplog data from primary is unavailable",
                ),
            ),
        );
        let StreamStoreError::Unreadable(unreadable) = &temporary else {
            panic!("expected an unreadable oplog, got {temporary:?}");
        };
        assert_eq!(unreadable.agent_id, agent_id);
        assert!(unreadable.is_transient());
        assert!(
            unreadable
                .to_string()
                .contains("oplog data from primary is unavailable")
        );
    }

    /// A recovery for a cause that is not a read is not an unreadable oplog.
    #[test]
    fn a_recovery_for_another_cause_is_not_an_unreadable_agent() {
        let agent_id = crate::test_agent_id("probe-identity-recovery");
        let error = agent_read_error(
            &agent_id,
            WorkerExecutorError::recovery_required("stream output 7 is incomplete"),
        );
        assert!(
            matches!(&error, StreamStoreError::Oplog(details)
                if details.contains("stream output 7 is incomplete")),
            "{error:?}"
        );
    }
}
