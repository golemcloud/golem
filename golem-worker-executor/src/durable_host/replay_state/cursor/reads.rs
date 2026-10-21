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
    ///   for its owner's later identity-validated claim (see [`CursorState::retained_starts`]),
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
        self.try_get_oplog_entry_inner(None, None, None, UnclaimedStarts::Retain, condition)
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
        self.try_get_oplog_entry_inner(None, None, None, UnclaimedStarts::Leave, condition)
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
                None,
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
            Some(abandoned),
            UnclaimedStarts::Retain,
            condition,
        )
        .await
    }

    async fn try_get_oplog_entry_inner(
        &mut self,
        expected_delivery: Option<(Option<OplogIndex>, OplogIndex)>,
        guest_order: Option<&GuestReadOrder>,
        mut abandoned: Option<&mut AbandonedStarts>,
        unclaimed_starts: UnclaimedStarts,
        mut condition: impl FnMut(&OplogEntry) -> bool,
    ) -> Result<Option<(OplogIndex, OplogEntry)>, WorkerExecutorError> {
        self.blocked_on_completion_delivery = false;
        if self.st.skip_hints_after_delivery {
            self.skip_forward().await?;
            self.st.skip_hints_after_delivery = false;
        }
        loop {
            if self.cursor.is_live() {
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
                        .retained_starts
                        .range(..read_idx)
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
                if abandoned.is_some() && self.st.retained_starts.contains_key(start_index) {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "AgentInvocationFinished",
                        format!(
                            "CompletionDelivered at {read_idx} references unclaimed durable call Start at {start_index} — the recorded guest received this completion but replay reached the invocation boundary without claiming it"
                        ),
                    ));
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

            if self.st.claimed_starts.contains(&read_idx) {
                // A `Start` already claimed out-of-position by an identity-keyed scan-ahead claim
                // (`claim_owned_start`): its owner registered a resolver awaiter at claim time, so
                // just consume it here and keep draining — it must never be handed to a positional
                // reader.
                self.st.claimed_starts.remove(&read_idx);
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

            if abandoned.is_some()
                && let OplogEntry::Start { function_name, .. } = &entry
                && !AbandonedStarts::can_drain(function_name)
            {
                // The invocation-boundary reader reached a replay-side-effecting `Start` nobody
                // claimed. Its commit effects must not fire from the boundary drain, and the
                // replayed guest can no longer claim it (see `AbandonedStarts::can_drain`).
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "AgentInvocationFinished",
                    format!(
                        "unclaimed {function_name:?} Start at {read_idx} — a \
                         replay-side-effecting record cannot be tolerated as \
                         abandoned at the invocation boundary"
                    ),
                ));
            }

            if condition(&entry) {
                self.commit_consumed_entry(read_idx, &entry).await?;
                if let Some(abandoned) = abandoned.as_deref_mut() {
                    // The replayed guest has produced its invocation result: whatever is still
                    // retained is live-only abandoned progress nobody can claim anymore.
                    self.fold_retained_into_abandoned(abandoned)?;
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
    /// - an entity body replays once its invocation `Start` is claimed
    ///   (`reconstruction_claims`), and a `Start` the cursor retained or a scan-ahead claimed is
    ///   still going to be claimed.
    ///
    /// Anything else is an entity body nobody will reconstruct: waiting would hang replay, so the
    /// head is reported as the divergence it is. The invocation-boundary reader never waits on
    /// another Store: every entity body of the invocation has terminated before
    /// `AgentInvocationFinished`, so an unconsumed body entry there is dead history.
    pub(in crate::durable_host::replay_state) async fn check_parked_positional_read(
        &self,
        reader: PositionalReader,
    ) -> Result<(), WorkerExecutorError> {
        if self.blocked_on_completion_delivery || self.cursor.is_live() {
            return Ok(());
        }
        let Some((head_idx, head)) = self.st.replay_buffer.front() else {
            return Ok(());
        };
        let owner = match (reader, head_owner(head)) {
            // A positional reader never classifies a nested `Start` left at the head: its parent
            // may be a call of the reading Store.
            (_, HeadOwner::Unattributed | HeadOwner::ParentStart(_))
            | (PositionalReader::Ordinary, HeadOwner::Agent) => return Ok(()),
            (PositionalReader::InvocationBoundary, _) => {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "AgentInvocationFinished",
                    format!(
                        "{head:?} at {head_idx} was recorded by an entity body that replay reached the invocation boundary without reconstructing"
                    ),
                ));
            }
            (PositionalReader::Ordinary, HeadOwner::EntityBody(owner)) => owner,
        };
        let owner_can_consume = self.st.retained_starts.contains_key(&owner)
            || self.st.claimed_starts.contains(&owner)
            || self.cursor.reconstruction_claims.is_body_active(owner)
            || self
                .future_entity_start_can_be_claimed(*head_idx, owner)
                .await?;
        if owner_can_consume {
            Ok(())
        } else {
            Err(WorkerExecutorError::unexpected_oplog_entry(
                "an entry recorded by this Store",
                format!(
                    "{head:?} at {head_idx} was recorded by the entity body started at {owner}, which is neither retained, claimed nor replaying"
                ),
            ))
        }
    }
}

