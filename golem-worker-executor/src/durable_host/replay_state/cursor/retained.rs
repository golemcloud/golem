use super::*;

/// Whether the replay-visible terminal of the call started at `start_index` is a `Cancelled`
/// without a partial result: the guest dropped the call before it delivered anything. The scan
/// reads the oplog up to the replay target and skips deleted regions; nothing is consumed.
pub(super) async fn cancelled_before_delivery(
    cursor: &ReplayCursor,
    state: &CursorState,
    start_index: OplogIndex,
) -> Result<bool, UnreadableOplog> {
    let replay_target = cursor.replay_target();
    let mut next = start_index.next();
    while next <= replay_target {
        let available = u64::from(replay_target) - u64::from(next) + 1;
        let entries = cursor
            .oplog
            .read_exact(next, CHUNK_SIZE.min(available))
            .await?;
        let last_read = *entries.last_key_value().unwrap().0;
        if let Some((_, terminal)) = entries.into_iter().find(|(index, entry)| {
            *index <= replay_target
                && !state.skipped_regions.is_in_deleted_region(*index)
                && terminal_start_index(entry) == Some(start_index)
        }) {
            return Ok(matches!(
                terminal,
                OplogEntry::Cancelled { partial: None, .. }
            ));
        }
        next = last_read.next();
    }
    Ok(false)
}

pub(super) fn custom_subtree_entry_is_drainable(state: &CursorState, entry: &OplogEntry) -> bool {
    let custom_root = |member| {
        state
            .custom_subtrees
            .iter()
            .find_map(|(root, members)| members.contains(&member).then_some(*root))
    };
    match entry {
        OplogEntry::Start {
            observational_owner,
            parent_start_index,
            ..
        } => observational_owner
            .and_then(custom_root)
            .or_else(|| parent_start_index.and_then(custom_root))
            .is_some(),
        entry => terminal_start_index(entry)
            .and_then(custom_root)
            .is_some_and(|root| terminal_start_index(entry) != Some(root)),
    }
}

impl CursorTx<'_> {
    pub(in crate::durable_host::replay_state) fn retained_claims(&mut self) -> RetainedClaims<'_> {
        RetainedClaims {
            cursor: self.cursor,
            st: &mut self.st,
            resolved_retained_claim: &mut self.resolved_retained_claim,
            notify_progress: &mut self.notify_progress,
        }
    }

    /// Marks a claimed `Start` as a replay-inert subtree root: a custom invocation `Start`, or the
    /// `Start` of a call that the guest cancelled before it delivered anything, whose host-internal
    /// calls replay never makes again. Descendants the cursor already committed past and retained
    /// (a physical call recorded under this owner, a nested call under such a call) are records
    /// of the root's recorded outcome: they are moved out of the retained map into the subtree so
    /// no later claim can take them, exactly as the cursor consumes not-yet-reached descendants
    /// when it gets to them.
    pub(in crate::durable_host::replay_state) fn register_replay_inert_subtree_root(
        &mut self,
        root: OplogIndex,
    ) {
        self.retained_claims()
            .register_replay_inert_subtree_root(root);
    }
}

