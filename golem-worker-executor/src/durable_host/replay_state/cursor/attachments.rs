use super::*;

impl ReplayState {
    /// Finds the original logical input and its retained boundary's native read attempt.
    pub(crate) fn tool_attachment_observation(
        &self,
        attachment: golem_common::model::oplog::attachment::ToolAttachmentId,
        invocation: &golem_common::model::IdempotencyKey,
        ordinal: u64,
    ) -> Option<(OplogIndex, Option<(OplogIndex, u64)>)> {
        let index = self.cursor.tool_attachment_reconstruction.lock().unwrap();
        let (original, boundary) = index
            .logical_observations
            .get(&(attachment, invocation.clone(), ordinal))
            .copied()?;
        Some((
            original,
            boundary.map(|boundary| {
                (
                    boundary,
                    index.history.observations[&boundary].read_attempt_ordinal,
                )
            }),
        ))
    }

    /// Downloads the original body while retaining the requested boundary's observation metadata.
    pub(crate) async fn hydrate_tool_attachment_observation(
        &self,
        mut source: OplogIndex,
        max_bytes: u64,
    ) -> Result<
        (
            OplogIndex,
            golem_common::model::oplog::attachment::ToolAttachmentReadObservation,
            Option<Vec<u8>>,
        ),
        WorkerExecutorError,
    > {
        let requested_index = source;
        let (requested_read, read) = {
            let index = self.cursor.tool_attachment_reconstruction.lock().unwrap();
            let history = &index.history;
            let read = history
                .observations
                .get(&source)
                .ok_or_else(|| WorkerExecutorError::runtime("missing attachment observation"))?;
            history.validate_read(source, read).map_err(|error| {
                WorkerExecutorError::runtime(format!("invalid attachment observation: {error:?}"))
            })?;
            while let Some(original) = history.observations[&source].reobserves {
                source = original;
            }
            use golem_common::model::oplog::attachment::ToolAttachmentReadOutcome;
            let length = match history.observations[&source].outcome {
                ToolAttachmentReadOutcome::TypedChunk(end) => history.writes[&end.0]
                    .accepted_length
                    .expect("validated acceptance"),
                ToolAttachmentReadOutcome::RawBytes { length, .. } => length,
                _ => 0,
            };
            if length > max_bytes {
                return Err(WorkerExecutorError::runtime(
                    "recorded attachment observation exceeds its hydration capacity",
                ));
            }
            (read.clone(), history.observations[&source].clone())
        };
        let entries = self.cursor.oplog.read_exact(source, 1).await?;
        let Some(OplogEntry::ToolAttachmentRead {
            observation,
            payload,
            ..
        }) = entries.get(&source)
        else {
            return Err(WorkerExecutorError::runtime(
                "missing original attachment entry",
            ));
        };
        if observation.as_ref() != &read {
            return Err(WorkerExecutorError::runtime(
                "changed attachment observation",
            ));
        }
        let bytes = match payload {
            Some(payload) => Some(
                self.cursor
                    .oplog
                    .download_payload(payload.clone())
                    .await
                    .map_err(WorkerExecutorError::runtime)?,
            ),
            None => None,
        };
        // Re-read acceptance after the download: a Jump can remove it while hydration awaits.
        let current = self.cursor.tool_attachment_reconstruction.lock().unwrap();
        if current.history.observations.get(&source) != Some(&read)
            || current.history.observations.get(&requested_index) != Some(&requested_read)
        {
            return Err(WorkerExecutorError::runtime(
                "attachment observation outside replay target",
            ));
        }
        current
            .history
            .validate_read_payload(source, &read, bytes.as_deref())
            .map_err(|error| {
                WorkerExecutorError::runtime(format!("invalid attachment body: {error:?}"))
            })?;
        if let golem_common::model::oplog::attachment::ToolAttachmentReadOutcome::TypedChunk(end) =
            read.outcome
        {
            let write = &current.history.writes[&end.0];
            let request = current.requests.get(&write.start).ok_or_else(|| {
                WorkerExecutorError::runtime("missing accepted attachment write request")
            })?;
            if !request.matches_input(bytes.as_deref().expect("validated typed payload")) {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "attachment body matching the accepted write digest",
                    format!("attachment observation at {source} differs from its accepted write"),
                ));
            }
        }
        drop(current);
        let bytes = bytes.map(|bytes| bytes.into_boxed_slice().into_vec());
        Ok((source, requested_read, bytes))
    }

    pub(crate) fn historical_tool_attachment_write(
        &self,
        request: &golem_common::model::oplog::payload::tool_attachment::ToolAttachmentWriteRequest,
    ) -> Result<Option<super::super::HistoricalAttachmentWrite>, WorkerExecutorError> {
        use golem_common::model::oplog::attachment::{
            AcceptedAttachmentWriteEnd, ToolAttachmentReadOutcome,
        };

        let index = self.cursor.tool_attachment_reconstruction.lock().unwrap();
        let Some(start) = index
            .accepted_by_operation
            .get(&(request.attachment, request.operation_ordinal))
        else {
            return Ok(None);
        };
        let start = start.ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                "one retained acceptance for an attachment operation",
                "multiple accepted writes share an attachment operation ordinal",
            )
        })?;
        if index.requests.get(&start) != Some(request) {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "regenerated attachment argument matching its retained request",
                format!("attachment write differs from Start at {start}"),
            ));
        }
        let end = AcceptedAttachmentWriteEnd(index.accepted[&start]);
        let original_typed_read = index
            .source_observations
            .get(&end.0)
            .into_iter()
            .flatten()
            .find(|read| {
                index.history.observations[read].outcome
                    == ToolAttachmentReadOutcome::TypedChunk(end)
            })
            .copied();
        Ok(Some(super::super::HistoricalAttachmentWrite {
            start,
            end,
            original_typed_read,
        }))
    }

    pub(crate) fn deferred_tool_attachment_body(
        &self,
        read: OplogIndex,
        expected: golem_common::model::oplog::attachment::AcceptedAttachmentWriteEnd,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<u8>>> + Send + 'static {
        let cursor = std::sync::Arc::downgrade(&self.cursor);
        async move {
            let replay = Self {
                cursor: cursor
                    .upgrade()
                    .ok_or_else(|| anyhow::anyhow!("attachment replay owner no longer exists"))?,
            };
            let max_bytes = replay
                .cursor
                .tool_attachment_reconstruction
                .lock()
                .unwrap()
                .history
                .writes
                .get(&expected.0)
                .and_then(|write| write.accepted_length)
                .ok_or_else(|| anyhow::anyhow!("attachment backing has no accepted length"))?;
            let (_, observation, bytes) = replay
                .hydrate_tool_attachment_observation(read, max_bytes)
                .await?;
            anyhow::ensure!(
                observation.outcome == golem_common::model::oplog::attachment::ToolAttachmentReadOutcome::TypedChunk(expected),
                "attachment backing changed its accepted write"
            );
            bytes.ok_or_else(|| anyhow::anyhow!("attachment backing has no body"))
        }
    }

    pub(crate) fn accepted_tool_attachment_write_end(
        &self,
        start: OplogIndex,
    ) -> Result<
        golem_common::model::oplog::attachment::AcceptedAttachmentWriteEnd,
        WorkerExecutorError,
    > {
        self.cursor
            .tool_attachment_reconstruction
            .lock()
            .unwrap()
            .accepted
            .get(&start)
            .copied()
            .map(golem_common::model::oplog::attachment::AcceptedAttachmentWriteEnd)
            .ok_or_else(|| {
                WorkerExecutorError::runtime(format!(
                    "tool attachment write at {start} has no retained acceptance"
                ))
            })
    }

    pub(crate) fn selected_tool_attachment_terminal_end(
        &self,
        start: OplogIndex,
        attachment: golem_common::model::oplog::attachment::ToolAttachmentId,
        terminal: &golem_common::model::oplog::attachment::ToolAttachmentTerminal,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        use golem_common::model::oplog::attachment::ToolAttachmentControl;

        let index = self.cursor.tool_attachment_reconstruction.lock().unwrap();
        index
            .history
            .controls
            .iter()
            .find_map(|(end, control)| {
                (control.retained
                    && control.producer_start == Some(start)
                    && control.attachment == attachment
                    && matches!(&control.transition,
                        ToolAttachmentControl::TerminalSelected(selected) if selected == terminal))
                .then_some(*end)
            })
            .ok_or_else(|| {
                WorkerExecutorError::runtime(format!(
                    "tool attachment terminal at {start} has no matching retained selection"
                ))
            })
    }

    pub(crate) fn has_restorable_tool_attachment_terminal(
        &self,
        attachment: golem_common::model::oplog::attachment::ToolAttachmentId,
    ) -> bool {
        let index = self.cursor.tool_attachment_reconstruction.lock().unwrap();
        index
            .controls_by_attachment
            .get(&attachment)
            .into_iter()
            .flatten()
            .any(|end| {
                matches!(
                    index.history.controls[end].transition,
                    golem_common::model::oplog::attachment::ToolAttachmentControl::TerminalSelected(
                        _
                    )
                )
            })
    }

    /// Restores a retained terminal only after its boundary is crossed. An absent terminal
    /// can be repaired only at the natural tail, after all historical producers have run.
    pub(crate) async fn await_tool_attachment_terminal(
        &self,
        attachment: golem_common::model::oplog::attachment::ToolAttachmentId,
    ) -> Result<Option<OplogIndex>, WorkerExecutorError> {
        let terminal = {
            let index = self.cursor.tool_attachment_reconstruction.lock().unwrap();
            index.history.controls.iter().find_map(|(end, control)| {
                (control.retained && control.attachment == attachment
                    && matches!(control.transition,
                        golem_common::model::oplog::attachment::ToolAttachmentControl::TerminalSelected(_)))
                    .then_some(*end)
            })
        };
        let Some(terminal) = terminal else {
            self.await_natural_tail_end(None).await?;
            return Ok(None);
        };
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            self.run_owned_cursor_op(move |state| async move {
                state
                    .with_tx(async |tx| {
                        tx.try_get_oplog_entry(|_| false).await?;
                        Ok(())
                    })
                    .await
            })
            .await?;
            if self.last_replayed_index() >= terminal {
                return Ok(Some(terminal));
            }
            progress.await;
        }
    }

    pub(crate) fn prepare_tool_attachment_range_validation(
        &self,
        source: golem_common::model::oplog::attachment::AcceptedAttachmentWriteEnd,
        bytes: &[u8],
    ) -> Result<Vec<super::super::AttachmentRangeValidation>, WorkerExecutorError> {
        use golem_common::model::oplog::attachment::ToolAttachmentReadOutcome;
        let index = self.cursor.tool_attachment_reconstruction.lock().unwrap();
        let write = index
            .history
            .writes
            .get(&source.0)
            .ok_or_else(|| WorkerExecutorError::runtime("missing accepted attachment write"))?;
        let request = index.requests.get(&write.start).ok_or_else(|| {
            WorkerExecutorError::runtime("missing accepted attachment write request")
        })?;
        if !write.retained || !request.matches_input(bytes) {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "regenerated attachment bytes matching the retained accepted write",
                format!(
                    "regenerated attachment differs from accepted write at {}",
                    source.0
                ),
            ));
        }
        let mut validations = Vec::new();
        for read_index in index
            .source_observations
            .get(&source.0)
            .into_iter()
            .flatten()
        {
            let read = &index.history.observations[read_index];
            let ToolAttachmentReadOutcome::RawBytes { offset, length, .. } = read.outcome else {
                continue;
            };
            let range = usize::try_from(offset)
                .ok()
                .zip(usize::try_from(length).ok())
                .and_then(|(offset, length)| {
                    offset
                        .checked_add(length)
                        .and_then(|end| bytes.get(offset..end))
                })
                .ok_or_else(|| {
                    WorkerExecutorError::runtime(
                        "attachment observation exceeds regenerated source",
                    )
                })?;
            validations.push(super::super::AttachmentRangeValidation {
                index: *read_index,
                observation: read.clone(),
                digest: *blake3::hash(range).as_bytes(),
            });
        }
        Ok(validations)
    }

    pub(crate) async fn validate_tool_attachment_ranges(
        &self,
        validations: Vec<super::super::AttachmentRangeValidation>,
    ) -> Result<(), WorkerExecutorError> {
        for validation in validations {
            let _slot = self
                .cursor
                .attachment_validation_slot
                .acquire()
                .await
                .expect("attachment validation slot is never closed");
            let golem_common::model::oplog::attachment::ToolAttachmentReadOutcome::RawBytes {
                length,
                ..
            } = validation.observation.outcome
            else {
                unreachable!("only raw ranges require deferred validation")
            };
            let (_, read, payload) = self
                .hydrate_tool_attachment_observation(validation.index, length)
                .await?;
            if read != validation.observation
                || payload
                    .as_deref()
                    .is_none_or(|bytes| blake3::hash(bytes).as_bytes() != &validation.digest)
            {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "recorded attachment bytes equal regenerated source range",
                    format!(
                        "attachment observation at {} differs from regenerated source",
                        validation.index
                    ),
                ));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn validate_tool_attachment_regenerated_bytes(
        &self,
        source: golem_common::model::oplog::attachment::AcceptedAttachmentWriteEnd,
        bytes: &[u8],
    ) -> Result<(), WorkerExecutorError> {
        let validations = self.prepare_tool_attachment_range_validation(source, bytes)?;
        self.validate_tool_attachment_ranges(validations).await
    }

    /// Prefetched terminals identify outcomes, but do not authorize their resident effects.
    pub(crate) async fn await_tool_attachment_acceptance(
        &self,
        start: OplogIndex,
    ) -> Result<
        golem_common::model::oplog::attachment::AcceptedAttachmentWriteEnd,
        WorkerExecutorError,
    > {
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            let end = self.accepted_tool_attachment_write_end(start)?;
            if self.last_replayed_index() >= end.0 {
                return Ok(end);
            }
            self.run_owned_cursor_op(move |state| async move {
                state
                    .with_tx(async |tx| {
                        tx.try_get_oplog_entry(|_| false).await?;
                        Ok(())
                    })
                    .await
            })
            .await?;
            if self.last_replayed_index() >= end.0 {
                return Ok(end);
            }
            progress.await;
        }
    }

    pub(crate) fn register_tool_attachment(
        &self,
        identity: golem_common::model::oplog::attachment::ToolAttachmentId,
        controller: &crate::durable_host::tool::attachment::AttachmentController,
    ) -> Result<(), WorkerExecutorError> {
        let mut attachments = self.cursor.tool_attachment_reconstruction.lock().unwrap();
        attachments
            .resident
            .insert(identity, controller.downgrade());
        attachments.refresh_controller(identity)
    }

    pub(crate) fn has_open_tool_attachments(&self) -> bool {
        let attachments = self.cursor.tool_attachment_reconstruction.lock().unwrap();
        attachments
            .resident
            .values()
            .filter_map(|controller| controller.upgrade())
            .any(|controller| controller.has_open_endpoints())
            || attachments
                .pending_writes
                .values()
                .any(|writes| !writes.is_empty())
    }

    /// Marks an accepted output write's regenerated arguments validated and its resident
    /// output restored. This must precede the write's acknowledgment delivery gate.
    pub fn complete_tool_attachment_reconstruction(
        &self,
        start: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        let mut attachments = self.cursor.tool_attachment_reconstruction.lock().unwrap();
        if let Some(end) = attachments.accepted.get(&start).copied() {
            attachments.completed.insert(start);
            let identity = attachments.history.writes[&end].attachment;
            if let Some(pending) = attachments.pending_writes.get_mut(&identity) {
                pending.remove(&end);
            }
            attachments.refresh_controller(identity)?;
        }
        drop(attachments);
        self.cursor.progress.notify_waiters();
        Ok(())
    }

    /// Waits for retained accepted output writes beneath the root, including unclaimed
    /// Starts and descendants through durable scopes and nested entity bodies.
    pub async fn await_tool_attachment_reconstruction(
        &self,
        root: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            if let Some(failure) = self.cursor.delivery_failure.lock().unwrap().clone() {
                return Err(failure);
            }
            if let Some(failure) = self.cursor.owner_tool_operations.selected_owner_failure() {
                return Err(historical_reconstruction_owner_failure(failure));
            }
            if !self
                .cursor
                .tool_attachment_reconstruction
                .lock()
                .unwrap()
                .pending_under(root)
            {
                return Ok(());
            }
            tokio::select! {
                biased;
                failure = self.cursor.owner_tool_operations.wait_for_owner_failure() => {
                    return Err(historical_reconstruction_owner_failure(failure));
                }
                _ = progress => {}
            }
        }
    }
}
