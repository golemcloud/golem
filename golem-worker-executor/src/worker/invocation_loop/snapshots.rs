use super::*;

impl<Ctx: WorkerCtx> InvocationLoop<Ctx> {
    /// Waits, before this generation, with `mark`, ends for the automatic update at the head of
    /// the queue, for the upload of the record `name` that the update selects once confirmed, as
    /// [`Worker::confirm_filesystem_snapshot_before_an_update`] says. A requested retirement of
    /// the owner and a stop of the running worker, which closes its command channel, end the
    /// wait, or skip it when they came first. Commands that arrive during the wait are kept in
    /// `deferred_wakeups` for the next generation.
    pub(super) async fn confirm_filesystem_snapshot_before_an_update(
        &mut self,
        name: FilesystemSnapshotName,
        mark: TreeMark,
        deferred_wakeups: &mut VecDeque<WorkerCommand>,
    ) {
        let retiring = self.parent.owner_retirement_requested.clone();
        let parent = self.parent.clone();
        let confirmation = parent.confirm_filesystem_snapshot_before_an_update(name, mark);
        tokio::pin!(confirmation);
        loop {
            tokio::select! {
                biased;
                () = retiring.cancelled() => return,
                command = self.receiver.recv() => match command {
                    Some(WorkerCommand::InternalStatusChanged) => {
                        if !deferred_wakeups
                            .iter()
                            .any(|command| matches!(command, WorkerCommand::InternalStatusChanged))
                        {
                            deferred_wakeups.push_back(WorkerCommand::InternalStatusChanged);
                        }
                    }
                    Some(command) => Self::defer_wakeup(deferred_wakeups, command),
                    None => return,
                },
                () = &mut confirmation => return,
            }
        }
    }
}

impl<Ctx: WorkerCtx> InnerInvocationLoop<'_, Ctx> {
    pub(super) fn abort_idle_snapshot_task(&mut self) {
        if let Some(task) = self.idle_snapshot_task.take() {
            task.abort();
        }
    }
}

impl<Ctx: WorkerCtx> InnerInvocationLoop<'_, Ctx> {
    pub(super) async fn handle_idle_status(
        &mut self,
        status: &AgentStatusRecord,
    ) -> CommandOutcome {
        match self.periodic_snapshot_action(status) {
            PeriodicSnapshotAction::DueNow => {
                self.inject_snapshot_as_next_action().await;
                return CommandOutcome::Continue;
            }
            PeriodicSnapshotAction::Wait(delay) => {
                self.schedule_idle_snapshot(delay);
                return CommandOutcome::WaitForWakeup;
            }
            PeriodicSnapshotAction::NotNeeded => {}
        }

        CommandOutcome::WaitForWakeup
    }

    pub(super) async fn on_external_invocation_completed(&mut self) -> bool {
        self.invocations_since_snapshot += 1;
        match self.parent.snapshot_policy() {
            SnapshotPolicy::EveryNInvocation { count } => {
                if self.invocations_since_snapshot >= *count as u64 {
                    self.invocations_since_snapshot = 0;
                    self.inject_snapshot_as_next_action().await;
                    true
                } else {
                    false
                }
            }
            SnapshotPolicy::Periodic { .. } => {
                let status = self.parent.get_last_known_status().await;
                if matches!(
                    self.periodic_snapshot_action(&status),
                    PeriodicSnapshotAction::DueNow
                ) {
                    self.inject_snapshot_as_next_action().await;
                    true
                } else {
                    false
                }
            }
            SnapshotPolicy::Disabled => false,
        }
    }

    pub(super) fn periodic_snapshot_action(
        &self,
        status: &AgentStatusRecord,
    ) -> PeriodicSnapshotAction {
        let SnapshotPolicy::Periodic { period } = self.parent.snapshot_policy() else {
            return PeriodicSnapshotAction::NotNeeded;
        };

        let created_at = self.parent.get_initial_worker_metadata().created_at;
        let last_snapshot_timestamp = snapshot_baseline_timestamp(
            status
                .last_automatic_snapshot
                .as_ref()
                .map(|last| last.timestamp),
            *self.last_periodic_attempt,
            created_at,
        );

        snapshot_action_at(last_snapshot_timestamp, *period, Timestamp::now_utc())
    }

    pub(super) fn schedule_idle_snapshot(&mut self, delay: Duration) {
        self.abort_idle_snapshot_task();
        self.idle_snapshot_task = Some(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
        }));
    }

    pub(super) async fn inject_snapshot_as_next_action(&self) {
        let mut queue = self.active.write().await;
        queue.retain(|invocation| !invocation.is_abandoned());
        queue.push_front(QueuedWorkerInvocation::SaveSnapshot);
    }
}