impl CursorState {
    async fn positional_admission_refusal(
        &self,
        cursor: &ReplayCursor,
        scope: Option<OplogIndex>,
        admission: Option<&PositionalAdmission>,
    ) -> Result<Option<SerializableToolAdmissionRefusal>, UnreadableOplog> {
        let Some((operation, request)) = admission else {
            return Ok(None);
        };
        let Some(owner) = scope else {
            return Ok(None);
        };
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
    /// The error returned when a positional oplog reader expects a next entry but the cursor is
    /// at end-of-replay.
    fn end_of_replay_error(&self) -> WorkerExecutorError {
        WorkerExecutorError::unexpected_oplog_entry(
            "next oplog entry to replay",
            format!(
                "end of replay for {} at index {}; replay target = {}",
                self.cursor.owned_agent_id,
                self.cursor.last_replayed_index(),
                self.cursor.replay_target(),
            ),
        )
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
                        if terminal_start_index(&entry).is_some_and(|start_index| {
                            st.concurrent_resolver.owns_terminal(start_index, index)
                        }) || custom_subtree_entry_is_drainable(&st, &entry)
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
                            && st.claimed_starts.contains(&index)
                        {
                            return Ok(None);
                        }
                        if owner.is_some_and(|owner| {
                            owner != root && st.concurrent_resolver.is_awaited(owner)
                        }) {
                            return Ok(None);
                        }
                        if terminal_start_index(&entry) == Some(root)
                            && st.concurrent_resolver.is_awaited(root)
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

    /// Returns the earliest retained, still unclaimed durable-call `Start` in `root`'s call tree
    /// that is not owned by a still running reconstructed entity body (see
    /// [`CursorState::retained_starts`]).
    ///
    /// The cursor steps over unclaimed `Start`s instead of parking on them, so a body that
    /// returned without re-issuing one of its recorded calls no longer stalls the cursor. Callers
    /// ask this once the body has settled: any such `Start` is the same structural divergence
    /// [`Self::await_unconsumed_scope_entry`] reports for a stalled cursor.
    pub(crate) async fn unclaimed_retained_descendant(
        &self,
        root: OplogIndex,
        active_entity_bodies: HashSet<OplogIndex>,
    ) -> Result<Option<OplogIndex>, WorkerExecutorError> {
        if !self.has_unclaimed_retained_starts() {
            return Ok(None);
        }
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let st = cursor.state.lock().await;
            let Some(last_retained) = st.retained_starts.keys().next_back().copied() else {
                return Ok(None);
            };
            if last_retained <= root {
                return Ok(None);
            }

            let mut parents = HashMap::new();
            let mut next = root.next();
            while next <= last_retained {
                let available = u64::from(last_retained) - u64::from(next) + 1;
                let entries = cursor
                    .oplog
                    .read_exact(next, CHUNK_SIZE.min(available))
                    .await?;
                let last_read = *entries.last_key_value().unwrap().0;
                for (index, entry) in entries {
                    if index > last_retained {
                        break;
                    }
                    if let OplogEntry::Start {
                        parent_start_index: Some(parent),
                        ..
                    } = &entry
                        && !st.skipped_regions.is_in_deleted_region(index)
                    {
                        parents.insert(index, *parent);
                    }
                }
                next = last_read.next();
            }

            for index in st.retained_starts.keys().copied() {
                if index <= root {
                    continue;
                }
                let mut owner = index;
                let mut owned_by_active_body = false;
                let descendant = loop {
                    let Some(parent) = parents.get(&owner) else {
                        break false;
                    };
                    if *parent == root {
                        break true;
                    }
                    if active_entity_bodies.contains(parent) {
                        owned_by_active_body = true;
                        break false;
                    }
                    owner = *parent;
                };
                if descendant && !owned_by_active_body {
                    return Ok(Some(index));
                }
            }
            Ok(None)
        })
        .await
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
            if self.is_live() {
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
                    return Err(WorkerExecutorError::runtime(failure));
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
                    None if tx.cursor.is_live() => Some(PositionalRead::ReplayEnded),
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
            if self.is_replay() {
                let (oplog_index, oplog_entry) = self.get_oplog_entry(None).await?;
                match oplog_entry {
                    OplogEntry::AgentInvocationStarted {
                        idempotency_key,
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
                            .download_payload_classified(payload)
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
                            "AgentInvocationStarted",
                            format!("{oplog_entry:?}"),
                        ));
                    }
                }
            } else {
                break Ok(None);
            }
        }
    }

    pub async fn get_oplog_entry_agent_invocation_finished(
        &self,
    ) -> Result<Option<AgentInvocationResult>, WorkerExecutorError> {
        // The walk to the finished marker tolerates live-only abandoned durable-call records
        // (see `AbandonedStarts`): the replayed guest has already produced its invocation
        // result, so any still-unclaimed `Start` (and its terminal) can never be claimed and is
        // dead partial progress of a branch the guest abandoned at a point replay did not
        // reproduce.
        let mut abandoned = AbandonedStarts::default();
        loop {
            if self.is_replay() {
                let Some((_, oplog_entry)) = self
                    .get_oplog_entry_at_invocation_boundary(&mut abandoned)
                    .await?
                else {
                    break Ok(None);
                };
                match oplog_entry {
                    OplogEntry::AgentInvocationFinished { result, .. } => {
                        std::mem::take(&mut abandoned).finish(&self.cursor.owned_agent_id)?;

                        let result: AgentInvocationResult = self
                            .cursor
                            .oplog
                            .download_payload(result)
                            .await
                            .map_err(|err| {
                                WorkerExecutorError::runtime(format!(
                                    "failed to deserialize agent invocation result payload: {err}"
                                ))
                            })?;

                        break Ok(Some(result));
                    }
                    entry if entry.is_hint() => {}
                    _ => {
                        break Err(WorkerExecutorError::unexpected_oplog_entry(
                            "AgentInvocationFinished",
                            format!("{oplog_entry:?}"),
                        ));
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
            if self.is_live() {
                // An acknowledged guest boundary defers hint skipping until this read. Those
                // hints can exhaust an incomplete invocation's tail without a Finished entry.
                return Ok(None);
            }
            progress.await;
        }
    }
}
