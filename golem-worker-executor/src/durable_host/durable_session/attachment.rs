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

impl StreamSession {
    /// Validates frame epoch, attachment attempt, durable stream ID, and role.
    pub async fn validate_frame(
        &self,
        transport_stream_id: u64,
        durable_stream_id: Option<golem_schema::proto::golem::common::Uuid>,
        epoch: u64,
        expected_role: SessionStreamRole,
    ) -> Result<DurableStreamHandle, SessionError> {
        let AuthoritativeAttachmentState {
            epoch: current_epoch,
            attempt_id: current_attempt_id,
            attached,
        } = self.authoritative_attachment_state().await?;
        if epoch != current_epoch || self.attachment_epoch != current_epoch {
            return Err(
                if epoch < current_epoch || self.attachment_epoch < current_epoch {
                    SessionError::from(
                        "StaleEpoch: durable stream frame uses a fenced attachment epoch"
                            .to_string(),
                    )
                } else {
                    SessionError::from(
                        "InvalidEpoch: durable stream frame uses a future attachment epoch"
                            .to_string(),
                    )
                },
            );
        }
        if !attached || self.attachment_attempt_id != Some(current_attempt_id) {
            return Err(SessionError::from(
                "StaleEpoch: durable stream frame uses a detached attachment".to_string(),
            ));
        }
        let durable_stream_id: uuid::Uuid = durable_stream_id
            .ok_or_else(|| "durable stream frame has no durable stream ID".to_string())?
            .into();
        let mappings = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned");
        let mapping = mappings
            .get(&transport_stream_id)
            .ok_or_else(|| format!("unknown durable transport stream ID {transport_stream_id}"))?;
        if mapping.handle.stream_id.0 != durable_stream_id
            || mapping.role.direction() != expected_role.direction()
        {
            return Err(SessionError::from(
                "transport stream mapping does not match the durable stream ID and role"
                    .to_string(),
            ));
        }
        Ok(mapping.handle.clone())
    }

    /// Returns the attachment epoch represented by this runtime.
    pub fn attachment_epoch(&self) -> u64 {
        self.attachment_epoch
    }

    /// Rejects this runtime if durable authority has detached or advanced its epoch.
    pub async fn ensure_current_attachment(&self) -> Result<(), SessionError> {
        let AuthoritativeAttachmentState {
            epoch,
            attempt_id,
            attached,
        } = self.authoritative_attachment_state().await?;
        if epoch != self.attachment_epoch
            || self.attachment_attempt_id != Some(attempt_id)
            || !attached
        {
            return Err(SessionError::from(
                "StaleEpoch: durable attachment has been fenced".to_string(),
            ));
        }
        Ok(())
    }

