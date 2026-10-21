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

use crate::worker::*;

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub(in crate::worker) fn durable_stream_consumer_auth_ctx(
        &self,
    ) -> Result<Arc<AuthCtx>, WorkerExecutorError> {
        let surface = crate::durable_host::owner_effective_surface_from_component_metadata(
            self.current_component.load().as_ref(),
            &self.owned_agent_id,
            &self.owner_context,
        )?;
        Ok(Arc::new(AuthCtx::agent_with_effective_surface(
            self.initial_worker_metadata.created_by,
            self.initial_worker_metadata.created_by_email.clone(),
            surface,
        )))
    }
    pub(in crate::worker) fn has_durable_stream_history(&self) -> bool {
        self.durable_stream_producer.has_history()
            || self.last_known_status.load().has_durable_stream_history
    }
    pub(in crate::worker) async fn reconcile_durable_stream_attachments_for(
        &self,
        data: &ResolvedWorkerData<Ctx>,
    ) -> Result<bool, WorkerExecutorError> {
        if !data.durable_stream_producer.has_history()
            && !data.last_known_status.load().has_durable_stream_history
        {
            return Ok(false);
        }
        let probe =
            DbDirectStreamAttachmentConsumerProbe::new(self.worker_service(), self.oplog_service());
        let config = &self.deps.config().durable_stream;
        let producer = self.durable_stream_producer_for(data).await?;
        if !producer.has_reconcilable_attachments().await {
            return Ok(false);
        }
        producer
            .reconcile_attachments_configured(
                Timestamp::now_utc().to_millis(),
                config.reconciliation_batch_size,
                &probe,
            )
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        Ok(producer.has_reconcilable_attachments().await)
    }
    pub(in crate::worker) async fn run_durable_stream_maintenance(
        &self,
        producer: &DurableStreamStore,
    ) -> bool {
        let mut needs_retry = false;
        if !producer.deletion_started().await {
            match self.recover_durable_stream_topologies(true, None).await {
                Ok(pending) => {
                    needs_retry |= pending;
                    if let Err(error) = self.recover_finished_durable_streaming_sessions().await {
                        if self.retire_if_shard_lost(&error) {
                            return false;
                        }
                        needs_retry = true;
                        warn!(
                            agent_id = %self.agent_id(),
                            error = %error,
                            "Failed to recover finished durable stream sessions"
                        );
                    }
                }
                Err(error) => {
                    if self.retire_if_shard_lost(&error) {
                        return false;
                    }
                    needs_retry = true;
                    warn!(
                        agent_id = %self.agent_id(),
                        error = %error,
                        "Failed to recover durable stream sessions"
                    );
                }
            }
        }
        needs_retry
    }
    pub(in crate::worker) async fn recover_durable_stream_topologies(
        &self,
        retry_remote_cancellations: bool,
        session: Option<&StreamSessionKey>,
    ) -> Result<bool, WorkerExecutorError> {
        let status = self.state_actor.try_status().await?;
        if !self.durable_stream_producer.has_history() && !status.has_durable_stream_history {
            return Ok(false);
        }
        let cache = self.durable_topology_recovery.clone();
        let oplog = self.oplog.clone();
        let service = self.worker_service();
        let owner = self.owned_agent_id.clone();
        let mode = self.agent_mode();
        let fingerprint = self.initial_worker_metadata.fingerprint;
        let current = status.oplog_idx;
        let session = session.cloned();
        let refresh = tokio::spawn(async move {
            let mut cache = cache.lock().await;
            cache
                .refresh_through(
                    current,
                    oplog.as_ref(),
                    service.as_ref(),
                    &owner,
                    mode,
                    fingerprint,
                )
                .await?;
            if cache.consumer_deleting {
                return Ok::<_, SessionError>(Vec::new());
            }
            Ok(cache
                .dirty
                .iter()
                .filter(|key| session.as_ref().is_none_or(|session| session == *key))
                .filter_map(|key| {
                    cache
                        .sessions
                        .get(key)
                        .map(|control| (key.clone(), control.clone()))
                })
                .collect::<Vec<_>>())
        });
        let sessions = self
            .tasks
            .finish_on_drop(refresh)
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        let mut recoverable = Vec::new();
        let mut first_error = None;
        for (key, metadata) in sessions {
            let session_reference = if metadata.prepared_position().is_some() {
                StreamRegistrationInvocation::Local(key.idempotency_key.clone())
            } else {
                StreamRegistrationInvocation::Remote(key.clone())
            };
            let streams = StreamSession::new(
                self.durable_stream_producer().await?,
                self.oplog.clone(),
                session_reference.clone(),
                std::iter::empty(),
            );
            streams
                .reconcile_local_cancellation_intents(None)
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
            let mut cancellations_done = !streams
                .current_control_metadata()
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
                .has_cancellation_intents();
            if retry_remote_cancellations && !cancellations_done {
                let result = streams
                    .with_rpc(self.rpc())
                    .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?)
                    .reconcile_foreign_cancellation_intents(
                        self.deps.config().durable_stream.reconciliation_interval,
                    )
                    .await;
                match result {
                    Ok(()) => cancellations_done = true,
                    Err(error) => {
                        first_error.get_or_insert(error);
                    }
                }
            }
            let topologies = metadata
                .recovery_topologies(&self.owned_agent_id, &key)
                .map_err(WorkerExecutorError::runtime)?;
            if topologies.is_empty() {
                if cancellations_done {
                    self.durable_topology_recovery
                        .lock()
                        .await
                        .acknowledge_recovery(&key, metadata.covered_through());
                }
                continue;
            }
            if matches!(session_reference, StreamRegistrationInvocation::Local(_)) {
                if let Some(status) = self
                    .oplog
                    .raw_durable_stream_session_status(&key)
                    .await
                    .status
                    .map_err(WorkerExecutorError::runtime)?
                    && let Some(error) = status.lifecycle_error
                {
                    return Err(WorkerExecutorError::runtime(error));
                }
                let prepared = self
                    .read_stream_session_record(metadata.prepared_position().ok_or_else(|| {
                        WorkerExecutorError::runtime(
                            "local durable topology has no Prepared session authority",
                        )
                    })?)
                    .await?;
                let attached = self
                    .read_stream_session_record(metadata.attached_position().ok_or_else(|| {
                        WorkerExecutorError::runtime(
                            "local durable topology has no attachment authority",
                        )
                    })?)
                    .await?;
                let (
                    StreamSessionRecord::Prepared(prepared),
                    StreamSessionRecord::Attached(attached),
                ) = (prepared, attached)
                else {
                    return Err(WorkerExecutorError::runtime(
                        "durable session index references incorrect control records",
                    ));
                };
                if prepared.session_key != key.idempotency_key
                    || attached.session_key != key.idempotency_key
                    || attached.attempt_id != prepared.attempt.attempt_id
                    || attached.epoch == 0
                    || attached.attachment_id != prepared.attempt.attachment_id
                    || !matches!(self.oplog.read(attached.pending_invocation_oplog_index).await?,
                        OplogEntry::PendingAgentInvocation { idempotency_key, .. } if idempotency_key == key.idempotency_key)
                {
                    return Err(WorkerExecutorError::runtime(
                        "durable Attached record does not exactly identify its Prepared attempt and pending invocation",
                    ));
                }
            }
            recoverable.push((
                key,
                session_reference,
                metadata.covered_through(),
                topologies,
                cancellations_done,
            ));
        }
        if recoverable.is_empty() {
            if let Some(error) = first_error {
                return Err(error.into_worker_executor_error(WorkerExecutorError::runtime));
            }
            return Ok(self.durable_topology_recovery.lock().await.needs_recovery());
        }
        let producer = self.durable_stream_producer().await?;
        let auth_ctx = self.durable_stream_consumer_auth_ctx()?;
        for (session_key, session_reference, covered_through, topologies, cancellations_done) in
            recoverable
        {
            let mut succeeded = cancellations_done;
            for topology in topologies {
                let attachment = topology.attachment_key;
                let mapping = topology.mapping;
                let control = RoutedStreamAttachmentControl::new(
                    self.rpc(),
                    mapping.clone(),
                    auth_ctx.clone(),
                );
                let streams = StreamSession::new(
                    producer.clone(),
                    self.oplog.clone(),
                    session_reference.clone(),
                    std::iter::empty(),
                )
                .with_consumer_invocation(attachment.consumer_invocation.clone())
                .with_rpc(self.rpc())
                .with_consumer_journal(self.durable_stream_consumer_journal())
                .with_auth_ctx(auth_ctx.clone());
                if let Err(error) = streams.require_local_session_attachment(&attachment).await {
                    succeeded = false;
                    first_error.get_or_insert(error);
                    continue;
                }
                if let Err(error) = streams
                    .activate_forwarded_mapping(
                        attachment,
                        mapping,
                        Arc::new(control),
                        Timestamp::now_utc().to_millis(),
                    )
                    .await
                {
                    succeeded = false;
                    first_error.get_or_insert(error);
                }
            }
            if succeeded {
                self.durable_topology_recovery
                    .lock()
                    .await
                    .acknowledge_recovery(&session_key, covered_through);
            }
        }
        if let Some(error) = first_error {
            return Err(error.into_worker_executor_error(WorkerExecutorError::runtime));
        }
        Ok(self.durable_topology_recovery.lock().await.needs_recovery())
    }
    pub(crate) async fn control_durable_stream_attachment(
        &self,
        request: StreamAttachmentControlRequest,
    ) -> Result<bool, DurableStreamRemoteError<WorkerExecutorError>> {
        if !request.is_well_formed() {
            return Err(WorkerExecutorError::invalid_request(
                "malformed durable stream attachment control request",
            )
            .into());
        }
        let key = request.operation.key();
        if let StreamAttachmentControlOperation::SourceUnavailable {
            key,
            reader_id,
            source_offset,
            consumer_read_ordinal,
        } = &request.operation
        {
            if key.consumer_environment_id != self.owned_agent_id.environment_id
                || key.consumer != self.owned_agent_id.agent_id
                || key.expected_consumer_fingerprint != self.initial_worker_metadata.fingerprint
                || request.mapping.is_some()
            {
                return Err(WorkerExecutorError::invalid_request(
                    "source-unavailable overlay does not match the consumer incarnation",
                )
                .into());
            }
            return self
                .load_durable_stream_producer()
                .await
                .map_err(|error| {
                    DurableStreamRemoteError::from_producer(error, WorkerExecutorError::runtime)
                })?
                .commit_source_unavailable_overlay(
                    None,
                    key.clone(),
                    *reader_id,
                    *source_offset,
                    *consumer_read_ordinal,
                )
                .await
                .map_err(|error| {
                    DurableStreamRemoteError::from_producer(error, WorkerExecutorError::runtime)
                });
        }
        if key.producer_environment_id != self.owned_agent_id.environment_id
            || key.producer != self.owned_agent_id.agent_id
            || key.expected_producer_fingerprint != self.initial_worker_metadata.fingerprint
        {
            return Err(WorkerExecutorError::invalid_request(
                "durable stream attachment does not match the producer incarnation",
            )
            .into());
        }
        let mapping = request.mapping.as_ref().ok_or_else(|| {
            WorkerExecutorError::invalid_request(
                "routed durable stream attachment control requires the exact session mapping",
            )
        })?;
        if mapping.handle.stream_id != key.stream_id
            || mapping.handle.producer_environment_id != key.producer_environment_id
            || mapping.handle.producer != key.producer
            || mapping.handle.expected_producer_fingerprint != key.expected_producer_fingerprint
        {
            return Err(WorkerExecutorError::invalid_request(
                "durable stream attachment mapping does not match the producer key",
            )
            .into());
        }
        let producer = self.load_durable_stream_producer().await.map_err(|error| {
            DurableStreamRemoteError::from_producer(error, WorkerExecutorError::runtime)
        })?;
        producer
            .validate_handle(&mapping.handle)
            .await
            .map_err(|error| {
                DurableStreamRemoteError::from_producer(error, WorkerExecutorError::invalid_request)
            })?;
        let probe =
            DbDirectStreamAttachmentConsumerProbe::new(self.worker_service(), self.oplog_service());
        let consumer_status = if let StreamAttachmentControlOperation::Cancel {
            role,
            reason,
            details,
            ..
        } = &request.operation
        {
            let intent = golem_common::model::durable_stream::StreamConsumerCancelIntentRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: StreamRegistrationInvocation::Remote(key.session_key.clone()),
                consumer_invocation: key.consumer_invocation.idempotency_key.clone(),
                source: StreamRecordReference::Foreign(mapping.handle.clone()),
                epoch: key.epoch,
                role: *role,
                reason: *reason,
                details: details.clone(),
            };
            probe
                .committed_cancellation_status(key, mapping, &intent)
                .await
        } else {
            probe.status_exact(key, Some(mapping)).await
        }
        .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        let authorized = match &request.operation {
            StreamAttachmentControlOperation::Prepare { .. } => matches!(
                consumer_status,
                ConsumerAttachmentStatus::Prepared | ConsumerAttachmentStatus::Active
            ),
            StreamAttachmentControlOperation::Activate { .. }
            | StreamAttachmentControlOperation::Detach { .. }
            | StreamAttachmentControlOperation::Cancel { .. } => {
                consumer_status == ConsumerAttachmentStatus::Active
            }
            StreamAttachmentControlOperation::Finalize { reason, .. } => {
                *reason == StreamAttachmentFinalizationReason::ConsumerDeleted
                    && consumer_status == ConsumerAttachmentStatus::Deleting
            }
            StreamAttachmentControlOperation::SourceUnavailable { .. } => {
                unreachable!("source-unavailable controls return before producer authorization")
            }
        };
        if !authorized {
            return Err(WorkerExecutorError::invalid_request(format!(
                "consumer durable topology does not authorize this attachment transition: {consumer_status:?}"
            )).into());
        }
        let producer_now_millis = Timestamp::now_utc().to_millis();
        let replayed = match request.operation {
            StreamAttachmentControlOperation::Prepare { key, .. } => producer
                .prepare_attachment(key, producer_now_millis)
                .await
                .map(|outcome| outcome.replayed),
            StreamAttachmentControlOperation::Activate { key, .. } => producer
                .activate_attachment(key, producer_now_millis)
                .await
                .map(|outcome| outcome.replayed),
            StreamAttachmentControlOperation::Detach { key } => {
                producer.detach_attachment(&key).await.map(|_| false)
            }
            StreamAttachmentControlOperation::Cancel {
                key,
                role,
                reason,
                details,
            } => {
                let role_matches = match mapping.role.direction() {
                    SessionStreamRole::Input => matches!(
                        role,
                        golem_common::model::durable_stream::StreamCancelRole::InputProducer
                            | golem_common::model::durable_stream::StreamCancelRole::InputConsumer
                    ),
                    SessionStreamRole::Output => matches!(
                        role,
                        golem_common::model::durable_stream::StreamCancelRole::OutputProducer
                            | golem_common::model::durable_stream::StreamCancelRole::OutputConsumer
                    ),
                    SessionStreamRole::ToolStdin
                    | SessionStreamRole::ToolStdout
                    | SessionStreamRole::ToolStderr => unreachable!(),
                };
                if !role_matches {
                    return Err(WorkerExecutorError::invalid_request(
                        "durable stream cancellation role does not match its session mapping",
                    )
                    .into());
                }
                let source = &mapping.handle.source_invocation;
                if matches!(
                    role,
                    golem_common::model::durable_stream::StreamCancelRole::OutputConsumer
                        | golem_common::model::durable_stream::StreamCancelRole::InputConsumer
                ) && source.callee_environment_id == self.owned_agent_id.environment_id
                    && source.callee == self.owned_agent_id.agent_id
                    && source.callee_fingerprint == self.initial_worker_metadata.fingerprint
                    && producer
                        .registered_stream_role(&mapping.handle)
                        .await
                        .map_err(|error| {
                            DurableStreamRemoteError::from_producer(
                                error,
                                WorkerExecutorError::runtime,
                            )
                        })?
                        .direction()
                        == SessionStreamRole::Output
                    && let Some(prepared) = self
                        .prepared_stream_session(&source.idempotency_key)
                        .await?
                    && stream_output_consumer_is_observer(&prepared.attempt, &key)?
                {
                    return producer
                        .finalize_attachment(
                            key,
                            StreamAttachmentFinalizationReason::ConsumerFinalized,
                            producer_now_millis,
                        )
                        .await
                        .map(|outcome| outcome.replayed)
                        .map_err(|error| {
                            DurableStreamRemoteError::from_producer(
                                error,
                                WorkerExecutorError::runtime,
                            )
                        });
                }
                producer
                    .cancel_open(None, key.stream_id, role, reason, details)
                    .await
                    .map(|_| false)
            }
            StreamAttachmentControlOperation::Finalize { key, reason, .. } => producer
                .finalize_attachment(key, reason, producer_now_millis)
                .await
                .map(|outcome| outcome.replayed),
            StreamAttachmentControlOperation::SourceUnavailable { .. } => {
                unreachable!("source-unavailable controls return before producer execution")
            }
        }
        .map_err(|error| {
            DurableStreamRemoteError::from_producer(error, WorkerExecutorError::runtime)
        })?;
        Ok(replayed)
    }
    pub(crate) async fn read_durable_stream_by_handle(
        &self,
        request: golem_common::model::durable_stream::StreamHandleReadRequest,
    ) -> Result<Vec<u8>, DurableStreamRemoteError<WorkerExecutorError>> {
        let handle = &request.handle;
        if handle.producer_environment_id != self.owned_agent_id.environment_id
            || handle.producer != self.owned_agent_id.agent_id
            || handle.expected_producer_fingerprint != self.initial_worker_metadata.fingerprint
        {
            return Err(WorkerExecutorError::invalid_request(
                "stream handle does not match the producer incarnation",
            )
            .into());
        }
        let result = self
            .load_durable_stream_producer()
            .await
            .map_err(|error| {
                DurableStreamRemoteError::from_producer(error, WorkerExecutorError::runtime)
            })?
            .read_by_handle(request)
            .await
            .map_err(|error| {
                DurableStreamRemoteError::from_producer(error, WorkerExecutorError::invalid_request)
            })?;
        golem_common::serialization::serialize(&result)
            .map_err(WorkerExecutorError::runtime)
            .map_err(Into::into)
    }
    #[tracing::instrument(name = "durable_stream.read_request", level = "debug", skip_all)]
    pub(crate) async fn read_durable_stream_segment(
        &self,
        request: AttachedStreamSegmentRequest,
    ) -> Result<Vec<CommittedProducerStreamEvent>, DurableStreamRemoteError<WorkerExecutorError>>
    {
        if !request.is_well_formed() {
            return Err(WorkerExecutorError::invalid_request(
                "malformed durable stream segment request",
            )
            .into());
        }
        let key = &request.attachment;
        if key.producer_environment_id != self.owned_agent_id.environment_id
            || key.producer != self.owned_agent_id.agent_id
            || key.expected_producer_fingerprint != self.initial_worker_metadata.fingerprint
        {
            return Err(WorkerExecutorError::invalid_request(
                "durable stream segment does not match the producer incarnation",
            )
            .into());
        }
        let producer = self.load_durable_stream_producer().await.map_err(|error| {
            DurableStreamRemoteError::from_producer(error, WorkerExecutorError::runtime)
        })?;
        producer
            .validate_handle(&request.mapping.handle)
            .await
            .map_err(|error| {
                DurableStreamRemoteError::from_producer(error, WorkerExecutorError::invalid_request)
            })?;
        let probe =
            DbDirectStreamAttachmentConsumerProbe::new(self.worker_service(), self.oplog_service());
        if probe
            .status_exact(key, Some(&request.mapping))
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
            != ConsumerAttachmentStatus::Active
        {
            return Err(WorkerExecutorError::invalid_request(
                "consumer durable topology does not authorize this stream read",
            )
            .into());
        }
        let read = || async {
            if request.wait_for_events {
                producer
                    .wait_for_attached_segment(
                        key,
                        &request.mapping.handle,
                        Timestamp::now_utc().to_millis(),
                        request.after,
                    )
                    .await
            } else {
                producer
                    .read_attached_segment(
                        key,
                        &request.mapping.handle,
                        Timestamp::now_utc().to_millis(),
                        request.after,
                        request.through,
                    )
                    .await
            }
        };
        let mut events = read().await;
        if matches!(events, Err(StreamStoreError::InvalidAttachmentState)) {
            // Consumer activation can commit before its producer RPC arrives. Repair only this
            // gap from the exact authority checked above; healthy reads never enter the writer.
            producer
                .activate_attachment(key.clone(), Timestamp::now_utc().to_millis())
                .await
                .map_err(|error| {
                    DurableStreamRemoteError::from_producer(error, WorkerExecutorError::runtime)
                })?;
            events = read().await;
        }
        let events = events.map_err(|error| {
            DurableStreamRemoteError::from_producer(error, WorkerExecutorError::runtime)
        })?;
        if request.wait_for_events
            && probe
                .status_exact(key, Some(&request.mapping))
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?
                != ConsumerAttachmentStatus::Active
        {
            return Err(WorkerExecutorError::invalid_request(
                "consumer durable topology no longer authorizes this stream read",
            )
            .into());
        }
        Ok(events)
    }
    pub(crate) fn start_durable_stream_attachment_reconciler(this: &Arc<Self>) {
        let worker = Arc::downgrade(this);
        let slot = this.durable_stream_producer.clone();
        let shutdown = this.durable_stream_attachment_reconciler.shutdown.clone();
        // This maintenance task must not pin an idle worker; its active passes own their worker
        // separately. The root still belongs to the shared oplog generation.
        let owner = this.oplog.task_owner().cloned().unwrap_or_default();
        let scope = tasks::TaskScope::default();
        if scope.bind(&owner).is_err() {
            return;
        }
        let interval_duration = this.deps.config().durable_stream.reconciliation_interval;
        let handle = tokio::spawn(async move {
            scope
                .run(async move {
                    tokio::task::yield_now().await;
                    loop {
                        let slot_changed = slot.changed().notified();
                        tokio::pin!(slot_changed);
                        slot_changed.as_mut().enable();
                        if shutdown.is_cancelled() || slot.is_retired() {
                            break;
                        }
                        let Some(worker) = worker.upgrade() else {
                            break;
                        };
                        if worker.cache_retirement_in_progress() {
                            drop(worker);
                            if !wait_for_durable_stream_maintenance(&shutdown, slot_changed, None)
                                .await
                            {
                                break;
                            }
                            continue;
                        }
                        if !worker.has_durable_stream_history() {
                            drop(worker);
                            if !wait_for_durable_stream_maintenance(&shutdown, slot_changed, None)
                                .await
                            {
                                break;
                            }
                            continue;
                        }
                        let producer = match worker.durable_stream_producer().await {
                            Ok(producer) => producer,
                            Err(error) => {
                                if slot.is_retired() {
                                    break;
                                }
                                if let Some(reason) = RetirementReason::of_error(&error) {
                                    worker.spawn_lost_shard_retirement(reason);
                                    break;
                                }
                                warn!(
                                    agent_id = %worker.agent_id(),
                                    error = %error,
                                    "Failed to load durable stream maintenance state"
                                );
                                drop(worker);
                                if !wait_for_durable_stream_retry(&shutdown, interval_duration)
                                    .await
                                {
                                    break;
                                }
                                continue;
                            }
                        };
                        let changed = producer.session_records_changed().notified();
                        tokio::pin!(changed);
                        changed.as_mut().enable();

                        // Remote attachment control may acquire another cold worker that refers back
                        // to us. Only run this after local construction has published readiness.
                        let needs_retry = worker.run_durable_stream_maintenance(&producer).await;
                        if let Some(reason) = worker.lost_shard_reason() {
                            worker.spawn_lost_shard_retirement(reason);
                            break;
                        }
                        drop(worker);

                        if !wait_for_durable_stream_maintenance(
                            &shutdown,
                            async {
                                tokio::select! {
                                    _ = changed => {},
                                    _ = slot_changed => {},
                                }
                            },
                            needs_retry.then_some(interval_duration),
                        )
                        .await
                        {
                            break;
                        }
                    }
                })
                .await;
        });
        this.durable_stream_attachment_reconciler.start(handle);
    }
    #[cfg(feature = "test-utils")]
    pub async fn wait_for_durable_stream_attachment_reconciler(&self) {
        self.durable_stream_attachment_reconciler.wait().await;
    }
}