impl<Ctx: WorkerCtx> Invocation<'_, Ctx> {
    /// Try to perform the save-snapshot step of a manual update on the worker
    pub(super) async fn manual_update(
        &mut self,
        target_revision: ComponentRevision,
        update_attempt_index: OplogIndex,
    ) -> CommandOutcome {
        let span = span!(
            Level::INFO,
            "manual_update",
            agent_id = %self.owned_agent_id.agent_id,
            target_revision = %target_revision,
            agent_type = self.parent
                .parsed_agent_id
                .as_ref()
                .map(|id| id.agent_type.to_string())
                .unwrap_or_else(|| "-".to_string()),
        );

        self.manual_update_inner(target_revision, update_attempt_index)
            .instrument(span)
            .await
    }

    /// The inner implementation of the manual update command
    pub(super) async fn manual_update_inner(
        &mut self,
        target_revision: ComponentRevision,
        update_attempt_index: OplogIndex,
    ) -> CommandOutcome {
        // The saved snapshot becomes the replay cut point of the snapshot-based update: after the
        // update, replay starts from the snapshot and skips everything before it. No durable call
        // or scope may span that cut, so refuse the update while any is still open.
        if let Some(blocker) = self.store.data().snapshot_boundary_blocker() {
            return self
                .fail_update(
                    target_revision,
                    update_attempt_index,
                    format!("cannot take a snapshot for the update: {blocker}"),
                )
                .await;
        }
        let snapshots = self.parent.agent_filesystem_snapshots();
        let agent_snapshots = AgentSnapshots::agent(
            &self.owned_agent_id,
            self.parent.initial_worker_metadata.fingerprint,
        );
        let (snapshot, filesystem_snapshot, retention) = match update_snapshot(
            &mut UpdateHost {
                invocation: self,
                target_revision,
                update_attempt_index,
            },
            &snapshots,
            &agent_snapshots,
        )
        .await
        {
            UpdateSnapshot::Saved {
                snapshot,
                name,
                retention,
            } => (snapshot, name, retention),
            UpdateSnapshot::Fail(details) => {
                return self
                    .fail_update(target_revision, update_attempt_index, details)
                    .await;
            }
            UpdateSnapshot::WriteNothing => {
                return CommandOutcome::BreakInnerLoop(RetryDecision::None);
            }
            UpdateSnapshot::Guest(outcome) => return outcome,
        };
        match self
            .store
            .data()
            .get_public_state()
            .oplog()
            .create_snapshot_based_update_description(
                target_revision,
                snapshot.data,
                snapshot.mime_type,
                filesystem_snapshot,
            )
            .await
        {
            Ok(update_description) => {
                // Enqueue the update, then reactivate the worker; stop processing the
                // queue to avoid race conditions. Retention runs once the update record
                // committed. Refused by the fence, nothing restarts here: the update
                // stays pending for the shard's new owner.
                match self
                    .parent
                    .owner_retirement_requested
                    .run_until_cancelled(
                        self.parent.enqueue_update_for_attempt(
                            update_description,
                            Some(update_attempt_index),
                        ),
                    )
                    .await
                {
                    None => CommandOutcome::BreakInnerLoop(RetryDecision::None),
                    Some(Err(error)) if self.parent.retire_if_shard_lost(&error) => {
                        CommandOutcome::BreakInnerLoop(RetryDecision::None)
                    }
                    Some(Err(_)) => CommandOutcome::BreakInnerLoop(RetryDecision::Immediate),
                    Some(Ok(_)) => {
                        // The status holds the new pending update by now, so its name is kept.
                        if let Some(retention) = retention {
                            retention.delete_older_snapshots(
                                &crate::worker::snapshot_selection::names_in_use(
                                    &self.parent.last_known_status.load(),
                                ),
                            );
                        }
                        CommandOutcome::BreakInnerLoop(RetryDecision::Immediate)
                    }
                }
            }
            Err(error) => {
                self.fail_update(
                    target_revision,
                    update_attempt_index,
                    format!("failed to store the snapshot for manual update: {error}"),
                )
                .await
            }
        }
    }

    /// Runs the save hook of the guest for a manual update to `target_revision`. A hook that
    /// gives no snapshot records the failed update, or writes nothing on a lost shard, and gives
    /// the outcome of the loop.
    pub(super) async fn snapshot_guest_for_update(
        &mut self,
        target_revision: ComponentRevision,
        update_attempt_index: OplogIndex,
    ) -> Result<golem_common::model::oplog::RawSnapshotData, CommandOutcome> {
        let idempotency_key = {
            let ctx = self.store.data_mut();
            let idempotency_key = IdempotencyKey::fresh();
            ctx.set_current_idempotency_key(idempotency_key.clone())
                .await;
            idempotency_key
        };
        let component_metadata = self.store.data().component_metadata().metadata.clone();

        let save_snapshot_invocation = AgentInvocation::SaveSnapshot { idempotency_key };
        let lowered = match lower_invocation(
            save_snapshot_invocation,
            &component_metadata,
            self.parent.parsed_agent_id.as_ref(),
        ) {
            Ok(lowered) => lowered,
            Err(err) => {
                warn!("Failed to lower save-snapshot invocation: {err}");
                return Err(self
                    .fail_update(
                        target_revision,
                        update_attempt_index,
                        format!("failed to lower save-snapshot invocation: {err}"),
                    )
                    .await);
            }
        };

        let invocation_context = InvocationContextStack::fresh();
        let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
        if let Err(err) = self
            .store
            .data_mut()
            .set_current_invocation_context(invocation_context)
            .await
        {
            warn!("Failed to install invocation context for manual update save-snapshot: {err}");
            return Err(self
                .fail_update(
                    target_revision,
                    update_attempt_index,
                    format!("failed to install invocation context for save-snapshot: {err}"),
                )
                .await);
        }

        let result = self
            .invoke_in_snapshotting_mode(lowered, local_span_ids, inherited_span_ids)
            .await;

        match result {
            Ok(InvokeResult::Succeeded { result, .. }) => {
                let AgentInvocationResult::SaveSnapshot { snapshot } = *result else {
                    return Err(self
                        .fail_update(
                            target_revision,
                            update_attempt_index,
                            "failed to get a snapshot for manual update: invalid snapshot result"
                                .to_string(),
                        )
                        .await);
                };
                Ok(snapshot)
            }

            Ok(InvokeResult::Failed { error, .. }) => {
                let stderr = self
                    .store
                    .data()
                    .get_public_state()
                    .event_service()
                    .get_last_invocation_errors();
                let error = error.to_string(&stderr);
                Err(self
                    .fail_update(
                        target_revision,
                        update_attempt_index,
                        format!("failed to get a snapshot for manual update: {error}"),
                    )
                    .await)
            }
            Ok(InvokeResult::Exited { .. }) => Err(self
                .fail_update(
                    target_revision,
                    update_attempt_index,
                    "failed to get a snapshot for manual update: it called exit".to_string(),
                )
                .await),
            Ok(InvokeResult::Interrupted { interrupt_kind, .. }) => {
                // A `FailedUpdate` drops the pending update from the status: for a lost shard
                // nothing is written, and the update stays pending for the new owner.
                if self
                    .parent
                    .retire_if_shard_lost(&WorkerExecutorError::Interrupted {
                        kind: interrupt_kind,
                    })
                {
                    return Err(CommandOutcome::BreakInnerLoop(RetryDecision::None));
                }
                Err(self
                    .fail_update(
                        target_revision,
                        update_attempt_index,
                        format!("failed to get a snapshot for manual update: {interrupt_kind:?}"),
                    )
                    .await)
            }
            Err(error) => Err(self
                .fail_update(
                    target_revision,
                    update_attempt_index,
                    format!("failed to get a snapshot for manual update: {error:?}"),
                )
                .await),
        }
    }
}