    /// Waits until durable attachment authority fences this runtime.
    pub async fn wait_for_attachment_revocation(&self) -> Result<(), SessionError> {
        loop {
            let changed = self.producer.session_records_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match self.ensure_current_attachment().await {
                Ok(()) => changed.await,
                Err(error) if error.starts_with("StaleEpoch:") => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }

    /// Reads attachment epoch, attempt, and attached state from durable oplog metadata.
    pub async fn authoritative_attachment_state(
        &self,
    ) -> Result<AuthoritativeAttachmentState, SessionError> {
        let raw = self
            .oplog
            .raw_durable_stream_session_status(&self.session_key)
            .await;
        let status = raw
            .status?
            .ok_or_else(|| "durable session has no attachment authority".to_string())?;
        if let Some(error) = status.lifecycle_error {
            return Err(SessionError::from(error));
        }
        match (
            status.attachment_epoch,
            status.attachment_attempt_id,
            status.attachment_attached,
        ) {
            (Some(epoch), Some(attempt_id), Some(attached)) => Ok(AuthoritativeAttachmentState {
                epoch,
                attempt_id,
                attached,
            }),
            _ => Err(SessionError::from(
                "durable session has no attachment authority".to_string(),
            )),
        }
    }

    /// Durably detaches the current transport without cancelling producer streams.
    pub async fn detach_current(&self) -> Result<bool, SessionError> {
        let session = self.clone();
        self.producer
            .run_admitted(None, 0, true, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                let session_for_write = session.clone();
                admission
                    .submit(move |_, context| async move {
                        session_for_write.detach_current_owned(&context).await
                    })
                    .await
            })
            .await
    }

    async fn detach_current_owned(
        &self,
        context: &StreamWriteContext,
    ) -> Result<bool, SessionError> {
        let AuthoritativeAttachmentState {
            epoch,
            attempt_id,
            attached,
        } = self.authoritative_attachment_state().await?;
        if !attached
            || epoch != self.attachment_epoch
            || self.attachment_attempt_id != Some(attempt_id)
        {
            return Ok(false);
        }
        self.try_append_record_owned(
            context,
            StreamSessionRecord::Detached(StreamSessionDetachedRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: self.session_key.idempotency_key.clone(),
                attachment_id: golem_common::model::durable_stream::AttachmentId::primary(
                    self.session_key.callee_environment_id,
                    &self.session_key.callee,
                    &self.session_key.idempotency_key,
                )
                .map_err(|error| error.to_string())?,
                owner_attempt_id: attempt_id,
                epoch,
            }),
        )
        .await?;
        self.commit_consumer_journal().await?;
        Ok(true)
    }

