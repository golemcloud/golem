use super::*;

/// Acceptance predicate of the unconditional positional readers of the Store whose entries are
/// attributed to `scope` (`None` for the primary agent Store, the entity invocation `Start` index
/// for an entity Store). A positional reader takes whatever non-hint entry is at the head *except*:
///
/// - an unclaimed durable-call `Start`, which only its owner's claim may consume
///   ([`CursorTx::claim_start_matching`]): the cursor retains such a `Start` and the reader
///   continues past it. The only `Start`s a positional reader still receives are the
///   dedicated-positional-consumer pairs with commit effects (`GolemApiFork`);
/// - an entry recorded by another Store sharing this cursor (an entity body interleaved with its
///   owner). Only that Store's own positional reader may consume it; this reader parks and waits
///   for cursor progress (see [`CursorTx::check_parked_positional_read`]).
pub(super) fn positional_reader_accepts(
    scope: Option<OplogIndex>,
) -> impl FnMut(&OplogEntry) -> bool {
    move |entry| {
        !CursorTx::is_retainable_start(entry)
            && match entry.entity_attribution() {
                EntityAttribution::Unattributed => true,
                EntityAttribution::Agent => scope.is_none(),
                EntityAttribution::EntityBody(owner) => scope == Some(owner),
            }
    }
}

pub(super) fn is_active_stream_observation(
    cursor: &ReplayCursor,
    state: &CursorState,
    index: OplogIndex,
) -> bool {
    cursor
        .stream_observations
        .lock()
        .unwrap()
        .get(&index)
        .is_some_and(|parent| {
            parent.is_none_or(|parent| {
                !state.skipped_regions.is_in_deleted_region(parent)
                    && !state
                        .custom_subtrees
                        .values()
                        .any(|members| members.contains(&parent))
            })
        })
}

