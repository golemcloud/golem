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
    pub(super) fn cancellation_attachment_key(
        &self,
        handle: &DurableStreamHandle,
        intent: &StreamConsumerCancelIntentRecord,
    ) -> Result<StreamAttachmentKey, SessionError> {
        let mut key = self.attachment_key(handle, intent.epoch)?;
        key.consumer_invocation =
            self.producer
                .qualify_session(&StreamRegistrationInvocation::Local(
                    intent.consumer_invocation.clone(),
                ));
        Ok(key)
    }

    /// Durably requests cancellation for all still-open streams in the session.
    pub async fn cancel_session_streams(&self) -> Result<bool, SessionError> {
        if !self.has_local_session_authority() {
            return Err("session cancellation requires the session owner".into());
        }
        let session = self.clone();
        self.producer
            .run_admitted(None, 0, true, move |_, admission| async move {
                let guard = session.session_lock.lock().await;
                let metadata = session.current_control_metadata().await?;
                if !metadata.can_request_cancellation()? {
                    return Ok::<_, SessionError>(false);
                }
                let epoch = session.authoritative_attachment_state().await?.epoch;
                let Some(records) =
                    metadata.cancellation_records(epoch, &session.session_reference)?
                else {
                    return Ok::<_, SessionError>(false);
                };
                drop(metadata);
                if !records.is_empty() {
                    let attribution = session.entity_parent_start_index;
                    admission
                        .submit(move |owner, context| async move {
                            owner
                                .append_session_records_owned(&context, attribution, records)
                                .await
                        })
                        .await
                        .map_err(SessionError::from)?;
                }
                drop(guard);
                session
                    .reconcile_local_cancellation_intents(Some(&admission))
                    .await?;
                Ok(true)
            })
            .await
    }

    /// The caller resolves the canonical slot under this session guard inside an owned lifecycle operation.
    pub(crate) async fn tombstone_slot_owned(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        slot: String,
        stream: Option<(DurableStreamHandle, SessionStreamRole)>,
        session_guard: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<bool, SessionError> {
        if !self.has_local_session_authority() {
            return Err("slot deletion requires the session owner".into());
        }
        self.recover_session_mappings().await?;
        let metadata = self.current_control_metadata().await?;
        if !metadata.is_prepared() {
            return Err("unknown durable stream session".into());
        }
        if metadata.is_slot_tombstoned(&slot) {
            return Ok(false);
        }
        let mut records = vec![StreamSessionRecord::Tombstoned(
            StreamSlotTombstonedRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: self.session_reference.clone(),
                slot,
                role: stream
                    .as_ref()
                    .map_or(SessionStreamRole::Output, |(_, role)| *role),
            },
        )];
        if let Some((handle, role)) = stream {
            let binding = self
                .mapping_for_handle(&handle, role)
                .and_then(|mapping| self.binding(mapping.transport_stream_id))
                .ok_or_else(|| "deleted slot has no persisted stream mapping".to_string())?;
            if !metadata.has_persisted_source(&binding.source, role) {
                return Err("deleted slot has no persisted stream mapping".into());
            }
            if metadata.cancellation_intent(&binding.source).is_none() {
                let epoch = self.authoritative_attachment_state().await?.epoch;
                records.push(StreamSessionRecord::ConsumerCancelIntent(
                    StreamConsumerCancelIntentRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: self.session_reference.clone(),
                        consumer_invocation: self.consumer_invocation.idempotency_key.clone(),
                        source: binding.source,
                        epoch,
                        role: match role.direction() {
                            SessionStreamRole::Input => StreamCancelRole::InputProducer,
                            SessionStreamRole::Output => StreamCancelRole::OutputConsumer,
                            SessionStreamRole::ToolStdin
                            | SessionStreamRole::ToolStdout
                            | SessionStreamRole::ToolStderr => unreachable!(),
                        },
                        reason: StreamCancelReason::Cancelled,
                        details: None,
                    },
                ));
            }
        }
        drop(metadata);
        let attribution = self.entity_parent_start_index;
        admission
            .submit(move |owner, context| async move {
                owner
                    .append_session_records_owned(&context, attribution, records)
                    .await
            })
            .await
            .map_err(SessionError::from)?;
        drop(session_guard);
        self.reconcile_local_cancellation_intents(Some(admission))
            .await?;
        Ok(true)
    }

    /// Journals consumer cancellation before forwarding it to the producer.
    pub async fn cancel_stream(
        &self,
        transport_stream_id: u64,
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
        expected_attachment_epoch: Option<u64>,
    ) -> Result<(), SessionError> {
        let session = self.clone();
        self.producer
            .run_admitted(
                None,
                details.as_ref().map_or(0, String::len),
                true,
                move |_, admission| async move {
                    session
                        .cancel_stream_owned(
                            &admission,
                            transport_stream_id,
                            role,
                            reason,
                            details,
                            expected_attachment_epoch,
                        )
                        .await
                },
            )
            .await
    }

    async fn cancel_stream_owned(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        transport_stream_id: u64,
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
        expected_attachment_epoch: Option<u64>,
    ) -> Result<(), SessionError> {
        let session_guard = self.session_lock.lock().await;
        if let Some(expected_epoch) = expected_attachment_epoch {
            if expected_epoch != self.attachment_epoch {
                return Err(SessionError::from(
                    "StaleEpoch: durable stream cancellation was fenced".to_string(),
                ));
            }
            self.ensure_current_attachment().await?;
        }
        let mapping = self
            .mapping(transport_stream_id)
            .ok_or_else(|| format!("unknown durable input stream {transport_stream_id}"))?;
        let expected_role = match role {
            StreamCancelRole::InputProducer | StreamCancelRole::InputConsumer => {
                SessionStreamRole::Input
            }
            StreamCancelRole::OutputProducer | StreamCancelRole::OutputConsumer => {
                SessionStreamRole::Output
            }
            StreamCancelRole::System => {
                return Err(SessionError::from(
                    "system-authored durable stream cancellation is internal".to_string(),
                ));
            }
        };
        if mapping.role.direction() != expected_role {
            return Err(SessionError::from(
                "durable stream cancellation role does not match its mapping".to_string(),
            ));
        }
        let binding = self
            .binding(transport_stream_id)
            .ok_or_else(|| format!("unknown durable input binding {transport_stream_id}"))?;
        let epoch = if self.has_local_session_authority() {
            match self.attachment_attempt_id {
                // Streams bound to a transport attempt may only cancel while that attempt is
                // still the current attachment; a superseded attempt is fenced.
                Some(_) => {
                    self.ensure_current_attachment().await?;
                    self.attachment_epoch
                }
                // Streams handed to the callee guest are not bound to any transport attempt:
                // the guest owns the consumer side for the whole invocation, so its cancellation
                // targets whatever attachment epoch is currently authoritative.
                None => self.authoritative_attachment_state().await?.epoch,
            }
        } else {
            let has_intent = self
                .current_control_metadata()
                .await?
                .cancellation_intent(&binding.source)
                .is_some();
            if !self.producer.owns_handle_identity(&mapping.handle)
                && !self.has_journaled_consumer_terminal(&mapping).await?
                && !has_intent
            {
                let attachment = self.attachment_key(&mapping.handle, self.attachment_epoch)?;
                if self.topology_state(&attachment, Some(&mapping)).await?
                    != ConsumerAttachmentStatus::Active
                {
                    let rpc = self.rpc.clone().ok_or_else(|| {
                        "foreign durable stream control routing is unavailable".to_string()
                    })?;
                    let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                        "foreign durable stream consumer authorization is unavailable".to_string()
                    })?;
                    let control =
                        RoutedStreamAttachmentControl::new(rpc, mapping.clone(), auth_ctx);
                    self.activate_forwarded_mapping_under_lock(
                        admission,
                        attachment,
                        mapping.clone(),
                        &control,
                        Timestamp::now_utc().to_millis(),
                    )
                    .await?;
                }
            }
            self.validate_recovered_mapping(&mapping).await?;
            self.attachment_epoch
        };
        let intent = StreamConsumerCancelIntentRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: self.session_reference.clone(),
            consumer_invocation: self.consumer_invocation.idempotency_key.clone(),
            source: binding.source.clone(),
            epoch,
            role,
            reason,
            details,
        };
        let persisted_intent = self
            .current_control_metadata()
            .await?
            .cancellation_intent(&binding.source)
            .cloned();
        let intent = match persisted_intent {
            Some(existing) => existing,
            None => {
                self.try_append_record(
                    admission,
                    StreamSessionRecord::ConsumerCancelIntent(intent.clone()),
                )
                .await?;
                self.commit_consumer_journal().await?;
                intent
            }
        };
        self.apply_cancel_intent_owned(admission, mapping, intent, session_guard)
            .await
    }

    /// Applies committed cancellation intents to streams produced by the local agent.
    pub async fn reconcile_local_cancellation_intents(
        &self,
        admission: Option<&Arc<StreamWriteAdmission>>,
    ) -> Result<(), SessionError> {
        let metadata = self.current_control_metadata().await?;
        let mut pending = Vec::new();
        for work in metadata.pending_cancellations() {
            let work = work?;
            if matches!(work.binding.source, StreamRecordReference::Local(_)) {
                let mapping = self
                    .producer
                    .materialize_binding(&work.binding)
                    .await
                    .map_err(SessionError::from)?;
                pending.push((mapping, work.intent));
            }
        }
        drop(metadata);
        for (mapping, intent) in pending {
            let session = self.clone();
            self.producer
                .run_admitted(
                    admission,
                    intent.details.as_ref().map_or(0, String::len),
                    true,
                    move |_, admission| async move {
                        let _guard = session.session_lock.lock().await;
                        let session_for_write = session.clone();
                        admission
                            .submit(move |_, context| async move {
                                let session = session_for_write;
                                session
                                    .producer
                                    .validate_handle(&mapping.handle)
                                    .await
                                    .map_err(SessionError::from)?;
                                // The terminal dispatcher owns publication; recovery only waits for
                                // durable cancellation, not for a slow live reader to make room.
                                session
                                    .producer
                                    .commit_cancel_open(
                                        Some(&context),
                                        mapping.handle.stream_id,
                                        intent.role,
                                        intent.reason,
                                        intent.details.clone(),
                                    )
                                    .await
                                    .map_err(SessionError::from)?;
                                session
                                    .producer
                                    .append_session_record_owned(
                                        &context,
                                        session.entity_parent_start_index,
                                        StreamSessionRecord::ConsumerCancelApplied(
                                            StreamConsumerCancelAppliedRecord {
                                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                                intent,
                                            },
                                        ),
                                    )
                                    .await
                                    .map_err(SessionError::from)?;
                                Ok::<_, SessionError>(())
                            })
                            .await
                    },
                )
                .await?;
        }
        Ok(())
    }

    /// Forwards committed cancellation intents and records their durable application.
    pub async fn reconcile_foreign_cancellation_intents(
        &self,
        timeout: Duration,
    ) -> Result<(), SessionError> {
        let metadata = self.current_control_metadata().await?;
        let mut pending = Vec::new();
        for work in metadata.pending_cancellations() {
            let work = work?;
            if matches!(work.binding.source, StreamRecordReference::Local(_)) {
                continue;
            }
            let mapping = self
                .producer
                .materialize_binding(&work.binding)
                .await
                .map_err(SessionError::from)?;
            let intent = work.intent;
            let key = self.cancellation_attachment_key(&mapping.handle, &intent)?;
            pending.push((key, mapping, intent));
        }
        drop(metadata);
        let mut first_error = None;
        for (key, mapping, intent) in pending {
            let rpc = self
                .rpc
                .clone()
                .ok_or("foreign cancellation routing is unavailable")?;
            let auth_ctx = self
                .auth_ctx
                .clone()
                .ok_or("foreign cancellation authorization is unavailable")?;
            let receipt_intent = intent.clone();
            let attribution = self.entity_parent_start_index;
            let result = self
                .producer
                .run_admitted(
                    None,
                    intent.details.as_ref().map_or(0, String::len),
                    true,
                    move |_, admission| async move {
                        tokio::time::timeout(
                            timeout,
                            RoutedStreamAttachmentControl::new(rpc, mapping, auth_ctx)
                                .cancel_stream(key, intent.role, intent.reason, intent.details),
                        )
                        .await
                        .map_err(|_| StreamStoreError::RecoveryRequired)?
                        .map_err(SessionError::from)?;
                        admission
                            .submit(move |owner, context| async move {
                                owner
                                    .append_session_record_owned(
                                        &context,
                                        attribution,
                                        StreamSessionRecord::ConsumerCancelApplied(
                                            StreamConsumerCancelAppliedRecord {
                                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                                intent: receipt_intent,
                                            },
                                        ),
                                    )
                                    .await
                                    .map_err(SessionError::from)
                            })
                            .await
                    },
                )
                .await;
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub(super) async fn apply_cancel_intent_owned(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        mapping: StreamSessionMappingRecord,
        intent: StreamConsumerCancelIntentRecord,
        session_guard: tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<(), SessionError> {
        if self
            .current_control_metadata()
            .await?
            .is_cancellation_applied(&intent)
        {
            return Ok(());
        }
        if matches!(intent.source, StreamRecordReference::Local(_)) {
            let attribution = self.entity_parent_start_index;
            admission
                .submit(move |owner, context| async move {
                    let pending = owner
                        .commit_cancel_open(
                            Some(&context),
                            mapping.handle.stream_id,
                            intent.role,
                            intent.reason,
                            intent.details.clone(),
                        )
                        .await
                        .map_err(SessionError::from)?;
                    owner
                        .append_session_record_owned(
                            &context,
                            attribution,
                            StreamSessionRecord::ConsumerCancelApplied(
                                StreamConsumerCancelAppliedRecord {
                                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                                    intent: intent.clone(),
                                },
                            ),
                        )
                        .await
                        .map_err(SessionError::from)?;
                    if let Some(pending) = pending {
                        owner
                            .publish_committed_cancellation(Some(&context), pending)
                            .await
                            .map_err(SessionError::from)?;
                    }
                    Ok::<_, SessionError>(())
                })
                .await?;
            drop(session_guard);
        } else {
            drop(session_guard);
            let rpc = self.rpc.clone().ok_or_else(|| {
                "foreign durable stream cancellation routing is unavailable".to_string()
            })?;
            let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                "foreign durable stream cancellation authorization is unavailable".to_string()
            })?;
            let key = self.cancellation_attachment_key(&mapping.handle, &intent)?;
            self.producer
                .remote_until_retired(
                    RoutedStreamAttachmentControl::new(rpc, mapping, auth_ctx).cancel_stream(
                        key,
                        intent.role,
                        intent.reason,
                        intent.details,
                    ),
                )
                .await
                .map_err(SessionError::from)?;
        }
        Ok(())
    }

    pub(crate) async fn cancel_unbound_rpc_inputs(
        &self,
        accepted: &[StreamSessionMappingRecord],
    ) -> Result<(), SessionError> {
        let unbound = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .values()
            .filter(|mapping| {
                mapping.role.direction() == SessionStreamRole::Input
                    && !accepted.iter().any(|accepted| {
                        accepted.role == mapping.role && accepted.handle == mapping.handle
                    })
            })
            .map(|mapping| mapping.handle.clone())
            .collect::<Vec<_>>();
        for handle in unbound {
            self.producer
                .cancel_unbound_rpc_input(&self.session_key, &handle)
                .await
                .map_err(SessionError::from)?;
        }
        Ok(())
    }
}