impl RetainedClaims<'_> {
    pub(in crate::durable_host::replay_state) fn register_replay_inert_subtree_root(
        &mut self,
        root: OplogIndex,
    ) {
        let mut members = HashSet::from([root]);
        let adopted: Vec<OplogIndex> = self
            .st
            .retained_starts
            .iter()
            .filter_map(|(idx, retained)| {
                let OplogEntry::Start {
                    observational_owner,
                    parent_start_index,
                    ..
                } = &retained.entry
                else {
                    unreachable!("only Start entries are retained");
                };
                // Retained entries are visited in oplog order and a parent always precedes its
                // children, so one pass adopts whole retained chains.
                let member = observational_owner.is_some_and(|owner| members.contains(&owner))
                    || parent_start_index.is_some_and(|parent| members.contains(&parent));
                member.then(|| {
                    members.insert(*idx);
                    *idx
                })
            })
            .collect();
        for idx in adopted {
            let retained = self
                .st
                .retained_starts
                .remove(&idx)
                .expect("adopted retained Starts are taken from the retained map");
            // Adoption consumes the record on the root's behalf, so it publishes the position and
            // the replay events the cursor withheld while the Start was retained, like an
            // in-position consumption would.
            self.publish_claimed_position(
                idx,
                retained.terminal.map(|(terminal_idx, _)| terminal_idx),
            );
            self.publish_deferred_events(retained.deferred_events);
            if let Some(wallet) = &self.cursor.wallet {
                wallet.finish_replayed_entity(idx);
            }
        }
        self.publish_retained_count();
        self.st.custom_subtrees.insert(root, members);
        *self.notify_progress = true;
    }
}

impl CursorTx<'_> {
    /// Whether a `Start` nobody has claimed yet may be committed past and retained for its owner
    /// instead of being handed to (or parking) the reader that reached it. Only replay-inert
    /// `Start`s qualify; see [`AbandonedStarts::can_drain`] for the excluded commit effects.
    pub(in crate::durable_host::replay_state) fn is_retainable_start(entry: &OplogEntry) -> bool {
        matches!(entry, OplogEntry::Start { function_name, .. } if AbandonedStarts::can_drain(function_name))
    }

    fn publish_retained_count(&mut self) {
        self.retained_claims().publish_retained_count();
    }

    pub(super) fn retain_start(&mut self, idx: OplogIndex, entry: OplogEntry) {
        if matches!(
            &entry,
            OplogEntry::Start {
                function_name: HostFunctionName::GolemEntityInvoke
                    | HostFunctionName::GolemToolInvocationRejected,
                ..
            }
        ) && let Some(wallet) = &self.cursor.wallet
        {
            wallet.register_replayed_entity(idx);
        }
        let enclosing_entities = start_parent(&entry)
            .and_then(|parent| self.st.entity_start_routes.get(&parent))
            .cloned()
            .unwrap_or_default();
        self.st.retained_starts.insert(
            idx,
            RetainedStart {
                entry,
                enclosing_entities,
                terminal: None,
                deferred_events: Vec::new(),
            },
        );
        self.publish_retained_count();
    }

    /// Publishes the replay events a retained `Start` withheld, once that `Start` is consumed on
    /// its owner's behalf (claimed, adopted into a custom subtree, folded into the abandoned
    /// tolerance, or released at a live invocation end).
    pub(super) fn publish_deferred_events(
        &mut self,
        deferred_events: Vec<(OplogIndex, ReplayEvent)>,
    ) {
        debug_assert!(
            self.retaining_for.is_none(),
            "deferred replay events are published outside a retained commit"
        );
        self.retained_claims()
            .publish_deferred_events(deferred_events);
    }

    /// Attaches a committed terminal to the retained `Start` it closes. Returns `false` when the
    /// terminal does not belong to a retained `Start`.
    pub(super) fn attach_retained_terminal(
        &mut self,
        terminal_idx: OplogIndex,
        entry: &OplogEntry,
    ) -> Result<bool, WorkerExecutorError> {
        let Some(start_index) = terminal_start_index(entry) else {
            return Ok(false);
        };
        let Some(retained) = self.st.retained_starts.get_mut(&start_index) else {
            return Ok(false);
        };
        if let Some((prior_idx, prior)) = &retained.terminal {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "at most one End/Cancelled per durable call Start",
                format!(
                    "{} at {terminal_idx} closing Start {start_index} already closed by {} at {prior_idx}",
                    terminal_kind(entry),
                    terminal_kind(prior)
                ),
            ));
        }
        retained.terminal = Some((terminal_idx, entry.clone()));
        Ok(true)
    }

    /// Moves every retained `Start` into the invocation-boundary abandoned-record tolerance. The
    /// replayed guest has produced its invocation result, so no owner can claim them anymore.
    pub(super) fn fold_retained_into_abandoned(
        &mut self,
        abandoned: &mut AbandonedStarts,
    ) -> Result<(), WorkerExecutorError> {
        let retained = std::mem::take(&mut self.st.retained_starts);
        self.publish_retained_count();
        for (idx, retained) in retained {
            let OplogEntry::Start {
                function_name,
                parent_start_index,
                ..
            } = &retained.entry
            else {
                unreachable!("only Start entries are retained");
            };
            abandoned.record_start(idx, function_name.clone(), *parent_start_index);
            if let Some(wallet) = &self.cursor.wallet {
                wallet.finish_replayed_entity(idx);
            }
            if let Some((terminal_idx, terminal)) = &retained.terminal {
                abandoned.record_terminal(idx, *terminal_idx, terminal_kind(terminal))?;
            }
            // The boundary reader consumes abandoned records in position, so the hints trailing
            // them take effect as they would have without the retention.
            self.publish_deferred_events(retained.deferred_events);
        }
        Ok(())
    }

    /// Claims the first retained `Start` (in oplog order, outside deleted regions) whose identity
    /// `matches_identity` accepts and — when the claim pins a request payload — whose recorded
    /// request equals `expected_request` (the payload may need blob I/O, hence the async body).
    /// A claimed `Start` is registered with the resolver like any other claim; when its terminal
    /// was already committed the registration is resolved immediately, otherwise the caller
    /// awaits the terminal through the ordinary cursor drain (or gets `Incomplete` at the replay
    /// tail).
    pub(in crate::durable_host::replay_state) async fn claim_retained_start(
        &mut self,
        matches_identity: &(dyn Fn(&OplogEntry) -> bool + Sync),
    ) -> Result<Option<(ReplayCallHandle, Box<OplogEntry>)>, WorkerExecutorError> {
        self.retained_claims()
            .claim_retained_start(matches_identity)
            .await
    }
}