impl CursorTx<'_> {
    /// The single cursor transaction body.
    ///
    /// Before evaluating the caller's `condition`, it **auto-drains** any *awaited terminals* at the
    /// cursor head: `End`/`Cancelled` entries whose `start_index` currently has a registered
    /// resolver awaiter. Each is committed and routed back to its awaiter (via
    /// [`Self::on_committed_replay_entry`]), then the loop continues. This is what makes concurrent
    /// replay correct: a positional reader (a scope/marker consumer, or another call's claim) never
    /// steals a host call's terminal that belongs to a different, concurrently-replaying call — it
    /// drains those to their owners first and only then looks at the next non-terminal entry.
    /// *Orphan terminals* — `End`/`Cancelled` whose `Start` lies inside a skipped/deleted region —
    /// are likewise auto-drained (consumed without an awaiter), see [`Self::is_orphan_terminal`].
    ///
    /// On the first non-drainable entry (a non-terminal, or an `End`/`Cancelled` nobody awaits):
    /// - if `condition` matches, it is committed and returned;
    /// - if it is a durable-call `Start` nobody has claimed, it is committed past and **retained**
    ///   for its owner's later identity-validated claim (see [`CursorState::starts`]),
    ///   and the drain continues. The reader is never handed another operation's `Start`, and it
    ///   never parks on one either: the owner of that `Start` may need the Store this reader holds
    ///   before it can claim it;
    /// - otherwise `None` is returned. The speculative read advanced nothing observable (the cursor
    ///   is published only on commit), so there is nothing to roll back. The auto-drained terminals
    ///   and retained `Start`s stay committed — that is the correct contract under concurrent
    ///   replay: draining another call's terminal or retaining its `Start` is real progress even
    ///   when this caller's own predicate then fails.
    pub(in crate::durable_host::replay_state) async fn try_get_oplog_entry(
        &mut self,
        condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        self.try_get_oplog_entry_inner(
            None,
            None,
            BoundaryRead::None,
            UnclaimedStarts::Retain,
            condition,
        )
        .await
    }

    /// [`Self::try_get_oplog_entry`] for a `Start` claim's head fast path: an unclaimed `Start`
    /// that is not this claim's own is left at the head instead of being retained, so a claim
    /// whose `Start` is recorded further ahead (or not at all) falls through to the scan-ahead /
    /// missing-`Start` classification against an unchanged cursor. Retaining is the job of the
    /// readers that must make progress past that `Start` (positional consumers, terminal drains).
    pub(in crate::durable_host::replay_state) async fn try_get_oplog_entry_leaving_unclaimed_starts(
        &mut self,
        condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        self.try_get_oplog_entry_inner(
            None,
            None,
            BoundaryRead::None,
            UnclaimedStarts::Leave,
            condition,
        )
        .await
    }

    /// Consumes a delivery marker or consumer observation at its exact position and transfers
    /// the global advance gate. A native stream observation has no durable-call Start.
    pub(in crate::durable_host::replay_state) async fn consume_delivery_boundary(
        &mut self,
        start_index: Option<OplogIndex>,
        marker_index: OplogIndex,
        guest_order: Option<&GuestReadOrder>,
    ) -> Result<Option<tokio::sync::OwnedMutexGuard<()>>, WorkerExecutorError> {
        let consumed = self
            .try_get_oplog_entry_inner(
                Some((start_index, marker_index)),
                guest_order,
                BoundaryRead::None,
                UnclaimedStarts::Retain,
                |_| false,
            )
            .await?;
        if consumed.is_some() {
            Ok(Some(
                self.advance_gate
                    .take()
                    .expect("cursor transactions always own the advance gate"),
            ))
        } else {
            Ok(None)
        }
    }

    /// [`Self::try_get_oplog_entry`] with the invocation-boundary tolerance for live-only
    /// abandoned durable-call records enabled: never-claimed `Start`s (and the `End`/`Cancelled`
    /// terminals closing them) are drained into `abandoned` instead of being handed to the
    /// positional reader. Only the agent-invocation-finished reader uses this — see
    /// [`AbandonedStarts`] for why the tolerance is sound there and nowhere else.
    pub(in crate::durable_host::replay_state) async fn try_get_oplog_entry_at_invocation_boundary(
        &mut self,
        abandoned: &mut AbandonedStarts,
        condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        self.try_get_oplog_entry_inner(
            None,
            None,
            BoundaryRead::InvocationBoundary(abandoned),
            UnclaimedStarts::Retain,
            condition,
        )
        .await
    }

    async fn try_get_oplog_entry_inner(
        &mut self,
        expected_delivery: Option<(Option<OplogIndex>, OplogIndex)>,
        guest_order: Option<&GuestReadOrder>,
        mut boundary: BoundaryRead<'_>,
        unclaimed_starts: UnclaimedStarts,
        mut condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        self.blocked_on_completion_delivery = false;
        if self.st.skip_hints_after_delivery {
            self.skip_forward().await?;
            self.st.skip_hints_after_delivery = false;
        }
        loop {
            if let BoundaryRead::Settlement { anchor, .. } = &boundary
                && self.st.first_settled_after(*anchor).is_some()
            {
                // The walk ends at a settlement; the entries after it belong to the next
                // invocation.
                return Ok(None);
            }

            if self.cursor.at_replay_tail() {
                // No further entries to read: nothing to drain, condition cannot match.
                return Ok(None);
            }

            let (read_idx, entry) = self.raw_read_next_oplog_entry().await?;

            if self.is_stream_observation(read_idx) {
                // A native read waits for every earlier retained Start of its invocation,
                // including host-internal children. Live host calls run those children eagerly
                // before the guest can observe the read; reserving an import is not enough.
                let unreached_guest_call = guest_order.is_some_and(|order| {
                    self.st
                        .starts
                        .retained_iter()
                        .take_while(|(index, _)| **index < read_idx)
                        .any(|(_, retained)| {
                            matches!(&retained.entry, OplogEntry::Start {
                            invocation_id: Some(id), ..
                        } if id.anchor == order.anchor)
                        })
                });
                if expected_delivery == Some((None, read_idx)) && !unreached_guest_call {
                    self.commit_consumed_entry(read_idx, &entry).await?;
                    return Ok(Some((read_idx, entry)));
                }
                self.st.replay_buffer.push_front((read_idx, entry));
                self.blocked_on_completion_delivery = true;
                return Ok(None);
            }

            if self
                .cursor
                .stream_observations
                .lock()
                .unwrap()
                .contains_key(&read_idx)
            {
                // Registering a custom subtree can make a previously buffered guest boundary
                // replay-inert. No observer re-executes that subtree, so reconsider the record
                // as a hint instead of leaving it at the cursor head.
                self.commit_consumed_entry(read_idx, &entry).await?;
                continue;
            }

            if let OplogEntry::CompletionDelivered { start_index, .. } = &entry {
                if expected_delivery == Some((Some(*start_index), read_idx)) {
                    self.commit_consumed_entry(read_idx, &entry).await?;
                    return Ok(Some((read_idx, entry)));
                }
                if self.is_custom_subtree_descendant(*start_index) {
                    // A completed custom invocation replays as one logical result, while an
                    // incomplete one re-executes its whole body. Either way, no replay delivery
                    // token exists for a physical call in its observational subtree, so its
                    // marker is replay-inert rather than a guest-delivery boundary.
                    self.commit_consumed_entry(read_idx, &entry).await?;
                    self.st.skip_hints_after_delivery = false;
                    self.skip_forward().await?;
                    continue;
                }
                if self.st.skipped_regions.is_in_deleted_region(*start_index) {
                    // The call belongs to an abandoned timeline, so no replay delivery token can
                    // exist for its surviving marker. Consume it like an orphan terminal and keep
                    // normal hint skipping enabled: there is no guest boundary to hold here.
                    debug!(
                        "Skipping orphan CompletionDelivered at {read_idx} whose Start {start_index} lies in a skipped region"
                    );
                    self.commit_consumed_entry(read_idx, &entry).await?;
                    self.st.skip_hints_after_delivery = false;
                    self.skip_forward().await?;
                    continue;
                }
                if boundary.tolerates_abandoned()
                    && self.st.starts.get_retained(start_index).is_some()
                    && !self.marker_body_can_still_consume(&boundary, *start_index)
                {
                    return Err(WorkerExecutorError::replay_divergence(ReplayDivergence {
                        oplog_index: read_idx,
                        expected: ReplayStep::described(format!(
                            "CompletionDelivered at {read_idx} references unclaimed durable call Start at {start_index} — the recorded guest received this completion but replay reached the invocation boundary without claiming it"
                        )),
                        actual: ReplayStep::invocation_end(),
                        mismatched: Vec::new(),
                    }));
                }

                // This is a reserved guest-delivery boundary. Leave it at the cursor head for the
                // matching completion token; even an unconditional positional reader may not
                // steal it or advance beyond it.
                self.st.replay_buffer.push_front((read_idx, entry));
                self.blocked_on_completion_delivery = true;
                return Ok(None);
            }

            if self.is_awaited_terminal(read_idx, &entry) {
                // An `End`/`Cancelled` owned by a concurrently-replaying call: commit it and hand it
                // back to its awaiter, then keep draining. Never returned to this caller.
                self.commit_consumed_entry(read_idx, &entry).await?;
                continue;
            }

            if self.is_orphan_terminal(&entry) {
                // An `End`/`Cancelled` whose `Start` lies inside a skipped/deleted region (a
                // jump/revert/fork/snapshot cut between a `Start` and its terminal): nobody can
                // ever claim or await it, so consume it here and keep draining instead of handing
                // it to a positional reader as an unexpected entry.
                debug!(
                    "Skipping orphan terminal at {read_idx} whose Start lies in a skipped region"
                );
                self.commit_consumed_entry(read_idx, &entry).await?;
                continue;
            }

            if self.st.starts.is_claimed_ahead(&read_idx) {
                // A `Start` already claimed out-of-position by an identity-keyed scan-ahead claim
                // stays reserved even if registration failed or was explicitly removed. Consume
                // its position without changing any remaining terminal obligation; it must never
                // be handed to a positional reader.
                self.validate_start_owner(read_idx, &entry)?;
                self.st.starts.consume_claimed_start(&read_idx);
                self.commit_consumed_entry(read_idx, &entry).await?;
                continue;
            }

            if let OplogEntry::Start {
                observational_owner: Some(owner),
                ..
            } = &entry
                && let Some(root) = self.custom_subtree_root(*owner)
            {
                // The owning custom invocation replays as one logical operation, so its physical
                // calls are observational records only: consume their Starts without registering
                // resolver awaiters or waiting for terminals.
                self.commit_consumed_entry(read_idx, &entry).await?;
                if let Some(wallet) = &self.cursor.wallet {
                    wallet.finish_replayed_entity(read_idx);
                }
                self.st
                    .custom_subtrees
                    .get_mut(&root)
                    .expect("custom subtree root disappeared while consuming an observational call")
                    .insert(read_idx);
                continue;
            }

            if let OplogEntry::Start {
                parent_start_index: Some(parent_start_index),
                ..
            } = &entry
                && let Some(root) = self.custom_subtree_root(*parent_start_index)
            {
                // A custom invocation returns its persisted root result without running its body.
                // Nested calls, scopes, and custom invocations therefore belong to the same replay-
                // inert tree through their ordinary parent identity, irrespective of interleaving.
                self.commit_consumed_entry(read_idx, &entry).await?;
                if let Some(wallet) = &self.cursor.wallet {
                    wallet.finish_replayed_entity(read_idx);
                }
                self.st
                    .custom_subtrees
                    .get_mut(&root)
                    .expect("custom subtree root disappeared while consuming a descendant")
                    .insert(read_idx);
                continue;
            }

            if terminal_start_index(&entry)
                .is_some_and(|start_index| self.is_custom_subtree_descendant(start_index))
            {
                self.commit_consumed_entry(read_idx, &entry).await?;
                continue;
            }

            if self.attach_retained_terminal(read_idx, &entry)? {
                // The `End`/`Cancelled` of a retained, still-unclaimed `Start`: keep it with the
                // `Start` for the owner's later claim and keep draining. Like an awaited terminal
                // it is never handed to a positional reader; like the retained `Start` it does
                // not publish the non-hint position before the claim.
                self.commit_retained_entry(read_idx, &entry).await?;
                continue;
            }

            if matches!(boundary, BoundaryRead::None)
                && self.st.is_returned_open_invocation_terminal(&entry)
            {
                // Once the export of an open invocation returned, only the open-anchor reader
                // consumes its terminal. Any other reader waits for it, so background work
                // recorded after the terminal is not handed the terminal while the primary Store
                // has not read it yet.
                self.st.replay_buffer.push_front((read_idx, entry));
                return Ok(None);
            }

            if condition(&entry) {
                self.commit_consumed_entry(read_idx, &entry).await?;
                if let BoundaryRead::InvocationBoundary(abandoned) = &mut boundary
                    && let Some((started, _)) = primary_invocation_terminal_id(&entry)
                        .and_then(|id| self.st.owners.open_invocation(id))
                {
                    // The replayed guest has produced its invocation result: the retained
                    // `Start`s of the invocation are live-only abandoned progress nobody can
                    // claim anymore. The boundary read holds the Store outside the concurrent
                    // runtime, so it cannot wait for a body.
                    let own = self.classify_retained_at_boundary(started, read_idx)?;
                    self.fold_retained_into_abandoned(abandoned, &own)?;
                    let st = &mut *self.st;
                    st.owners.close(started, read_idx, |index| {
                        st.starts.get_retained(&index).is_some()
                    });
                }
                return Ok(Some((read_idx, entry)));
            } else if unclaimed_starts == UnclaimedStarts::Retain
                && Self::is_retainable_start(&entry)
            {
                // A durable-call `Start` this reader is not entitled to: neither its own entry
                // (ownership is validated at claim time) nor something to park on (its owner may
                // still need the Store this reader holds). Commit past it and retain it for the
                // owner's claim; the terminal that closes it is attached above when reached. The
                // non-hint position stays where the owner will observe it at its `begin_function`.
                self.validate_start_owner(read_idx, &entry)?;
                self.retain_start(read_idx, entry.clone());
                self.commit_retained_entry(read_idx, &entry).await?;
                continue;
            } else {
                // Predicate failed: the speculative read published nothing, so the cursor,
                // skipped-region state, and side effects are already untouched.
                self.st.replay_buffer.push_front((read_idx, entry));
                return Ok(None);
            }
        }
    }

    /// Classifies a positional read (`positional_reader_accepts`) that returned nothing while the
    /// cursor is still replaying. Such a read parked either at a reserved `CompletionDelivered`
    /// marker or at an entry another Store recorded through this shared cursor (an entity body
    /// interleaved with its owner, or the owner interleaved with the body). Parking on another
    /// Store's entry is legitimate only while that Store can still consume it:
    ///
    /// - the primary agent Store replays for as long as the cursor does;
    /// - an entity body can while `CursorState::body_can_still_consume` holds for it: it
    ///   replays or its `Start` is claimed ahead, or its `Start` is retained and its dispatcher
    ///   (the enclosing body, or the primary Store of its open invocation) can still run.
    ///
    /// Anything else is an entity body nobody will reconstruct: waiting would hang replay, so the
    /// head is reported as the divergence it is. The invocation-boundary reader never waits on
    /// another Store: every entity body of the invocation has terminated before
    /// `AgentInvocationFinished`, so an unconsumed body entry there is dead history. The
    /// open-anchor reader waits like an ordinary reader: the background work of its invocation,
    /// including entity bodies, still replays.
    pub(in crate::durable_host::replay_state) async fn check_parked_positional_read(
        &self,
        reader: PositionalReader,
    ) -> Result<(), WorkerExecutorError> {
        if self.blocked_on_completion_delivery || self.cursor.at_replay_tail() {
            return Ok(());
        }
        let Some((head_idx, head)) = self.st.replay_buffer.front() else {
            return Ok(());
        };
        let owner = match (reader, head_owner(head)) {
            // A positional reader never classifies a nested `Start` left at the head: its parent
            // may be a call of the reading Store.
            (_, HeadOwner::Unattributed | HeadOwner::ParentStart(_))
            | (PositionalReader::Ordinary | PositionalReader::OpenAnchor, HeadOwner::Agent) => {
                return Ok(());
            }
            (PositionalReader::InvocationBoundary, _) => {
                return Err(WorkerExecutorError::replay_divergence(ReplayDivergence {
                    oplog_index: *head_idx,
                    expected: ReplayStep::described(format!(
                        "{head:?} at {head_idx} was recorded by an entity body that replay reached the invocation boundary without reconstructing"
                    )),
                    actual: ReplayStep::invocation_end(),
                    mismatched: Vec::new(),
                }));
            }
            (
                PositionalReader::Ordinary | PositionalReader::OpenAnchor,
                HeadOwner::EntityBody(owner),
            ) => owner,
        };
        if self
            .st
            .body_can_still_consume(&self.cursor.reconstruction_claims, owner, None)
        {
            Ok(())
        } else {
            Err(WorkerExecutorError::unexpected_oplog_entry(
                *head_idx,
                format!(
                    "{head:?} at {head_idx} was recorded by the entity body started at {owner}, which can no longer consume it"
                ),
                "an entry recorded by this Store",
            ))
        }
    }
}