impl<Ctx: WorkerCtx> Invocation<'_, Ctx> {
    /// Records an attempted worker update as failed
    pub(super) async fn fail_update(
        &self,
        target_revision: ComponentRevision,
        update_attempt_index: OplogIndex,
        error: String,
    ) -> CommandOutcome {
        // Refused, the shard is lost: it stops rather than carrying on at a revision
        // whose failed update was never recorded.
        match self
            .store
            .data()
            .on_worker_update_failed(crate::worker::start_outcome::failed_admission_of(
                target_revision,
                update_attempt_index,
                error,
            ))
            .await
        {
            Ok(()) => CommandOutcome::Continue,
            Err(_) => CommandOutcome::BreakInnerLoop(RetryDecision::None),
        }
    }
}

impl<Ctx: WorkerCtx> Invocation<'_, Ctx> {
    pub(super) async fn take_guest_snapshot(&mut self) -> CommandOutcome {
        // A committed snapshot is a replay cut point (snapshot-based recovery skips everything
        // before it), so no durable call or scope may span it. Skip this periodic snapshot when
        // the worker is not at a safe boundary; the next scheduled snapshot will retry.
        if let Some(blocker) = self.store.data().snapshot_boundary_blocker() {
            warn!("Skipping periodic snapshot: {blocker}");
            return CommandOutcome::Continue;
        }
        let snapshots = self.parent.agent_filesystem_snapshots();
        let agent_snapshots = AgentSnapshots::agent(
            &self.owned_agent_id,
            self.parent.initial_worker_metadata.fingerprint,
        );
        match periodic_snapshot(&mut PeriodicHost(self), &snapshots, &agent_snapshots).await {
            PeriodicResult::Continue => CommandOutcome::Continue,
            PeriodicResult::Guest(outcome) => outcome,
            PeriodicResult::NotWritten(failure) => periodic_failure_outcome(&failure),
        }
    }

    /// Runs the lowered save hook of the guest in snapshotting mode, marks the store suspended
    /// after it, and removes the spans of its invocation context.
    pub(super) async fn invoke_in_snapshotting_mode(
        &mut self,
        lowered: LoweredInvocation,
        local_span_ids: HashSet<SpanId>,
        inherited_span_ids: HashSet<SpanId>,
    ) -> Result<InvokeResult, WorkerExecutorError> {
        self.store
            .data_mut()
            .durable_ctx_mut()
            .begin_call_snapshotting_function();
        let result =
            invoke_observed_and_traced(lowered, self.store, self.instance, InvocationMode::Replay)
                .await;
        self.store.data().set_suspended();
        self.store
            .data_mut()
            .durable_ctx_mut()
            .end_call_snapshotting_function_if_active();
        local_span_ids
            .iter()
            .chain(&inherited_span_ids)
            .for_each(|span_id| {
                let _ = self.store.data_mut().remove_span(span_id);
            });
        result
    }

    /// Runs the save hook of the guest for a periodic snapshot. A hook that gives no snapshot
    /// gives the outcome of the loop.
    pub(super) async fn snapshot_guest_for_periodic(
        &mut self,
    ) -> Result<golem_common::model::oplog::RawSnapshotData, CommandOutcome> {
        let idempotency_key = IdempotencyKey::fresh();
        self.store
            .data_mut()
            .set_current_idempotency_key(idempotency_key.clone())
            .await;
        let component_metadata = self.store.data().component_metadata().metadata.clone();

        let save_snapshot_invocation = AgentInvocation::SaveSnapshot { idempotency_key };
        let lowered = match lower_invocation(
            save_snapshot_invocation,
            &component_metadata,
            self.parent.parsed_agent_id.as_ref(),
        ) {
            Ok(lowered) => lowered,
            Err(err) => {
                warn!("Failed to lower save-snapshot invocation: {err}");
                return Err(CommandOutcome::Continue);
            }
        };

        let invocation_context = InvocationContextStack::fresh();
        let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
        if let Err(err) = self
            .store
            .data_mut()
            .set_current_invocation_context(invocation_context)
            .await
        {
            warn!("Failed to install invocation context for periodic save-snapshot: {err}");
            return Err(CommandOutcome::Continue);
        }

        let result = self
            .invoke_in_snapshotting_mode(lowered, local_span_ids, inherited_span_ids)
            .await;

        if let Some(outcome) = periodic_snapshot_failure_outcome(&result) {
            match &result {
                Ok(InvokeResult::Failed { .. }) => {
                    warn!(
                        "Periodic snapshot save function failed; restarting worker to recover the store"
                    );
                }
                Err(err) => {
                    warn!(
                        "Periodic snapshot save invocation error: {err}; restarting worker to recover the store"
                    );
                }
                _ => unreachable!(),
            }

            return Err(outcome);
        }

        match result {
            Ok(InvokeResult::Succeeded { result, .. }) => {
                let AgentInvocationResult::SaveSnapshot { snapshot } = *result else {
                    warn!("Periodic snapshot returned unexpected result format");
                    return Err(CommandOutcome::Continue);
                };
                Ok(snapshot)
            }
            Ok(InvokeResult::Exited { .. }) => {
                warn!("Worker exited during periodic snapshot save");
                Err(CommandOutcome::BreakInnerLoop(RetryDecision::None))
            }
            Ok(InvokeResult::Interrupted { .. }) => {
                warn!("Worker interrupted during periodic snapshot save");
                Err(CommandOutcome::BreakInnerLoop(RetryDecision::None))
            }
            Ok(InvokeResult::Failed { .. }) | Err(_) => unreachable!(),
        }
    }
}

