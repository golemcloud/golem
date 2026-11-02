use super::*;

pub(in crate::durable_host::replay_state) fn is_auto_skippable_hint(entry: &OplogEntry) -> bool {
    entry.is_hint() && !matches!(entry, OplogEntry::CompletionDelivered { .. })
}

impl CursorTx<'_> {
    /// Commits a just-read entry: apply its commit-only side effects, publish the cursor advance,
    /// skip any trailing hint entries, advance the non-hint marker, route it to the concurrent
    /// resolver, and mark that cursor progress should be signalled once the lock is released.
    pub(in crate::durable_host::replay_state) async fn commit_consumed_entry(
        &mut self,
        read_idx: OplogIndex,
        entry: &OplogEntry,
    ) -> Result<(), WorkerExecutorError> {
        self.commit_entry(read_idx, entry, None).await
    }

    /// Commits a retained `Start` the reader is not entitled to, or the terminal attached to
    /// such a `Start`. The cursor moves past it physically, but neither the non-hint position nor
    /// the replay events of the entry and its trailing hints are published: the owner has not
    /// consumed it yet, publishing the position here would let the owner's `begin_function`
    /// observe a position the live run never saw before its own `Start`, and publishing the
    /// events would let a boundary apply them before the owner looks for them. Both are published
    /// when the owner claims the `Start` (`publish_claimed_position`, `publish_deferred_events`).
    pub(super) async fn commit_retained_entry(
        &mut self,
        read_idx: OplogIndex,
        entry: &OplogEntry,
    ) -> Result<(), WorkerExecutorError> {
        let start_idx = terminal_start_index(entry).unwrap_or(read_idx);
        debug_assert!(
            self.st.retained_starts.contains_key(&start_idx),
            "retained commits target a retained Start"
        );
        self.commit_entry(read_idx, entry, Some(start_idx)).await
    }

    /// Commits `entry`; `retained_for` names the retained `Start` on whose behalf a reader that is
    /// not its owner commits past it (see [`Self::commit_retained_entry`]), in which case the
    /// non-hint position stays unpublished and every replay event recorded by this commit is
    /// deferred to that `Start`.
    async fn commit_entry(
        &mut self,
        read_idx: OplogIndex,
        entry: &OplogEntry,
        retained_for: Option<OplogIndex>,
    ) -> Result<(), WorkerExecutorError> {
        self.retaining_for = retained_for.map(|start_idx| (start_idx, read_idx));
        let result = self
            .commit_entry_inner(read_idx, entry, retained_for.is_none())
            .await;
        self.retaining_for = None;
        result
    }

    async fn commit_entry_inner(
        &mut self,
        read_idx: OplogIndex,
        entry: &OplogEntry,
        publish_non_hint_position: bool,
    ) -> Result<(), WorkerExecutorError> {
        // Apply the fallible commit-only side effects *before* publishing the cursor advance, so a
        // failure (e.g. a corrupt `GolemApiFork` payload) cannot leave the cursor advanced while
        // resolver routing / progress signalling below never run — a partial-publish on the error
        // path. None of these effects depend on the cursor position.
        self.apply_commit_effects(read_idx, entry).await?;
        if let OplogEntry::Start {
            parent_start_index,
            function_name,
            ..
        } = entry
        {
            let mut entities = parent_start_index
                .and_then(|parent| self.st.entity_start_routes.get(&parent))
                .cloned()
                .unwrap_or_default();
            if matches!(
                function_name,
                HostFunctionName::GolemEntityInvoke | HostFunctionName::GolemToolInvocationRejected
            ) {
                entities.push(read_idx);
            }
            for entity in &entities {
                self.st
                    .entity_route_members
                    .entry(*entity)
                    .or_default()
                    .insert(read_idx);
            }
            if !entities.is_empty() {
                self.st.entity_start_routes.insert(read_idx, entities);
            }
        } else if let Some(start) = terminal_start_index(entry)
            && let Some(members) = self.st.entity_route_members.remove(&start)
        {
            for member in members {
                let entities = self
                    .st
                    .entity_start_routes
                    .get_mut(&member)
                    .expect("reverse entity routes reference an active forward route");
                entities.retain(|entity| *entity != start);
                if entities.is_empty() {
                    self.st.entity_start_routes.remove(&member);
                }
            }
        }
        // Publish the cursor advance now (and only now): committing is the single point where the
        // speculative read of `read_idx` becomes globally observable. This also performs the
        // skipped-region jump for the next read via `get_out_of_skipped_region`, and must precede
        // `skip_forward` (which reads forward from the advanced cursor).
        self.move_replay_idx(read_idx).await?;
        if matches!(entry, OplogEntry::CompletionDelivered { .. })
            || self.is_stream_observation(read_idx)
        {
            self.st.skip_hints_after_delivery = true;
        } else {
            self.skip_forward().await?;
        }
        if publish_non_hint_position && !entry.is_hint() {
            self.cursor
                .position
                .last_replayed_non_hint_index
                .set(read_idx);
        }
        // Committed-consume hook: this entry is now permanently consumed (speculative reads never
        // reach here — they return before committing), so it is safe to feed the concurrent replay
        // resolver.
        self.on_committed_replay_entry(read_idx, entry);
        self.notify_progress = true;
        Ok(())
    }

    /// Skips trailing hint entries following the just-committed entry, recording any log hints,
    /// then leaves the cursor on the next non-hint entry without consuming it.
    pub(in crate::durable_host::replay_state) async fn skip_forward(
        &mut self,
    ) -> Result<(), WorkerExecutorError> {
        // Skipping hint entries and recording log entries
        let mut logs: HashMap<(u64, u64), usize> = HashMap::new();
        while self.cursor.is_replay() {
            // Speculative peek: does not advance the published cursor. The cursor is advanced (via
            // `move_replay_idx`) only when a hint entry is actually skipped past below; the first
            // non-hint entry leaves the cursor untouched, so no speculative position is ever
            // globally observable.
            let (read_idx, entry) = self.raw_read_next_oplog_entry().await?;
            match self.should_skip_to(read_idx, &entry).await {
                Some(skip_to) => {
                    // This hint entry is being permanently consumed, so its commit-only side
                    // effects fire here (they must NOT fire on the rolled-back probe in the `None`
                    // branch below).
                    if let Some((start_idx, _)) = self.retaining_for {
                        self.retaining_for = Some((start_idx, read_idx));
                    }
                    self.apply_commit_effects(read_idx, &entry).await?;

                    // Recording seen log entries
                    if let OplogEntry::Log {
                        level,
                        context,
                        message,
                        ..
                    } = &entry
                    {
                        let hash = ReplayCursor::hash_log_entry(*level, context, message);
                        *logs.entry(hash).or_insert(0) += 1;
                    }

                    // Publish the advance past this hint (also performs the skipped-region jump for
                    // the next read). Leaving last_replayed_non_hint_index unchanged, because this is
                    // a hint entry.
                    self.move_replay_idx(skip_to).await?;
                }
                None => {
                    // We've found the first non-hint entry; the speculative peek advanced nothing, so
                    // the cursor and skipped-region state already point just before it. Hand the
                    // peeked entry back to the prefetch buffer: dropping it would leave the buffer
                    // head past the cursor, and the next read would discard the whole prefetched
                    // chunk and fetch it again from the oplog.
                    self.st.replay_buffer.push_front((read_idx, entry));
                    break;
                }
            }
        }

        self.cursor.set_log_hashes(logs);
        Ok(())
    }

    /// Checks whether the currently read `entry` is a hint entry valid for replay, or
    /// if a new oplog index should be tried instead.
    ///
    /// For hint entries, the next tried oplog index is the next one.
    ///
    /// If the entry is a hint entry, the result is `Some` and contains the current last
    /// read index, so the next read will get the next one.
    /// If the entry is not a hint entry the result is `None`.
    pub(in crate::durable_host::replay_state) async fn should_skip_to(
        &self,
        read_idx: OplogIndex,
        entry: &OplogEntry,
    ) -> Option<OplogIndex> {
        if is_auto_skippable_hint(entry) && !self.is_stream_observation(read_idx) {
            // Advance to the hint entry itself; the caller publishes this (via `move_replay_idx`) so
            // the next read gets `read_idx.next()`.
            Some(read_idx)
        } else {
            None
        }
    }

    /// Applies the replay side effects of an entry that is being **permanently consumed** at
    /// `read_idx`. Split out of the raw read so it fires only on commit, never on a rolled-back
    /// speculative read. Called for the entry returned to a caller, and for each hint /
    /// hint entry skipped past in [`Self::skip_forward`].
    pub(in crate::durable_host::replay_state) async fn apply_commit_effects(
        &mut self,
        read_idx: OplogIndex,
        oplog_entry: &OplogEntry,
    ) -> Result<(), WorkerExecutorError> {
        if let Some(wallet) = &self.cursor.wallet {
            if let Some(parent) = oplog_entry.entity_parent_start_index() {
                // Hints can precede even the claim of their future entity Start. Validate the
                // exact reference before retaining its prefix; a released claim cannot recreate it.
                if wallet.entity_replay_wallet(parent).is_err()
                    && !self.st.claimed_starts.contains(&parent)
                    && self.custom_subtree_root(parent).is_none()
                    && self
                        .future_entity_start_can_be_claimed(read_idx, parent)
                        .await?
                {
                    wallet.register_replayed_entity(parent);
                }
                wallet.capture_entity_prefix(parent);
            }
            if matches!(
                oplog_entry,
                OplogEntry::Start {
                    function_name: HostFunctionName::GolemEntityInvoke
                        | HostFunctionName::GolemToolInvocationRejected,
                    ..
                }
            ) {
                wallet.capture_entity_prefix(read_idx);
            }
        }
        {
            let mut attachments = self.cursor.tool_attachment_reconstruction.lock().unwrap();
            if let Some(control) = attachments.history.controls.get(&read_idx) {
                let attachment = control.attachment;
                attachments.crossed_controls.insert(read_idx);
                attachments.refresh_controller(attachment)?;
            }
        }
        // record side effects that need to be applied at the next opportunity
        if let OplogEntry::SuccessfulUpdate {
            target_revision, ..
        } = oplog_entry
        {
            self.record_wallet_replay_event(
                read_idx,
                ReplayEvent::UpdateReplayed {
                    new_revision: *target_revision,
                },
            );
        }
        // The sequential adapter persists GolemApiFork as a matched
        // `Start { function_name: GolemApiFork, .. }` + `End { response: Some(..), .. }`
        // pair. On Start we remember the `Start`'s `OplogIndex`, on the matching
        // End (via `start_index`) we decode the response and emit `ForkReplayed`
        // if necessary.
        match oplog_entry {
            OplogEntry::AgentInvocationStarted { wallet_pin, .. } => {
                self.record_wallet_replay_event(
                    read_idx,
                    ReplayEvent::InvocationWalletPinned {
                        oplog_index: read_idx,
                        wallet_pin: wallet_pin.as_ref().clone(),
                    },
                );
            }
            OplogEntry::CardInstalled {
                card,
                wallet_generation,
                ..
            } => {
                self.record_wallet_replay_event(
                    read_idx,
                    ReplayEvent::CardInstalled {
                        card: card.as_ref().clone(),
                        wallet_generation: *wallet_generation,
                    },
                );
            }
            OplogEntry::CardDerived {
                card,
                wallet_generation,
                entity_parent_start_index,
                ..
            } => {
                self.record_wallet_replay_event(
                    read_idx,
                    ReplayEvent::CardDerived {
                        card: card.as_ref().clone(),
                        wallet_generation: *wallet_generation,
                    },
                );
                if let Some(wallet) = &self.cursor.wallet {
                    wallet.record_derivation(*entity_parent_start_index, card.as_ref().clone())?;
                }
            }
            OplogEntry::AgentInvocationFinished { .. } => {
                if let Some(wallet) = &self.cursor.wallet {
                    wallet.finish_replayed_invocation();
                }
            }
            OplogEntry::CardTransferStarted {
                transfer_id,
                card_id,
                source_holder,
                target_holder,
                source_wallet_generation,
                ..
            } => {
                self.record_wallet_replay_event(
                    read_idx,
                    ReplayEvent::CardTransferStarted {
                        transfer_id: *transfer_id,
                        card_id: *card_id,
                        source_holder: source_holder.clone(),
                        target_holder: target_holder.clone(),
                        source_wallet_generation: *source_wallet_generation,
                    },
                );
            }
            OplogEntry::CardTransferred {
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                card,
                target_wallet_generation,
                ..
            } => {
                self.record_wallet_replay_event(
                    read_idx,
                    ReplayEvent::CardTransferred {
                        transfer_id: *transfer_id,
                        source_card_id: *source_card_id,
                        installed_card_id: *installed_card_id,
                        target_holder: target_holder.clone(),
                        card: card.as_ref().clone(),
                        target_wallet_generation: *target_wallet_generation,
                    },
                );
            }
            OplogEntry::CardTransferConfirmed {
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                ..
            } => {
                self.record_wallet_replay_event(
                    read_idx,
                    ReplayEvent::CardTransferConfirmed {
                        transfer_id: *transfer_id,
                        source_card_id: *source_card_id,
                        installed_card_id: *installed_card_id,
                        target_holder: target_holder.clone(),
                    },
                );
            }
            OplogEntry::CardRevokedCascade {
                revoked_card_ids,
                local_wallet_generation,
                ..
            } => {
                self.record_wallet_replay_event(
                    read_idx,
                    ReplayEvent::CardRevokedCascade {
                        card_ids: revoked_card_ids.clone(),
                        local_wallet_generation: *local_wallet_generation,
                    },
                );
            }
            OplogEntry::CardRevoked {
                card_id,
                wallet_generation,
                ..
            } => {
                self.record_wallet_replay_event(
                    read_idx,
                    ReplayEvent::CardRevoked {
                        card_id: *card_id,
                        wallet_generation: *wallet_generation,
                    },
                );
            }
            OplogEntry::CardExpired {
                card_id,
                wallet_generation,
                ..
            } => {
                self.record_wallet_replay_event(
                    read_idx,
                    ReplayEvent::CardExpired {
                        card_id: *card_id,
                        wallet_generation: *wallet_generation,
                    },
                );
            }
            OplogEntry::Start { function_name, .. }
                if function_name == &HostFunctionName::GolemApiFork =>
            {
                self.st.pending_fork_starts.insert(read_idx);
            }
            OplogEntry::End {
                start_index,
                response: Some(response_payload),
                ..
            } => {
                let is_pending = self.st.pending_fork_starts.remove(start_index);
                if is_pending {
                    let response = self
                        .cursor
                        .oplog
                        .download_payload(response_payload.clone())
                        .await
                        .map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "failed to download GolemApiFork oplog payload at index {read_idx}: {err}"
                            ))
                        })?;
                    let result: HostResponseGolemApiFork =
                        if let HostResponse::GolemApiFork(result) = response {
                            result
                        } else {
                            return Err(WorkerExecutorError::unexpected_oplog_entry(
                                "HostResponse::GolemApiFork",
                                format!("{response:?}"),
                            ));
                        };
                    if result.result == Ok(ForkResult::Forked) {
                        self.record_replay_event(ReplayEvent::ForkReplayed {
                            new_phantom_id: result.forked_phantom_id,
                        });
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Advances the published cursor to `new_idx`, applying any skipped-region jump, and synthesizes
    /// a single [`ReplayEvent::ReplayFinished`] if this advance is the one that exhausts replay.
    ///
    /// This is the single chokepoint for every replay-mode position advance — direct consumption of
    /// the target entry, skipping past trailing hint entries, and jumping over a skipped region (via
    /// [`Self::get_out_of_skipped_region`]) all funnel through here. Detecting the transition here
    /// (rather than only when the *consumed* entry index equals `replay_target`) guarantees
    /// `ReplayFinished` is queued whenever the cursor reaches the target, including when it gets
    /// there via a skip/jump that never consumes the target entry. Consumers withhold the event
    /// until the primary publishes live admission after reconstruction settlement. The forced
    /// transition in [`Self::switch_to_live`] is the only other path to live and emits its own
    /// `ReplayFinished`.
    ///
    /// Exactly-once holds because the `was_replay && is_live` edge is true only on the single advance
    /// that crosses into live: once live, the replay-driving loops stop and no further
    /// `move_replay_idx` runs until the replay target is grown (`set_replay_target`) or a new cursor
    /// is built (`new`), each of which starts a fresh replay epoch that emits its own
    /// `ReplayFinished` on completion.
    pub(in crate::durable_host::replay_state) async fn move_replay_idx(
        &mut self,
        new_idx: OplogIndex,
    ) -> Result<(), UnreadableOplog> {
        let was_replay = self.cursor.is_replay();
        self.cursor.position.last_replayed_index.set(new_idx);
        self.get_out_of_skipped_region().await?;
        if self.cursor.is_live()
            && let Some(wallet) = &self.cursor.wallet
        {
            wallet.finish_replay_prefix();
        }
        if was_replay && self.cursor.is_live() {
            self.record_replay_event(ReplayEvent::ReplayFinished);
        }
        Ok(())
    }

    pub(in crate::durable_host::replay_state) async fn get_out_of_skipped_region(
        &mut self,
    ) -> Result<(), UnreadableOplog> {
        let initial_snapshot_skip_end = self.st.initial_snapshot_skip_end.take();
        // Loop: after jumping a region, the freshly looked-up next region may start immediately
        // after the jump target (adjacent regions recorded separately), requiring another jump.
        while self.cursor.is_replay() {
            match self.st.next_skipped_region.clone() {
                Some(region) if region.start == (self.cursor.last_replayed_index().next()) => {
                    let target = region.end.next(); // we want to continue reading _after_ the region
                    debug!(
                        "Worker reached skipped region at {}, jumping to {} (oplog size: {})",
                        region.start,
                        target,
                        self.cursor.replay_target()
                    );
                    self.cursor
                        .position
                        .last_replayed_index
                        .set(target.previous()); // so we set the last replayed index to the end of the region

                    let events_region = match initial_snapshot_skip_end {
                        Some(snapshot_end) if region.end <= snapshot_end => None,
                        Some(snapshot_end) => Some(OplogRegion {
                            start: region.start.max(snapshot_end.next()),
                            end: region.end,
                        }),
                        None => Some(region),
                    };
                    if let Some(events_region) = events_region {
                        self.record_card_events_in_region(&events_region).await?;
                    }

                    // The lookup must start *after* the just-jumped region: `find_next_deleted_region`
                    // matches regions starting at-or-after the given index, so looking up from the
                    // region's own end would re-find a single-entry region (start == end) and leave
                    // the genuinely next region untracked.
                    let next = self
                        .st
                        .skipped_regions
                        .find_next_deleted_region(self.cursor.last_replayed_index().next());
                    self.st.next_skipped_region = next;
                }
                _ => break,
            }
        }
        Ok(())
    }

    async fn record_card_events_in_region(
        &mut self,
        region: &OplogRegion,
    ) -> Result<(), UnreadableOplog> {
        let mut next = region.start;
        while next <= region.end {
            let remaining = region.end.as_u64() - next.as_u64() + 1;
            let entries = self
                .cursor
                .oplog
                .read_exact(next, CHUNK_SIZE.min(remaining))
                .await?;
            let last_read = *entries.last_key_value().unwrap().0;
            for (index, entry) in entries {
                match entry {
                    OplogEntry::CardInstalled {
                        card,
                        wallet_generation,
                        ..
                    } => self.record_wallet_replay_event(
                        index,
                        ReplayEvent::CardInstalled {
                            card: *card,
                            wallet_generation,
                        },
                    ),
                    OplogEntry::CardDerived {
                        card,
                        wallet_generation,
                        ..
                    } => self.record_wallet_replay_event(
                        index,
                        ReplayEvent::CardDerived {
                            card: *card,
                            wallet_generation,
                        },
                    ),
                    OplogEntry::CardTransferStarted {
                        transfer_id,
                        card_id,
                        source_holder,
                        target_holder,
                        source_wallet_generation,
                        ..
                    } => self.record_wallet_replay_event(
                        index,
                        ReplayEvent::CardTransferStarted {
                            transfer_id,
                            card_id,
                            source_holder,
                            target_holder,
                            source_wallet_generation,
                        },
                    ),
                    OplogEntry::CardTransferred {
                        transfer_id,
                        source_card_id,
                        installed_card_id,
                        target_holder,
                        card,
                        target_wallet_generation,
                        ..
                    } => self.record_wallet_replay_event(
                        index,
                        ReplayEvent::CardTransferred {
                            transfer_id,
                            source_card_id,
                            installed_card_id,
                            target_holder,
                            card: *card,
                            target_wallet_generation,
                        },
                    ),
                    OplogEntry::CardTransferConfirmed {
                        transfer_id,
                        source_card_id,
                        installed_card_id,
                        target_holder,
                        ..
                    } => self.record_wallet_replay_event(
                        index,
                        ReplayEvent::CardTransferConfirmed {
                            transfer_id,
                            source_card_id,
                            installed_card_id,
                            target_holder,
                        },
                    ),
                    OplogEntry::CardRevokedCascade {
                        revoked_card_ids,
                        local_wallet_generation,
                        ..
                    } => self.record_wallet_replay_event(
                        index,
                        ReplayEvent::CardRevokedCascade {
                            card_ids: revoked_card_ids,
                            local_wallet_generation,
                        },
                    ),
                    OplogEntry::CardRevoked {
                        card_id,
                        wallet_generation,
                        ..
                    } => self.record_wallet_replay_event(
                        index,
                        ReplayEvent::CardRevoked {
                            card_id,
                            wallet_generation,
                        },
                    ),
                    OplogEntry::CardExpired {
                        card_id,
                        wallet_generation,
                        ..
                    } => self.record_wallet_replay_event(
                        index,
                        ReplayEvent::CardExpired {
                            card_id,
                            wallet_generation,
                        },
                    ),
                    _ => {}
                }
            }
            next = last_read.next();
        }
        Ok(())
    }

    /// Feeds the concurrent replay resolver when an `End`/`Cancelled` entry is *committed*
    /// (permanently consumed). Resolves only calls that are actually being awaited
    /// (`resolve_if_pending`), so the `End`/`Cancelled` of any call not tracked by the resolver is
    /// ignored instead of leaking.
    pub(in crate::durable_host::replay_state) fn on_committed_replay_entry(
        &mut self,
        idx: OplogIndex,
        entry: &OplogEntry,
    ) {
        if let Some(start_index) = terminal_start_index(entry) {
            let resolution = self.terminal_resolution(start_index, idx, entry);
            self.st
                .concurrent_resolver
                .resolve_if_pending(start_index, idx, resolution);
        }
    }

    /// Publishes a replay event for the primary Store to apply at its next boundary. While a
    /// retained `Start` is being committed past on its owner's behalf, the event is deferred to
    /// that `Start` instead (see [`RetainedStart::deferred_events`]); `ReplayFinished` describes
    /// the cursor rather than an entry and is never deferred.
    fn record_wallet_replay_event(&mut self, index: OplogIndex, event: ReplayEvent) {
        let event = match &self.cursor.wallet {
            Some(owner) => ReplayEvent::Projected {
                receipt: owner.reserve_replay_event(index, &event),
                event: Box::new(event),
            },
            None => event,
        };
        self.record_replay_event(event);
    }

    pub(in crate::durable_host::replay_state) fn record_replay_event(
        &mut self,
        event: ReplayEvent,
    ) {
        if let Some((start_idx, entry_idx)) = self.retaining_for
            && !matches!(event, ReplayEvent::ReplayFinished)
        {
            self.st
                .retained_starts
                .get_mut(&start_idx)
                .expect("replay events are deferred to a retained Start")
                .deferred_events
                .push((entry_idx, event));
            return;
        }
        self.cursor
            .pending_replay_events
            .lock()
            .unwrap()
            .push(event);
    }
}