impl DurableTopologyRecoveryCache {
    pub(in crate::worker) fn needs_recovery(&self) -> bool {
        !self.consumer_deleting && !self.dirty.is_empty()
    }

    pub(in crate::worker) fn acknowledge_recovery(
        &mut self,
        key: &StreamSessionKey,
        covered_through: OplogIndex,
    ) {
        if self
            .sessions
            .get(key)
            .is_some_and(|control| control.covered_through() == covered_through)
        {
            self.dirty.remove(key);
        }
    }

    pub(in crate::worker) async fn reload(
        &mut self,
        service: &dyn WorkerService,
        owner: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
    ) -> Result<(), SessionError> {
        let metadata = service
            .lookup_durable_stream_recovery_metadata(owner, mode, fingerprint)
            .await?;
        *self = Self {
            initialized: true,
            covered_through: metadata.covered_through,
            consumer_deleting: metadata.consumer_deleting.is_some_and(|record| {
                record.consumer_environment_id == owner.environment_id
                    && record.consumer == owner.agent_id
                    && record.consumer_fingerprint == fingerprint
            }),
            ..Default::default()
        };
        for (key, control) in metadata.sessions {
            if control.needs_recovery(owner, &key) {
                self.dirty.insert(key.clone());
                self.sessions.insert(key, control);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn refresh(
        &mut self,
        oplog: &dyn Oplog,
        service: &dyn WorkerService,
        owner: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
    ) -> Result<(), SessionError> {
        let current = oplog.current_oplog_index().await;
        self.refresh_through(current, oplog, service, owner, mode, fingerprint)
            .await
    }

    pub(crate) async fn refresh_through(
        &mut self,
        current: OplogIndex,
        oplog: &dyn Oplog,
        service: &dyn WorkerService,
        owner: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
    ) -> Result<(), SessionError> {
        if !self.initialized {
            self.reload(service, owner, mode, fingerprint).await?;
        }
        'suffix: while self.covered_through < current {
            let count = (current.as_u64() - self.covered_through.as_u64()).min(1024);
            let entries = oplog.read_exact(self.covered_through.next(), count).await?;
            for (index, entry) in &entries {
                let OplogEntry::StreamSession { record, .. } = entry else {
                    continue;
                };
                let record = oplog.download_payload(record.clone()).await?;
                if !record.has_supported_format() {
                    return Err(
                        "unsupported or malformed durable Stream Session record version".into(),
                    );
                }
                if matches!(record, StreamSessionRecord::ForkCut(_)) {
                    self.reload(service, owner, mode, fingerprint).await?;
                    if self.covered_through < *index {
                        return Err("fork marker is not committed to the session index".into());
                    }
                    continue 'suffix;
                }
                if let StreamSessionRecord::ConsumerDeleting(record) = &record
                    && record.consumer_environment_id == owner.environment_id
                    && record.consumer == owner.agent_id
                    && record.consumer_fingerprint == fingerprint
                {
                    self.consumer_deleting = true;
                }
                if !matches!(
                    record,
                    StreamSessionRecord::Prepared(_)
                        | StreamSessionRecord::Attached(_)
                        | StreamSessionRecord::ResumeAttempt(_)
                        | StreamSessionRecord::Detached(_)
                        | StreamSessionRecord::TopologyPrepared(_)
                        | StreamSessionRecord::TopologyActivated(_)
                        | StreamSessionRecord::AttachmentFinalized(_)
                        | StreamSessionRecord::ConsumerTerminal(_)
                        | StreamSessionRecord::SourceUnavailable(_)
                        | StreamSessionRecord::Finished(_)
                        | StreamSessionRecord::ConsumerCancelIntent(_)
                        | StreamSessionRecord::ConsumerCancelApplied(_)
                        | StreamSessionRecord::CancelRequested(_)
                        | StreamSessionRecord::Tombstoned(_)
                        | StreamSessionRecord::Mapping(_)
                        | StreamSessionRecord::InvocationResult(_)
                        | StreamSessionRecord::ConsumerItemValue(_)
                ) {
                    continue;
                }
                let Some(key) = stream_session_record_key(
                    &record,
                    owner.environment_id,
                    &owner.agent_id,
                    fingerprint,
                ) else {
                    continue;
                };
                if !self.sessions.contains_key(&key) {
                    let control = service
                        .lookup_durable_stream_control_metadata(owner, mode, fingerprint, &key)
                        .await?;
                    self.sessions.insert(key.clone(), control);
                }
                let control = self.sessions.get_mut(&key).unwrap();
                if *index > control.covered_through() {
                    control.apply(
                        *index,
                        &key,
                        &record,
                        owner.environment_id,
                        &owner.agent_id,
                        fingerprint,
                    );
                }
                self.dirty.insert(key.clone());
            }
            self.covered_through = *entries
                .keys()
                .next_back()
                .ok_or("empty topology recovery suffix")?;
            self.sessions
                .retain(|key, control| control.needs_recovery(owner, key));
            self.dirty.retain(|key| self.sessions.contains_key(key));
        }
        Ok(())
    }
}

impl DurableStreamAttachmentReconciler {
    pub(in crate::worker) fn new(executor_shutdown: CancellationToken) -> Self {
        Self {
            shutdown: executor_shutdown.child_token(),
            handle: Mutex::new(None),
        }
    }

    pub(in crate::worker) fn start(&self, handle: JoinHandle<()>) {
        let mut stored = self
            .handle
            .try_lock()
            .expect("durable stream reconciler started while stopping");
        assert!(
            stored.is_none(),
            "durable stream reconciler already started"
        );
        *stored = Some(handle);
    }

    pub(in crate::worker) async fn stop(&self) {
        self.shutdown.cancel();
        self.wait().await;
    }

    pub(in crate::worker) async fn wait(&self) {
        let mut stored = self.handle.lock().await;
        if let Some(handle) = stored.as_mut() {
            let _ = handle.await;
        }
        stored.take();
    }
}

impl Drop for DurableStreamAttachmentReconciler {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.handle.get_mut().take();
    }
}

pub(in crate::worker) async fn wait_for_durable_stream_maintenance<F>(
    shutdown: &CancellationToken,
    changed: F,
    interval: Option<Duration>,
) -> bool
where
    F: std::future::Future<Output = ()>,
{
    if let Some(interval) = interval {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => false,
            _ = changed => true,
            _ = tokio::time::sleep(interval) => true,
        }
    } else {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => false,
            _ = changed => true,
        }
    }
}

pub(in crate::worker) async fn wait_for_durable_stream_retry(
    shutdown: &CancellationToken,
    interval: Duration,
) -> bool {
    wait_for_durable_stream_maintenance(shutdown, std::future::pending(), Some(interval)).await
}