impl CursorState {
    pub(super) async fn disposal_admission_refusal(
        &self,
        cursor: &ReplayCursor,
        operation: &OwnerToolOperation,
    ) -> Result<Option<SerializableToolAdmissionRefusal>, WorkerExecutorError> {
        let Some((refusal, terminal)) = operation.expected_disposal_refusal() else {
            return Ok(None);
        };
        if cursor.delivery_in_progress.load(Ordering::Acquire) {
            return Ok(None);
        }
        if let Some(failure) = cursor.delivery_failure.lock().unwrap().clone() {
            return Err(failure);
        }
        let Some(owner) = operation.invocation_id().map(|id| id.start_index()) else {
            return Ok(None);
        };
        if terminal > cursor.replay_target() {
            return Ok(None);
        }
        // Collection is not a guest demand. Its recorded refusal terminates this owner only
        // after every earlier real call and guest observation has been reconstructed.
        let boundary = self.entity_replay_progress.completed.get(&owner)
            .filter(|boundary| boundary.terminal_index == terminal
                && terminal > owner
                && self.entity_replay_horizon >= cursor.replay_target()
                && !self.skipped_regions.is_in_deleted_region(owner)
                && !self.skipped_regions.is_in_deleted_region(terminal)
                && boundary.last_required.is_none_or(|index| index > owner && index < terminal
                    && !self.skipped_regions.is_in_deleted_region(index)))
            .ok_or_else(|| WorkerExecutorError::unexpected_oplog_entry(terminal,
                "folded entity replay boundary matching the disposal terminal and replay horizon",
                format!("entity {owner}, terminal {terminal}, horizon {}", cursor.replay_target()),
            ))?;
        if boundary
            .last_required
            .is_some_and(|index| index > cursor.last_replayed_index())
        {
            return Ok(None);
        }
        if self.starts.retained_summary().entities.contains_key(&owner) {
            return Ok(None);
        }
        if let Some(failure) = cursor.delivery_failure.lock().unwrap().clone() {
            return Err(failure);
        }
        Ok(Some(refusal))
    }

    async fn positional_admission_refusal(
        &self,
        cursor: &ReplayCursor,
        scope: Option<OplogIndex>,
        admission: Option<&PositionalAdmission>,
    ) -> Result<Option<SerializableToolAdmissionRefusal>, WorkerExecutorError> {
        let Some((operation, request)) = admission else {
            return Ok(None);
        };
        let Some(owner) = scope else {
            return Ok(None);
        };
        if let Some(refusal) = self.disposal_admission_refusal(cursor, operation).await? {
            return Ok(Some(refusal));
        }
        let Some((expected, terminal_index)) = operation.expected_admission_refusal() else {
            return Ok(None);
        };
        if expected != (SerializableToolAdmissionRefusal::BeforePositional { request: *request })
            && operation.observed_admission_refusal().is_none()
        {
            return Ok(None);
        }
        let next = cursor.last_replayed_index().next();
        let end = terminal_index.min(cursor.replay_target().next());
        let remaining = cursor
            .scan_oplog(
                next,
                end,
                &self.skipped_regions,
                self.skipped_regions.find_next_deleted_region(next),
                OplogIndex::NONE,
                |entry, _, _| {
                    positional_reader_accepts(Some(owner))(entry)
                        && !entry.is_hint()
                        && !matches!(entry, OplogEntry::End { .. } | OplogEntry::Cancelled { .. })
                },
                |_, _, _| true,
            )
            .await?;
        // Any earlier positional record wins, including a different kind that must still
        // produce ordinary positional divergence rather than being hidden by the refusal.
        Ok(matches!(remaining, OplogEntryLookupResult::NotFound { .. }).then_some(expected))
    }
}

impl ReplayState {
    pub(crate) async fn disposal_admission_refusal(
        &self,
        operation: &OwnerToolOperation,
    ) -> Result<Option<SerializableToolAdmissionRefusal>, WorkerExecutorError> {
        if operation.expected_disposal_refusal().is_none() {
            return Ok(None);
        }
        let operation = operation.clone();
        self.run_owned_cursor_op(move |replay| async move {
            let state = replay.cursor.state.lock().await;
            state
                .disposal_admission_refusal(&replay.cursor, &operation)
                .await
        })
        .await
    }

