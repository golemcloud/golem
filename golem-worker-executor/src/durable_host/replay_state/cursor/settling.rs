use super::*;

pub(in crate::durable_host::replay_state) fn historical_reconstruction_owner_failure(
    failure: crate::durable_host::tool::operation::OwnerFailureWinner,
) -> WorkerExecutorError {
    match failure {
        crate::durable_host::tool::operation::OwnerFailureWinner::Infrastructure(error) => error,
        crate::durable_host::tool::operation::OwnerFailureWinner::Trap(_) => {
            WorkerExecutorError::runtime(
                "owner failed while waiting for historical entity reconstruction",
            )
        }
        crate::durable_host::tool::operation::OwnerFailureWinner::Lifecycle(kind) => {
            WorkerExecutorError::Interrupted { kind }
        }
    }
}

impl ReplayCursor {
    fn begin_settling(&self) {
        let _ = self.transition_phase.compare_exchange(
            ReplayTransitionPhase::Replaying as u8,
            ReplayTransitionPhase::Settling as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    pub(super) fn publish_live(&self) {
        self.transition_phase
            .store(ReplayTransitionPhase::Live as u8, Ordering::Release);
        self.progress.notify_waiters();
    }

    pub(super) fn is_live_published(&self) -> bool {
        self.transition_phase.load(Ordering::Acquire) == ReplayTransitionPhase::Live as u8
    }
}

impl CursorTx<'_> {
    /// Switches the cursor to live mode: records `ReplayFinished` if replay was still in progress,
    /// clamps the cursor head to the replay target, and wakes every still-suspended awaiter with
    /// `Incomplete` (any durable call whose `Start` was committed but whose terminal never was).
    pub(in crate::durable_host::replay_state) fn switch_to_live(&mut self) -> OplogIndex {
        let replay_target = self.cursor.replay_target();
        if self.cursor.last_replayed_index() != replay_target {
            self.record_replay_event(ReplayEvent::ReplayFinished);
        }
        self.cursor.begin_settling();
        self.cursor.position.last_replayed_index.set(replay_target);
        // Replay is over: any durable call whose `Start` was committed but whose terminal never was
        // is incomplete. Wake every still-suspended awaiter so it returns `Incomplete` instead of
        // sleeping forever waiting for a cursor that will not advance again.
        self.st.concurrent_resolver.fail_all_pending_incomplete();
        // Scan-ahead-claimed `Start`s the cursor never reached are moot now: their awaiters were
        // just failed with `Incomplete`, and the cursor will not read again. Retained `Start`s are
        // deliberately kept: a still-unadmitted owner claims them after this switch (its terminal,
        // if retained, resolves it; otherwise it observes `Incomplete` exactly like a claim whose
        // terminal the cursor never reached), and durable-call admission keeps replaying such
        // calls while any retained `Start` exists (see `WorkerState::durable_call_is_live`).
        self.st.claimed_starts.clear();
        self.st.custom_subtrees.clear();
        self.st.replay_buffer.clear();
        self.notify_progress = true;
        replay_target
    }

    pub(in crate::durable_host::replay_state) async fn finish_primary_settling(
        &mut self,
        expected_target: OplogIndex,
        linear_memory: &crate::services::linear_memory::LinearMemoryTracker,
    ) -> Result<LivePublicationOutcome, UnreadableOplog> {
        let phase = self.cursor.transition_phase.load(Ordering::Acquire);
        let replay_target = self.cursor.replay_target();
        let last_replayed_index = self.cursor.last_replayed_index();
        if replay_target != expected_target {
            return Ok(LivePublicationOutcome::ReplayResumed);
        }
        if phase == ReplayTransitionPhase::Live as u8 {
            return Ok(if last_replayed_index == expected_target {
                LivePublicationOutcome::AlreadyLiveAtSameTarget
            } else {
                LivePublicationOutcome::ReplayResumed
            });
        }
        if phase != ReplayTransitionPhase::Settling as u8 {
            return Ok(LivePublicationOutcome::ReplayResumed);
        }

        let mut incomplete_reconstructions =
            self.st.concurrent_resolver.pending_reconstruction_starts();
        if let Some(first_start) = incomplete_reconstructions.iter().min().copied() {
            let mut next = first_start.next();
            while next <= replay_target && !incomplete_reconstructions.is_empty() {
                let available = u64::from(replay_target) - u64::from(next) + 1;
                let entries = self
                    .cursor
                    .read_oplog(next, CHUNK_SIZE.min(available))
                    .await?;
                let last_read = entries
                    .last()
                    .expect("the fixed replay target must remain readable")
                    .0;
                for (index, entry) in entries {
                    if index > replay_target {
                        break;
                    }
                    if !self.st.skipped_regions.is_in_deleted_region(index)
                        && let Some(start_index) = terminal_start_index(&entry)
                    {
                        incomplete_reconstructions.remove(&start_index);
                    }
                }
                next = last_read.next();
            }
        }

        // Reconstruction registration and terminal routing use this same cursor transaction. An
        // unresolved claim may be classified as incomplete only while the target is still the one
        // scanned above. Every claim with a visible terminal remains a publication fence until its
        // body has validated and dropped the reconstruction guard.
        if !self
            .st
            .concurrent_resolver
            .only_pending_reconstruction_fences_remain(&incomplete_reconstructions)
        {
            return Ok(LivePublicationOutcome::ReconstructionClaimsActive);
        }

        #[cfg(test)]
        {
            let publication_gate = self.cursor.primary_publication_gate.lock().unwrap().take();
            if let Some((entered, release)) = publication_gate {
                entered.wait().await;
                release.wait().await;
            }
        }

        let owner_tool_operations = self.cursor.owner_tool_operations.clone();
        if owner_tool_operations.commit_if_owner_open(|| {
            self.switch_to_live();
            linear_memory.switch_to_live();
            self.cursor.publish_live();
        }) {
            Ok(LivePublicationOutcome::Published)
        } else {
            Ok(LivePublicationOutcome::OwnerFailed)
        }
    }

    pub(in crate::durable_host::replay_state) fn finish_non_primary_settling(
        &mut self,
        expected_target: OplogIndex,
    ) -> LivePublicationOutcome {
        let phase = self.cursor.transition_phase.load(Ordering::Acquire);
        let replay_target = self.cursor.replay_target();
        let last_replayed_index = self.cursor.last_replayed_index();
        if replay_target != expected_target {
            return LivePublicationOutcome::ReplayResumed;
        }
        if phase == ReplayTransitionPhase::Live as u8 && last_replayed_index == expected_target {
            return LivePublicationOutcome::AlreadyLiveAtSameTarget;
        }
        if phase != ReplayTransitionPhase::Settling as u8 {
            return LivePublicationOutcome::ReplayResumed;
        }

        self.switch_to_live();
        LivePublicationOutcome::Published
    }
}

impl ReplayState {
    pub(in crate::durable_host::replay_state) fn record_delivery_failure(
        &self,
        failure: WorkerExecutorError,
    ) {
        let mut current = self.cursor.delivery_failure.lock().unwrap();
        if current.is_none() {
            tracing::error!("{failure}");
            *current = Some(failure);
        }
    }