impl ReplayState {
    #[cfg(test)]
    pub fn take_new_replay_events(&self) -> Vec<ReplayEvent> {
        self.take_new_replay_events_for(true)
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn test_stage_replay_events(&self, events: Vec<ReplayEvent>) {
        self.cursor
            .pending_replay_events
            .lock()
            .unwrap()
            .extend(events);
    }

    pub(crate) fn take_new_replay_events_for(&self, root: bool) -> Vec<ReplayEvent> {
        let mut pending = self.cursor.pending_replay_events.lock().unwrap();
        if root && self.is_live_published() {
            std::mem::take(&mut *pending)
        } else {
            let events = std::mem::take(&mut *pending);
            let mut ready = Vec::with_capacity(events.len());
            for event in events {
                if matches!(event.kind(), ReplayEvent::ReplayFinished)
                    || (!root
                        && matches!(
                            event.kind(),
                            ReplayEvent::UpdateReplayed { .. }
                                | ReplayEvent::InvocationWalletPinned { .. }
                        ))
                {
                    pending.push(event);
                } else {
                    ready.push(event);
                }
            }
            ready
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn pending_card_derivation(&self, card_id: CardId) -> Option<(StoredCard, u64)> {
        self.cursor
            .pending_replay_events
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|event| match event.kind() {
                ReplayEvent::CardDerived {
                    card,
                    wallet_generation,
                } if card.card_id() == card_id => Some((card.clone(), *wallet_generation)),
                _ => None,
            })
    }
}