    /// A recorded collection refusal is an owner terminal, not a demand to collect again.
    /// This observer never consumes records: real calls and reads must advance replay first.
    pub(crate) async fn await_disposal_admission_refusal(
        &self,
        operation: OwnerToolOperation,
    ) -> WorkerExecutorError {
        if operation.expected_disposal_refusal().is_none() {
            return std::future::pending().await;
        }
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            let ready = self.disposal_admission_refusal(&operation).await;
            match ready {
                Ok(Some(refusal)) => {
                    let error = operation.reproduce_admission_refusal(refusal).await;
                    self.cursor.progress.notify_waiters();
                    return error;
                }
                Err(error) => return error,
                Ok(None) => progress.await,
            }
        }
    }

    /// The error returned when a positional oplog reader expects a next entry but the cursor is
    /// at end-of-replay.
    fn end_of_replay_error(&self) -> WorkerExecutorError {
        WorkerExecutorError::replay_divergence(ReplayDivergence {
            oplog_index: self.cursor.last_replayed_index().next(),
            expected: ReplayStep::end_of_history(),
            actual: ReplayStep::described(format!(
                "a read of the next oplog entry to replay for {}; replay target = {}",
                self.cursor.owned_agent_id,
                self.cursor.replay_target(),
            )),
            mismatched: Vec::new(),
        })
    }

    /// Reports whether the replay-visible owner oplog contains a terminal for `start_index`.
    /// Entity reconstruction uses this before starting its fresh Store so the invocation context
    /// distinguishes reconstruction of a completed body from repair of an incomplete Start. The
    /// scan is read-only and respects fork/revert deleted regions.
    pub(crate) async fn has_visible_terminal(
        &self,
        start_index: OplogIndex,
    ) -> Result<bool, WorkerExecutorError> {
        Ok(self.visible_terminal_entry(start_index).await?.is_some())
    }

    /// Reports whether the replay-visible oplog contains any recorded work in the durable scope
    /// rooted at `start_index`. An incomplete filesystem-capable tool with no such work can release
    /// its historical body-reconstruction fence while it continues staging fresh input; a tool
    /// whose prior body started must retain the fence and reconstruct those descendants first.
    ///
    /// A guest read of a durable session input is recorded in the session's attribution, not the
    /// body's, and names the body only in its observation. Such a read is work of the body too.
    pub(crate) async fn has_visible_scope_descendant(
        &self,
        start_index: OplogIndex,
    ) -> Result<bool, WorkerExecutorError> {
        let replay_target = self.replay_target();
        if start_index >= replay_target {
            return Ok(false);
        }
        let skipped_regions = {
            let state = self.cursor.state.lock().await;
            state.skipped_regions.clone()
        };
        if self
            .cursor
            .stream_observations
            .lock()
            .unwrap()
            .iter()
            .any(|(index, parent)| {
                *parent == Some(start_index)
                    && *index <= replay_target
                    && !skipped_regions.is_in_deleted_region(*index)
            })
        {
            return Ok(true);
        }
        let mut projection = OplogScopeProjection::new(start_index);
        let mut next = start_index.next();
        while next <= replay_target {
            let available = u64::from(replay_target) - u64::from(next) + 1;
            let entries = self
                .cursor
                .oplog
                .read_exact(next, CHUNK_SIZE.min(available))
                .await?;
            let last_read = *entries.last_key_value().unwrap().0;
            for (index, entry) in entries {
                if index > replay_target {
                    break;
                }
                if !skipped_regions.is_in_deleted_region(index)
                    && projection.includes(index, &entry)
                {
                    return Ok(true);
                }
            }
            next = last_read.next();
        }
        Ok(false)
    }

    /// Returns the replay-visible terminal for `start_index` without advancing the positional
    /// cursor. Tool replay uses the terminal's body-execution decision before allocating a
    /// transient Store; the ordinary reconstruction path subsequently consumes the same terminal.
    pub(crate) async fn visible_terminal_entry(
        &self,
        start_index: OplogIndex,
    ) -> Result<Option<OplogEntry>, WorkerExecutorError> {
        Ok(self
            .visible_terminal_record(start_index)
            .await?
            .map(|(_, entry)| entry))
    }

    pub(crate) async fn visible_terminal_record(
        &self,
        start_index: OplogIndex,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        let replay_target = self.replay_target();
        if start_index >= replay_target {
            return Ok(None);
        }
        let skipped_regions = {
            let state = self.cursor.state.lock().await;
            state.skipped_regions.clone()
        };
        let mut next = start_index.next();
        while next <= replay_target {
            let available = u64::from(replay_target) - u64::from(next) + 1;
            let entries = self
                .cursor
                .oplog
                .read_exact(next, CHUNK_SIZE.min(available))
                .await?;
            let last_read = *entries.last_key_value().unwrap().0;
            if let Some((index, entry)) = entries.into_iter().find(|(index, entry)| {
                *index <= replay_target
                    && !skipped_regions.is_in_deleted_region(*index)
                    && terminal_start_index(entry) == Some(start_index)
            }) {
                return Ok(Some((index, entry)));
            }
            next = last_read.next();
        }
        Ok(None)
    }

    /// Waits until the replay cursor is blocked on a record in `root`'s call tree that no still
    /// running reconstructed entity body or live resolver awaiter can consume. This turns a body
    /// that returned before replaying its recorded subtree into permanent structural divergence
    /// instead of leaving the outer terminal awaiter parked forever.
    pub(crate) async fn await_unconsumed_scope_entry(
        &self,
        root: OplogIndex,
        mut active_entity_bodies: tokio::sync::watch::Receiver<HashSet<OplogIndex>>,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        loop {
            let active_bodies = active_entity_bodies.borrow_and_update().clone();
            let bodies_changed = active_entity_bodies.changed();
            tokio::pin!(bodies_changed);
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();

            if let Some(index) = self.unconsumed_scope_head(root, active_bodies).await? {
                return Ok(index);
            }

            tokio::select! {
                _ = progress.as_mut() => {}
                changed = &mut bodies_changed => {
                    if changed.is_err() {
                        return Err(WorkerExecutorError::runtime(
                            "owner reconstruction body tracker closed during replay",
                        ));
                    }
                }
            }
        }
    }

    pub(in crate::durable_host::replay_state) async fn unconsumed_scope_head(
        &self,
        root: OplogIndex,
        active_entity_bodies: HashSet<OplogIndex>,
    ) -> Result<Option<OplogIndex>, WorkerExecutorError> {
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let st = cursor.state.lock().await;
            let head = cursor.last_replayed_index().next();
            if head > cursor.replay_target() {
                return Ok(None);
            }

            let mut projection = OplogScopeProjection::new(root);
            let mut parents = HashMap::new();
            let mut next = root;
            while next <= head {
                let available = u64::from(head) - u64::from(next) + 1;
                let entries = cursor
                    .oplog
                    .read_exact(next, CHUNK_SIZE.min(available))
                    .await?;
                let last_read = *entries.last_key_value().unwrap().0;

                for (index, entry) in entries {
                    if index > head {
                        break;
                    }
                    if st.skipped_regions.is_in_deleted_region(index) {
                        continue;
                    }
                    let included = projection.includes(index, &entry);
                    if included
                        && let OplogEntry::Start {
                            parent_start_index, ..
                        } = &entry
                        && let Some(parent) = parent_start_index
                    {
                        parents.insert(index, *parent);
                    }

                    if index == head {
                        if !included {
                            return Ok(None);
                        }
                        if is_auto_skippable_hint(&entry)
                            && !is_active_stream_observation(cursor, &st, index)
                        {
                            return Ok(None);
                        }
                        if terminal_start_index(&entry)
                            .is_some_and(|start_index| st.starts.owns_terminal(start_index, index))
                            || custom_subtree_entry_is_drainable(&st, &entry)
                        {
                            return Ok(None);
                        }
                        let owner = scope_entry_owner(index, &entry);
                        if owner.is_some_and(|mut owner| {
                            while owner != root {
                                if active_entity_bodies.contains(&owner) {
                                    return true;
                                }
                                let Some(parent) = parents.get(&owner) else {
                                    break;
                                };
                                owner = *parent;
                            }
                            false
                        }) {
                            return Ok(None);
                        }
                        if matches!(entry, OplogEntry::Start { .. })
                            && st.starts.is_claimed_ahead(&index)
                        {
                            return Ok(None);
                        }
                        if owner.is_some_and(|owner| owner != root && st.starts.is_awaited(owner)) {
                            return Ok(None);
                        }
                        if terminal_start_index(&entry) == Some(root) && st.starts.is_awaited(root)
                        {
                            return Ok(None);
                        }
                        return Ok(Some(index));
                    }
                }
                next = last_read.next();
            }
            Ok(None)
        })
        .await
    }

    /// The first retained `Start` whose nearest entity body is `body`. A settled body that leaves
    /// one did not claim a call it recorded.
    pub(crate) async fn retained_record_of_body(
        &self,
        body: OplogIndex,
    ) -> Result<Option<OplogIndex>, WorkerExecutorError> {
        self.run_owned_cursor_op(move |state| async move {
            let st = state.cursor.state.lock().await;
            Ok(st.retained_record_of_body(body))
        })
        .await
    }

    /// Resolves once the entity body started at `body` is not a replaying body with a committed
    /// recorded `Cancelled`. Every record of such a body precedes its `Cancelled`, so a read of
    /// the body that reaches the replay tail would follow its recorded cancellation point: it
    /// waits until the body is aborted, which drops a read of the body itself.
    pub(crate) async fn await_cancelled_body_abort(
        &self,
        body: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        loop {
            let mut bodies = self.cursor.reconstruction_claims.subscribe_bodies();
            if !bodies.borrow_and_update().contains(&body) {
                return Ok(());
            }
            let cancelled = self
                .run_owned_cursor_op(move |state| async move {
                    let st = state.cursor.state.lock().await;
                    Ok(st.owners.is_cancelled_entity(body))
                })
                .await?;
            if !cancelled {
                return Ok(());
            }
            if bodies.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    /// Resolves once no retained `Start` has `body` as its nearest entity body: the body claimed
    /// every call it recorded before the cursor's position.
    pub(crate) async fn await_body_records_claimed(
        &self,
        body: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            if self.retained_record_of_body(body).await?.is_none() {
                return Ok(());
            }
            self.wait_while_blocked(BlockedOn::CursorProgress, progress)
                .await?;
        }
    }

    /// Whether `oplog_index` lies in a deleted (skipped) oplog region. Session replay uses this
    /// validity guard, so a failed cursor read propagates as an error rather than defaulting to
    /// an answer.
    pub async fn is_in_skipped_region(
        &self,
        oplog_index: OplogIndex,
    ) -> Result<bool, WorkerExecutorError> {
        self.run_owned_cursor_op(move |state| async move {
            let st = state.cursor.state.lock().await;
            Ok(st.skipped_regions.is_in_deleted_region(oplog_index))
        })
        .await
    }

    /// Reads the next oplog entry, and skips every hint entry following it.
    /// Returns the oplog index of the entry read, no matter how many more hint entries
    /// were read.
    ///
    /// Returns an error if the underlying read fails (e.g. missing oplog entry,
    /// corrupted GolemApiFork payload) so the worker can fail the agent with a
    /// non-retriable trap rather than panicking the executor.
    pub async fn get_oplog_entry(
        &self,
        scope: Option<OplogIndex>,
    ) -> Result<(OplogIndex, OplogEntry), WorkerExecutorError> {
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            if let Some(entry) = self
                .with_tx(async |tx| {
                    let entry = tx
                        .try_get_oplog_entry(positional_reader_accepts(scope))
                        .await?;
                    if entry.is_none() {
                        tx.check_parked_positional_read(PositionalReader::Ordinary)
                            .await?;
                    }
                    Ok(entry)
                })
                .await?
            {
                return Ok(entry);
            }
            if self.at_replay_tail() {
                return Err(self.end_of_replay_error());
            }
            // An unconditional reader returns `None` during replay only at a reserved
            // `CompletionDelivered` marker or at another Store's entry. Wait for its owner to
            // consume it.
            progress.await;
        }
    }

    /// Atomically classifies the next positional read as either an entry or the replay tail.
    /// A reserved completion-delivery boundary or another Store's entry at the head is waited out
    /// rather than mistaken for the tail.
    pub(crate) async fn get_oplog_entry_or_replay_end_with_admission(
        &self,
        scope: Option<OplogIndex>,
        admission: Option<PositionalAdmission>,
    ) -> Result<PositionalRead, WorkerExecutorError> {
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            let gate = self.cursor.advance_gate.clone().lock_owned();
            tokio::pin!(gate);
            let gate = loop {
                let changed = self.cursor.progress.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let st = self.cursor.state.lock().await;
                if let Some(failure) = self.cursor.delivery_failure.lock().unwrap().clone() {
                    return Err(failure);
                }
                let refusal = st
                    .positional_admission_refusal(&self.cursor, scope, admission.as_ref())
                    .await?;
                drop(st);
                if let Some(refusal) = refusal {
                    let error = admission
                        .as_ref()
                        .unwrap()
                        .0
                        .reproduce_admission_refusal(refusal)
                        .await;
                    self.cursor.progress.notify_waiters();
                    return Err(error);
                }
                tokio::select! {
                    guard = &mut gate => break guard,
                    _ = changed => {},
                }
            };
            let mut tx = self.cursor.tx_with_gate(gate).await?;
            if let Some(refusal) = tx
                .st
                .positional_admission_refusal(&self.cursor, scope, admission.as_ref())
                .await?
            {
                self.cursor.finish_tx(tx);
                let error = admission
                    .as_ref()
                    .unwrap()
                    .0
                    .reproduce_admission_refusal(refusal)
                    .await;
                self.cursor.progress.notify_waiters();
                return Err(error);
            }
            let read: Result<Option<PositionalRead>, WorkerExecutorError> = async {
                let entry = tx
                    .try_get_oplog_entry(positional_reader_accepts(scope))
                    .await?;
                Ok(match entry {
                    Some((index, entry)) => Some(PositionalRead::Entry(index, entry)),
                    None if tx.cursor.at_replay_tail() => Some(PositionalRead::ReplayEnded),
                    None => {
                        tx.check_parked_positional_read(PositionalReader::Ordinary)
                            .await?;
                        None
                    }
                })
            }
            .await;
            self.cursor.finish_tx(tx);
            let read: Option<PositionalRead> = read?;
            if let Some(read) = read {
                return Ok(read);
            }
            progress.await;
        }
    }

    /// Reads the next oplog entry, and if it matches the given condition, skips
    /// every hint entry following it and returns the oplog index of the entry read.
    /// If the condition is not met, returns `None` and the candidate entry is left unconsumed with
    /// the cursor, skipped-region state, and side effects untouched. (Any *awaited terminals* sitting
    /// ahead of the candidate are drained to their awaiters first — see
    /// [`CursorTx::try_get_oplog_entry`] — and those drains stay committed.)
    ///
    /// Auto-skipped hint entries manipulate worker status but are non-deterministic from the
    /// replay's point of view.
    #[cfg(test)]
    pub async fn try_get_oplog_entry(
        &self,
        condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        let mut condition = condition;
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            let (entry, blocked_on_completion_delivery) = self
                .with_tx(async |tx| {
                    let entry = tx.try_get_oplog_entry(&mut condition).await?;
                    Ok((entry, tx.blocked_on_completion_delivery))
                })
                .await?;
            if entry.is_some() || !blocked_on_completion_delivery {
                return Ok(entry);
            }
            progress.await;
        }
    }

    /// Returns true if the given log entry has unmatched persisted occurrences since the last
    /// non-hint oplog entry.
    pub async fn seen_log(&self, level: LogLevel, context: &str, message: &str) -> bool {
        if self.cursor.position.has_seen_logs.load(Ordering::Relaxed) {
            let hash = ReplayCursor::hash_log_entry(level, context, message);
            self.cursor.log_hashes.lock().unwrap().contains_key(&hash)
        } else {
            false
        }
    }

    /// Removes one occurrence of a seen log from the multiset (identical log entries may be
    /// persisted multiple times and each must be matched by exactly one re-emitted entry). If the
    /// multiset becomes empty, `seen_log` becomes a cheap operation
    pub async fn remove_seen_log(&self, level: LogLevel, context: &str, message: &str) {
        let hash = ReplayCursor::hash_log_entry(level, context, message);
        let log_hashes = &mut *self.cursor.log_hashes.lock().unwrap();
        if let Some(count) = log_hashes.get_mut(&hash) {
            *count -= 1;
            if *count == 0 {
                log_hashes.remove(&hash);
            }
        }
        self.cursor
            .position
            .has_seen_logs
            .store(!log_hashes.is_empty(), Ordering::Relaxed);
    }

    pub async fn get_oplog_entry_agent_invocation_started(
        &self,
    ) -> Result<Option<AgentInvocationStartedEntry>, WorkerExecutorError> {
        loop {
            if self.has_replay_remaining() {
                let (oplog_index, oplog_entry) = self.get_oplog_entry(None).await?;
                match oplog_entry {
                    OplogEntry::AgentInvocationStarted {
                        invocation_id: idempotency_key,
                        operation,
                        payload,
                        trace_id,
                        trace_states,
                        invocation_context: spans,
                        wallet_pin,
                        ..
                    } => {
                        let invocation_payload = self
                            .cursor
                            .oplog
                            .download_request_classified(*operation, payload)
                            .await
                            .map_err(|error| match error {
                                crate::services::oplog::OplogPayloadDownloadError::Backend(
                                    error,
                                ) => WorkerExecutorError::recovery_required(format!(
                                    "failed to download agent invocation payload: {error:#}"
                                )),
                                crate::services::oplog::OplogPayloadDownloadError::Corrupt(
                                    error,
                                ) => WorkerExecutorError::unexpected_oplog_entry(
                                    oplog_index,
                                    "valid agent invocation payload",
                                    format!("{error:#}"),
                                ),
                            })?;

                        let invocation_context =
                            InvocationContextStack::from_oplog_data(trace_id, trace_states, spans);

                        break Ok(Some(AgentInvocationStartedEntry {
                            oplog_index,
                            idempotency_key,
                            invocation_payload,
                            invocation_context,
                            wallet_pin: *wallet_pin,
                        }));
                    }
                    entry if entry.is_hint() => {}
                    _ => {
                        break Err(WorkerExecutorError::unexpected_oplog_entry(
                            oplog_index,
                            format!("{oplog_entry:?}"),
                            "AgentInvocationStarted",
                        ));
                    }
                }
            } else {
                break Ok(None);
            }
        }
    }

    /// The index and result of the `AgentInvocationFinished` for `invocation_id`, or `None`
    /// when no replay remains or at the natural replay tail. A recorded terminal naming another
    /// invocation is a divergence.
    pub async fn get_oplog_entry_agent_invocation_finished(
        &self,
        invocation_id: &golem_common::model::InvocationId,
    ) -> Result<Option<(OplogIndex, AgentInvocationResult)>, WorkerExecutorError> {
        match self
            .get_oplog_entry_agent_invocation_terminal(invocation_id)
            .await?
        {
            Some(terminal) => self.agent_invocation_finished(terminal).await.map(Some),
            None => Ok(None),
        }
    }

    /// The index and result of `terminal`, an invocation terminal the cursor consumed, when it
    /// is an `AgentInvocationFinished`. Any other entry is a divergence.
    pub async fn agent_invocation_finished(
        &self,
        terminal: (OplogIndex, OplogEntry),
    ) -> Result<(OplogIndex, AgentInvocationResult), WorkerExecutorError> {
        match terminal {
            (index, OplogEntry::AgentInvocationFinished { result, .. }) => {
                let result = self
                    .cursor
                    .oplog
                    .download_payload(result)
                    .await
                    .map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "failed to deserialize agent invocation result payload: {err}"
                        ))
                    })?;
                Ok((index, result))
            }
            (index, entry) => Err(WorkerExecutorError::unexpected_oplog_entry(
                index,
                format!("{entry:?}"),
                "AgentInvocationFinished",
            )),
        }
    }

    pub async fn get_oplog_entry_agent_method_failure(
        &self,
        invocation_id: &golem_common::model::InvocationId,
        anchor: OplogIndex,
        error: &golem_common::model::oplog::AgentError,
        parent: Option<OplogIndex>,
    ) -> Result<Option<OplogIndex>, WorkerExecutorError> {
        self.get_oplog_entry_agent_invocation_terminal(invocation_id)
            .await?
            .map(|terminal| Self::agent_method_failure(terminal, anchor, error, parent))
            .transpose()
    }

    /// The index of `terminal`, an invocation terminal the cursor consumed, when it is the
    /// `AgentInvocationFailed` that records `error` for the invocation started at `anchor` with
    /// the entity parent `parent`. Any other entry is a divergence.
    pub fn agent_method_failure(
        terminal: (OplogIndex, OplogEntry),
        anchor: OplogIndex,
        error: &golem_common::model::oplog::AgentError,
        parent: Option<OplogIndex>,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        match terminal {
            (
                index,
                OplogEntry::AgentInvocationFailed {
                    error: recorded,
                    start_index,
                    entity_parent_start_index,
                    ..
                },
            ) if start_index == anchor
                && &recorded == error
                && entity_parent_start_index == parent =>
            {
                Ok(index)
            }
            (index, entry) => Err(WorkerExecutorError::unexpected_oplog_entry(
                index,
                format!("{entry:?}"),
                format!("invocation-local error {error:?} for {anchor} with parent {parent:?}"),
            )),
        }
    }

    async fn get_oplog_entry_agent_invocation_terminal(
        &self,
        invocation_id: &golem_common::model::InvocationId,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        // The walk to the finished marker tolerates live-only abandoned durable-call records
        // (see `AbandonedStarts`): the replayed guest has already produced its invocation
        // result, so any still-unclaimed `Start` (and its terminal) can never be claimed and is
        // dead partial progress of a branch the guest abandoned at a point replay did not
        // reproduce.
        let mut abandoned = AbandonedStarts::default();
        loop {
            if self.has_replay_remaining() {
                let Some((index, oplog_entry)) = self
                    .get_oplog_entry_at_invocation_boundary(&mut abandoned)
                    .await?
                else {
                    break Ok(None);
                };
                match oplog_entry {
                    entry @ (OplogEntry::AgentInvocationFinished { .. }
                    | OplogEntry::AgentInvocationFailed { .. }) => {
                        let matches_invocation = matches!(
                            &entry,
                            OplogEntry::AgentInvocationFinished { invocation_id: recorded, .. }
                                | OplogEntry::AgentInvocationFailed { invocation_id: recorded, .. }
                                if recorded == invocation_id
                        );
                        if !matches_invocation {
                            break Err(WorkerExecutorError::unexpected_oplog_entry(
                                index,
                                format!("{entry:?}"),
                                format!("invocation terminal for invocation {invocation_id}"),
                            ));
                        }
                        std::mem::take(&mut abandoned).finish(&self.cursor.owned_agent_id)?;
                        break Ok(Some((index, entry)));
                    }
                    entry if entry.is_hint() => {}
                    _ => {
                        break Err(WorkerExecutorError::replay_divergence(ReplayDivergence {
                            oplog_index: index,
                            expected: ReplayStep::described(format!("{oplog_entry:?}")),
                            actual: ReplayStep::invocation_end(),
                            mismatched: Vec::new(),
                        }));
                    }
                }
            } else {
                break Ok(None);
            }
        }
    }

    /// [`Self::get_oplog_entry`] for the agent-invocation-finished reader: drains live-only
    /// abandoned durable-call records into `abandoned` instead of handing them to the positional
    /// reader (see [`AbandonedStarts`]).
    pub(in crate::durable_host::replay_state) async fn get_oplog_entry_at_invocation_boundary(
        &self,
        abandoned: &mut AbandonedStarts,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            if let Some(entry) = self
                .with_tx(async |tx| {
                    let entry = tx
                        .try_get_oplog_entry_at_invocation_boundary(
                            abandoned,
                            positional_reader_accepts(None),
                        )
                        .await?;
                    if entry.is_none() {
                        tx.check_parked_positional_read(PositionalReader::InvocationBoundary)
                            .await?;
                    }
                    Ok(entry)
                })
                .await?
            {
                return Ok(Some(entry));
            }
            if self.at_replay_tail() {
                // An acknowledged guest boundary defers hint skipping until this read. Those
                // hints can exhaust an incomplete invocation's tail without a Finished entry.
                return Ok(None);
            }
            progress.await;
        }
    }
}