impl RetainedClaims<'_> {
    fn publish_retained_count(&mut self) {
        self.st.retained_entity_counts.clear();
        for retained in self.st.retained_starts.values() {
            for entity in &retained.enclosing_entities {
                *self.st.retained_entity_counts.entry(*entity).or_default() += 1;
            }
        }
        self.cursor.unclaimed_retained_starts.publish(
            self.st
                .retained_starts
                .values()
                .map(|retained| &retained.entry),
        );
    }

    pub(super) fn publish_claimed_position(
        &self,
        start_idx: OplogIndex,
        terminal_idx: Option<OplogIndex>,
    ) {
        let claimed = terminal_idx.map_or(start_idx, |terminal_idx| terminal_idx.max(start_idx));
        let position = &self.cursor.position.last_replayed_non_hint_index;
        if claimed > position.get() {
            position.set(claimed);
        }
    }

    pub(super) fn publish_deferred_events(
        &mut self,
        deferred_events: Vec<(OplogIndex, ReplayEvent)>,
    ) {
        self.cursor
            .pending_replay_events
            .lock()
            .unwrap()
            .extend(deferred_events.into_iter().map(|(_, event)| event));
    }

    /// Returns the guest-delivery marker for the durable call starting at `start_index`, if one
    /// exists and lies outside any deleted region. A marker in a reverted/jumped-away region
    /// belongs to an abandoned timeline; without a replacement marker, a still-visible accessor
    /// `End` uses replay-tail delivery.
    ///
    /// The construction scan is bounded by the initial replay target, and target growth rescans
    /// only the newly visible range (see [`ReplayState::set_replay_target`]). A returned marker
    /// therefore never encodes knowledge beyond the target. Debug targets still validate delivery
    /// boundaries, whereas fork/revert remove the future marker and recover the retained prefix.
    fn completion_marker(&self, start_index: OplogIndex) -> Option<CompletionMarker> {
        let marker = *self
            .cursor
            .completion_markers
            .lock()
            .unwrap()
            .get(&start_index)?;
        (!self.st.skipped_regions.is_in_deleted_region(marker.index())).then_some(marker)
    }

    pub(in crate::durable_host::replay_state) async fn claim_start(
        &mut self,
        claim: &StartClaim,
    ) -> Result<Option<(ReplayCallHandle, Box<OplogEntry>)>, WorkerExecutorError> {
        let Some((mut handle, entry)) = self
            .claim_retained_start(&|entry| claim.matches_start_identity(entry))
            .await?
        else {
            return Ok(None);
        };
        if claim.is_reconstruction_claim() {
            let reconstruction = self.st.concurrent_resolver.register_reconstruction(
                handle.start_idx(),
                self.cursor.wallet.as_ref().map(Arc::downgrade),
            );
            handle.attach_historical_reconstruction(reconstruction);
        }
        if claim.is_custom_invocation()
            || (claim.makes_host_internal_calls()
                && cancelled_before_delivery(self.cursor, self.st, handle.start_idx()).await?)
        {
            self.register_replay_inert_subtree_root(handle.start_idx());
        }
        Ok(Some((handle, entry)))
    }

    pub(super) async fn claim_retained_start(
        &mut self,
        matches_identity: &(dyn Fn(&OplogEntry) -> bool + Sync),
    ) -> Result<Option<(ReplayCallHandle, Box<OplogEntry>)>, WorkerExecutorError> {
        let candidates: Vec<OplogIndex> = self
            .st
            .retained_starts
            .keys()
            .copied()
            .filter(|idx| !self.st.skipped_regions.is_in_deleted_region(*idx))
            .collect();
        for idx in candidates {
            let accepted = {
                let retained = self
                    .st
                    .retained_starts
                    .get(&idx)
                    .expect("retained Start candidates are taken from the retained map");
                matches_identity(&retained.entry)
            };
            if !accepted {
                continue;
            }
            let retained = self
                .st
                .retained_starts
                .remove(&idx)
                .expect("retained Start candidates are taken from the retained map");
            self.publish_retained_count();
            self.publish_claimed_position(
                idx,
                retained
                    .terminal
                    .as_ref()
                    .map(|(terminal_idx, _)| *terminal_idx),
            );
            // The owner consumes the `Start` (and its attached terminal) now, so the hints the
            // cursor skipped past on its behalf become observable to it, in oplog order, before
            // it reads the replayed completion.
            self.publish_deferred_events(retained.deferred_events);
            let handle = match retained.terminal {
                Some((terminal_idx, terminal)) => {
                    let receiver = self
                        .st
                        .concurrent_resolver
                        .register(idx, start_parent(&retained.entry));
                    let resolution = self.terminal_resolution(idx, terminal_idx, &terminal);
                    let resolved = self.st.concurrent_resolver.resolve_if_pending(
                        idx,
                        terminal_idx,
                        resolution,
                    );
                    debug_assert!(resolved, "a freshly registered retained claim must resolve");
                    *self.resolved_retained_claim = Some(idx);
                    *self.notify_progress = true;
                    ReplayCallHandle::new(idx, receiver)
                }
                None => {
                    self.register_claimed_start(idx, start_parent(&retained.entry))
                        .await?
                }
            };
            return Ok(Some((handle, Box::new(retained.entry))));
        }
        Ok(None)
    }
}