pub(super) fn periodic_snapshot_failure_outcome(
    result: &Result<InvokeResult, WorkerExecutorError>,
) -> Option<CommandOutcome> {
    match result {
        Ok(InvokeResult::Failed { .. }) | Err(_) => {
            Some(CommandOutcome::BreakInnerLoop(RetryDecision::Immediate))
        }
        _ => None,
    }
}

/// Waits for `attempt` and gives its outcome with the time that `now` gives when it ended. A
/// periodic snapshot waits a period from the end of the last attempt, so an attempt that takes
/// longer than the period does not make the next one due at once.
pub(super) async fn with_end<T>(
    attempt: impl Future<Output = T>,
    now: impl FnOnce() -> Timestamp,
) -> (T, Timestamp) {
    let outcome = attempt.await;
    (outcome, now())
}

/// The time that the period of the next periodic snapshot starts at: the later of the last
/// automatic snapshot record and the end of the last periodic snapshot, which can end without a
/// record, or the creation of the agent before both. So a snapshot that writes no record, or that
/// takes longer than the period, does not make the next one due at once.
pub(super) fn snapshot_baseline_timestamp(
    last_snapshot_timestamp: Option<Timestamp>,
    last_attempt: Option<Timestamp>,
    created_at: Timestamp,
) -> Timestamp {
    last_snapshot_timestamp
        .into_iter()
        .chain(last_attempt)
        .max()
        .unwrap_or(created_at)
}