impl CursorTx<'_> {
    /// One transaction of [`ReplayState::get_oplog_entry_agent_invocation_terminal_in_open_anchor`].
    /// Returns `None` while the reader must wait for cursor progress or at the replay tail.
    async fn try_get_open_anchor_terminal(
        &mut self,
        invocation_id: golem_common::model::InvocationId,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        let Some((anchor, export_returned)) = self
            .st
            .owners
            .open_invocation(&invocation_id)
            .and_then(|(anchor, open)| Some((anchor, open.export_returned.as_ref()?)))
        else {
            return Err(WorkerExecutorError::runtime(format!(
                "the invocation {invocation_id} has no open invocation anchor"
            )));
        };
        export_returned.mark();
        let settled_before = |st: &CursorState, bound: Option<OplogIndex>| {
            st.first_settled_after(anchor)
                .filter(|settled| bound.is_none_or(|bound| *settled < bound))
        };
        let settled_first = |settled: OplogIndex| {
            boundary_divergence(
                settled,
                format!(
                    "AgentInvocationSettled at {settled}, before the terminal of invocation \
                     {invocation_id}"
                ),
            )
        };
        if let Some(settled) = settled_before(&self.st, None) {
            return Err(settled_first(settled));
        }
        let read = self
            .try_get_oplog_entry_inner(
                None,
                None,
                BoundaryRead::OpenAnchor,
                UnclaimedStarts::Retain,
                |entry| is_invocation_terminal_of(entry, &invocation_id),
            )
            .await?;
        if let Some(settled) = settled_before(&self.st, read.as_ref().map(|(index, _)| *index)) {
            return Err(settled_first(settled));
        }
        if read.is_some() {
            return Ok(read);
        }
        if !self.blocked_on_completion_delivery
            && let Some((index, head)) = self.st.replay_buffer.front()
            && (matches!(head, OplogEntry::AgentInvocationStarted { .. })
                || primary_invocation_terminal_id(head).is_some())
        {
            // No background work consumes an invocation boundary, so waiting would hang.
            return Err(boundary_divergence(
                *index,
                format!("{head:?} at {index}, before the terminal of invocation {invocation_id}"),
            ));
        }
        self.check_parked_positional_read(PositionalReader::OpenAnchor)
            .await?;
        Ok(None)
    }

    /// One transaction of [`ReplayState::close_invocation_anchor`]. Returns `None` while an entity
    /// body that can still consume the entry at the head has not consumed it, or a retained
    /// `Start` of the invocation lies under a body that can still claim it.
    async fn settle_open_anchor(
        &mut self,
        anchor: OplogIndex,
        invocation_id: golem_common::model::InvocationId,
    ) -> Result<Option<SettledAt>, WorkerExecutorError> {
        if self
            .st
            .owners
            .open_invocation_at(anchor)
            .is_none_or(|open| open.id != invocation_id || !open.records_result_at_return())
        {
            return Err(WorkerExecutorError::runtime(format!(
                "the invocation anchor {anchor} of invocation {invocation_id} is not open"
            )));
        }
        if self.st.settled_after(anchor, &invocation_id).is_none() {
            // Every guest task of the invocation exited, so nothing can claim the records left
            // before the settlement: the walk retains them for the abandoned-record tolerance.
            self.try_get_oplog_entry_inner(
                None,
                None,
                BoundaryRead::Settlement { anchor },
                UnclaimedStarts::Retain,
                |_| false,
            )
            .await?;
        }
        let settled = self.st.settled_after(anchor, &invocation_id);
        if let Some(other) = self.st.first_settled_after(anchor)
            && settled.is_none_or(|settled| other < settled)
        {
            return Err(boundary_divergence(
                other,
                format!(
                    "the AgentInvocationSettled of another invocation at {other}, before the \
                     settlement of invocation {invocation_id}"
                ),
            ));
        }
        let Some(settled) = settled else {
            if self.cursor.at_replay_tail() && !self.blocked_on_completion_delivery {
                // The live run appends the settlement. The retained `Start`s stay retained: the
                // caller releases them at the live end of the drain
                // (`release_retained_starts_at_live_invocation_end`).
                self.finish_closed_anchor(anchor, self.cursor.replay_target().next());
                return Ok(Some(SettledAt::ReplayTail));
            }
            // An entity body replays in its own Store and can outlive the primary Store's guest
            // tasks, so its entry at the head waits for it while it can still consume it.
            if let Some(body) = self.head_body()
                && self.st.body_can_still_consume(
                    &self.cursor.reconstruction_claims,
                    body,
                    Some(anchor),
                )
            {
                return Ok(None);
            }
            // Any other entry at the head is consumed only by a guest read of the primary Store
            // or by a body that can no longer consume it, and every guest task of the invocation
            // exited.
            let (index, head) = self
                .st
                .replay_buffer
                .front()
                .map(|(index, entry)| (*index, format!("{entry:?} at {index}")))
                .unwrap_or_else(|| {
                    (
                        self.cursor.last_replayed_index().next(),
                        "no entry".to_string(),
                    )
                });
            return Err(boundary_divergence(
                index,
                format!(
                    "{head}, before the settlement of invocation {invocation_id}, which no ended \
                     background work of the invocation can consume"
                ),
            ));
        };
        let Some(own) = self.classify_retained_at_close(anchor, settled)? else {
            return Ok(None);
        };
        let mut abandoned = AbandonedStarts::default();
        self.fold_retained_into_abandoned(&mut abandoned, &own)?;
        abandoned.finish(&self.cursor.owned_agent_id)?;
        self.finish_closed_anchor(anchor, settled);
        Ok(Some(SettledAt::Recorded(settled)))
    }

    /// Whether the settlement walk may leave the delivery marker of the retained `Start` at
    /// `start_index` at the head: the body that encloses that `Start` can still claim it and
    /// consume the marker.
    fn marker_body_can_still_consume(
        &self,
        boundary: &BoundaryRead<'_>,
        start_index: OplogIndex,
    ) -> bool {
        let BoundaryRead::Settlement { anchor, .. } = boundary else {
            return false;
        };
        self.st
            .owners
            .owner(start_index)
            .and_then(|owner| owner.body)
            .is_some_and(|body| {
                self.st.body_can_still_consume(
                    &self.cursor.reconstruction_claims,
                    body,
                    Some(*anchor),
                )
            })
    }

    /// Classifies the retained `Start`s recorded before `end` for the close of the invocation
    /// started at `closing`, whose primary guest tasks have exited. Returns the indices of the
    /// invocation's own `Start`s, or `None` when one of them lies under a body that can still
    /// claim it. Every `Start` that classifies as a divergence is reported as one.
    fn classify_retained_at_close(
        &self,
        closing: OplogIndex,
        end: OplogIndex,
    ) -> Result<Option<std::collections::BTreeSet<OplogIndex>>, WorkerExecutorError> {
        let mut wait = false;
        let own = self.classify_retained(closing, end, |_| {
            wait = true;
            Ok(())
        })?;
        Ok((!wait).then_some(own))
    }

    /// [`Self::classify_retained_at_close`] for a close that cannot wait for a body: a `Start`
    /// under a body that can still claim it is reported as a divergence.
    fn classify_retained_at_boundary(
        &self,
        closing: OplogIndex,
        end: OplogIndex,
    ) -> Result<std::collections::BTreeSet<OplogIndex>, WorkerExecutorError> {
        self.classify_retained(closing, end, |index| {
            Err(boundary_divergence(
                index,
                format!(
                    "the retained Start at {index}, which lies under an entity body that can \
                     still claim it, at the boundary of the invocation started at {closing}"
                ),
            ))
        })
    }

    /// Classifies the retained `Start`s recorded before `end` for the close of the invocation
    /// started at `closing`, returning the invocation's own `Start`s. `on_wait` decides each
    /// `Start` under a body that can still claim it.
    fn classify_retained(
        &self,
        closing: OplogIndex,
        end: OplogIndex,
        mut on_wait: impl FnMut(OplogIndex) -> Result<(), WorkerExecutorError>,
    ) -> Result<std::collections::BTreeSet<OplogIndex>, WorkerExecutorError> {
        let mut own = std::collections::BTreeSet::new();
        let below: Vec<OplogIndex> = self
            .st
            .starts
            .retained_keys()
            .copied()
            .filter(|index| *index < end)
            .collect();
        for index in below {
            match self.st.classify(
                &self.cursor.reconstruction_claims,
                index,
                closing,
                Some(closing),
            ) {
                Ok(Class::Own) => {
                    own.insert(index);
                }
                Ok(Class::Keep) => {}
                Ok(Class::Wait) => on_wait(index)?,
                Err(divergence) => return Err(boundary_divergence(index, divergence)),
            }
        }
        Ok(own)
    }

    /// The entity body that consumes the entry at the cursor head, when a body records it: the
    /// body named by its entity attribution, the nearest body of the `Start` a completion marker
    /// closes, or the body a stream observation belongs to.
    fn head_body(&self) -> Option<OplogIndex> {
        let (_, head) = self.st.replay_buffer.front()?;
        let body_of_start = |start: OplogIndex| {
            if self.st.owners.is_entity(start) {
                return Some(start);
            }
            self.st.owners.owner(start).and_then(|owner| owner.body)
        };
        match head {
            OplogEntry::CompletionDelivered { start_index, .. }
            | OplogEntry::CompletionDiscarded { start_index, .. } => self
                .st
                .owners
                .owner(*start_index)
                .and_then(|owner| owner.body),
            OplogEntry::HostStreamFrame {
                parent_start_index, ..
            } => body_of_start(*parent_start_index),
            entry => match head_owner(entry) {
                HeadOwner::EntityBody(body) => Some(body),
                HeadOwner::Unattributed | HeadOwner::Agent | HeadOwner::ParentStart(_) => None,
            },
        }
    }

    /// Finishes the wallet replay of the invocation anchored at `anchor`, closes the anchor at
    /// the entry at `at`, and prunes the settlements no open or later anchor can need. A close at
    /// the replay tail is at the entry after the replay target, where the live run appends the
    /// settlement.
    fn finish_closed_anchor(&mut self, anchor: OplogIndex, at: OplogIndex) {
        if let Some(wallet) = &self.cursor.wallet {
            wallet.finish_replayed_invocation();
        }
        let st = &mut *self.st;
        st.owners
            .close(anchor, at, |index| st.starts.get_retained(&index).is_some());
        // An anchor needs only the settlements after it, and later anchors lie after this one.
        let keep_from = self
            .st
            .owners
            .first_open_invocation()
            .map_or(anchor, |lowest_open| anchor.min(lowest_open))
            .next();
        self.st.settled_invocations = self.st.settled_invocations.split_off(&keep_from);
        self.notify_progress = true;
    }
}

