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

impl ForwardingOplogState {
    async fn while_forwarding<F: std::future::Future>(&self, future: F) -> Option<F::Output> {
        tokio::select! {
            biased;
            _ = self.forwarding_retired.cancelled() => None,
            result = future => Some(result),
        }
    }

    pub(super) fn debug_assert_invariants(&self) {
        match &self.buffer {
            Some(buffer) => {
                debug_assert!(!buffer.is_empty());
                debug_assert_eq!(
                    self.buffer_start_idx.range_end(buffer.len() as u64),
                    self.last_oplog_idx
                );
            }
            None => debug_assert_eq!(self.buffer_start_idx, self.last_oplog_idx.next()),
        }
    }

    pub(super) fn cache_is_required(&self) -> bool {
        !self.last_known_status.get().active_plugins.is_empty()
            || self
                .plugin_state
                .values()
                .any(|state| state.sending_up_to > state.confirmed_up_to)
    }

    pub(super) fn record_cached(
        &mut self,
        entries: impl IntoIterator<Item = (OplogIndex, OplogEntry)>,
    ) {
        let mut entries = entries.into_iter();
        let Some((first_idx, first_entry)) = entries.next() else {
            return;
        };

        debug_assert_eq!(first_idx, self.last_oplog_idx.next());
        if self.buffer.is_none() {
            self.buffer_start_idx = first_idx;
            self.buffer = Some(VecDeque::new());
        }
        self.buffer.as_mut().unwrap().push_back(first_entry);
        self.last_oplog_idx = first_idx;

        for (idx, entry) in entries {
            debug_assert_eq!(idx, self.last_oplog_idx.next());
            self.buffer.as_mut().unwrap().push_back(entry);
            self.last_oplog_idx = idx;
        }
    }

    pub(super) fn record_uncached(&mut self, indices: impl IntoIterator<Item = OplogIndex>) {
        for idx in indices {
            debug_assert_eq!(idx, self.last_oplog_idx.next());
            self.last_oplog_idx = idx;
        }
        self.buffer = None;
        self.buffer_start_idx = self.last_oplog_idx.next();
    }

    /// Cursor-driven flush: for each plugin with unsent entries, send a batch.
    /// Handles retries of in-flight batches, first-send target resolution,
    /// pre-send and confirmation checkpoint writes.
    ///
    /// Complete ranges are read from the in-memory suffix when available; all other
    /// ranges are read whole from the persisted oplog without splicing sources.
    pub async fn try_flush(&mut self) {
        let Some(_activity) = self.forwarding_activity.try_enter() else {
            return;
        };
        if self.forwarding_retired.is_cancelled() {
            return;
        }
        // A fenced oplog is finished: nothing may be sent or checkpointed from it again. Without
        // this, every tick and threshold flush picks the same batch and writes its checkpoint
        // again, taking an index on an oplog that refuses the commit. The bookkeeping below is
        // skipped as well, so the mirrored buffer is no longer pruned and the commit count no
        // longer reset; both last only until the fenced worker is given up.
        if self.inner.fence().is_some() {
            return;
        }
        let status = self.last_known_status.get();
        let flush_set = self.reconcile_plugin_state(&status);

        if flush_set.is_empty() {
            self.finish_empty_flush();
        } else if let Some(component_metadata) = self.get_component_metadata(&status).await {
            let committed_tail = self.last_committed_idx;

            for grant_id in flush_set {
                if self.forwarding_retired.is_cancelled() {
                    return;
                }
                self.flush_one_plugin(grant_id, committed_tail, &status, &component_metadata)
                    .await;
            }

            self.prune_buffer();
            self.finish_flush_cycle();
        }
    }