    pub(in crate::durable_host) fn fail_completion_delivery(
        &self,
        start_index: OplogIndex,
        marker_index: OplogIndex,
        reason: impl Into<String>,
    ) {
        self.record_delivery_failure(WorkerExecutorError::runtime(format!(
            "replay could not reproduce CompletionDelivered at {marker_index} for durable call Start at {start_index}: {}",
            reason.into()
        )));
        self.cursor.progress.notify_waiters();
    }

    /// Poisons replay because a markerless completed durable call (its recorded run crashed after
    /// the `End` became durable but before the completion crossed to the guest) hit a delivery
    /// boundary while it was still tail-gated: its delivery token must first wait for the replay
    /// tail via `CompletionDelivery::prepare_delivery`.
    pub(in crate::durable_host) fn fail_tail_delivery(
        &self,
        start_index: OplogIndex,
        reason: impl Into<String>,
    ) {
        self.record_delivery_failure(WorkerExecutorError::runtime(format!(
            "replay could not withhold the markerless completion of durable call Start at {start_index} until the end of the replay tail: {}",
            reason.into()
        )));
        self.cursor.progress.notify_waiters();
    }

    /// Ends a failed replay without publishing its incomplete authority state for live use.
    pub(in crate::durable_host) async fn abandon_replay_for_failure(
        &self,
    ) -> Result<(), WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move {
            state
                .with_tx(async |tx| {
                    if let Some(wallet) = &tx.cursor.wallet {
                        wallet.retire();
                    }
                    tx.switch_to_live();
                    Ok(())
                })
                .await
        })
        .await
    }

    pub(in crate::durable_host::replay_state) async fn switch_cursor_to_live(
        &self,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move {
            let replay_target = state
                .with_tx(async |tx| {
                    let replay_target = tx.switch_to_live();
                    Ok(replay_target)
                })
                .await?;
            Ok(replay_target)
        })
        .await
    }

    async fn begin_primary_settling(&self) -> Result<OplogIndex, WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move {
            state
                .with_tx(async |tx| {
                    tx.cursor.begin_settling();
                    Ok(tx.cursor.replay_target())
                })
                .await
        })
        .await
    }

    #[cfg(test)]
    async fn wait_for_reconstruction_fences(&self) -> Result<(), WorkerExecutorError> {
        tokio::select! {
            biased;
            failure = self.cursor.owner_tool_operations.wait_for_owner_failure() => {
                Err(historical_reconstruction_owner_failure(failure))
            }
            _ = self.cursor.reconstruction_claims.wait_for_fences() => Ok(()),
        }
    }

    #[cfg(test)]
    pub(in crate::durable_host::replay_state) async fn test_wait_for_reconstruction_fences(
        &self,
    ) -> Result<(), WorkerExecutorError> {
        self.wait_for_reconstruction_fences().await
    }

    /// Waits until `Start` claims of this replay have started to wait for an active entity body
    /// at least `waits` times. A claim that wakes and waits again counts once more.
    #[cfg(test)]
    pub(crate) async fn test_wait_for_claims_blocked_on_active_body(&self, waits: u64) {
        let mut counted = self.cursor.active_body_waits.subscribe();
        counted
            .wait_for(|counted| *counted >= waits)
            .await
            .expect("the replay cursor owns the active-body wait counter");
    }

    #[cfg(feature = "test-utils")]
    pub(crate) fn test_is_settling(&self) -> bool {
        self.cursor.transition_phase.load(Ordering::Acquire)
            == ReplayTransitionPhase::Settling as u8
    }

    pub(crate) async fn switch_to_live(
        &self,
        linear_memory: &crate::services::linear_memory::LinearMemoryTracker,
        role: ReplayToLiveRole,
    ) -> Result<ReplayToLiveOutcome, WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move {
            state
                .with_tx(async |tx| {
                    tx.try_get_oplog_entry(|_| false).await?;
                    if tx.cursor.is_replay() {
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            "consumed replay prefix before live authority publication",
                            "unconsumed positional entry or delivery boundary",
                        ));
                    }
                    Ok(())
                })
                .await
        })
        .await?;
        if role == ReplayToLiveRole::PrimaryAgent {
            let replay_target = self.begin_primary_settling().await?;
            self.finish_settling_to_live(linear_memory, role, replay_target)
                .await
        } else {
            let replay_target = self.switch_cursor_to_live().await?;
            linear_memory.switch_to_live();
            Ok(ReplayToLiveOutcome::Live { replay_target })
        }
    }

    pub(crate) async fn finish_settling_to_live(
        &self,
        linear_memory: &crate::services::linear_memory::LinearMemoryTracker,
        role: ReplayToLiveRole,
        replay_target: OplogIndex,
    ) -> Result<ReplayToLiveOutcome, WorkerExecutorError> {
        if role == ReplayToLiveRole::PrimaryAgent {
            loop {
                if let Some(failure) = self.cursor.owner_tool_operations.selected_owner_failure() {
                    return Err(historical_reconstruction_owner_failure(failure));
                }
                let mut reconstruction_fences =
                    self.cursor.reconstruction_claims.subscribe_fences();
                let progress = self.cursor.progress.notified();
                tokio::pin!(progress);
                progress.as_mut().enable();
                let publication_linear_memory = linear_memory.clone();
                let publication = self
                    .run_owned_cursor_op(move |state| async move {
                        state
                            .with_tx(async |tx| {
                                Ok(tx
                                    .finish_primary_settling(
                                        replay_target,
                                        &publication_linear_memory,
                                    )
                                    .await?)
                            })
                            .await
                    })
                    .await?;
                match publication {
                    LivePublicationOutcome::Published => break,
                    LivePublicationOutcome::AlreadyLiveAtSameTarget => {
                        linear_memory.switch_to_live();
                        break;
                    }
                    LivePublicationOutcome::ReconstructionClaimsActive => {
                        tokio::select! {
                            biased;
                            failure = self.cursor.owner_tool_operations.wait_for_owner_failure() => {
                                return Err(historical_reconstruction_owner_failure(failure));
                            }
                            changed = reconstruction_fences.changed() => {
                                changed.expect("replay cursor retains the reconstruction claim state");
                            }
                            _ = progress.as_mut() => {}
                        }
                    }
                    LivePublicationOutcome::OwnerFailed => {
                        let failure = self
                            .cursor
                            .owner_tool_operations
                            .selected_owner_failure()
                            .expect("failed live publication must retain the owner winner");
                        return Err(historical_reconstruction_owner_failure(failure));
                    }
                    LivePublicationOutcome::ReplayResumed => {
                        return Ok(ReplayToLiveOutcome::ReplayResumed);
                    }
                }
            }
            Ok(ReplayToLiveOutcome::Live { replay_target })
        } else {
            let publication = self
                .run_owned_cursor_op(move |state| async move {
                    state
                        .with_tx(async |tx| Ok(tx.finish_non_primary_settling(replay_target)))
                        .await
                })
                .await?;
            match publication {
                LivePublicationOutcome::Published
                | LivePublicationOutcome::AlreadyLiveAtSameTarget => {
                    linear_memory.switch_to_live();
                    Ok(ReplayToLiveOutcome::Live { replay_target })
                }
                LivePublicationOutcome::ReconstructionClaimsActive => {
                    unreachable!("non-primary settlement does not inspect reconstruction claims")
                }
                LivePublicationOutcome::OwnerFailed => {
                    unreachable!("non-primary settlement does not arbitrate owner publication")
                }
                LivePublicationOutcome::ReplayResumed => Ok(ReplayToLiveOutcome::ReplayResumed),
            }
        }
    }

    pub(crate) fn historical_reconstruction_bodies(
        &self,
    ) -> tokio::sync::watch::Receiver<HashSet<OplogIndex>> {
        self.cursor.reconstruction_claims.subscribe_bodies()
    }

    pub(crate) fn ensure_reconstruction_claims_empty(&self) -> Result<(), WorkerExecutorError> {
        self.cursor.reconstruction_claims.ensure_empty()
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.cursor.is_live()
    }

    /// Returns whether the primary owner has published live admission after reconstruction
    /// settlement. Cursor exhaustion alone is intentionally not sufficient.
    pub(crate) fn is_live_published(&self) -> bool {
        self.cursor.is_live_published()
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        self.cursor.is_replay()
    }
}