impl CursorTx<'_> {
    /// Drops retained `Start`s (and detaches retained terminals and deferred replay events) that
    /// `deleted` now hides.
    pub(super) fn prune_retained_starts(&mut self, mut deleted: impl FnMut(OplogIndex) -> bool) {
        if let Some(wallet) = &self.cursor.wallet {
            wallet.prune_entity_prefixes(&mut deleted);
        }
        self.st.entity_start_routes.retain(|start, entities| {
            entities.retain(|entity| !deleted(*entity));
            !deleted(*start) && !entities.is_empty()
        });
        let mut members: HashMap<OplogIndex, HashSet<OplogIndex>> = HashMap::new();
        for (start, entities) in &self.st.entity_start_routes {
            for entity in entities {
                members.entry(*entity).or_default().insert(*start);
            }
        }
        self.st.entity_route_members = members;
        self.st.retained_starts.retain(|idx, _| {
            if deleted(*idx) {
                if let Some(wallet) = &self.cursor.wallet {
                    wallet.finish_replayed_entity(*idx);
                }
                false
            } else {
                true
            }
        });
        for retained in self.st.retained_starts.values_mut() {
            retained
                .enclosing_entities
                .retain(|entity| !deleted(*entity));
            if retained
                .terminal
                .as_ref()
                .is_some_and(|(terminal_idx, _)| deleted(*terminal_idx))
            {
                retained.terminal = None;
            }
            retained
                .deferred_events
                .retain(|(event_idx, _)| !deleted(*event_idx));
        }
        self.publish_retained_count();
    }

    /// The [`Resolution`] a durable call starting at `start_index` receives from its committed
    /// terminal `entry` at `terminal_idx`, honouring the call's guest-delivery marker (if any).
    pub(super) fn terminal_resolution(
        &mut self,
        start_index: OplogIndex,
        terminal_idx: OplogIndex,
        entry: &OplogEntry,
    ) -> Resolution {
        self.retained_claims()
            .terminal_resolution(start_index, terminal_idx, entry)
    }
}