pub(super) fn snapshot_action_at(
    last_snapshot_timestamp: Timestamp,
    period: Duration,
    now: Timestamp,
) -> PeriodicSnapshotAction {
    let period_millis = period.as_millis();
    if period_millis == 0 {
        return PeriodicSnapshotAction::DueNow;
    }

    let now = now.to_millis() as u128;
    let due_at = last_snapshot_timestamp.to_millis() as u128 + period_millis;

    if now >= due_at {
        PeriodicSnapshotAction::DueNow
    } else {
        PeriodicSnapshotAction::Wait(Duration::from_millis((due_at - now) as u64))
    }
}

/// Waits for a capture or a check of the agent filesystem and gives `record` its outcome with the
/// label `label` gives, or with the label of its error, and the time it took.
pub(super) async fn measured_capture<Outcome, Capturing>(
    capture: impl FnOnce() -> Capturing,
    label: fn(&Outcome) -> &'static str,
    record: impl FnOnce(&'static str, std::time::Duration),
) -> Result<Outcome, CaptureError>
where
    Capturing: Future<Output = Result<Outcome, CaptureError>>,
{
    let started = std::time::Instant::now();
    let result = capture().await;
    record(
        result.as_ref().map_or_else(CaptureError::label, label),
        started.elapsed(),
    );
    result
}

/// Logs a capture that failed, and gives the outcome of a capture that did not.
pub(super) fn logged_capture<Outcome>(result: Result<Outcome, CaptureError>) -> Option<Outcome> {
    result
        .inspect_err(|error| {
            warn!(error = %error, "Skipping the snapshot: the agent filesystem was not captured")
        })
        .ok()
}

/// Checks whether `filesystem` of `parent` holds only initial files, records the check in the
/// capture metric, and logs a check that failed.
pub(super) async fn measured_check<Ctx: WorkerCtx>(
    parent: &Worker<Ctx>,
    filesystem: &ResidentFilesystem,
    wait: std::time::Duration,
) -> Result<InitialFilesCheck, CaptureError> {
    let snapshots = parent.agent_filesystem_snapshots();
    measured_capture(
        || check_initial_files(filesystem, wait),
        InitialFilesCheck::label,
        |outcome, elapsed| snapshots.record_capture(outcome, elapsed),
    )
    .await
    .inspect_err(|error| {
        warn!(
            error = %error,
            "The check of the agent filesystem failed: the snapshot writes no record, and an \
             update fails"
        )
    })
}

/// What the loop does after a periodic snapshot record did not reach the oplog. A payload that
/// was not made skips the snapshot. A refused write means the shard has a new owner: the loop
/// stops without an error. A failed archive write is latched, so every later write fails the same
/// way, and an oplog that does not read for good cannot go on either: the loop stops and carries
/// the error to the stop, which gives it to the waiting callers. Any other failed write, and an
/// oplog whose storage did not answer, retries.
pub(super) fn periodic_failure_outcome(failure: &PeriodicFailure) -> CommandOutcome {
    match failure {
        PeriodicFailure::Entry(_) => CommandOutcome::Continue,
        PeriodicFailure::Write(OplogError::Fenced(_)) => {
            CommandOutcome::BreakInnerLoop(RetryDecision::None)
        }
        PeriodicFailure::Write(error) => match error.agent_failure() {
            Some(failure @ WorkerExecutorError::OplogFailed { .. }) => {
                CommandOutcome::BreakOuterLoop(Some(failure))
            }
            Some(_) | None => CommandOutcome::BreakInnerLoop(RetryDecision::Immediate),
        },
    }
}

/// The invocation loop as the host of a periodic snapshot.
impl<Ctx: WorkerCtx> PeriodicSnapshotHost for PeriodicHost<'_, '_, Ctx> {
    type Stop = CommandOutcome;

    async fn snapshot_guest(
        &mut self,
    ) -> Result<golem_common::model::oplog::RawSnapshotData, CommandOutcome> {
        self.0.snapshot_guest_for_periodic().await
    }

    fn since(&self) -> Option<ConfirmedFilesystemSnapshot> {
        let baselines = self.0.parent.start_baselines_now();
        self.0
            .filesystem_snapshot_slot
            .since(baselines.periodic.as_ref(), baselines.authoritative)
    }

    async fn capture(
        &self,
        wait: std::time::Duration,
        since: Option<TreeMark>,
    ) -> Option<CaptureOutcome> {
        let snapshots = self.0.parent.agent_filesystem_snapshots();
        logged_capture(
            measured_capture(
                || capture(self.0.filesystem, wait, since),
                CaptureOutcome::label,
                |outcome, elapsed| snapshots.record_capture(outcome, elapsed),
            )
            .await,
        )
    }

    async fn check_initial_files(
        &self,
        wait: std::time::Duration,
    ) -> Result<InitialFilesCheck, CaptureError> {
        measured_check(&self.0.parent, self.0.filesystem, wait).await
    }

    async fn entry(
        &self,
        snapshot: golem_common::model::oplog::RawSnapshotData,
        name: Option<FilesystemSnapshotName>,
    ) -> Result<OplogEntry, String> {
        let serialized = golem_common::serialization::serialize(&snapshot.data)
            .map_err(|err| format!("Failed to serialize snapshot data: {err}"))?;
        let payload = self
            .0
            .parent
            .oplog
            .upload_raw_payload(serialized)
            .await
            .map_err(|err| format!("Failed to upload periodic snapshot payload: {err}"))?
            .into_payload::<Vec<u8>>()
            .map_err(|err| format!("Failed to convert snapshot payload: {err}"))?;
        Ok(OplogEntry::snapshot(
            payload,
            snapshot.mime_type,
            self.0
                .store
                .data()
                .durable_ctx()
                .agent_wallet_cards_snapshot(),
            self.0.store.data().durable_ctx().wallet_generation(),
            name,
        ))
    }

    async fn write(
        &self,
        confirmed_at_once: Option<FilesystemSnapshotName>,
        entry: OplogEntry,
    ) -> Result<(), OplogError> {
        match confirmed_at_once {
            Some(name) => match self
                .0
                .parent
                .oplog
                .add_pair(
                    entry,
                    Box::new(move |_| OplogEntry::snapshot_confirmed(name)),
                )
                .await
            {
                Ok(_) => Ok(()),
                Err(OplogError::Fenced(fence)) => Err(self.0.parent.retired_by(fence)),
                Err(error) => Err(error),
            },
            None => self.0.parent.add_to_oplog(entry).await.map(drop),
        }?;
        self.0
            .parent
            .commit_oplog_and_update_state(CommitLevel::Always)
            .await?;
        debug!("Periodic snapshot saved successfully");
        // A snapshot is committed between invocations, so no jumpable region is open: a clean
        // boundary to checkpoint the status, aligning the checkpoint with the snapshot index.
        self.0
            .parent
            .checkpoint_status(status_checkpointer::CheckpointReason::Snapshot)
            .await;
        Ok(())
    }

    fn confirm(&self, mark: TreeMark) -> Confirm {
        confirm_by(Arc::downgrade(&self.0.parent), mark)
    }

    fn initial_files_written(&self, mark: TreeMark) {
        self.0.filesystem_snapshot_slot.record(None, mark);
    }
}

/// The invocation loop as the host of the snapshots of a manual update to `target_revision`.
impl<Ctx: WorkerCtx> UpdateSnapshotHost for UpdateHost<'_, '_, Ctx> {
    type Stop = CommandOutcome;

    async fn snapshot_guest(
        &mut self,
    ) -> Result<golem_common::model::oplog::RawSnapshotData, CommandOutcome> {
        self.invocation
            .snapshot_guest_for_update(self.target_revision, self.update_attempt_index)
            .await
    }

    async fn capture_whole(&self, wait: std::time::Duration) -> Option<WholeCapture> {
        let snapshots = self.invocation.parent.agent_filesystem_snapshots();
        logged_capture(
            measured_capture(
                || capture_whole(self.invocation.filesystem, wait),
                WholeCapture::label,
                |outcome, elapsed| snapshots.record_capture(outcome, elapsed),
            )
            .await,
        )
    }

    async fn check_initial_files(
        &self,
        wait: std::time::Duration,
    ) -> Result<InitialFilesCheck, CaptureError> {
        measured_check(&self.invocation.parent, self.invocation.filesystem, wait).await
    }

    fn terminal(&self) -> tokio::sync::watch::Receiver<bool> {
        self.invocation.parent.terminal_interrupt()
    }

    fn lost_shard(&self) -> crate::worker::LostShard {
        self.invocation.parent.lost_shard()
    }
}