    /// Sync plugin_state with the current status: add newly active plugins,
    /// keep existing local state for already-tracked plugins, remove stale
    /// entries that are neither active nor in-flight.
    /// Returns the flush set (active ∪ in-flight plugin grant IDs).
    fn reconcile_plugin_state(
        &mut self,
        status: &AgentStatusRecord,
    ) -> Vec<EnvironmentPluginGrantId> {
        for grant_id in &status.active_plugins {
            if let Entry::Vacant(e) = self.plugin_state.entry(*grant_id) {
                let live = if let Some(cp) = status.oplog_processor_checkpoints.get(grant_id) {
                    LivePluginState {
                        target_agent_id: cp.target_agent_id.clone(),
                        confirmed_up_to: cp.confirmed_up_to,
                        sending_up_to: cp.sending_up_to,
                        send_in_progress: false,
                        last_batch_start: cp.last_batch_start,
                    }
                } else {
                    LivePluginState {
                        target_agent_id: None,
                        confirmed_up_to: OplogIndex::NONE,
                        sending_up_to: OplogIndex::NONE,
                        send_in_progress: false,
                        last_batch_start: OplogIndex::NONE,
                    }
                };
                e.insert(live);
            }
        }

        self.plugin_state.retain(|grant_id, state| {
            status.active_plugins.contains(grant_id) || state.sending_up_to > state.confirmed_up_to
        });

        self.plugin_state
            .iter()
            .filter(|(id, state)| {
                status.active_plugins.contains(id) || state.sending_up_to > state.confirmed_up_to
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Fetch component metadata needed for the flush.
    /// Returns `None` if component metadata cannot be retrieved.
    async fn get_component_metadata(&self, status: &AgentStatusRecord) -> Option<Component> {
        match self
            .while_forwarding(self.components.get_metadata(
                self.initial_worker_metadata.owned_agent_id().component_id(),
                Some(status.component_revision),
            ))
            .await?
        {
            Ok(component_metadata) => Some(component_metadata),
            Err(err) => {
                tracing::error!(
                    "Failed to get component metadata for oplog processor flush: {err}"
                );
                None
            }
        }
    }

    /// Flush a single plugin: resolve target, read entries, write checkpoints, send.
    ///
    /// Uses enqueue-and-confirm: after a successful `send()` (which only enqueues
    /// the invocation), we immediately write the confirmation checkpoint. A background
    /// monitoring task is spawned to observe errors and emit plugin_error events.
    async fn flush_one_plugin(
        &mut self,
        grant_id: EnvironmentPluginGrantId,
        committed_tail: OplogIndex,
        status: &AgentStatusRecord,
        component_metadata: &Component,
    ) {
        let live = match self.plugin_state.get(&grant_id) {
            Some(s) if !s.send_in_progress => s.clone(),
            _ => return,
        };

        let agent_type =
            ParsedAgentId::parse_agent_type_name(&self.initial_worker_metadata.agent_id.agent_id)
                .ok();
        let plugin = match component_metadata
            .metadata
            .owner_plugins(self.initial_worker_metadata.owner_kind, agent_type.as_ref())
            .and_then(|plugins| {
                plugins
                    .iter()
                    .find(|p| p.environment_plugin_grant_id == grant_id)
            }) {
            Some(p) => p.clone(),
            None => return,
        };

        let batch = if live.sending_up_to > live.confirmed_up_to {
            // RETRY: exact same range — never widen
            Some((live.confirmed_up_to.next(), live.sending_up_to, true))
        } else if live.confirmed_up_to < committed_tail {
            // NEW BATCH: from confirmed+1 to committed tail
            Some((live.confirmed_up_to.next(), committed_tail, false))
        } else {
            None
        };

        let (batch_start, batch_end, is_retry) = match batch {
            Some(b) => b,
            None => return,
        };

        tracing::info!(
            plugin_name = plugin.plugin_name,
            source_agent = %self.initial_worker_metadata.agent_id,
            batch_start = %batch_start,
            batch_end = %batch_end,
            is_retry,
            "Oplog processor: flushing batch to plugin"
        );

        let target_agent_id = if let Some(id) = &live.target_agent_id {
            id.clone()
        } else {
            let Some(result) = self
                .while_forwarding(
                    self.oplog_plugins
                        .resolve_target(self.initial_worker_metadata.environment_id, &plugin),
                )
                .await
            else {
                return;
            };
            match result {
                Ok(id) => {
                    tracing::info!(
                        plugin_name = plugin.plugin_name,
                        target_agent = %id,
                        "Oplog processor: resolved target plugin worker"
                    );
                    if self
                        .write_checkpoint(
                            grant_id,
                            &id,
                            live.confirmed_up_to,
                            live.confirmed_up_to,
                            live.last_batch_start,
                        )
                        .await
                        .is_err()
                    {
                        return;
                    }
                    if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                        s.target_agent_id = Some(id.clone());
                    }
                    id
                }
                Err(err) => {
                    tracing::error!("Failed to resolve target for plugin {grant_id}: {err}");
                    return;
                }
            }
        };

        // Capture locality before the send so error handling uses the
        // same classification as the actual send path.
        let Some(target_is_local) = self
            .while_forwarding(self.oplog_plugins.is_local(&target_agent_id))
            .await
        else {
            return;
        };
        let target_is_local = target_is_local.unwrap_or(true);

        if let Some(s) = self.plugin_state.get_mut(&grant_id) {
            s.send_in_progress = true;
        }

        let batch_count = (batch_end.as_u64() - batch_start.as_u64() + 1) as usize;
        let entries = match self.read_batch(batch_start, batch_count).await {
            Ok(entries) => entries,
            Err(error) => {
                tracing::error!(
                    grant_id = %grant_id,
                    error = %error,
                    "Oplog processor: failed to read the batch to deliver"
                );
                if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                    s.send_in_progress = false;
                }
                return;
            }
        };
        if self.forwarding_retired.is_cancelled() {
            return;
        }

        // If ALL entries are checkpoint entries, skip — nothing meaningful to deliver.
        // Advance the cursor past them so we don't re-read the same range forever.
        if entries
            .iter()
            .all(|e| matches!(e, OplogEntry::OplogProcessorCheckpoint { .. }))
        {
            if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                s.confirmed_up_to = batch_end;
                s.sending_up_to = batch_end;
                s.send_in_progress = false;
            }
            return;
        }

        if !is_retry {
            if self
                .write_checkpoint(
                    grant_id,
                    &target_agent_id,
                    live.confirmed_up_to,
                    batch_end,
                    batch_start,
                )
                .await
                .is_err()
            {
                return;
            }
            if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                s.sending_up_to = batch_end;
            }
        }