impl RetainedClaims<'_> {
    pub(super) fn terminal_resolution(
        &self,
        start_index: OplogIndex,
        terminal_idx: OplogIndex,
        entry: &OplogEntry,
    ) -> Resolution {
        let marker = self.completion_marker(start_index);
        match entry {
            OplogEntry::End {
                response,
                forced_commit,
                ..
            } => match marker {
                Some(CompletionMarker::Discarded(marker_idx)) => {
                    Resolution::CompletedButDiscarded {
                        end_idx: terminal_idx,
                        marker_idx,
                        response: response.clone(),
                    }
                }
                Some(CompletionMarker::Delivered(marker_idx)) => Resolution::Completed {
                    end_idx: terminal_idx,
                    response: response.clone(),
                    forced_commit: *forced_commit,
                    delivery_marker: Some(marker_idx),
                },
                None => Resolution::Completed {
                    end_idx: terminal_idx,
                    response: response.clone(),
                    forced_commit: *forced_commit,
                    delivery_marker: None,
                },
            },
            OplogEntry::Cancelled { partial, .. } => Resolution::Cancelled {
                cancelled_idx: terminal_idx,
                partial: partial.clone(),
            },
            _ => unreachable!("terminal_resolution is only called with End/Cancelled entries"),
        }
    }
}

impl CursorTx<'_> {
    /// Registers a resolver for a claimed `Start`. Successful calls carrying a delivery marker are
    /// resolved from a non-consuming lookahead to their `End`: this lets the host continuation run
    /// while the positional cursor remains available for durable operations recorded before the
    /// completion became guest-visible. The matching terminal remains resolver-owned and is
    /// auto-drained when those intervening operations advance the cursor to it.
    pub(super) async fn register_claimed_start(
        &mut self,
        start_idx: OplogIndex,
        parent_start_index: Option<OplogIndex>,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        self.retained_claims()
            .register_claimed_start(start_idx, parent_start_index)
            .await
    }
}