    #[tracing::instrument(
        name = "durable_stream.resume_attempt",
        skip_all,
        fields(
            attachment_id = %record.attempt.attachment_id.0,
            attempt_id = %record.attempt.attempt_id.0,
            operation = ?record.attempt.operation,
            expected_epoch = record.attempt.expected_epoch,
            accepted_epoch = record.accepted_epoch,
        )
    )]
    /// Commits a resume or takeover attempt before the new epoch is used for frames.
    pub async fn commit_resume_attempt(
        &self,
        record: StreamSessionResumeAttemptRecord,
    ) -> Result<(), SessionError> {
        let memory = golem_common::serialization::serialize(&record)?.len();
        let session = self.clone();
        self.producer
            .run_admitted(None, memory, true, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                let session_for_write = session.clone();
                admission
                    .submit(move |_, context| async move {
                        session_for_write
                            .commit_resume_attempt_owned(&context, record)
                            .await
                    })
                    .await
            })
            .await
    }

    async fn commit_resume_attempt_owned(
        &self,
        context: &StreamWriteContext,
        record: StreamSessionResumeAttemptRecord,
    ) -> Result<(), SessionError> {
        let AuthoritativeAttachmentState {
            epoch: current_epoch,
            attached,
            ..
        } = self.authoritative_attachment_state().await?;
        if record.attempt.expected_epoch < current_epoch {
            return Err(SessionError::from(format!(
                "StaleEpoch: current attachment epoch is {current_epoch}"
            )));
        }
        if record.attempt.expected_epoch > current_epoch {
            return Err(SessionError::from(format!(
                "InvalidEpoch: current attachment epoch is {current_epoch}"
            )));
        }
        match (record.attempt.operation, attached) {
            (StreamResumeOperation::Resume, false) | (StreamResumeOperation::Takeover, true) => {}
            _ => {
                return Err(
                    SessionError::from("InvalidAttachmentState: resume requires Detached and takeover requires Attached"
                        .to_string()),
                );
            }
        }
        let result = self
            .producer
            .append_session_record_attributed(
                Some(context),
                self.entity_parent_start_index,
                StreamSessionRecord::ResumeAttempt(record),
            )
            .await
            .map_err(SessionError::from);
        if result.is_ok() {
            tracing::debug!("Durable Stream Session resume attempt committed");
        }
        result
    }

    /// Ensures an open local session is attached before producer output begins.
    pub async fn require_local_session_attachment(
        &self,
        attachment: &StreamAttachmentKey,
    ) -> Result<(), SessionError> {
        if !self.has_local_session_authority() {
            return Ok(());
        }
        let status = self
            .oplog
            .raw_durable_stream_session_status(&self.session_key)
            .await
            .status?
            .ok_or_else(|| "durable topology has no local session authority".to_string())?;
        if let Some(error) = status.lifecycle_error {
            return Err(SessionError::from(error));
        }
        let prepared_attempt = status
            .prepared_attempt_id
            .ok_or_else(|| "durable topology has no Prepared session authority".to_string())?;
        let (attached_epoch, attached_attempt_id) = match (
            status.attachment_epoch,
            status.attachment_attempt_id,
            status.attachment_attached,
        ) {
            (Some(epoch), Some(attempt), Some(_)) => (epoch, attempt),
            _ => {
                return Err(SessionError::from(
                    "durable topology cannot activate before session attachment".to_string(),
                ));
            }
        };
        let initial_attachment = status.initial_attachment_epoch == Some(attached_epoch)
            && status.initial_attachment_attempt_id == Some(attached_attempt_id);
        if (initial_attachment && attached_attempt_id != prepared_attempt)
            || attached_epoch != attachment.epoch
            || AttachmentId::primary(
                self.session_key.callee_environment_id,
                &self.session_key.callee,
                &self.session_key.idempotency_key,
            )
            .map_err(|error| error.to_string())?
                != attachment.attachment_id
            || (initial_attachment
                && !matches!(
                    self.oplog
                        .read(
                            status.initial_pending_invocation_oplog_index.ok_or_else(|| {
                                "durable initial attachment has no pending invocation".to_string()
                            })?
                        )
                        .await?,
                    OplogEntry::PendingAgentInvocation { idempotency_key, .. }
                        if idempotency_key == self.session_key.idempotency_key
                ))
        {
            return Err(SessionError::from(
                "durable topology attachment does not exactly match its local session authority"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub(super) fn has_local_session_authority(&self) -> bool {
        matches!(
            self.session_reference,
            StreamRegistrationInvocation::Local(_)
        )
    }

    pub(super) fn validate_forwarded_mapping(
        &self,
        attachment: &StreamAttachmentKey,
        mapping: &StreamSessionMappingRecord,
    ) -> Result<(), SessionError> {
        if attachment.session_key != self.session_key
            || attachment.consumer_invocation != self.consumer_invocation
            || attachment.consumer_environment_id != self.producer.environment_id()
            || attachment.consumer != *self.producer.agent_id()
            || attachment.expected_consumer_fingerprint != self.producer.fingerprint()
            || mapping.handle.stream_id != attachment.stream_id
            || mapping.handle.producer_environment_id != attachment.producer_environment_id
            || mapping.handle.producer != attachment.producer
            || mapping.handle.expected_producer_fingerprint
                != attachment.expected_producer_fingerprint
        {
            return Err(SessionError::from(
                "forwarded stream attachment does not match the durable session or handle"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub(super) async fn topology_state(
        &self,
        attachment: &StreamAttachmentKey,
        expected_mapping: Option<&StreamSessionMappingRecord>,
    ) -> Result<ConsumerAttachmentStatus, SessionError> {
        if attachment.session_key != self.session_key
            || attachment.consumer_invocation != self.consumer_invocation
            || attachment.consumer_environment_id != self.producer.environment_id()
            || attachment.consumer != *self.producer.agent_id()
            || attachment.expected_consumer_fingerprint != self.producer.fingerprint()
        {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        let metadata = self.current_control_metadata().await?;
        let state = metadata.topology_status(attachment, expected_mapping)?;
        if matches!(
            state,
            ConsumerAttachmentStatus::IncarnationMismatch | ConsumerAttachmentStatus::EpochMismatch
        ) {
            return Ok(state);
        }
        let local_session_authority = self.has_local_session_authority();
        let attached_epoch = metadata
            .topology_epoch()
            .or_else(|| (!local_session_authority).then_some(attachment.epoch));
        match attached_epoch {
            Some(epoch) if epoch == attachment.epoch => Ok(state),
            Some(_) => Ok(ConsumerAttachmentStatus::EpochMismatch),
            None if state == ConsumerAttachmentStatus::Active => Err(SessionError::from(
                "durable topology activation precedes session attachment".to_string(),
            )),
            None => Ok(state),
        }
    }

    /// Recovers durable mappings discovered inside previously journaled input values.
    pub async fn recover_nested_input_mappings(&self) -> Result<(), SessionError> {
        self.recover_session_mappings().await
    }

    /// Reconstructs visible mappings and unresolved attachment topology from the journal.
    pub async fn recover_session_mappings(&self) -> Result<(), SessionError> {
        // Clones share both the mapping table and its coverage. Keep the cursor locked across
        // validation so another output pump cannot observe coverage before the mappings exist.
        let mut covered = self.recovered_mappings_through.lock().await;
        let metadata = self.current_control_metadata().await?;
        self.next_transport_stream_id.fetch_max(
            metadata.next_reserved_transport_stream_id()?,
            Ordering::AcqRel,
        );
        let RecoveredMappings {
            covered_through: horizon,
            mappings,
        } = metadata.recoverable_mappings_after(*covered);
        drop(metadata);
        let materialized = self
            .producer
            .materialize_bindings(&mappings)
            .await
            .map_err(SessionError::from)?;
        for (binding, mapping) in mappings.into_iter().zip(materialized) {
            self.insert_binding_mapping(binding, mapping)?;
        }
        *covered = horizon;
        Ok(())
    }

    /// Checks whether the consumer journal already contains a terminal for this stream.
    pub async fn has_journaled_consumer_terminal(
        &self,
        mapping: &StreamSessionMappingRecord,
    ) -> Result<bool, SessionError> {
        if self
            .mapping(mapping.transport_stream_id)
            .as_ref()
            .is_some_and(|known| known != mapping)
        {
            return Err("consumer stream differs from its recorded mapping".into());
        }
        let metadata = self.current_control_metadata().await?;
        let binding = self
            .binding(mapping.transport_stream_id)
            .unwrap_or_else(|| StreamBindingRecord::foreign(mapping));
        metadata
            .has_consumer_terminal(&binding)
            .map_err(SessionError::from)
    }

    pub(super) async fn validate_recovered_mapping(
        &self,
        mapping: &StreamSessionMappingRecord,
    ) -> Result<(), SessionError> {
        if self.producer.owns_handle_identity(&mapping.handle) {
            return self
                .producer
                .validate_handle(&mapping.handle)
                .await
                .map_err(SessionError::from);
        }
        if self.has_journaled_consumer_terminal(mapping).await? {
            return Ok(());
        }
        {
            let metadata = self.current_control_metadata().await?;
            let source = self
                .binding(mapping.transport_stream_id)
                .unwrap_or_else(|| StreamBindingRecord::foreign(mapping))
                .source;
            if let Some(intent) = metadata.cancellation_intent(&source) {
                let key = self.cancellation_attachment_key(&mapping.handle, intent)?;
                if metadata.has_committed_cancellation(&key, mapping, intent)? {
                    return Ok(());
                }
            }
        }
        let attachment = self.attachment_key(&mapping.handle, self.reader_epoch().await?)?;
        if self.topology_state(&attachment, Some(mapping)).await?
            == ConsumerAttachmentStatus::Active
        {
            Ok(())
        } else {
            Err(SessionError::from(
                "foreign durable stream mapping is not topology-activated".to_string(),
            ))
        }
    }

    pub(super) async fn reader_epoch(&self) -> Result<u64, SessionError> {
        if self.has_local_session_authority() && self.attachment_attempt_id.is_none() {
            Ok(self.authoritative_attachment_state().await?.epoch)
        } else {
            Ok(self.attachment_epoch)
        }
    }

    pub(crate) fn attachment_key(
        &self,
        handle: &DurableStreamHandle,
        epoch: u64,
    ) -> Result<StreamAttachmentKey, SessionError> {
        Ok(StreamAttachmentKey {
            attachment_id: golem_common::base_model::durable_stream::AttachmentId::primary(
                self.session_key.callee_environment_id,
                &self.session_key.callee,
                &self.session_key.idempotency_key,
            )
            .map_err(|error| error.to_string())?,
            stream_id: handle.stream_id,
            epoch,
            session_key: self.session_key.clone(),
            producer_environment_id: handle.producer_environment_id,
            producer: handle.producer.clone(),
            expected_producer_fingerprint: handle.expected_producer_fingerprint,
            consumer_environment_id: self.producer.environment_id(),
            consumer: self.producer.agent_id().clone(),
            expected_consumer_fingerprint: self.producer.fingerprint(),
            consumer_invocation: self.consumer_invocation.clone(),
        })
    }

    /// Rejects resume cursors that skip or contradict committed consumer progress.
    pub async fn validate_resume_cursors(
        &self,
        cursors: &[golem_common::model::durable_stream::StreamResumeCursor],
    ) -> Result<(), SessionError> {
        for cursor in cursors {
            let mapping = self
                .mappings
                .read()
                .expect("durable stream mapping lock poisoned")
                .iter()
                .find_map(|(_, mapping)| {
                    (mapping.handle.stream_id == cursor.stream_id
                        && mapping.role.direction() == SessionStreamRole::Output)
                        .then(|| mapping.clone())
                })
                .ok_or_else(|| {
                    format!(
                        "resume cursor names no output mapping for durable stream {}",
                        cursor.stream_id
                    )
                })?;
            let Some(after) = cursor.last_observed_offset else {
                continue;
            };
            if self.producer.owns_handle_identity(&mapping.handle) {
                self.producer
                    .read_segment(&mapping.handle, Some(after), Some(after))
                    .await
                    .map_err(SessionError::from)?;
            } else {
                let rpc = self.rpc.clone().ok_or_else(|| {
                    "foreign durable stream source routing is unavailable".to_string()
                })?;
                let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                    "foreign durable stream consumer authorization is unavailable".to_string()
                })?;
                let source = RoutedAttachedStreamSegmentSource::new(
                    rpc,
                    mapping.clone(),
                    auth_ctx,
                    self.producer.clone(),
                );
                source
                    .read_attached_segment(
                        &self.attachment_key(&mapping.handle, self.reader_epoch().await?)?,
                        &mapping.handle,
                        Timestamp::now_utc().to_millis(),
                        Some(after),
                        Some(after),
                    )
                    .await
                    .map_err(SessionError::from)?;
            }
        }
        Ok(())
    }

    pub(super) async fn wait_for_active_attachment(
        &self,
        handle: &DurableStreamHandle,
    ) -> Result<(), SessionError> {
        loop {
            let changed = self.producer.session_records_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            // A terminal output only reconstructs committed events. Its consumer may
            // already have detached permanently, so replay cannot require an attachment.
            if self
                .producer
                .input_high_water(handle.stream_id)
                .await
                .map_err(SessionError::from)?
                .is_some_and(|high_water| high_water.terminal)
            {
                return Ok(());
            }
            let active = self
                .producer
                .has_active_attachment(&self.session_key, handle)
                .await
                .map_err(SessionError::from)?;
            if active {
                return Ok(());
            }
            changed.await;
        }
    }
}

#[async_trait::async_trait]
impl StreamAttachmentConsumerProbe for StreamSession {
    async fn status(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError> {
        self.topology_state(key, None)
            .await
            .map_err(StreamStoreError::from)
    }
}