impl ReplayState {
    /// Opens the invocation anchor of the agent-method or constructor invocation whose
    /// `AgentInvocationStarted` the cursor consumed at `anchor`, the last one it consumed: the
    /// open invocation records its result at the export's return. While the anchor is open, only
    /// [`Self::get_oplog_entry_agent_invocation_terminal_in_open_anchor`] consumes the
    /// invocation's terminal, the retained `Start`s stay claimable after that terminal, and the
    /// wallet replay of the invocation stays open until [`Self::close_invocation_anchor`] closes
    /// the invocation.
    ///
    /// `export_returned` is the invocation's return signal, which the export call sets when it
    /// returns.
    pub(crate) async fn open_invocation_anchor(
        &self,
        anchor: OplogIndex,
        export_returned: ExportReturned,
    ) -> Result<(), WorkerExecutorError> {
        self.run_owned_cursor_op(move |state| async move {
            let mut st = state.cursor.state.lock().await;
            let last = st.owners.last_invocation();
            if last != Some(anchor) {
                return Err(WorkerExecutorError::runtime(format!(
                    "cannot open the invocation anchor {anchor}: the last consumed \
                     AgentInvocationStarted is at {last:?}"
                )));
            }
            match st.owners.open_invocation_at_mut(anchor) {
                Some(open) if open.export_returned.is_none() => {
                    open.export_returned = Some(export_returned.clone());
                    Ok(())
                }
                _ => Err(WorkerExecutorError::runtime(format!(
                    "the invocation anchor {anchor} was already opened"
                ))),
            }
        })
        .await
    }

