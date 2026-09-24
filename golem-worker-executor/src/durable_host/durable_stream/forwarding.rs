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
use crate::durable_host::stream_session::preflight_proto_recursive_stream_value;
use crate::services::stream_session_index::StreamSessionIndexService;
use golem_common::model::durable_stream::{
    StreamReaderForwardAcceptedRecord, StreamReaderForwardDestination,
    StreamReaderForwardIntentRecord, StreamReaderForwardPublication,
};
use prost::Message;

/// Inspects historical handoff acceptance without constructing either worker or granting authority.
pub(crate) struct ReaderForwardInspector {
    workers: Arc<dyn WorkerService>,
    oplog: Arc<dyn OplogService>,
}

impl ReaderForwardInspector {
    pub(crate) fn new(workers: Arc<dyn WorkerService>, oplog: Arc<dyn OplogService>) -> Self {
        Self { workers, oplog }
    }

    /// Inspects without holding the source writer, then durably settles the exact retained intent.
    /// The receipt preserves source attribution and does not finalize any attachment dependency.
    pub(crate) async fn settle(
        &self,
        producer: &DurableStreamStore,
        mode: AgentMode,
        intent_index: OplogIndex,
        intent: StreamReaderForwardIntentRecord,
    ) -> Result<bool, StreamStoreError> {
        let owner = OwnedAgentId::new(producer.environment_id, &producer.producer);
        if !self
            .destination_accepted(&owner, mode, intent_index, &intent.destination)
            .await?
        {
            return Ok(false);
        }
        let memory = golem_common::serialization::serialize(&intent)
            .map_err(StreamStoreError::Oplog)?
            .len();
        producer
            .run_lifecycle(None, memory, move |producer, context| async move {
                context.begin_lifecycle_publication().await?;
                let session = producer.qualify_session(&intent.session_key);
                let mut control = SessionControlMetadata::default();
                producer
                    .refresh_control_metadata(&session, &mut control)
                    .await
                    .map_err(StreamStoreError::CorruptHistory)?;
                if control.reader_binding(intent.reader_id).is_err()
                    || control
                        .reader_forward_intent(intent.reader_id)
                        .map_err(StreamStoreError::CorruptHistory)?
                        != Some(&(intent_index, intent.clone()))
                {
                    return Ok(false);
                }
                if control
                    .has_accepted_reader_forward(intent.reader_id)
                    .map_err(StreamStoreError::CorruptHistory)?
                {
                    return Ok(true);
                }
                let entry = producer
                    .oplog
                    .read_exact(intent_index, 1)
                    .await
                    .remove(&intent_index);
                let Some(OplogEntry::StreamSession {
                    entity_parent_start_index,
                    record,
                    ..
                }) = entry
                else {
                    return Err(StreamStoreError::CorruptHistory(
                        "forwarding intent projection references another record".into(),
                    ));
                };
                if producer
                    .oplog
                    .download_payload(record)
                    .await
                    .map_err(StreamStoreError::Oplog)?
                    != StreamSessionRecord::ReaderForwardIntent(intent.clone())
                {
                    return Err(StreamStoreError::CorruptHistory(
                        "forwarding intent differs from its projection".into(),
                    ));
                }
                producer
                    .append_session_record_owned(
                        &context,
                        entity_parent_start_index,
                        StreamSessionRecord::ReaderForwardAccepted(
                            StreamReaderForwardAcceptedRecord {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: intent.session_key,
                                intent_oplog_index: intent_index,
                            },
                        ),
                    )
                    .await?;
                Ok(true)
            })
            .await
    }

