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
    pub(crate) async fn archive_oplog(
        self: &Arc<Self>,
        last_oplog_index: OplogIndex,
        wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError> {
        let _lifecycle = self
            .oplog_service()
            .lock_lifecycle(&self.owned_agent_id.agent_id)
            .await;
        let instance = self.instance.lock().await;
        if instance.ensure_not_deleting().is_err() || self.oplog.is_retired() {
            return Ok(None);
        }
        if !self.active_agents().contains_worker_generation(self).await {
            return Err(WorkerExecutorError::runtime(
                "Archival worker left the active cache; retry with the current owner",
            ));
        }
        drop(instance);
        let current_oplog_index = self
            .oplog_service()
            .try_get_last_index(&self.owned_agent_id, self.agent_mode())
            .await
            .map_err(WorkerExecutorError::runtime)?;
        if !scheduled_archive_is_current(current_oplog_index, last_oplog_index) {
            return Ok(None);
        }
        let result = match wait {
            ArchiveWait::Queued => match MultiLayerOplog::try_archive(&self.oplog).await {
                Ok(Some(more)) => Some(more),
                Ok(None) => EphemeralOplog::try_archive(&self.oplog)
                    .await
                    .map_err(WorkerExecutorError::runtime)?,
                Err(error) => return Err(WorkerExecutorError::runtime(error)),
            },
            ArchiveWait::Finished => match MultiLayerOplog::try_archive_blocking(&self.oplog).await
            {
                Ok(Some(more)) => Some(more),
                Ok(None) => EphemeralOplog::try_archive_blocking(&self.oplog)
                    .await
                    .map_err(WorkerExecutorError::runtime)?,
                Err(error) => return Err(WorkerExecutorError::runtime(error)),
            },
        };
        if result == Some(false) {
            self.worker_service()
                .remove_cached_status(
                    &self.owned_agent_id,
                    self.initial_worker_metadata.fingerprint,
                )
                .await?;
        }
        Ok(result)
    }
    pub async fn get_latest_worker_metadata(&self) -> AgentMetadata {
        let updated_status = self.state_actor.status().await.as_ref().clone();
        let result = self.get_initial_worker_metadata();
        AgentMetadata {
            last_known_status: updated_status,
            ..result
        }
    }
    /// Returns the authoritative projection after preceding actor transactions complete, without
    /// committing buffered entries. The immutable `Arc` avoids copying the large result index.
    pub async fn get_last_known_status(&self) -> Arc<AgentStatusRecord> {
        self.state_actor.status().await
    }
    /// The projection as [`Self::get_last_known_status`] gives it, rebuilt first when a failed
    /// rebuild left it detached. A rebuild that fails again gives the refusal, reported as in
    /// [`Self::commit_oplog_and_update_state`].
    pub(crate) async fn get_repaired_status(
        &self,
    ) -> Result<Arc<AgentStatusRecord>, WorkerExecutorError> {
        self.state_actor
            .repaired_status()
            .await
            .map_err(|refusal| self.refused_by(refusal).into())
    }
    /// Whether the published status is detached from the oplog: a rebuild runs, or one failed.
    pub(crate) fn last_known_status_detached(&self) -> bool {
        self.last_known_status_detached.load(Ordering::Acquire)
    }
    pub(crate) async fn get_export_fork_status(
        &self,
    ) -> Result<Arc<AgentStatusRecord>, WorkerExecutorError> {
        self.state_actor.try_status().await
    }
    pub(crate) async fn read_export_fork_candidate(
        &self,
        index: OplogIndex,
    ) -> Result<golem_common::model::durable_stream::StreamExportForkCandidate, WorkerExecutorError>
    {
        if let OplogEntry::StreamSession { record, .. } = self.oplog.read(index).await?
            && let StreamSessionRecord::ExportForkAdmitted(record) = self
                .oplog
                .download_payload(record)
                .await
                .map_err(WorkerExecutorError::runtime)?
        {
            return Ok(record.candidate);
        }
        Err(WorkerExecutorError::runtime(
            "Fork admission index does not reference an admission record",
        ))
    }
    pub(crate) async fn reserve_export_fork(
        self: &Arc<Self>,
        target: AgentId,
        request_hash: Vec<u8>,
        candidate: golem_common::model::durable_stream::StreamExportForkCandidate,
        session_limit: u32,
        rate_limit: u32,
    ) -> Result<crate::services::worker_fork::admission::Admission, WorkerExecutorError> {
        use crate::services::worker_fork::admission::{self, Admission};

        let instance_guard = self.lock_non_stopping_worker_owned().await;
        instance_guard.ensure_not_deleting()?;
        if self.owner_retirement_requested.is_cancelled() || self.oplog.is_retired() {
            return Err(self.retirement_error());
        }
        let status = self.state_actor.try_status().await?;
        if candidate.export.source_fingerprint != self.initial_worker_metadata.fingerprint
            || status
                .deleted_regions
                .is_in_deleted_region(candidate.horizon)
        {
            return Ok(Admission::Conflict);
        }
        let record = match admission::reserve(
            &status.export_fork_admissions,
            target,
            request_hash,
            candidate,
            session_limit,
            rate_limit,
            Timestamp::now_utc().to_millis(),
        ) {
            Ok(record) => record,
            Err(admission) => return Ok(admission),
        };
        let record = StreamSessionRecord::ExportForkAdmitted(record);
        let summary = DurableStreamEventSummary::session(&record);
        let payload = self
            .oplog
            .upload_payload_owned(record)
            .await
            .map_err(WorkerExecutorError::runtime)?;
        self.state_actor
            .append_and_commit_attached(
                OplogEntry::stream_session(None, payload, summary),
                self.clone(),
                instance_guard,
                None,
            )
            .await?;
        Ok(Admission::Reserved)
    }
    /// Appends an oplog entry without forcing a durable commit. Callers that
    /// require ordering must await the append before exposing subsequent work.
    pub async fn add_to_oplog(&self, entry: OplogEntry) -> Result<OplogIndex, OplogError> {
        self.oplog
            .add(entry)
            .await
            .map_err(|error| self.retire_if_fenced(error))
    }
    /// Gives `error`, and records the retirement for a lost shard first when it is a fence.
    pub(in crate::worker) fn retire_if_fenced(&self, error: OplogError) -> OplogError {
        match error {
            OplogError::Fenced(fence) => self.retired_by(fence),
            error => error,
        }
    }
    /// Appends an entry on a path that has no way to report the failure to its caller.
    ///
    /// A fenced write means the shard moved while this agent was resident. The retirement is
    /// recorded so the stop that follows drops the agent from this executor rather than writing to
    /// an oplog another executor owns now, and `OplogIndex::NONE` is returned for the entry that was
    /// not written - the same "no index" value a debugging session's discarded write returns.
    ///
    /// Recorded rather than stopped here on purpose: these callers run under the worker lifecycle
    /// lock and inside the wasm store, where stopping would deadlock on the lock it already
    /// holds. The fence latches on the oplog, so the invocation's next write is refused too and
    /// unwinds the loop, which is where the stop belongs.
    ///
    /// A failed archive write is latched on the oplog in the same way, so it also gives
    /// `OplogIndex::NONE`, and the next write that can report it stops the agent with the cause.
    /// Every other storage failure keeps the fail-stop behaviour it has always had.
    pub async fn add_to_oplog_or_retire(&self, entry: OplogEntry) -> OplogIndex {
        match self.add_to_oplog(entry).await {
            Ok(index) => index,
            Err(error) => match error.into_refusal() {
                Ok(_) => OplogIndex::NONE,
                Err(error) => panic!("oplog write: {error}"),
            },
        }
    }
    /// Commits the buffered entries and folds them into the published status.
    ///
    /// A commit the oplog refused is returned as the refusal: `OplogError::Fenced`, for which the
    /// agent is marked retired for a lost shard, `OplogError::ArchiveWrite`, or
    /// `OplogError::Unreadable` when the status after the commit does not read. It has to be
    /// reported rather than folded into "nothing changed": below the
    /// commit threshold an add only buffers, so this commit is where a takeover is found, and a
    /// caller about to run a side effect, publish a result or acknowledge a request must not do
    /// it for entries that never reached the storage. The status actor has already started the
    /// retirement that drops the agent from this executor.
    pub async fn commit_oplog_and_update_state(
        &self,
        commit_level: CommitLevel,
    ) -> Result<OplogIndex, OplogError> {
        match self.state_actor.commit_and_update_state(commit_level).await {
            Ok((index, _)) => Ok(index),
            Err(refusal) => Err(self.refused_by(refusal)),
        }
    }
    /// Enqueues an actor-owned commit + fold and waits only until the commit is acknowledged.
    /// Later ordered status reads remain behind the fold on the same FIFO queue.
    ///
    /// A refused commit is acknowledged with the refusal, reported as in
    /// [`Self::commit_oplog_and_update_state`]. No acknowledgement at all means the actor
    /// stopped, which stays fatal.
    pub async fn commit_oplog_before_status_update(
        &self,
        commit_level: CommitLevel,
    ) -> Result<(), OplogError> {
        match self
            .state_actor
            .enqueue_commit_and_update_state_notifying(commit_level)
            .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(refusal)) => Err(self.refused_by(refusal)),
            Err(_) => panic!(
                "Worker state actor for {} stopped before acknowledging commit",
                self.owned_agent_id
            ),
        }
    }
    /// Adds an entry and commits it, reporting a refusal of either as in
    /// [`Self::commit_oplog_and_update_state`].
    ///
    /// Every other storage failure keeps the fail-stop behaviour of
    /// [`Self::add_to_oplog_or_retire`].
    pub async fn add_and_commit_oplog(&self, entry: OplogEntry) -> Result<OplogIndex, OplogError> {
        let index = self.add_to_oplog(entry).await?;
        self.commit_oplog_and_update_state(CommitLevel::Always)
            .await?;
        Ok(index)
    }
    /// Records the lost shard whose oplog refused a write, and returns the refusal for the
    /// caller to propagate.
    pub(crate) fn retired_by(&self, fence: OplogFence) -> OplogError {
        self.record_retirement(
            InterruptKind::ShardLost,
            RetirementReason::Fenced(Some(fence.clone())),
        );
        OplogError::Fenced(fence)
    }
    /// Returns a refusal of the oplog for the caller to propagate, and records the lost shard
    /// first when the refusal is a fence.
    pub(crate) fn refused_by(&self, refusal: OplogRefusal) -> OplogError {
        match refusal {
            OplogRefusal::Fenced(fence) => self.retired_by(fence),
            refusal => refusal.into(),
        }
    }
    pub async fn queue_card_revocation(
        &self,
        card_id: CardId,
    ) -> Result<Option<OplogIndex>, OplogError> {
        Ok(self
            .queue_card_revocations(&[card_id])
            .await?
            .into_iter()
            .next())
    }
    pub async fn queue_card_revocations(
        &self,
        card_ids: &[CardId],
    ) -> Result<Vec<OplogIndex>, OplogError> {
        let boundary_lock = self.card_event_boundary_lock.clone();
        let _boundary_guard = boundary_lock.lock().await;
        self.queue_card_revocations_locked(card_ids).await
    }
    pub(crate) async fn queue_card_revocations_locked(
        &self,
        card_ids: &[CardId],
    ) -> Result<Vec<OplogIndex>, OplogError> {
        let status = self.state_actor.status().await;
        let pending_revocations = status
            .pending_card_events
            .iter()
            .filter_map(|pending_event| match &pending_event.event {
                QueuedCardEvent::Revoke(event) => Some(event.card_id),
                QueuedCardEvent::Install(_)
                | QueuedCardEvent::TransferStarted(_)
                | QueuedCardEvent::TransferReceived(_) => None,
            })
            .collect::<HashSet<_>>();
        let mut card_ids = card_ids
            .iter()
            .copied()
            .filter(|card_id| {
                !status.revoked_cards.contains(card_id) && !pending_revocations.contains(card_id)
            })
            .collect::<Vec<_>>();
        card_ids.sort_unstable();
        card_ids.dedup();

        let mut queued_event_indices = Vec::with_capacity(card_ids.len());
        for card_id in card_ids {
            queued_event_indices.push(
                self.add_to_oplog(OplogEntry::card_event_queued(
                    None,
                    Box::new(QueuedCardEvent::revoke(card_id)),
                ))
                .await?,
            );
        }
        if !queued_event_indices.is_empty() {
            self.commit_oplog_and_update_state(CommitLevel::Always)
                .await?;
        }

        Ok(queued_event_indices)
    }
    pub async fn receive_card_transfer(
        self: &Arc<Self>,
        transfer_id: Uuid,
        source_card_id: CardId,
        card: StoredCard,
    ) -> Result<(), WorkerExecutorError> {
        loop {
            let instance_guard = self.lock_non_stopping_worker_owned().await;

            if instance_guard.ensure_not_deleting().is_err() {
                return Err(WorkerExecutorError::invalid_request(
                    "Cannot deliver a permission card to a deleting worker",
                ));
            }

            // Ordered updates can own the boundary while a preceding lifecycle notification needs
            // the instance lock. Never retain the instance lock while waiting for that boundary.
            let Ok(boundary_guard) = self.card_event_boundary_lock.clone().try_lock_owned() else {
                drop(instance_guard);
                let boundary_guard = self.card_event_boundary_lock.clone().lock_owned().await;
                drop(boundary_guard);
                continue;
            };

            let status = self.state_actor.status().await;
            if let Some(received) = status.received_card_transfers.get(&transfer_id) {
                return match received {
                    golem_common::model::ReceivedCardTransferState::Received {
                        source_card_id: recorded_source_card_id,
                        card: recorded_card,
                    } if *recorded_source_card_id == source_card_id && recorded_card == &card => {
                        Ok(())
                    }
                    golem_common::model::ReceivedCardTransferState::Received { .. }
                    | golem_common::model::ReceivedCardTransferState::Conflict => {
                        Err(WorkerExecutorError::invalid_request(
                            PERMISSION_CARD_TRANSFER_PAYLOAD_CONFLICT,
                        ))
                    }
                };
            }

            let parsed_agent_id = self.parsed_agent_id.as_ref().ok_or_else(|| {
                WorkerExecutorError::invalid_request(
                    "permission cards can only be delivered to agents",
                )
            })?;
            let component = self.current_component.load();
            let target_context =
                agent_monomorphization_context(&component, &self.owned_agent_id, parsed_agent_id);
            if !card_matches_agent_recipient(&card, &target_context) {
                return Err(WorkerExecutorError::invalid_request(
                    PERMISSION_CARD_INSTALL_RECIPIENT_MISMATCH,
                ));
            }

            // A refused entry is not delivered: the sender learns the shard moved instead of treating
            // a transfer this oplog never recorded as received.
            self.state_actor
                .append_and_commit_attached(
                    OplogEntry::card_event_queued(
                        None,
                        Box::new(QueuedCardEvent::transfer_received(
                            transfer_id,
                            source_card_id,
                            card,
                        )),
                    ),
                    self.clone(),
                    instance_guard,
                    Some(boundary_guard),
                )
                .await?;

            return Ok(());
        }
    }
    pub(crate) fn published_authority_generation(&self) -> Arc<AtomicU64> {
        self.published_authority_generation.clone()
    }
    /// [`Self::add_and_commit_oplog`] for a caller holding the worker lifecycle lock, which sends
    /// `wakeup` to the running loop when the commit changed the status.
    ///
    /// A refusal is returned rather than acknowledged: the management request that wrote the
    /// entry must not report it as accepted.
    pub(in crate::worker) async fn add_and_commit_oplog_internal(
        &self,
        instance_guard: &MutexGuard<'_, WorkerInstance>,
        entry: OplogEntry,
        wakeup: Option<WorkerCommand>,
    ) -> Result<OplogIndex, OplogError> {
        let index = self.add_to_oplog(entry).await?;
        // The caller holds the lifecycle lock. The actor only enqueues its notification and never
        // awaits that lock, so this commit and the caller's specific wakeup below cannot deadlock.
        let (_, changed) = self
            .state_actor
            .commit_and_update_state(CommitLevel::Always)
            .await
            .map_err(|refusal| self.refused_by(refusal))?;

        if changed
            && let Some(wakeup) = wakeup
            && let WorkerInstance::Running(running) = &**instance_guard
        {
            running.sender.send(wakeup).unwrap();
        };

        Ok(index)
    }
    /// Writes a *clean* status checkpoint from the current in-memory status if eligible (see
    /// [`status_checkpointer::StatusCheckpointer::maybe_checkpoint`]).
    ///
    /// Must only be called at structurally clean boundaries where no jumpable oplog region is open
    /// (snapshot save, idle suspend). Skipped while the status is detached, because then the
    /// in-memory status is not authoritative — checkpointing it could persist a baseline inside a
    /// region. Best-effort and bounded by the throttle; never blocks meaningfully.
    pub(crate) async fn checkpoint_status(&self, reason: status_checkpointer::CheckpointReason) {
        if self.last_known_status_detached.load(Ordering::Acquire) {
            return;
        }
        let status = self.last_known_status.load_full();
        self.status_checkpointer
            .maybe_checkpoint(&status, reason)
            .await;
    }
    /// Writes a *clean* status checkpoint *during* a long-running invocation, taken from the current
    /// committed in-memory status (the caller must only invoke this right after a durable commit, so
    /// `last_known_status` reflects the committed oplog tip).
    pub(crate) async fn checkpoint_status_mid_invocation(&self) {
        if self.last_known_status_detached.load(Ordering::Acquire) {
            return;
        }
        let status = self.last_known_status.load_full();
        self.status_checkpointer
            .maybe_checkpoint(
                &status,
                status_checkpointer::CheckpointReason::MidInvocation,
            )
            .await;
    }
    /// Synchronously persists any pending cached-status changes for this worker. Used at lifecycle
    /// boundaries (e.g. suspend) so the cached blob is up to date when the worker goes idle, rather
    /// than waiting for the next background sweep.
    pub(crate) async fn force_flush_status(&self) {
        // Best-effort: a failure is logged/metered and re-queued inside `flush`; the blob is
        // reconstructable from the oplog so it must not block the caller (e.g. suspend).
        if let Err(err) = self
            .status_flusher
            .flush(status_flusher::FlushReason::Forced)
            .await
        {
            debug!("Forced status flush failed (will retry in background): {err}");
        }
    }
}

// A scheduled archive may outlive later commits. Archiving the current prefix preserves the
// retry; an index behind the scheduled one indicates stale or recreated state.
pub(in crate::worker) fn scheduled_archive_is_current(
    current: OplogIndex,
    scheduled: OplogIndex,
) -> bool {
    current >= scheduled
}