impl RetainedClaims<'_> {
    pub(super) async fn register_claimed_start(
        &mut self,
        start_idx: OplogIndex,
        parent_start_index: Option<OplogIndex>,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let prefetched = if let Some(marker) = self.completion_marker(start_idx) {
            let marker_idx = marker.index();
            let scan = self
                .cursor
                .scan_oplog(
                    start_idx.next(),
                    marker_idx,
                    &self.st.skipped_regions,
                    self.st
                        .skipped_regions
                        .find_next_deleted_region(start_idx.next()),
                    OplogIndex::NONE,
                    |entry, _, index| {
                        terminal_start_index(entry) == Some(start_idx) && index < marker_idx
                    },
                    |_, _, _| true,
                )
                .await?;
            match scan {
                OplogEntryLookupResult::Found { index, entry, .. } => match *entry {
                    OplogEntry::End {
                        response,
                        forced_commit,
                        ..
                    } => Some((
                        index,
                        match marker {
                            CompletionMarker::Delivered(marker_idx) => Resolution::Completed {
                                end_idx: index,
                                response,
                                delivery_marker: Some(marker_idx),
                                forced_commit,
                            },
                            CompletionMarker::Discarded(marker_idx) => {
                                Resolution::CompletedButDiscarded {
                                    end_idx: index,
                                    marker_idx,
                                    response,
                                }
                            }
                        },
                    )),
                    OplogEntry::Cancelled { .. } => {
                        return Err(WorkerExecutorError::runtime(format!(
                            "corrupt oplog: successful-completion marker at {marker_idx} references cancelled durable call Start at {start_idx}"
                        )));
                    }
                    _ => unreachable!("the prefetch scan accepts only matching terminals"),
                },
                OplogEntryLookupResult::NotFound { .. } => {
                    return Err(WorkerExecutorError::runtime(format!(
                        "corrupt oplog: successful-completion marker at {marker_idx} references durable call Start at {start_idx} without a matching End before the marker"
                    )));
                }
            }
        } else {
            None
        };

        let receiver = self
            .st
            .concurrent_resolver
            .register(start_idx, parent_start_index);
        if let Some((terminal_idx, resolution)) = prefetched {
            self.st
                .concurrent_resolver
                .resolve_prefetched(start_idx, terminal_idx, resolution);
        }
        *self.notify_progress = true;
        Ok(ReplayCallHandle::new(start_idx, receiver))
    }
}

impl ReplayState {
    /// Whether the cursor has committed past durable-call `Start`s nobody has claimed yet (see
    /// [`CursorState::retained_starts`]). Does not take the cursor lock: read by durable-call
    /// admission from Store-holding host calls, which must not queue on it.
    pub(crate) fn has_unclaimed_retained_starts(&self) -> bool {
        self.cursor.unclaimed_retained_starts.any()
    }

    /// Whether one of the unclaimed retained `Start`s records a call charged to `quota`. A live
    /// Store about to issue a call of that quota class may still be its owner (replay is
    /// deterministic, so the owner issues its recorded calls in order), whereas a call charged to
    /// any other quota is known fresh. Same locking rule as
    /// [`Self::has_unclaimed_retained_starts`].
    pub(crate) fn retains_unclaimed_start_charged_to(&self, quota: QuotaClass) -> bool {
        self.cursor.unclaimed_retained_starts.any_charged_to(quota)
    }