    /// The caller must recheck the retained source intent under its writer before recording a receipt.
    /// Acceptance settles that reader only; foreign-source history dependencies remain independent.
    pub(crate) async fn destination_accepted(
        &self,
        owner: &OwnedAgentId,
        mode: AgentMode,
        intent_index: OplogIndex,
        destination: &StreamReaderForwardDestination,
    ) -> Result<bool, StreamStoreError> {
        let index = self.oplog.stream_session_index().ok_or_else(|| {
            StreamStoreError::Oplog("stream session index service is unavailable".into())
        })?;
        let (journal, mode, reference, binding, publication, local_intent) = match destination {
            StreamReaderForwardDestination::SessionBinding {
                session_key,
                binding,
                publication,
            } => (
                owner.clone(),
                mode,
                session_key.clone(),
                binding.clone(),
                publication.clone(),
                Some(intent_index),
            ),
            StreamReaderForwardDestination::InvocationInput {
                invocation,
                mapping,
            } => {
                let journal =
                    OwnedAgentId::new(invocation.callee_environment_id, &invocation.callee);
                let Some(mode) = self
                    .workers
                    .get_agent_mode(&journal)
                    .await
                    .map_err(|error| StreamStoreError::Oplog(error.to_string()))?
                else {
                    return Ok(false);
                };
                (
                    journal,
                    mode,
                    StreamRegistrationInvocation::Local(invocation.idempotency_key.clone()),
                    StreamBindingRecord::foreign(mapping),
                    StreamReaderForwardPublication::InvocationInput,
                    None,
                )
            }
        };
        loop {
            let before = index
                .lookup_retained_history(&journal, mode)
                .await
                .map_err(StreamStoreError::Oplog)?;
            if let StreamReaderForwardDestination::InvocationInput { invocation, .. } = destination
                && before.0 != invocation.callee_fingerprint
            {
                return Ok(false);
            }
            let session = reference.qualify(journal.environment_id, &journal.agent_id, before.0);
            let result = self
                .inspect_publication(
                    &index,
                    &journal,
                    mode,
                    before.0,
                    &before.1,
                    &session,
                    &reference,
                    &binding,
                    &publication,
                    local_intent,
                )
                .await;
            let after = index
                .lookup_retained_history(&journal, mode)
                .await
                .map_err(StreamStoreError::Oplog)?;
            if before == after {
                return result;
            }
        }
    }

    async fn session_record(
        &self,
        owner: &OwnedAgentId,
        mode: AgentMode,
        position: OplogIndex,
    ) -> Result<(Option<OplogIndex>, StreamSessionRecord), StreamStoreError> {
        let entry = self
            .oplog
            .read_exact(owner, mode, position, 1)
            .await
            .remove(&position);
        let Some(OplogEntry::StreamSession {
            entity_parent_start_index,
            record,
            ..
        }) = entry
        else {
            return Err(StreamStoreError::CorruptHistory(
                "session projection references a missing or different record".into(),
            ));
        };
        let record = self
            .oplog
            .download_payload(owner, mode, record)
            .await
            .map_err(StreamStoreError::Oplog)?;
        if !record.has_supported_format() {
            return Err(StreamStoreError::CorruptHistory(
                "malformed forwarding publication".into(),
            ));
        }
        Ok((entity_parent_start_index, record))
    }