    /// The index and entry of the `AgentInvocationFinished` or `AgentInvocationFailed` of the
    /// open-anchor invocation `invocation_id`, or `None` at the replay tail.
    ///
    /// The background work of the invocation replays concurrently and may consume entries
    /// recorded before the terminal. An unclaimed `Start` is retained for it, and every other
    /// entry its owner can still consume (a reserved delivery marker, an agent entry, an entity
    /// body entry) is left at the head while the reader waits for cursor progress. Another
    /// invocation's terminal, an `AgentInvocationStarted` or an `AgentInvocationSettled` before
    /// the terminal is a divergence. Consuming the terminal keeps the retained `Start`s and the
    /// wallet replay of the invocation open for its background work.
    ///
    /// The driver calls this only after the export returned, and the first call marks the
    /// anchor's [`ExportReturned`] if the driver did not mark it already: from then on the other
    /// readers leave the terminal at the head, and a claim that finds no `Start` before it waits
    /// for this read instead of reporting a divergence.
    ///
    /// Like the ordinary reader, it waits on an agent-attributed or unattributed head that a
    /// background task may still consume. Once every guest task exited, the driver's deadlock
    /// detection turns a wait on a head nobody consumes into an error.
    ///
    /// The reader holds the value `park` returns while it waits for cursor progress, and only
    /// then.
    pub(crate) async fn get_oplog_entry_agent_invocation_terminal_in_open_anchor<P>(
        &self,
        invocation_id: &golem_common::model::InvocationId,
        park: impl Fn() -> P,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        let invocation_id = *invocation_id;
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            let read = self
                .run_owned_cursor_op(move |state| async move {
                    state
                        .with_tx(async |tx| tx.try_get_open_anchor_terminal(invocation_id).await)
                        .await
                })
                .await?;
            if read.is_some() {
                return Ok(read);
            }
            if self.at_replay_tail() {
                return Ok(None);
            }
            let _parked = park();
            self.wait_while_blocked(BlockedOn::CursorProgress, progress)
                .await?;
        }
    }

    /// Closes the open invocation anchor `anchor` of `invocation_id` and returns where the
    /// invocation settled.
    ///
    /// The caller calls it after every guest task of the primary Store exited. At
    /// [`SettledAt::Recorded`] the recorded settlement ends the invocation. At
    /// [`SettledAt::ReplayTail`] the caller then calls
    /// `release_retained_starts_at_live_invocation_end` and appends the `AgentInvocationSettled`.
    ///
    /// The walk to the invocation's `AgentInvocationSettled`, found by invocation id even when
    /// the cursor already skipped it as a hint, tolerates live-only abandoned durable-call
    /// records (see [`AbandonedStarts`]): the never-claimed `Start`s of this invocation recorded
    /// before the settlement, and their terminals, are validated and dropped. The `Start`s of
    /// other invocations stay retained for their owners. An entry of an entity body that can still
    /// consume it makes the walk wait for that body. Any other entry before the settlement is a
    /// divergence, including a reserved delivery marker or a guest stream observation, which only
    /// an exited guest task could consume. At [`SettledAt::ReplayTail`] the retained `Start`s are
    /// kept. Either way the wallet replay of the invocation finishes and the anchor is closed.
    pub(crate) async fn close_invocation_anchor(
        &self,
        anchor: OplogIndex,
        invocation_id: &golem_common::model::InvocationId,
    ) -> Result<SettledAt, WorkerExecutorError> {
        let invocation_id = *invocation_id;
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            let bodies = self.cursor.reconstruction_claims.subscribe_bodies();
            let settled = self
                .run_owned_cursor_op(move |state| async move {
                    state
                        .with_tx(async |tx| tx.settle_open_anchor(anchor, invocation_id).await)
                        .await
                })
                .await?;
            if let Some(settled) = settled {
                return Ok(settled);
            }
            self.wait_while_blocked(BlockedOn::ActiveBody(bodies), progress)
                .await?;
        }
    }
}