    /// Releases every still-retained `Start` when a live primary invocation finishes. The
    /// recorded prefix those `Start`s came from belongs to the invocation that just produced its
    /// result, so no replayed owner can claim them anymore; keeping them would make every later
    /// durable call take the replay admission path (see `WorkerState::durable_call_is_live`) for
    /// nothing. Mirrors the invocation-boundary tolerance of [`AbandonedStarts`] for a replayed
    /// finish, including its structural rule: every released `Start` must already be closed by a
    /// recorded `End`/`Cancelled`. An unclosed retained `Start` is a divergence error rather than
    /// a warning, because appending `AgentInvocationFinished` after it would make the next
    /// reconstruction's boundary read reject the history deterministically.
    ///
    /// Nothing is released while an entity body is still being reconstructed: a retained `Start`
    /// may be a descendant of that body whose owner has not been admitted yet.
    pub(crate) async fn release_retained_starts_at_live_invocation_end(
        &self,
    ) -> Result<(), WorkerExecutorError> {
        if !self.has_unclaimed_retained_starts() {
            return Ok(());
        }
        let owned_agent_id = self.cursor.owned_agent_id.clone();
        self.run_owned_cursor_op(move |state| async move {
            let mut st = state.cursor.state.lock().await;
            if !state
                .cursor
                .reconstruction_claims
                .subscribe_bodies()
                .borrow()
                .is_empty()
            {
                tracing::debug!(
                    retained = st.retained_starts.len(),
                    "Keeping unclaimed retained Starts across a live invocation end while an entity body is still reconstructing"
                );
                return Ok(());
            }
            // A retained `Start` normally has its terminal attached when the cursor commits past
            // it. Replay may still have clamped the head to the target without reading every
            // entry in between, so an unattached terminal is looked up in the recorded prefix
            // before the `Start` is declared unclosed.
            let mut unclosed: HashSet<OplogIndex> = st
                .retained_starts
                .iter()
                .filter(|(_, retained)| retained.terminal.is_none())
                .map(|(idx, _)| *idx)
                .collect();
            let replay_target = state.cursor.replay_target();
            if let Some(first_unclosed) = unclosed.iter().min().copied() {
                let mut next = first_unclosed.next();
                while next <= replay_target && !unclosed.is_empty() {
                    let available = u64::from(replay_target) - u64::from(next) + 1;
                    let entries = state
                        .cursor
                        .read_oplog(next, CHUNK_SIZE.min(available))
                        .await?;
                    let Some(&(last_read, _)) = entries.last() else {
                        break;
                    };
                    for (index, entry) in entries {
                        if index > replay_target {
                            break;
                        }
                        if !st.skipped_regions.is_in_deleted_region(index)
                            && let Some(start_index) = terminal_start_index(&entry)
                        {
                            unclosed.remove(&start_index);
                        }
                    }
                    next = last_read.next();
                }
            }
            if !unclosed.is_empty() {
                let mut unclosed: Vec<_> = unclosed.into_iter().collect();
                unclosed.sort();
                let unclosed: Vec<_> = unclosed
                    .into_iter()
                    .map(|idx| {
                        let OplogEntry::Start { function_name, .. } = &st.retained_starts[&idx].entry
                        else {
                            unreachable!("only Start entries are retained");
                        };
                        format!("{idx} ({function_name:?})")
                    })
                    .collect();
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "an End/Cancelled closing every unclaimed retained Start before the live invocation finished",
                    format!(
                        "live invocation of {owned_agent_id} finished with unclosed unclaimed retained Start(s) at {}",
                        unclosed.join(", ")
                    ),
                ));
            }
            let released = std::mem::take(&mut st.retained_starts);
            st.retained_entity_counts.clear();
            state.cursor.unclaimed_retained_starts.clear();
            if released.is_empty() {
                return Ok(());
            }
            if let Some(wallet) = &state.cursor.wallet {
                for index in released.keys() {
                    wallet.finish_replayed_entity(*index);
                }
            }
            {
                let mut pending = state.cursor.pending_replay_events.lock().unwrap();
                for retained in released.values() {
                    pending.extend(
                        retained
                            .deferred_events
                            .iter()
                            .map(|(_, event)| event.clone()),
                    );
                }
            }
            let summary: Vec<_> = released
                .iter()
                .map(|(idx, retained)| {
                    let OplogEntry::Start {
                        function_name,
                        parent_start_index,
                        ..
                    } = &retained.entry
                    else {
                        unreachable!("only Start entries are retained");
                    };
                    format!(
                        "{idx}: {function_name:?} (parent: {parent_start_index:?}, terminal: {:?})",
                        retained
                            .terminal
                            .as_ref()
                            .map(|(terminal_idx, terminal)| (terminal_kind(terminal), *terminal_idx))
                    )
                })
                .collect();
            warn!(
                "replay of {owned_agent_id} left {} unclaimed durable-call Start(s) when the live \
                 invocation finished — recorded progress no replayed owner claimed: [{}]",
                released.len(),
                summary.join("; ")
            );
            Ok(())
        })
        .await
    }
}