    async fn inspect_publication(
        &self,
        index: &StreamSessionIndexService,
        journal: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        lineage: &StreamForkLineage,
        session: &StreamSessionKey,
        reference: &StreamRegistrationInvocation,
        binding: &StreamBindingRecord,
        publication: &StreamReaderForwardPublication,
        intent_index: Option<OplogIndex>,
    ) -> Result<bool, StreamStoreError> {
        let StreamRecordReference::Foreign(handle) = &binding.source else {
            return Err(StreamStoreError::CorruptHistory(
                "forwarding destination is not a foreign binding".into(),
            ));
        };
        let control = index
            .lookup_control_metadata(journal, mode, session)
            .await
            .map_err(StreamStoreError::Oplog)?;
        control
            .ensure_valid()
            .map_err(StreamStoreError::CorruptHistory)?;
        let Ok(reader) = control.reader_id(binding) else {
            return Ok(false);
        };
        let (position, attribution) = match publication {
            StreamReaderForwardPublication::InvocationInput => {
                // Entity inputs are introduced by an attributed Mapping, not an RPC Prepared.
                let position = control
                    .prepared_position()
                    .unwrap_or(reader.introducing_oplog_index);
                let (attribution, record) = self.session_record(journal, mode, position).await?;
                let matches = match record {
                    StreamSessionRecord::Prepared(record) => {
                        record.session_key == session.idempotency_key
                            && record.stream_mappings.contains(binding)
                    }
                    StreamSessionRecord::Mapping(record) => {
                        intent_index.is_some()
                            && attribution.is_some()
                            && matches!(reference, StreamRegistrationInvocation::Local(_))
                            && record.session_key == *reference
                            && record.mapping == *binding
                    }
                    _ => false,
                };
                if !matches {
                    return Ok(false);
                }
                (position, attribution)
            }
            StreamReaderForwardPublication::InvocationResult { handle_index } => {
                let Some(position) = control.result_position() else {
                    return Ok(false);
                };
                let (attribution, record) = self.session_record(journal, mode, position).await?;
                let StreamSessionRecord::InvocationResult(record) = record else {
                    return Err(StreamStoreError::CorruptHistory(
                        "forwarding result projection references another record".into(),
                    ));
                };
                if record.session_key != *reference
                    || record.stream_mappings.get(*handle_index as usize) != Some(binding)
                {
                    return Ok(false);
                }
                require_value_reference(&record.result, *handle_index)?;
                (position, attribution)
            }
            StreamReaderForwardPublication::ProducerItem {
                parent_stream,
                sequence,
                handle_index,
            } => {
                let stream = qualify_local_stream(
                    *parent_stream,
                    journal.environment_id,
                    &journal.agent_id,
                    fingerprint,
                )?;
                let (_, mut rows) = index
                    .lookup_producer_metadata(
                        journal,
                        mode,
                        vec![ProducerMetadataKey::Batch(stream, *sequence)],
                    )
                    .await
                    .map_err(StreamStoreError::Oplog)?;
                let Some(ProducerMetadataRow::Batch(position)) = rows.pop().flatten() else {
                    return Ok(false);
                };
                let entry = self
                    .oplog
                    .read_exact(journal, mode, position, 1)
                    .await
                    .remove(&position);
                let Some(OplogEntry::StreamItems {
                    entity_parent_start_index,
                    record,
                    ..
                }) = entry
                else {
                    return Err(StreamStoreError::CorruptHistory(
                        "forwarding item projection references another record".into(),
                    ));
                };
                let mut record = self
                    .oplog
                    .download_payload(journal, mode, record)
                    .await
                    .map_err(StreamStoreError::Oplog)?;
                lineage
                    .project_item_batch(position, &mut record)
                    .map_err(StreamStoreError::CorruptHistory)?;
                if record.stream_id != *parent_stream || record.first_sequence != *sequence {
                    return Err(StreamStoreError::CorruptHistory(
                        "forwarding item projection does not match its stream and sequence".into(),
                    ));
                }
                if record.nested_stream_ids.get(*handle_index as usize) != Some(&binding.source) {
                    return Ok(false);
                }
                let StreamItemsPayload::Values(values) = &record.payload else {
                    return Err(StreamStoreError::CorruptHistory(
                        "packed item cannot publish a nested handle".into(),
                    ));
                };
                let [value] = values.as_slice() else {
                    return Err(StreamStoreError::CorruptHistory(
                        "nested handle publication must contain one value".into(),
                    ));
                };
                require_value_reference(value, *handle_index)?;
                let registration = self
                    .oplog
                    .read_exact(journal, mode, parent_stream.0, 1)
                    .await
                    .remove(&parent_stream.0);
                let Some(OplogEntry::StreamRegistered { record, .. }) = registration else {
                    return Err(StreamStoreError::CorruptHistory(
                        "forwarding parent stream has no registration".into(),
                    ));
                };
                let registration = self
                    .oplog
                    .download_payload(journal, mode, record)
                    .await
                    .map_err(StreamStoreError::Oplog)?;
                if registration.source_invocation != *reference {
                    return Ok(false);
                }
                (position, entity_parent_start_index)
            }
        };
        if position > control.covered_through()
            || lineage.deleted_regions().is_in_deleted_region(position)
        {
            return Ok(false);
        }
        let segment = |position| {
            lineage
                .cuts()
                .iter()
                .filter(|(marker, _)| *marker <= position)
                .map(|(marker, _)| *marker)
                .max()
                .unwrap_or(OplogIndex::NONE)
        };
        if intent_index.is_some_and(|intent| segment(position) < segment(intent)) {
            return Ok(false);
        }
        let entity_scoped = intent_index.is_some()
            && matches!(reference, StreamRegistrationInvocation::Local(_))
            && attribution.is_some();
        if entity_scoped && control.prepared_position().is_some() {
            return Err(StreamStoreError::CorruptHistory(
                "entity-attributed publication in a session with attachment authority".into(),
            ));
        }
        if entity_scoped {
            return Ok(true);
        }
        let Some(witness) = index
            .lookup_topology_witness(journal, mode, session, binding, position)
            .await
            .map_err(StreamStoreError::Oplog)?
        else {
            // A binding naming this journal's own producer needs no foreign attachment.
            return if handle.producer_environment_id == journal.environment_id
                && handle.producer == journal.agent_id
                && handle.expected_producer_fingerprint == fingerprint
            {
                self.session_accepted(index, session, None).await
            } else {
                Ok(false)
            };
        };
        self.session_accepted(
            index,
            session,
            witness
                .epoch_authority
                .is_none()
                .then_some(witness.attachment.epoch),
        )
        .await
    }