        let metadata = AgentMetadata {
            agent_id: self.initial_worker_metadata.agent_id.clone(),
            owner_kind: self.initial_worker_metadata.owner_kind,
            env: self.initial_worker_metadata.env.clone(),
            environment_id: self.initial_worker_metadata.environment_id,
            created_by: self.initial_worker_metadata.created_by,
            created_by_email: self.initial_worker_metadata.created_by_email.clone(),
            config: self.initial_worker_metadata.config.clone(),
            created_at: self.initial_worker_metadata.created_at,
            parent: self.initial_worker_metadata.parent.clone(),
            last_known_status: status.clone(),
            original_phantom_id: self.initial_worker_metadata.original_phantom_id,
            fingerprint: self.initial_worker_metadata.fingerprint,
            agent_mode: self.initial_worker_metadata.agent_mode,
        };

        let Some(result) = self
            .while_forwarding(self.oplog_plugins.send(
                metadata,
                &plugin,
                &target_agent_id,
                batch_start,
                entries,
            ))
            .await
        else {
            return;
        };
        match result {
            Ok(()) => {
                tracing::info!(
                    plugin_name = plugin.plugin_name,
                    source_agent = %self.initial_worker_metadata.agent_id,
                    target_agent = %target_agent_id,
                    batch_start = %batch_start,
                    batch_end = %batch_end,
                    "Oplog processor: batch enqueued successfully"
                );
                // Enqueue succeeded — immediately confirm
                if self
                    .write_checkpoint(
                        grant_id,
                        &target_agent_id,
                        batch_end,
                        batch_end,
                        batch_start,
                    )
                    .await
                    .is_err()
                {
                    return;
                }
                if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                    s.confirmed_up_to = batch_end;
                    s.sending_up_to = batch_end;
                    s.send_in_progress = false;
                    s.last_batch_start = batch_start;
                }

                // Spawn background monitoring task to observe errors.
                // Compute the idempotency key here so only lightweight data
                // needs to be moved into the task.
                let idempotency_key = oplog_processor_idempotency_key(
                    &self.initial_worker_metadata.agent_id,
                    &plugin.environment_plugin_grant_id,
                    batch_start,
                    batch_end,
                );
                let worker_event_service = self.worker_event_service.clone();
                let oplog_plugins = self.oplog_plugins.clone();
                let environment_id = self.initial_worker_metadata.environment_id;
                let caller_account_id = self.initial_worker_metadata.created_by;
                let target_clone = target_agent_id.clone();
                let forwarding_retired = self.forwarding_retired.clone();
                // The task polls on after this flush returned, so it links back to
                // the flush rather than running inside its span. See `TraceOrigin`.
                let monitor_span = related_span!(
                    TraceOrigin::capture_current(),
                    tracing::Level::INFO,
                    "oplog_plugin_batch_monitor",
                    agent_id = %self.initial_worker_metadata.agent_id,
                    grant_id = %grant_id,
                    batch_start = %batch_start,
                    batch_end = %batch_end
                );
                let monitor = tokio::spawn(
                    async move {
                        // Poll until the invocation completes, with a timeout
                        let deadline = Instant::now() + Duration::from_secs(300);
                        loop {
                            if Instant::now() >= deadline {
                                tracing::warn!(
                                    "Plugin {grant_id}: monitoring timed out for batch [{batch_start}..{batch_end}]"
                                );
                                break;
                            }

                            tokio::select! {
                                biased;
                                _ = forwarding_retired.cancelled() => break,
                                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                            }

                            let result = tokio::select! {
                                biased;
                                _ = forwarding_retired.cancelled() => break,
                                result = oplog_plugins.lookup_invocation_status(
                                    environment_id,
                                    &target_clone,
                                    caller_account_id,
                                    &idempotency_key,
                                ) => result,
                            };
                            match result {
                                Ok(InvocationStatus::Complete) => {
                                    // Completed — nothing to do
                                    break;
                                }
                                Ok(InvocationStatus::Unknown) => {
                                    tracing::warn!(
                                        "Plugin {grant_id}: invocation status unknown for batch [{batch_start}..{batch_end}]"
                                    );
                                    break;
                                }
                                Ok(_) => {
                                    // Still pending — continue polling
                                }
                                Err(err) => {
                                    tracing::error!(
                                        "Plugin {grant_id} error monitoring batch [{batch_start}..{batch_end}]: {err}"
                                    );
                                    if let Some(event_service) = &worker_event_service {
                                        event_service.emit_event(
                                            InternalWorkerEvent::plugin_error(
                                                &format!("{grant_id}"),
                                                &format!("Error monitoring batch [{batch_start}..{batch_end}]: {err}"),
                                            ),
                                            true,
                                        );
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    .instrument(monitor_span),
                );
                self.monitor_tasks.push(monitor.into());
            }
            Err(err) => {
                tracing::error!("Failed to enqueue oplog entries to plugin {grant_id}: {err}");
                if let Some(event_service) = &self.worker_event_service {
                    event_service.emit_event(
                        InternalWorkerEvent::plugin_error(
                            &format!("{grant_id}"),
                            &format!("Failed to enqueue batch [{batch_start}..{batch_end}]: {err}"),
                        ),
                        true,
                    );
                }

                if target_is_local {
                    // Local failure: invalidate target so next flush creates a fresh instance
                    if self
                        .while_forwarding(self.oplog_plugins.invalidate_target(
                            self.initial_worker_metadata.environment_id,
                            &plugin,
                        ))
                        .await
                        .is_none()
                    {
                        return;
                    }
                    if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                        s.target_agent_id = None;
                        s.send_in_progress = false;
                    }
                } else {
                    // Remote failure: keep the recorded target to avoid duplicating
                    // delivery across two different plugin workers. The batch remains
                    // pending and will be retried against the same recorded target.
                    if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                        s.send_in_progress = false;
                    }
                }
            }
        }
    }

    /// Read `count` entries starting at `start` from the buffer if available,
    /// otherwise fall back to the persisted oplog.
    async fn read_batch(
        &self,
        start: OplogIndex,
        count: usize,
    ) -> Result<Vec<OplogEntry>, UnreadableOplog> {
        if count == 0 {
            return Ok(Vec::new());
        }

        if let Some(buffer) = &self.buffer
            && start >= self.buffer_start_idx
        {
            let buffer_end_idx = self.buffer_start_idx.range_end(buffer.len() as u64);
            let request_end_idx = start.range_end(count as u64);

            if request_end_idx <= buffer_end_idx {
                let offset = (start.as_u64() - self.buffer_start_idx.as_u64()) as usize;
                return Ok(buffer.iter().skip(offset).take(count).cloned().collect());
            }
        }

        Ok(self
            .inner
            .read_exact(start, count as u64)
            .await?
            .into_values()
            .collect())
    }

    /// Write an OplogProcessorCheckpoint entry, commit it, and update index tracking.
    ///
    /// `Err` means the agent's oplog refused the write: its shard has a new owner, or an archive
    /// layer did not store its entries. Nothing more may be written and forwarding stops. Giving
    /// the agent up is deliberately not this layer's call - a storage decorator has no business
    /// stopping agents - and it does not need to be: the refusal latches on the oplog, so the
    /// worker's own commit path is refused too and gives it up.
    ///
    /// Any other storage failure keeps the fail-stop behaviour it has always had.
    async fn write_checkpoint(
        &mut self,
        grant_id: EnvironmentPluginGrantId,
        target_agent_id: &AgentId,
        confirmed_up_to: OplogIndex,
        sending_up_to: OplogIndex,
        last_batch_start: OplogIndex,
    ) -> Result<(), OplogRefusal> {
        // Checked here as well as when the flush starts: the worker's own write can latch the
        // fence while the flush awaits a send or a lookup, and a checkpoint added after that
        // would still take an index for an entry that is never committed.
        if let Some(fence) = self.inner.fence() {
            return Err(OplogRefusal::Fenced(fence));
        }
        if self.pending_checkpoint_activity.is_none() {
            self.pending_checkpoint_activity = Some(
                self.forwarding_activity
                    .try_enter()
                    .expect("checkpoint belongs to an admitted flush"),
            );
        }
        let checkpoint = OplogEntry::OplogProcessorCheckpoint {
            timestamp: golem_common::model::Timestamp::now_utc(),
            plugin_grant_id: grant_id,
            target_agent_id: target_agent_id.clone(),
            confirmed_up_to,
            sending_up_to,
            last_batch_start,
        };
        let cache_entries = self.cache_is_required();
        let cached_checkpoint = cache_entries.then(|| checkpoint.clone());
        let idx = match self.inner.add(checkpoint).await {
            Ok(idx) => idx,
            Err(error) => match error.into_refusal() {
                Ok(refusal) => return Err(self.stop_forwarding(refusal)),
                Err(error) => panic!("oplog write: {error}"),
            },
        };
        if let Some(checkpoint) = cached_checkpoint {
            self.record_cached([(idx, checkpoint)]);
        } else {
            self.record_uncached([idx]);
        }
        let committed = match self.inner.commit(CommitLevel::Always).await {
            Ok(committed) => committed,
            Err(error) => match error.into_refusal() {
                Ok(refusal) => return Err(self.stop_forwarding(refusal)),
                Err(error) => panic!("oplog write: {error}"),
            },
        };
        if let Some(max_idx) = committed.keys().max().copied() {
            self.last_committed_idx = self.last_committed_idx.max(max_idx);
        }
        // Track all directly committed entries so ForwardingOplog::commit()
        // can surface them to the Worker for status folding
        self.pending_direct_commits.extend(committed);
        Ok(())
    }

    /// Logs the checkpoint that the oplog refused and hands the refusal back to the caller, which
    /// stops this flush. After a fence, every later flush sees the latched fence and returns before
    /// writing anything. A failed archive write latches no fence: a later flush runs, and the oplog
    /// refuses its checkpoint with the same error.
    fn stop_forwarding(&self, refusal: OplogRefusal) -> OplogRefusal {
        tracing::info!(
            source_agent = %self.initial_worker_metadata.agent_id,
            refusal = %refusal,
            "Oplog processor: checkpoint refused - forwarding stopped"
        );
        refusal
    }

    /// Prune buffer: drain entries that ALL active/in-flight plugins have confirmed past.
    fn prune_buffer(&mut self) {
        if self.plugin_state.is_empty() || self.buffer.is_none() {
            return;
        }

        let min_confirmed = self
            .plugin_state
            .values()
            .map(|s| s.confirmed_up_to)
            .min()
            .unwrap_or(OplogIndex::NONE);

        if min_confirmed >= self.buffer_start_idx {
            let drain_up_to = min_confirmed.as_u64() - self.buffer_start_idx.as_u64() + 1;
            let buffer = self.buffer.as_mut().unwrap();
            let drain_count = (drain_up_to as usize).min(buffer.len());
            buffer.drain(..drain_count);
            self.buffer_start_idx =
                OplogIndex::from_u64(self.buffer_start_idx.as_u64() + drain_count as u64);
            if buffer.is_empty() {
                self.buffer = None;
                self.buffer_start_idx = self.last_oplog_idx.next();
            }
        }
    }

    fn finish_empty_flush(&mut self) {
        self.buffer = None;
        self.buffer_start_idx = self.last_oplog_idx.next();
        self.last_send = Instant::now();
        self.commit_count = 0;
    }

    fn finish_flush_cycle(&mut self) {
        self.last_send = Instant::now();
        self.commit_count = 0;
        // Prune completed monitor tasks
        self.monitor_tasks.retain(|h| !h.is_finished());
    }

    /// Periodic locality recovery: for each plugin whose target worker is on a
    /// remote executor (after shard reassignment), check if the old target has
    /// caught up and, if so, migrate to a new local plugin worker.
    ///
    /// This is an optimization — correctness is preserved regardless because
    /// delivery always goes to the recorded `target_agent_id` with deterministic
    /// idempotency keys.
    pub(super) async fn try_locality_recovery(&mut self) {
        let Some(_activity) = self.forwarding_activity.try_enter() else {
            return;
        };
        if self.forwarding_retired.is_cancelled() {
            return;
        }
        // A migration is recorded with a checkpoint, which a fenced oplog no longer takes.
        if self.inner.fence().is_some() {
            return;
        }
        let status = self.last_known_status.get();
        // Ensure plugin_state is reconciled with current status
        self.reconcile_plugin_state(&status);
        let environment_id = self.initial_worker_metadata.environment_id;

        // Collect candidates: plugins with a target that might be non-local
        let candidates: Vec<(EnvironmentPluginGrantId, AgentId)> = self
            .plugin_state
            .iter()
            .filter(|(_, state)| {
                state.target_agent_id.is_some()
                    && !state.send_in_progress
                    && state.sending_up_to <= state.confirmed_up_to
            })
            .map(|(grant_id, state)| (*grant_id, state.target_agent_id.clone().unwrap()))
            .collect();

        if candidates.is_empty() {
            return;
        }

        let component_metadata = match self.get_component_metadata(&status).await {
            Some(cm) => cm,
            None => return,
        };

        for (grant_id, old_target) in candidates {
            // Check if the target is already local
            let Some(result) = self
                .while_forwarding(self.oplog_plugins.is_local(&old_target))
                .await
            else {
                return;
            };
            match result {
                Ok(true) => continue, // already local
                Ok(false) => {}       // non-local, try recovery
                Err(err) => {
                    tracing::warn!(
                        plugin = %grant_id,
                        error = %err,
                        "Locality recovery: failed to check locality"
                    );
                    continue;
                }
            }

            let agent_type = ParsedAgentId::parse_agent_type_name(
                &self.initial_worker_metadata.agent_id.agent_id,
            )
            .ok();
            let plugin = match component_metadata
                .metadata
                .owner_plugins(self.initial_worker_metadata.owner_kind, agent_type.as_ref())
                .and_then(|plugins| {
                    plugins
                        .iter()
                        .find(|p| p.environment_plugin_grant_id == grant_id)
                }) {
                Some(p) => p.clone(),
                None => continue,
            };

            let (confirmed, last_batch_start) = match self.plugin_state.get(&grant_id) {
                Some(s) => (s.confirmed_up_to, s.last_batch_start),
                None => continue,
            };

            // Recompute the idempotency key for the last confirmed batch
            if last_batch_start == confirmed {
                // No batch has been confirmed yet (sentinel value), skip
                tracing::debug!(
                    plugin = %grant_id,
                    "Locality recovery: no confirmed batch yet, skipping"
                );
                continue;
            }

            let last_key = oplog_processor_idempotency_key(
                &self.initial_worker_metadata.agent_id,
                &grant_id,
                last_batch_start,
                confirmed,
            );

            let Some(result) = self
                .while_forwarding(self.oplog_plugins.lookup_invocation_status(
                    environment_id,
                    &old_target,
                    self.initial_worker_metadata.created_by,
                    &last_key,
                ))
                .await
            else {
                return;
            };
            match result {
                Ok(InvocationStatus::Complete | InvocationStatus::Pending) => {
                    // Old target has received the batch (complete or queued), proceed with migration
                }
                Ok(status) => {
                    tracing::debug!(
                        plugin = %grant_id,
                        old_target = %old_target,
                        ?status,
                        "Locality recovery: old target has not received last batch yet, skipping"
                    );
                    continue;
                }
                Err(err) => {
                    tracing::warn!(
                        plugin = %grant_id,
                        old_target = %old_target,
                        error = %err,
                        "Locality recovery: failed to query old target status"
                    );
                    continue;
                }
            }

            // Migrate to a new local worker
            let Some(result) = self
                .while_forwarding(self.oplog_plugins.resolve_target(environment_id, &plugin))
                .await
            else {
                return;
            };
            let new_target = match result {
                Ok(t) => t,
                Err(err) => {
                    tracing::warn!(
                        plugin = %grant_id,
                        error = %err,
                        "Locality recovery: failed to resolve new local target"
                    );
                    continue;
                }
            };

            // Don't persist a no-op migration or migrate to another non-local target
            if new_target == old_target {
                tracing::debug!(
                    plugin = %grant_id,
                    target = %old_target,
                    "Locality recovery: resolved target is unchanged, skipping"
                );
                continue;
            }
            let Some(result) = self
                .while_forwarding(self.oplog_plugins.is_local(&new_target))
                .await
            else {
                return;
            };
            match result {
                Ok(true) => {}
                Ok(false) => {
                    tracing::debug!(
                        plugin = %grant_id,
                        new_target = %new_target,
                        "Locality recovery: resolved target is still non-local, skipping"
                    );
                    continue;
                }
                Err(err) => {
                    tracing::warn!(
                        plugin = %grant_id,
                        new_target = %new_target,
                        error = %err,
                        "Locality recovery: failed to verify locality of new target"
                    );
                    continue;
                }
            }

            if self
                .write_checkpoint(
                    grant_id,
                    &new_target,
                    confirmed,
                    confirmed,
                    last_batch_start,
                )
                .await
                .is_err()
            {
                return;
            }
            if let Some(s) = self.plugin_state.get_mut(&grant_id) {
                s.target_agent_id = Some(new_target.clone());
            }
            tracing::info!(
                source_worker = %self.initial_worker_metadata.agent_id,
                plugin = %grant_id,
                old_target = %old_target,
                new_target = %new_target,
                confirmed_up_to = confirmed.as_u64(),
                last_batch_start = last_batch_start.as_u64(),
                "Locality recovery: migrated plugin to new local target"
            );
        }
    }
}
