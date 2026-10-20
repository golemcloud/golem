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

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// Best-effort mid-invocation clean status checkpoint. Called from `end_durable_function` after
    /// it commits, so the worker's `last_known_status` reflects the committed tip. Writes a
    /// checkpoint only when we are at a structurally clean boundary. The actual write is further
    /// throttled by the checkpointer.
    pub(in crate::durable_host) async fn maybe_mid_invocation_checkpoint(&self) {
        if !self.state.at_clean_checkpoint_boundary() {
            return;
        }
        self.public_state
            .worker()
            .checkpoint_status_mid_invocation()
            .await;
    }

    pub async fn finalize_pending_snapshot_update(
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        let pending_update = store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .state
            .pending_update
            .lock()
            .await
            .take();
        match pending_update {
            Some(HydratedUpdate {
                reference,
                description: description @ UpdateDescription::SnapshotBased { .. },
            }) => {
                let target_revision = *description.target_revision();

                debug!("Finalizing snapshot update to revision {target_revision}");

                if let Err(result) =
                    Self::load_manual_update_snapshot(instance, store, description).await
                {
                    let action = store.as_context().data().durable_ctx().start_action(
                        Some(&reference),
                        start_outcome::RawStartError::ManualLoad(&result),
                    );
                    return Self::perform_start_action(store, action).await;
                }
                let component_metadata = store.as_context().data().component_metadata().clone();
                let agent_type_provision_config = store
                    .as_context()
                    .data()
                    .agent_type_provision_config()
                    .cloned();

                store
                    .as_context_mut()
                    .data_mut()
                    .on_worker_update_succeeded(
                        target_revision,
                        component_metadata.component_size,
                        HashSet::from_iter(
                            agent_type_provision_config
                                .into_iter()
                                .flat_map(|c| c.plugins)
                                .map(|installation| installation.environment_plugin_grant_id),
                        ),
                        start_outcome::success_details_of(&reference),
                    )
                    .await?;
                Ok(None)
            }
            _ => Err(WorkerExecutorError::runtime(
                "`finalize_pending_snapshot_update` can only be called with a snapshot update description",
            )),
        }
    }

    /// Loads the application snapshot of the pending snapshot-based update `description`. An
    /// interrupted load gives `ManualLoadResult::Interrupted` with its interrupt kind, and an
    /// exited guest gives `ManualLoadResult::Exited`.
    async fn load_manual_update_snapshot(
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        description: UpdateDescription,
    ) -> Result<(), start_outcome::ManualLoadResult> {
        use start_outcome::ManualLoadResult;
        let (data, mime_type) = match store
            .as_context_mut()
            .data_mut()
            .get_public_state()
            .oplog()
            .get_upload_description_payload(description)
            .await
        {
            Ok(Some(payload)) => payload,
            Ok(None) => {
                return Err(ManualLoadResult::Failed(
                    "Failed to find snapshot data for update".to_string(),
                ));
            }
            Err(error) => return Err(ManualLoadResult::Failed(error)),
        };
        let component_metadata = store
            .as_context()
            .data()
            .component_metadata()
            .metadata
            .clone();

        let idempotency_key = IdempotencyKey::fresh();
        store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .set_current_idempotency_key(idempotency_key.clone())
            .await;

        let load_snapshot_invocation = AgentInvocation::LoadSnapshot {
            idempotency_key,
            snapshot: RawSnapshotData { data, mime_type },
        };
        let agent_id = store.as_context().data().parsed_agent_id();
        let lowered = lower_invocation(
            load_snapshot_invocation,
            &component_metadata,
            agent_id.as_ref(),
        )
        .map_err(|err| {
            ManualLoadResult::Failed(format!(
                "Manual update failed to lower load-snapshot invocation: {err}"
            ))
        })?;

        let invocation_context = InvocationContextStack::fresh();
        let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
        store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .set_current_invocation_context(invocation_context)
            .await
            .map_err(|err| {
                ManualLoadResult::Failed(format!(
                    "Manual update failed to install invocation context: {err}"
                ))
            })?;

        store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .begin_call_snapshotting_function();

        let load_result =
            invoke_observed_and_traced(lowered, store, instance, InvocationMode::Replay).await;
        store.as_context_mut().data().set_suspended();

        store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .end_call_snapshotting_function_if_active();

        local_span_ids
            .into_iter()
            .chain(inherited_span_ids)
            .for_each(|span_id| {
                let _ = store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .remove_span(&span_id);
            });

        match load_result {
            Err(error) => Err(ManualLoadResult::Failed(format!(
                "Manual update failed to load snapshot: {error}"
            ))),
            Ok(InvokeResult::Failed { error, .. }) => {
                let stderr = store
                    .as_context()
                    .data()
                    .get_public_state()
                    .event_service()
                    .get_last_invocation_errors();
                let error = error.to_string(&stderr);
                Err(ManualLoadResult::Failed(format!(
                    "Manual update failed to load snapshot: {error}"
                )))
            }
            Ok(InvokeResult::Succeeded { result, .. }) => match *result {
                AgentInvocationResult::LoadSnapshot { error: None } => Ok(()),
                AgentInvocationResult::LoadSnapshot { error: Some(e) } => Err(
                    ManualLoadResult::Failed(format!("Manual update failed to load snapshot: {e}")),
                ),
                _ => Err(ManualLoadResult::Failed(
                    "Unexpected result value from the snapshot load function".to_string(),
                )),
            },
            Ok(InvokeResult::Interrupted { interrupt_kind, .. }) => {
                Err(ManualLoadResult::Interrupted(interrupt_kind))
            }
            Ok(InvokeResult::Exited { .. }) => Err(ManualLoadResult::Exited),
        }
    }

    /// What the start does with the failure `error` with the pending update `head`, as
    /// [`start_outcome::decide`] decides for the baseline of this start.
    pub(in crate::durable_host) fn start_action(
        &self,
        head: Option<&PendingUpdateRef>,
        error: start_outcome::RawStartError<'_>,
    ) -> start_outcome::StartAction {
        start_outcome::decide(
            &self.state.baseline_role,
            head,
            error,
            &self.owned_agent_id.agent_id,
            self.public_state.worker().retired_for_lost_shard(),
        )
    }

    /// Performs `action` of a failed start: it writes a failed update and starts again on the
    /// source revision, skips or rejects a periodic record and starts again, or ends the start
    /// with an error.
    pub(in crate::durable_host) async fn perform_start_action(
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        action: start_outcome::StartAction,
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        match action {
            start_outcome::StartAction::FailUpdate { entry, reject } => {
                store
                    .as_context_mut()
                    .data_mut()
                    .on_worker_update_failed(entry)
                    .await?;
                if let Some(index) = reject {
                    store
                        .as_context()
                        .data()
                        .get_public_state()
                        .worker()
                        .reject_periodic(index);
                }
                debug!("Retrying prepare_instance after failed update attempt");
                Ok(Some(RetryDecision::Immediate))
            }
            start_outcome::StartAction::SkipPeriodic(index) => {
                store
                    .as_context()
                    .data()
                    .get_public_state()
                    .worker()
                    .mark_periodic_unavailable(index);
                Ok(Some(RetryDecision::Immediate))
            }
            start_outcome::StartAction::RejectPeriodic(index) => {
                warn!(
                    snapshot_index = %index,
                    "The application snapshot of a periodic record does not load; the start selects its baseline again without this record: the previous usable periodic record, or else the authoritative baseline"
                );
                store
                    .as_context()
                    .data()
                    .get_public_state()
                    .worker()
                    .reject_periodic(index);
                Ok(Some(RetryDecision::Immediate))
            }
            start_outcome::StartAction::Error(error) => Err(error),
            start_outcome::StartAction::Retry(decision) => Ok(Some(decision)),
            start_outcome::StartAction::ShardLost => Err(WorkerExecutorError::Interrupted {
                kind: InterruptKind::ShardLost,
            }),
        }
    }

    pub(in crate::durable_host) async fn try_load_snapshot(
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        instance: &Instance,
    ) -> SnapshotRecoveryResult {
        let snapshot_index = store
            .as_context()
            .data()
            .durable_ctx()
            .state
            .last_snapshot_index;

        let snapshot_index = match snapshot_index {
            Some(idx) => idx,
            None => return SnapshotRecoveryResult::NotAttempted,
        };

        debug!("Attempting snapshot-based recovery from oplog index {snapshot_index}");

        let oplog_entry = match store
            .as_context()
            .data()
            .get_public_state()
            .oplog()
            .read(snapshot_index)
            .await
        {
            Ok(oplog_entry) => oplog_entry,
            Err(error) => return snapshot_entry_read_failure(error),
        };

        let (data_payload, mime_type) = match oplog_entry {
            OplogEntry::Snapshot {
                data, mime_type, ..
            } => (data, mime_type),
            OplogEntry::PendingUpdate {
                description:
                    UpdateDescription::SnapshotBased {
                        payload, mime_type, ..
                    },
                ..
            } => (payload, mime_type),
            _ => {
                let error = format!(
                    "Expected Snapshot entry at oplog index {snapshot_index}, found different entry"
                );
                Self::emit_snapshot_recovery_event(
                    store,
                    snapshot_index,
                    false,
                    Some(error.clone()),
                );
                return SnapshotRecoveryResult::Failed(WorkerExecutorError::runtime(error));
            }
        };

        let data = match store
            .as_context()
            .data()
            .get_public_state()
            .oplog()
            .download_payload_classified(data_payload)
            .await
        {
            Ok(data) => data,
            Err(err) => {
                let lost = matches!(err, OplogPayloadDownloadError::Corrupt(_));
                let error =
                    format!("Failed to download snapshot payload at {snapshot_index}: {err}");
                Self::emit_snapshot_recovery_event(
                    store,
                    snapshot_index,
                    false,
                    Some(error.clone()),
                );
                let result = payload_download_failure(
                    store
                        .as_context()
                        .data()
                        .durable_ctx()
                        .state
                        .snapshot_replay_purpose,
                    lost,
                    error,
                );
                if matches!(result, SnapshotRecoveryResult::Retry(_)) {
                    store
                        .as_context()
                        .data()
                        .get_public_state()
                        .worker()
                        .mark_periodic_unavailable(snapshot_index);
                }
                return result;
            }
        };

        if let Err(error) = load_load_snapshot_guest(&mut store.as_context_mut(), instance) {
            Self::emit_snapshot_recovery_event(
                store,
                snapshot_index,
                false,
                Some(error.to_string()),
            );
            return SnapshotRecoveryResult::Failed(error);
        }

        let component_metadata = store
            .as_context()
            .data()
            .component_metadata()
            .metadata
            .clone();

        let idempotency_key = IdempotencyKey::fresh();
        store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .set_current_idempotency_key(idempotency_key.clone())
            .await;

        let load_snapshot_invocation = AgentInvocation::LoadSnapshot {
            idempotency_key,
            snapshot: RawSnapshotData { data, mime_type },
        };
        let agent_id = store.as_context().data().parsed_agent_id();
        let lowered = match lower_invocation(
            load_snapshot_invocation,
            &component_metadata,
            agent_id.as_ref(),
        ) {
            Ok(lowered) => lowered,
            Err(err) => {
                let error =
                    format!("Snapshot recovery failed to lower load-snapshot invocation: {err}");
                Self::emit_snapshot_recovery_event(
                    store,
                    snapshot_index,
                    false,
                    Some(error.clone()),
                );
                return SnapshotRecoveryResult::Failed(WorkerExecutorError::runtime(error));
            }
        };

        let invocation_context = InvocationContextStack::fresh();
        let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
        if let Err(err) = store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .set_current_invocation_context(invocation_context)
            .await
        {
            let error = format!("Snapshot recovery failed to install invocation context: {err}");
            Self::emit_snapshot_recovery_event(store, snapshot_index, false, Some(error));
            return SnapshotRecoveryResult::Unavailable(err);
        }

        store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .begin_call_snapshotting_function();

        let load_result =
            invoke_observed_and_traced(lowered, store, instance, InvocationMode::Replay).await;
        store.as_context_mut().data().set_suspended();

        store
            .as_context_mut()
            .data_mut()
            .durable_ctx_mut()
            .end_call_snapshotting_function_if_active();

        for span_id in local_span_ids {
            let _ = store
                .as_context_mut()
                .data_mut()
                .durable_ctx_mut()
                .remove_span(&span_id);
        }
        for span_id in inherited_span_ids {
            let _ = store
                .as_context_mut()
                .data_mut()
                .durable_ctx_mut()
                .remove_span(&span_id);
        }

        let snapshot_divergence = load_result
            .as_ref()
            .is_ok_and(InvokeResult::is_snapshot_replay_divergence);
        if let Some(decision) = load_result.as_ref().ok().and_then(Self::load_retry) {
            return SnapshotRecoveryResult::Retry(decision);
        }
        let failed = match load_result {
            Err(error) => return SnapshotRecoveryResult::Unavailable(error),
            Ok(InvokeResult::Failed { error, .. }) if !snapshot_divergence => {
                return SnapshotRecoveryResult::Unavailable(
                    WorkerExecutorError::InvocationFailed {
                        error,
                        stderr: String::new(),
                    },
                );
            }
            Ok(InvokeResult::Failed { error, .. }) => {
                let stderr = store
                    .as_context()
                    .data()
                    .get_public_state()
                    .event_service()
                    .get_last_invocation_errors();
                let error = error.to_string(&stderr);
                Some(format!(
                    "Snapshot recovery failed to load snapshot: {error}"
                ))
            }
            Ok(InvokeResult::Succeeded { result, .. }) => match *result {
                AgentInvocationResult::LoadSnapshot { error } => {
                    error.map(|e| format!("Snapshot recovery load-snapshot returned error: {e}"))
                }
                _ => Some("Unexpected result value from load-snapshot function".to_string()),
            },
            Ok(_) => Some("Snapshot recovery interrupted".to_string()),
        };

        if let Some(error) = failed {
            warn!(%error, %snapshot_index, "Snapshot recovery failed");
            Self::emit_snapshot_recovery_event(store, snapshot_index, false, Some(error.clone()));
            SnapshotRecoveryResult::Failed(WorkerExecutorError::runtime(format!(
                "Snapshot recovery at {snapshot_index} failed: {error}"
            )))
        } else {
            debug!("Snapshot loaded successfully from oplog index {snapshot_index}");
            Self::emit_snapshot_recovery_event(store, snapshot_index, true, None);
            SnapshotRecoveryResult::Success
        }
    }

    /// Whether the replay after the application snapshot of this start, which diverged from the
    /// oplog with `error`, rejects its record now, as [`start_outcome::decide`] decides: only a
    /// periodic record, and only while the start still replays it.
    pub(in crate::durable_host) fn rejects_diverged_record(
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        error: &WorkerExecutorError,
    ) -> bool {
        let ctx = store.as_context().data().durable_ctx();
        ctx.state.snapshot_replay_purpose == SnapshotReplayPurpose::PeriodicRecovery
            && !ctx.is_live()
            && matches!(
                ctx.start_action(
                    None,
                    start_outcome::RawStartError::Replay {
                        error,
                        diverged: true,
                    },
                ),
                start_outcome::StartAction::RejectPeriodic(_)
            )
    }

    /// Rejects this periodic snapshot for the starts of this incarnation and gives an immediate
    /// retry. The next start selects its baseline again without this record: the previous usable
    /// periodic record, or else the authoritative baseline.
    pub(in crate::durable_host) fn abandon_diverged_automatic_snapshot(
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        error: &impl std::fmt::Display,
        emit_failure: bool,
    ) -> RetryDecision {
        let snapshot_index = store
            .as_context()
            .data()
            .durable_ctx()
            .state
            .last_snapshot_index
            .expect("an automatic snapshot tail is only replayed after loading a snapshot");
        let error = format!(
            "Snapshot recovery at {snapshot_index} diverged from the recorded oplog: {error}; retrying without this snapshot, from the previous usable periodic record or else the authoritative baseline"
        );
        warn!(%error, "Abandoning periodic snapshot");
        if emit_failure
            && store
                .as_context()
                .data()
                .durable_ctx()
                .state
                .snapshot_replay_purpose
                == SnapshotReplayPurpose::PeriodicRecovery
        {
            Self::emit_snapshot_recovery_event(store, snapshot_index, false, Some(error));
        }
        store
            .as_context()
            .data()
            .get_public_state()
            .worker()
            .reject_periodic(snapshot_index);
        RetryDecision::Immediate
    }

    fn emit_snapshot_recovery_event(
        store: &mut (impl AsContextMut<Data = Ctx> + Send),
        snapshot_index: OplogIndex,
        succeeded: bool,
        error: Option<String>,
    ) {
        if store
            .as_context()
            .data()
            .durable_ctx()
            .state
            .snapshot_replay_purpose
            == SnapshotReplayPurpose::AssistedUpdate
        {
            return;
        }
        store
            .as_context_mut()
            .data_mut()
            .get_public_state()
            .event_service()
            .emit_event(
                if succeeded {
                    InternalWorkerEvent::snapshot_recovery_succeeded(snapshot_index)
                } else {
                    InternalWorkerEvent::snapshot_recovery_failed(
                        snapshot_index,
                        error.unwrap_or_else(|| "unknown".to_string()),
                    )
                },
                true,
            );
    }

    pub(crate) fn end_call_snapshotting_function_if_active(&mut self) {
        if self.state.snapshotting_mode {
            self.end_call_snapshotting_function();
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> UpdateManagement for DurableWorkerCtx<Ctx> {
    fn snapshot_boundary_blocker(&self) -> Option<SnapshotBoundaryBlocker> {
        self.state.snapshot_boundary_blocker()
    }

    fn begin_call_snapshotting_function(&mut self) {
        if self.state.snapshotting_mode {
            warn!(
                "begin_call_snapshotting_function called while snapshotting is already active; \
                 leaving snapshotting active"
            );
            return;
        }
        self.state.snapshotting_mode = true;
    }

    fn end_call_snapshotting_function(&mut self) {
        if self.state.snapshotting_mode {
            self.state.snapshotting_mode = false;
        } else {
            warn!(
                "end_call_snapshotting_function called without a matching begin_call_snapshotting_function"
            );
        }
    }

    async fn on_worker_update_failed(
        &self,
        failed_update: OplogEntry,
    ) -> Result<(), WorkerExecutorError> {
        warn!(?failed_update, "Worker update attempt aborted");
        let worker = self.public_state.worker();
        worker.add_and_commit_oplog(failed_update).await?;
        Ok(())
    }

    async fn on_worker_update_succeeded(
        &self,
        target_revision: ComponentRevision,
        new_component_size: u64,
        new_active_plugins: HashSet<
            golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId,
        >,
        snapshot_assisted_details: Option<SnapshotAssistedUpdateDetails>,
    ) -> Result<(), WorkerExecutorError> {
        info!("Worker update to {} finished successfully", target_revision);
        let worker = self.public_state.worker();
        worker
            .persist_successful_update(
                &self.linear_memory,
                target_revision,
                new_component_size,
                new_active_plugins,
                snapshot_assisted_details,
            )
            .await?;
        Ok(())
    }
}

impl PrivateDurableWorkerState {
    /// Whether the current oplog tip is a structurally clean boundary at which a mid-invocation
    /// status checkpoint may be taken: we are live, no host call is in flight, and snapshotting
    /// is not active.
    pub fn at_clean_checkpoint_boundary(&self) -> bool {
        Self::clean_checkpoint_boundary(
            !self.is_live(),
            self.has_in_flight_live_host_calls(),
            self.durability_is_suppressed(),
        )
    }

    /// Pure form of [`Self::at_clean_checkpoint_boundary`], factored out so its truth table can
    /// be tested without constructing worker state. Each argument is a *blocking* condition: the
    /// boundary is clean iff all of them are `false`.
    pub(in crate::durable_host) fn clean_checkpoint_boundary(
        replaying: bool,
        in_flight_host_call: bool,
        snapshotting: bool,
    ) -> bool {
        !replaying && !in_flight_host_call && !snapshotting
    }

    /// The first condition currently blocking a snapshot, or `None` when the worker is at a safe
    /// snapshot boundary.
    ///
    /// A committed snapshot is a replay cut point: snapshot-based recovery (and snapshot-based
    /// update) skips every oplog entry before the snapshot. The invariant is that no durable
    /// construct may span that cut — a durable call or scope whose `Start` precedes the snapshot
    /// but whose `End`/`Cancelled` is recorded after it would leave a terminal whose `Start` the
    /// post-snapshot replay never sees (the orphan terminal is drained harmlessly, but the call
    /// itself cannot be restored from the snapshot). Snapshots are therefore only taken at a
    /// clean checkpoint boundary (no in-flight host calls and no snapshotting)
    /// with no durable host call in flight or tool attachment needing the skipped history.
    ///
    /// The sampled conditions must stay in sync with [`Self::at_clean_checkpoint_boundary`]:
    /// `blocker() == None` is equivalent to
    /// a clean checkpoint with no in-flight host call or open tool attachment.
    pub fn snapshot_boundary_blocker(&self) -> Option<SnapshotBoundaryBlocker> {
        SnapshotBoundaryConditions {
            replaying: !self.is_live(),
            snapshotting: self.snapshotting_mode,
            in_flight_host_call: self.has_in_flight_live_host_calls(),
            open_tool_attachment: self.replay_state.has_open_tool_attachments(),
        }
        .blocker()
    }
}

impl SnapshotBoundaryConditions {
    /// The first blocking condition in precedence order, or `None` when the worker is at a safe
    /// snapshot boundary.
    pub(super) fn blocker(self) -> Option<SnapshotBoundaryBlocker> {
        if self.replaying {
            Some(SnapshotBoundaryBlocker::Replaying)
        } else if self.snapshotting {
            Some(SnapshotBoundaryBlocker::Snapshotting)
        } else if self.in_flight_host_call {
            Some(SnapshotBoundaryBlocker::InFlightHostCall)
        } else if self.open_tool_attachment {
            Some(SnapshotBoundaryBlocker::OpenToolAttachment)
        } else {
            None
        }
    }
}

/// What a start gets when the snapshot record itself does not read. A storage that did not answer
/// is `Unavailable` with `RecoveryRequired`, which the start retries without a decision on the
/// record; any other read failure is `OplogFailed`, which fails the start in every column. Neither
/// is a snapshot that failed to load, so the record is never rejected for it.
pub(in crate::durable_host) fn snapshot_entry_read_failure(
    error: UnreadableOplog,
) -> SnapshotRecoveryResult {
    let transient = error.is_transient();
    let error = WorkerExecutorError::from(error);
    if transient {
        SnapshotRecoveryResult::Unavailable(error)
    } else {
        SnapshotRecoveryResult::Failed(error)
    }
}

/// What a start gets when the payload of the application snapshot of its baseline does not
/// download for `purpose`, with `error` as the text. `lost` tells that the payload is missing or
/// does not decode, rather than that its store failed. A periodic record is skipped for the start
/// attempt and the start retries at once (the caller marks it unavailable). A snapshot-assisted
/// attempt whose store failed retries with the same record through the recovery path, and
/// nothing marks the record; one whose payload is lost reports the selected record as lost. The
/// replay for a pending automatic update retries a failed store through the recovery path too,
/// and gets the runtime error for a lost payload. Any other baseline gets the runtime error.
pub(in crate::durable_host) fn payload_download_failure(
    purpose: SnapshotReplayPurpose,
    lost: bool,
    error: String,
) -> SnapshotRecoveryResult {
    match purpose {
        SnapshotReplayPurpose::PeriodicRecovery => {
            SnapshotRecoveryResult::Retry(RetryDecision::Immediate)
        }
        SnapshotReplayPurpose::AssistedUpdate if lost => {
            SnapshotRecoveryResult::Lost(WorkerExecutorError::runtime(error))
        }
        SnapshotReplayPurpose::AssistedUpdate => {
            SnapshotRecoveryResult::Unavailable(WorkerExecutorError::RecoveryRequired {
                retry_from: None,
                details: error,
            })
        }
        SnapshotReplayPurpose::AutomaticUpdate if !lost => {
            SnapshotRecoveryResult::Unavailable(WorkerExecutorError::RecoveryRequired {
                retry_from: None,
                details: error,
            })
        }
        SnapshotReplayPurpose::None | SnapshotReplayPurpose::AutomaticUpdate => {
            SnapshotRecoveryResult::Unavailable(WorkerExecutorError::runtime(error))
        }
    }
}

/// Performs `action`, the outcome of a failure of the update of the instance at the update point
/// of a start, on the p2 and the p3 path, and gives the error that ends the start. The update
/// point consumed the pending update, so a start that runs again starts on the source revision:
/// a written failed update, a skipped or a rejected record end the start with `Restart`, an
/// internal retry signal that records no interruption. A failed write ends it with the error of
/// the write, a lost shard with `ShardLost`, and an error of the table with the error that
/// `start_outcome::update_point_error` gives for it.
pub(crate) async fn perform_at_update_point<Ctx: WorkerCtx>(
    worker: &Arc<Worker<Ctx>>,
    action: start_outcome::StartAction,
) -> WorkerExecutorError {
    let restart = WorkerExecutorError::Interrupted {
        kind: InterruptKind::Restart,
    };
    match action {
        start_outcome::StartAction::FailUpdate { entry, reject } => {
            warn!(?entry, "Worker update attempt aborted");
            match worker.add_and_commit_oplog(entry).await {
                Ok(_) => {
                    if let Some(index) = reject {
                        worker.reject_periodic(index);
                    }
                    restart
                }
                Err(error) => WorkerExecutorError::from(error),
            }
        }
        start_outcome::StartAction::SkipPeriodic(index) => {
            worker.mark_periodic_unavailable(index);
            restart
        }
        start_outcome::StartAction::RejectPeriodic(index) => {
            worker.reject_periodic(index);
            restart
        }
        start_outcome::StartAction::Error(error) => start_outcome::update_point_error(error),
        start_outcome::StartAction::ShardLost => WorkerExecutorError::Interrupted {
            kind: InterruptKind::ShardLost,
        },
        start_outcome::StartAction::Retry(decision) => WorkerExecutorError::runtime(format!(
            "the update point has no snapshot load to retry ({decision:?})"
        )),
    }
}