    async fn session_accepted(
        &self,
        index: &StreamSessionIndexService,
        session: &StreamSessionKey,
        historical_epoch: Option<u64>,
    ) -> Result<bool, StreamStoreError> {
        let owner = OwnedAgentId::new(session.callee_environment_id, &session.callee);
        let Some(mode) = self
            .workers
            .get_agent_mode(&owner)
            .await
            .map_err(|error| StreamStoreError::Oplog(error.to_string()))?
        else {
            return Ok(false);
        };
        loop {
            let before = index
                .lookup_retained_history(&owner, mode)
                .await
                .map_err(StreamStoreError::Oplog)?;
            if before.0 != session.callee_fingerprint {
                return Ok(false);
            }
            let result = async {
                let Some(status) = index
                    .lookup_latest(&owner, mode, &session.idempotency_key)
                    .await
                    .map_err(StreamStoreError::Oplog)?
                else {
                    return Ok(false);
                };
                if let Some(error) = status.lifecycle_error {
                    return Err(StreamStoreError::CorruptHistory(error));
                }
                if status.prepared_attempt_id.is_none()
                    || status.prepared_attempt_id != status.initial_attachment_attempt_id
                    || status.initial_pending_invocation_oplog_index.is_none()
                    || status.initial_pending_invocation_oplog_index
                        != status.validated_initial_pending_invocation
                {
                    return Ok(false);
                }
                match historical_epoch {
                    Some(epoch) => Ok(index
                        .lookup_epoch_authority(&owner, mode, session, epoch)
                        .await
                        .map_err(StreamStoreError::Oplog)?
                        .is_some()),
                    None => Ok(true),
                }
            }
            .await;
            if before
                == index
                    .lookup_retained_history(&owner, mode)
                    .await
                    .map_err(StreamStoreError::Oplog)?
            {
                return result;
            }
        }
    }
}

fn require_value_reference(bytes: &[u8], handle_index: u64) -> Result<(), StreamStoreError> {
    let value = golem_api_grpc::proto::golem::schema::SchemaValue::decode(bytes)
        .map_err(|error| StreamStoreError::CorruptHistory(error.to_string()))?;
    let references =
        preflight_proto_recursive_stream_value(&value).map_err(StreamStoreError::CorruptHistory)?;
    if !references.contains(&handle_index) {
        return Err(StreamStoreError::CorruptHistory(
            "forwarding binding is absent from the published value".into(),
        ));
    }
    Ok(())
}
