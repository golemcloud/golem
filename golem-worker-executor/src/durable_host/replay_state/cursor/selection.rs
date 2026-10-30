use super::*;

pub(in crate::durable_host::replay_state) fn scope_entry_owner(
    index: OplogIndex,
    entry: &OplogEntry,
) -> Option<OplogIndex> {
    if let Some(owner) = entry.entity_parent_start_index() {
        return Some(owner);
    }

    match entry {
        OplogEntry::Start { .. } => Some(index),
        OplogEntry::ToolAttachmentRead { observation, .. } => match observation.observation {
            golem_common::model::durable_stream::StreamConsumerObservation::Guest {
                parent_start_index,
            } => parent_start_index,
            golem_common::model::durable_stream::StreamConsumerObservation::Host => None,
        },
        OplogEntry::End { start_index, .. }
        | OplogEntry::Cancelled { start_index, .. }
        | OplogEntry::CompletionDiscarded { start_index, .. }
        | OplogEntry::CompletionDelivered { start_index, .. } => Some(*start_index),
        OplogEntry::HostStreamFrame {
            parent_start_index, ..
        }
        | OplogEntry::Log {
            parent_start_index: Some(parent_start_index),
            ..
        } => Some(*parent_start_index),
        OplogEntry::Error { retry_from, .. } => Some(*retry_from),
        OplogEntry::BeginRemoteTransaction {
            original_begin_index: begin,
            ..
        } => Some(*begin),
        OplogEntry::PreCommitRemoteTransaction { begin_index, .. }
        | OplogEntry::PreRollbackRemoteTransaction { begin_index, .. }
        | OplogEntry::CommittedRemoteTransaction { begin_index, .. }
        | OplogEntry::RolledBackRemoteTransaction { begin_index, .. } => Some(*begin_index),
        OplogEntry::Create { .. }
        | OplogEntry::AgentInvocationStarted { .. }
        | OplogEntry::AgentInvocationFinished { .. }
        | OplogEntry::Suspend { .. }
        | OplogEntry::RecoverySucceeded { .. }
        | OplogEntry::Jump { .. }
        | OplogEntry::Interrupted { .. }
        | OplogEntry::Exited { .. }
        | OplogEntry::PendingAgentInvocation { .. }
        | OplogEntry::PendingUpdate { .. }
        | OplogEntry::SuccessfulUpdate { .. }
        | OplogEntry::FailedUpdate { .. }
        | OplogEntry::GrowMemory { .. }
        | OplogEntry::CreateResource { .. }
        | OplogEntry::DropResource { .. }
        | OplogEntry::Log {
            parent_start_index: None,
            ..
        }
        | OplogEntry::Restart { .. }
        | OplogEntry::Resumed { .. }
        | OplogEntry::ActivatePlugin { .. }
        | OplogEntry::DeactivatePlugin { .. }
        | OplogEntry::Revert { .. }
        | OplogEntry::CancelPendingInvocation { .. }
        | OplogEntry::Snapshot { .. }
        | OplogEntry::SnapshotConfirmed { .. }
        | OplogEntry::SpanClosed { .. }
        | OplogEntry::OplogProcessorCheckpoint { .. }
        | OplogEntry::SetRetryPolicy { .. }
        | OplogEntry::RemoveRetryPolicy { .. }
        | OplogEntry::CardEventQueued { .. }
        | OplogEntry::CardInstalled { .. }
        | OplogEntry::CardInstallFailed { .. }
        | OplogEntry::CardRevoked { .. }
        | OplogEntry::CardExpired { .. }
        | OplogEntry::CardDerived { .. }
        | OplogEntry::CardTransferStarted { .. }
        | OplogEntry::CardTransferred { .. }
        | OplogEntry::CardRevokedCascade { .. }
        | OplogEntry::CardTransferConfirmed { .. }
        | OplogEntry::ToolAttachmentControl { .. }
        | OplogEntry::StreamRegistered { .. }
        | OplogEntry::StreamItems { .. }
        | OplogEntry::StreamEnd { .. }
        | OplogEntry::StreamCancel { .. }
        | OplogEntry::StreamSession { .. } => None,
    }
}

/// Human-readable kind of a durable-call terminal entry, for diagnostics.
pub(in crate::durable_host::replay_state) fn terminal_kind(entry: &OplogEntry) -> &'static str {
    match entry {
        OplogEntry::End { .. } => "End",
        OplogEntry::Cancelled { .. } => "Cancelled",
        _ => unreachable!("terminal_kind is only called with End/Cancelled entries"),
    }
}

/// The `start_index` of the durable call `entry` terminates, when `entry` is a durable-call
/// terminal (`End` / `Cancelled`); `None` for every other entry kind.
pub(in crate::durable_host::replay_state) fn terminal_start_index(
    entry: &OplogEntry,
) -> Option<OplogIndex> {
    match entry {
        OplogEntry::End { start_index, .. } | OplogEntry::Cancelled { start_index, .. } => {
            Some(*start_index)
        }
        OplogEntry::Create { .. }
        | OplogEntry::Start { .. }
        | OplogEntry::CompletionDiscarded { .. }
        | OplogEntry::CompletionDelivered { .. }
        | OplogEntry::AgentInvocationStarted { .. }
        | OplogEntry::AgentInvocationFinished { .. }
        | OplogEntry::Suspend { .. }
        | OplogEntry::Error { .. }
        | OplogEntry::RecoverySucceeded { .. }
        | OplogEntry::Jump { .. }
        | OplogEntry::Interrupted { .. }
        | OplogEntry::Exited { .. }
        | OplogEntry::PendingAgentInvocation { .. }
        | OplogEntry::PendingUpdate { .. }
        | OplogEntry::SuccessfulUpdate { .. }
        | OplogEntry::FailedUpdate { .. }
        | OplogEntry::GrowMemory { .. }
        | OplogEntry::CreateResource { .. }
        | OplogEntry::DropResource { .. }
        | OplogEntry::Log { .. }
        | OplogEntry::Restart { .. }
        | OplogEntry::Resumed { .. }
        | OplogEntry::ActivatePlugin { .. }
        | OplogEntry::DeactivatePlugin { .. }
        | OplogEntry::Revert { .. }
        | OplogEntry::CancelPendingInvocation { .. }
        | OplogEntry::BeginRemoteTransaction { .. }
        | OplogEntry::PreCommitRemoteTransaction { .. }
        | OplogEntry::PreRollbackRemoteTransaction { .. }
        | OplogEntry::CommittedRemoteTransaction { .. }
        | OplogEntry::RolledBackRemoteTransaction { .. }
        | OplogEntry::Snapshot { .. }
        | OplogEntry::SnapshotConfirmed { .. }
        | OplogEntry::SpanClosed { .. }
        | OplogEntry::OplogProcessorCheckpoint { .. }
        | OplogEntry::SetRetryPolicy { .. }
        | OplogEntry::RemoveRetryPolicy { .. }
        | OplogEntry::CardEventQueued { .. }
        | OplogEntry::CardInstalled { .. }
        | OplogEntry::CardInstallFailed { .. }
        | OplogEntry::CardRevoked { .. }
        | OplogEntry::CardExpired { .. }
        | OplogEntry::CardDerived { .. }
        | OplogEntry::CardTransferStarted { .. }
        | OplogEntry::CardTransferred { .. }
        | OplogEntry::CardRevokedCascade { .. }
        | OplogEntry::CardTransferConfirmed { .. }
        | OplogEntry::HostStreamFrame { .. }
        | OplogEntry::ToolAttachmentControl { .. }
        | OplogEntry::ToolAttachmentRead { .. }
        | OplogEntry::StreamRegistered { .. }
        | OplogEntry::StreamItems { .. }
        | OplogEntry::StreamEnd { .. }
        | OplogEntry::StreamCancel { .. }
        | OplogEntry::StreamSession { .. } => None,
    }
}

/// The invocation id of a `Start` entry.
pub(super) fn start_invocation_id(entry: &OplogEntry) -> Option<DurableInvocationId> {
    match entry {
        OplogEntry::Start { invocation_id, .. } => *invocation_id,
        _ => None,
    }
}

/// Whether `entry` is the `Start` of an entity invocation, accepted or rejected.
pub(super) fn is_entity_invocation_start(entry: &OplogEntry) -> bool {
    matches!(
        entry,
        OplogEntry::Start {
            function_name: HostFunctionName::GolemEntityInvoke
                | HostFunctionName::GolemToolInvocationRejected,
            ..
        }
    )
}

/// Whether `entry` is the `Start` of a custom durable invocation: a numbered call `Start` of a
/// guest-named function. Scopes share the naming but record no request.
pub(super) fn is_custom_invocation_start(entry: &OplogEntry) -> bool {
    matches!(
        entry,
        OplogEntry::Start {
            function_name: HostFunctionName::Custom(_),
            invocation_id: Some(_),
            request: Some(_),
            ..
        }
    )
}

/// Decides the owner of `entry` when it is at the cursor head. Positional reads use it to decide
/// whether they may park on the head, and `Start` claims use it to decide whether a missing
/// `Start` waits for an active entity body.
pub(in crate::durable_host::replay_state) fn head_owner(entry: &OplogEntry) -> HeadOwner {
    match entry {
        OplogEntry::Start {
            parent_start_index: Some(parent),
            ..
        } => HeadOwner::ParentStart(*parent),
        _ => match entry.entity_attribution() {
            EntityAttribution::Unattributed => HeadOwner::Unattributed,
            EntityAttribution::Agent => HeadOwner::Agent,
            EntityAttribution::EntityBody(owner) => HeadOwner::EntityBody(owner),
        },
    }
}

/// Whether the head path drains `entry` instead of stopping at it: a hint, a `Start` claimed
/// ahead of the cursor, or a terminal a registered awaiter owns. A scan that passes the cursor
/// head uses it to find the entry the head path would stop at.
pub(in crate::durable_host::replay_state) fn is_drained_at_head(
    entry: &OplogEntry,
    facts: HeadDrainFacts,
) -> bool {
    !facts.guest_observation
        && (is_auto_skippable_hint(entry) || facts.claimed_ahead || facts.awaited_terminal)
}

/// The `parent_start_index` of a `Start` entry.
pub(in crate::durable_host::replay_state) fn start_parent(
    entry: &OplogEntry,
) -> Option<OplogIndex> {
    match entry {
        OplogEntry::Start {
            parent_start_index, ..
        } => *parent_start_index,
        _ => None,
    }
}

/// The nearest active entity body that encloses the record at `index`: `index` itself when it is
/// an active body, and otherwise the nearest active body that encloses its parent. `parent_of`
/// gives the parent recorded by a `Start` the cursor still holds. A parent always precedes its
/// child, so the walk only moves to earlier indices and ends.
pub(in crate::durable_host::replay_state) fn nearest_active_body(
    mut index: OplogIndex,
    active_bodies: &HashSet<OplogIndex>,
    parent_of: impl Fn(OplogIndex) -> Option<OplogIndex>,
) -> Option<OplogIndex> {
    loop {
        if active_bodies.contains(&index) {
            return Some(index);
        }
        index = parent_of(index).filter(|parent| *parent < index)?;
    }
}

/// Decides whether a `Start` claim that found no match while the cursor replays waits instead of
/// reporting the missing `Start`, and returns the entity body it waits for.
///
/// `head_body` is the nearest active body that encloses the entry at the cursor head, and
/// `claiming_body` the nearest active body that encloses the claim's parent. The claim waits for
/// `head_body`: that body, or the supervisor that drains its recorded terminal, can still consume
/// the head, and the claim's `Start` may then turn out to be recorded after it or not at all. The
/// claim does not wait when no active body encloses the head, when that body issued the claim
/// (only that body can consume its own entry, so it would wait for itself), or when the claim's
/// parent was appended live after the replay target. An entity nested in the head's body is
/// another Store, so its claim waits like any other.
pub(in crate::durable_host::replay_state) fn missing_start_waits_for(
    head_body: Option<OplogIndex>,
    claiming_body: Option<OplogIndex>,
    claim_is_live: bool,
) -> Option<OplogIndex> {
    let body = head_body?;
    (!claim_is_live && claiming_body != Some(body)).then_some(body)
}

impl CursorTx<'_> {
    /// An entity Store can append its first positional record before the asynchronously enqueued
    /// invocation `Start` is assigned its oplog index. The record carries that reserved future
    /// index as its owner, so leave it for the body while the identity claim scans ahead to the
    /// `Start`. This is accepted only when the referenced index is replay-visible and names an
    /// entity invocation claim; arbitrary forward attribution must not turn corrupt history into
    /// a wait.
    pub(super) async fn future_entity_start_can_be_claimed(
        &self,
        head_idx: OplogIndex,
        owner: OplogIndex,
    ) -> Result<bool, UnreadableOplog> {
        if owner <= head_idx
            || owner > self.cursor.replay_target()
            || self.st.skipped_regions.is_in_deleted_region(owner)
        {
            return Ok(false);
        }
        Ok(matches!(
            self.cursor.oplog.read(owner).await?,
            OplogEntry::Start {
                function_name: HostFunctionName::GolemEntityInvoke
                    | HostFunctionName::GolemToolInvocationRejected,
                ..
            }
        ))
    }

    /// The owner named by the entry buffered at the cursor head, when the buffer holds the next
    /// entry after the cursor and the head path stops at it (the claim's head fast path leaves it
    /// there).
    pub(in crate::durable_host::replay_state) fn buffered_head_owner(&self) -> Option<HeadOwner> {
        let next = self.cursor.last_replayed_index().next();
        self.st
            .replay_buffer
            .front()
            .filter(|(index, entry)| {
                *index == next && !is_drained_at_head(entry, self.head_drain_facts(*index, entry))
            })
            .map(|(_, entry)| head_owner(entry))
    }

    /// Collects the facts [`is_drained_at_head`] decides on for `entry` at `index`.
    pub(in crate::durable_host::replay_state) fn head_drain_facts(
        &self,
        index: OplogIndex,
        entry: &OplogEntry,
    ) -> HeadDrainFacts {
        HeadDrainFacts {
            claimed_ahead: self.st.claimed_starts.contains(&index),
            awaited_terminal: self.is_awaited_terminal(index, entry),
            guest_observation: self.is_stream_observation(index),
        }
    }

    /// Gathers the facts [`missing_start_waits_for`] decides on for a `Start` claim that found no
    /// match, and returns the subscription to wait on when the rule says the claim waits. `head`
    /// is the owner named by the entry at the cursor head, and the claim was issued from the
    /// Store whose call chain starts at `claim_parent`.
    ///
    /// Every fact comes from state the cursor already holds: the parents recorded by the claimed
    /// `Start`s that are still awaiting their terminal, the retained `Start`s and the active-body
    /// set. No oplog entry is read.
    pub(in crate::durable_host::replay_state) fn active_body_owning_head(
        &self,
        head: Option<HeadOwner>,
        claim_parent: Option<OplogIndex>,
    ) -> Option<tokio::sync::watch::Receiver<HashSet<OplogIndex>>> {
        if self.cursor.is_live() {
            return None;
        }
        let head_parent = match head? {
            HeadOwner::EntityBody(owner) | HeadOwner::ParentStart(owner) => owner,
            HeadOwner::Unattributed | HeadOwner::Agent => return None,
        };
        let mut bodies = self.cursor.reconstruction_claims.subscribe_bodies();
        let active_bodies = bodies.borrow_and_update().clone();
        if active_bodies.is_empty() {
            return None;
        }
        let parent_of = |index: OplogIndex| {
            self.st
                .concurrent_resolver
                .claimed_parent(index)
                .or_else(|| {
                    self.st
                        .retained_starts
                        .get(&index)
                        .and_then(|retained| start_parent(&retained.entry))
                })
        };
        let head_body = nearest_active_body(head_parent, &active_bodies, parent_of);
        let claim_is_live = claim_parent.is_some_and(|parent| parent > self.cursor.replay_target());
        let claiming_body =
            claim_parent.and_then(|parent| nearest_active_body(parent, &active_bodies, parent_of));
        missing_start_waits_for(head_body, claiming_body, claim_is_live).map(|_| bodies)
    }

    /// Decides a `Start` claim that found no matching `Start` before the replay target while the
    /// cursor replays, after [`Self::active_body_owning_head`] found no active body that owns the
    /// head.
    ///
    /// A crash prefix can hold the `Start` of one concurrent guest call and not the `Start` of
    /// another: guest calls reserve their invocation ids in guest call order, but append their
    /// `Start`s in host scheduling order. On replay the call without a `Start` can reach its claim
    /// before its sibling claims the recorded one, so a sibling's unclaimed `Start` at the head is
    /// not evidence of divergence. Like every reader that must make progress, the claim neither
    /// consumes nor waits on such a `Start`: the cursor retains it for its owner's claim, and a
    /// retained `Start` that nobody claims is reported at the invocation end. Which records the
    /// claim moves past is decided by [`Self::sibling_record_at_head`], so that the decision is
    /// the same whether or not the sibling has already claimed its own records.
    ///
    /// The claim is decided against the entry the cursor stops at: the replay target ends replay,
    /// a delivery boundary, an entry of an active body, or an entry of the body of a retained
    /// entity invocation waits for its owner, and anything else is divergence. A `Start` recorded
    /// with the claim's own invocation id belongs to the claiming guest call itself, so it is
    /// divergence before the cursor moves.
    pub(in crate::durable_host::replay_state) async fn decide_missing_start(
        &mut self,
        claim: &StartClaim,
    ) -> Result<MissingStart, WorkerExecutorError> {
        loop {
            self.try_get_oplog_entry_leaving_unclaimed_starts(|_| false)
                .await?;
            if self.cursor.is_live() {
                return Ok(MissingStart::ReplayEnded);
            }
            if self.blocked_on_completion_delivery {
                return Ok(MissingStart::Blocked(BlockedOn::CursorProgress));
            }
            match self.sibling_record_at_head().await? {
                SiblingRecord::Retain(index, entry) => {
                    self.retain_start(index, entry.clone());
                    self.commit_retained_entry(index, &entry).await?;
                }
                SiblingRecord::BodyOfRetainedInvocation => {
                    return Ok(MissingStart::Blocked(BlockedOn::CursorProgress));
                }
                SiblingRecord::None => break,
            }
        }
        match self.active_body_owning_head(
            self.buffered_head_owner(),
            claim.expected_parent_start_index(),
        ) {
            Some(bodies) => Ok(MissingStart::Blocked(BlockedOn::ActiveBody(bodies))),
            None => Err(WorkerExecutorError::unexpected_oplog_entry(
                claim.expected_description(),
                "no matching Start between the replay cursor and the replay target".to_string(),
            )),
        }
    }

    /// Classifies the entry buffered at the cursor head for [`Self::decide_missing_start`], as
    /// its owner would treat it once it claimed the records it owns:
    ///
    /// - a top-level `Start` recorded under an invocation id belongs to a guest call, and is
    ///   retained;
    /// - a `Start` under a retained or claimed `Start` with the same invocation id belongs to the
    ///   same guest call (the call of a scope, the child of a host call, a tool invocation of the
    ///   import that dispatched it), and is retained;
    /// - a record in the subtree of a retained custom invocation, through retained parents and
    ///   observational owners, is retained: the custom invocation adopts its retained subtree
    ///   when it is claimed;
    /// - a record of the body of a retained entity invocation waits: that body consumes it once
    ///   its invocation is claimed;
    /// - anything else is not a sibling's record.
    async fn sibling_record_at_head(&self) -> Result<SiblingRecord, UnreadableOplog> {
        let next = self.cursor.last_replayed_index().next();
        let Some((index, entry)) = self.st.replay_buffer.front() else {
            return Ok(SiblingRecord::None);
        };
        if *index != next || self.st.claimed_starts.contains(index) {
            return Ok(SiblingRecord::None);
        }
        let retained = |index: &OplogIndex| self.st.retained_starts.get(index).map(|r| &r.entry);
        let owner = match head_owner(entry) {
            HeadOwner::EntityBody(owner) | HeadOwner::ParentStart(owner) => Some(owner),
            HeadOwner::Agent | HeadOwner::Unattributed => None,
        };
        if owner
            .and_then(|owner| retained(&owner))
            .is_some_and(is_entity_invocation_start)
        {
            return Ok(SiblingRecord::BodyOfRetainedInvocation);
        }
        if !Self::is_retainable_start(entry) {
            return Ok(SiblingRecord::None);
        }
        let OplogEntry::Start {
            invocation_id,
            parent_start_index,
            observational_owner,
            ..
        } = entry
        else {
            return Ok(SiblingRecord::None);
        };
        let retain =
            if self.under_retained_custom_invocation(*parent_start_index, *observational_owner) {
                true
            } else if observational_owner.is_some() {
                false
            } else {
                match (invocation_id, parent_start_index) {
                    (Some(_), None) => true,
                    (_, Some(parent)) => match retained(parent) {
                        Some(parent) => {
                            invocation_id.is_some() && start_invocation_id(parent) == *invocation_id
                        }
                        None => {
                            invocation_id.is_some()
                                && self.st.concurrent_resolver.has_claim(*parent)
                                && start_invocation_id(&self.cursor.oplog.read(*parent).await?)
                                    == *invocation_id
                        }
                    },
                    (None, None) => false,
                }
            };
        if retain {
            Ok(SiblingRecord::Retain(*index, entry.clone()))
        } else {
            Ok(SiblingRecord::None)
        }
    }

    /// Whether a record with `parent` and `observer` lies in the subtree of a retained custom
    /// invocation, through retained parents and observers, as the custom invocation adopts its
    /// retained subtree when it is claimed (`register_custom_subtree_root`).
    fn under_retained_custom_invocation(
        &self,
        parent: Option<OplogIndex>,
        observer: Option<OplogIndex>,
    ) -> bool {
        let mut ancestors: Vec<OplogIndex> = parent.into_iter().chain(observer).collect();
        while let Some(index) = ancestors.pop() {
            let Some(retained) = self.st.retained_starts.get(&index) else {
                continue;
            };
            if is_custom_invocation_start(&retained.entry) {
                return true;
            }
            if let OplogEntry::Start {
                parent_start_index,
                observational_owner,
                ..
            } = &retained.entry
            {
                ancestors.extend(
                    parent_start_index
                        .iter()
                        .chain(observational_owner)
                        .filter(|ancestor| **ancestor < index),
                );
            }
        }
        false
    }

    /// Whether a `Start` nobody has claimed yet carries `invocation_id`: a retained one, or one
    /// between the cursor and the replay target.
    pub(in crate::durable_host::replay_state) fn unclaimed_start_with_invocation_id(
        &self,
        invocation_id: DurableInvocationId,
    ) -> bool {
        self.st
            .retained_starts
            .values()
            .any(|retained| start_invocation_id(&retained.entry) == Some(invocation_id))
            || self
                .unclaimed_start_ahead(invocation_id, |_| true)
                .is_some()
    }

    /// Whether `entry` is an `End`/`Cancelled` whose `start_index` currently has a registered
    /// resolver awaiter (and is therefore an *awaited terminal* the cursor auto-drains to its owner
    /// rather than handing to a positional reader).
    pub(in crate::durable_host::replay_state) fn is_awaited_terminal(
        &self,
        terminal_idx: OplogIndex,
        entry: &OplogEntry,
    ) -> bool {
        terminal_start_index(entry).is_some_and(|start_index| {
            self.st
                .concurrent_resolver
                .owns_terminal(start_index, terminal_idx)
        })
    }

    /// Whether `entry` is an `End`/`Cancelled` whose `start_index` lies inside a skipped/deleted
    /// region. Such an *orphan terminal* is left behind when a jump/revert/fork/snapshot deletes
    /// the region containing a call's `Start` but not its terminal. Its `Start` can never be
    /// claimed (both the positional head consume and the scan-ahead claim jump over deleted
    /// regions), so no awaiter can ever exist for it; the cursor consumes it like a no-op instead
    /// of surfacing it to a positional reader as an unexpected entry.
    pub(in crate::durable_host::replay_state) fn is_orphan_terminal(
        &self,
        entry: &OplogEntry,
    ) -> bool {
        terminal_start_index(entry)
            .is_some_and(|start_index| self.st.skipped_regions.is_in_deleted_region(start_index))
    }

    pub(super) fn custom_subtree_root(&self, member: OplogIndex) -> Option<OplogIndex> {
        self.st
            .custom_subtrees
            .iter()
            .find_map(|(root, members)| members.contains(&member).then_some(*root))
    }

    pub(super) fn is_custom_subtree_descendant(&self, member: OplogIndex) -> bool {
        self.custom_subtree_root(member)
            .is_some_and(|root| root != member)
    }

    pub(super) fn is_stream_observation(&self, index: OplogIndex) -> bool {
        is_active_stream_observation(self.cursor, &self.st, index)
    }
}

/// What a claim finds ahead of the cursor when its `Start` is not at the head.
enum StartAhead {
    Found(OplogIndex, Box<OplogEntry>),
    /// A guest delivery boundary comes first.
    Blocked,
    Missing,
}

impl CursorState {
    /// Probes for a remaining claim without consuming delivery or observation boundaries.
    /// Admission-refusal replay must prefer every recorded occurrence of an identical call,
    /// including an occurrence whose normal claim is currently blocked on another task.
    pub(in crate::durable_host::replay_state) async fn has_unclaimed_start_before(
        &self,
        cursor: &ReplayCursor,
        claim: &StartClaim,
        terminal_index: OplogIndex,
    ) -> Result<bool, WorkerExecutorError> {
        for (index, retained) in &self.retained_starts {
            if *index < terminal_index
                && !self.skipped_regions.is_in_deleted_region(*index)
                && claim.matches_start_identity(&retained.entry)
            {
                return Ok(true);
            }
        }
        let end = terminal_index.min(cursor.replay_target().next());
        let next = cursor.last_replayed_index().next();
        if let Some(invocation_id) = claim.expected_invocation_id() {
            return Ok(next < end
                && cursor
                    .start_invocation_ids
                    .lock()
                    .unwrap()
                    .get(&invocation_id)
                    .is_some_and(|starts| {
                        starts.range(next..end).any(|(index, recorded)| {
                            !self.claimed_starts.contains(index)
                                && !self.skipped_regions.is_in_deleted_region(*index)
                                && claim.matches_recorded(recorded)
                        })
                    }));
        }
        if next < end {
            let candidate = cursor
                .scan_oplog(
                    next,
                    end,
                    &self.skipped_regions,
                    self.skipped_regions.find_next_deleted_region(next),
                    OplogIndex::NONE,
                    |entry, _, index| {
                        !self.claimed_starts.contains(&index) && claim.matches_start_identity(entry)
                    },
                    |_, _, _| true,
                )
                .await?;
            match candidate {
                OplogEntryLookupResult::Found { .. } => return Ok(true),
                OplogEntryLookupResult::NotFound { .. } => {}
            }
        }
        Ok(false)
    }
}

impl CursorTx<'_> {
    /// Looks for the first not-yet-claimed `Start` entry matching `claim`, registering a
    /// resolver receiver keyed by the `Start`'s index and returning the registered handle together
    /// with the claimed entry. Shared core of every concurrent-replay `Start` claim.
    ///
    /// Claiming by identity rather than strict position is required because accessor host calls
    /// run concurrently: `Start` entries appended by concurrently running host tasks (sibling
    /// sends' scopes, per-chunk children of overlapping consume-body scopes, top-level calls
    /// racing with them) land in the oplog in network/scheduling order, which is not reproduced by
    /// replay — only the initiation order *within one guest task / parent chain* is. The head is
    /// consumed positionally when it already matches (the serial fast path costs nothing);
    /// otherwise the **first not-yet-claimed matching `Start`** between the cursor and the replay
    /// target is claimed ahead of the cursor: its index is recorded in
    /// [`CursorState::claimed_starts`] (so the cursor auto-consumes the entry when it reaches it,
    /// like an awaited terminal, and never hands it to another reader) and the resolver awaiter is
    /// registered immediately. A claim with an invocation id finds that `Start` among the `Start`s
    /// recorded with its id ([`ReplayCursor::start_invocation_ids`]); only a claim without one scans
    /// the oplog.
    ///
    /// The `Start` consume/claim and the resolver registration happen **atomically** within this
    /// transaction (under the cursor lock). This is required for concurrent replay: if the cursor
    /// advanced past the `Start` before the awaiter was registered, this call's `End` arriving at
    /// the head in that window would not be recognised as an awaited terminal and could be wrongly
    /// consumed by a positional reader.
    ///
    /// Because a terminal always follows its `Start`, a scan-ahead-claimed call's
    /// `End`/`Cancelled` is reached only after the cursor has consumed the claimed `Start`, so
    /// terminal routing is unaffected. Matching `Start`s that share the same identity are claimed
    /// in oplog order, preserving the deterministic per-task/per-parent chain order. A replay
    /// divergence (no matching `Start` recorded at all) is reported to the caller instead of as an
    /// immediate head mismatch.
    pub(in crate::durable_host::replay_state) async fn claim_start_matching(
        &mut self,
        claim: &StartClaim,
        consume_at_head: bool,
    ) -> Result<StartClaimAttempt, WorkerExecutorError> {
        let matches_identity = |entry: &OplogEntry| claim.matches_start_identity(entry);
        // Retained `Start`s precede everything at or beyond the cursor head, so claiming them
        // first keeps same-identity claims in oplog order.
        if let Some((handle, entry)) = self.claim_retained_start(&matches_identity).await? {
            return Ok(StartClaimAttempt::Claimed(handle, entry));
        }

        // Head fast path: auto-drains awaited terminals and already-claimed `Start`s, then
        // consumes the head iff it matches this claim's identity. Another operation's unclaimed
        // `Start` at the head is left there: this claim's `Start` is either recorded further
        // ahead (scan-ahead below) or missing, and both are classified against the unchanged
        // cursor. A claim that must not consume its `Start` at the head only drains, and claims
        // through the scan below.
        let head_match = if consume_at_head {
            self.try_get_oplog_entry_leaving_unclaimed_starts(&matches_identity)
                .await?
        } else {
            self.try_get_oplog_entry_leaving_unclaimed_starts(|_| false)
                .await?
        };
        if let Some((start_idx, entry)) = head_match {
            let handle = self
                .register_claimed_start(start_idx, start_parent(&entry))
                .await?;
            return Ok(StartClaimAttempt::Claimed(handle, Box::new(entry)));
        }
        if self.blocked_on_completion_delivery {
            return Ok(StartClaimAttempt::Blocked(BlockedOn::CursorProgress));
        }

        // The head belongs to someone else: find the first not-yet-claimed matching `Start` ahead
        // of the cursor, outside deleted regions. A delivery boundary before it bounds the claim:
        // a later `Start` must not be claimed before the recorded guest handoff at that boundary.
        let ahead = match claim.expected_invocation_id() {
            Some(invocation_id) => {
                let start = self.unclaimed_start_ahead(invocation_id, |recorded| {
                    claim.matches_recorded(recorded)
                });
                let bound = start.unwrap_or_else(|| self.cursor.replay_target().next());
                if self.delivery_boundary_before(bound) {
                    StartAhead::Blocked
                } else if let Some(index) = start {
                    let entry = self.cursor.oplog.read(index).await?;
                    StartAhead::Found(index, Box::new(entry))
                } else {
                    StartAhead::Missing
                }
            }
            None => self.scan_for_start_ahead(&matches_identity).await?,
        };
        match ahead {
            StartAhead::Found(index, entry) => {
                self.st.claimed_starts.insert(index);
                let handle = self
                    .register_claimed_start(index, start_parent(&entry))
                    .await?;
                Ok(StartClaimAttempt::Claimed(handle, entry))
            }
            StartAhead::Blocked => {
                self.blocked_on_completion_delivery = true;
                Ok(StartClaimAttempt::Blocked(BlockedOn::CursorProgress))
            }
            StartAhead::Missing => Ok(StartClaimAttempt::Missing),
        }
    }

    /// Scans the oplog ahead of the cursor for the first not-yet-claimed `Start` matching
    /// `matches_identity`, for a claim without an invocation id. The scan stops at the first
    /// delivery boundary.
    async fn scan_for_start_ahead(
        &self,
        matches_identity: &(impl Fn(&OplogEntry) -> bool + Sync),
    ) -> Result<StartAhead, WorkerExecutorError> {
        let already_claimed = &self.st.claimed_starts;
        let replay_target = self.cursor.replay_target();
        let scan_result = self
            .cursor
            .scan_oplog(
                self.cursor.last_replayed_index().next(),
                replay_target.next(),
                &self.st.skipped_regions,
                self.st.next_skipped_region.clone(),
                OplogIndex::NONE,
                |entry, _begin_idx, index| {
                    index <= replay_target
                        && !already_claimed.contains(&index)
                        && (matches_identity(entry)
                            || matches!(entry, OplogEntry::CompletionDelivered { .. })
                            || self.is_stream_observation(index))
                },
                |_, _, _| true,
            )
            .await?;
        Ok(match scan_result {
            OplogEntryLookupResult::Found { index, entry, .. } => {
                if matches!(entry.as_ref(), OplogEntry::CompletionDelivered { .. })
                    || self.is_stream_observation(index)
                {
                    StartAhead::Blocked
                } else {
                    StartAhead::Found(index, entry)
                }
            }
            OplogEntryLookupResult::NotFound { .. } => StartAhead::Missing,
        })
    }

    /// The first `Start` recorded with `invocation_id` that nobody has claimed, that lies between
    /// the cursor and the replay target outside any deleted region, and that `accepts`.
    fn unclaimed_start_ahead(
        &self,
        invocation_id: DurableInvocationId,
        accepts: impl Fn(&RecordedStart) -> bool,
    ) -> Option<OplogIndex> {
        let next = self.cursor.last_replayed_index().next();
        let target = self.cursor.replay_target();
        if next > target {
            return None;
        }
        self.cursor
            .start_invocation_ids
            .lock()
            .unwrap()
            .get(&invocation_id)?
            .range(next..=target)
            .find(|(index, recorded)| {
                !self.st.claimed_starts.contains(index)
                    && !self.st.skipped_regions.is_in_deleted_region(**index)
                    && accepts(recorded)
            })
            .map(|(index, _)| *index)
    }

    /// Whether a guest delivery boundary (a `CompletionDelivered` marker or a guest stream
    /// observation) lies between the cursor and `bound`, outside any deleted region.
    fn delivery_boundary_before(&self, bound: OplogIndex) -> bool {
        let next = self.cursor.last_replayed_index().next();
        if next >= bound {
            return false;
        }
        let delivered = self
            .cursor
            .completion_markers
            .lock()
            .unwrap()
            .delivered_in(next..bound)
            .any(|index| !self.st.skipped_regions.is_in_deleted_region(index));
        if delivered {
            return true;
        }
        let observations: Vec<OplogIndex> = self
            .cursor
            .stream_observations
            .lock()
            .unwrap()
            .range(next..bound)
            .map(|(index, _)| *index)
            .filter(|index| !self.st.skipped_regions.is_in_deleted_region(*index))
            .collect();
        observations
            .into_iter()
            .any(|index| self.is_stream_observation(index))
    }

    /// Returns the latest `Start` matching this claim in a jump-deleted region. An incomplete
    /// entity Store uses this to continue live locally while sibling Stores finish replaying the
    /// surviving owner-oplog tail; the latest match ensures an earlier abandoned attempt cannot
    /// hide another attempt beyond a retained positional entry.
    pub(in crate::durable_host::replay_state) async fn deleted_region_contains_start(
        &self,
        claim: &StartClaim,
    ) -> Result<Option<OplogIndex>, WorkerExecutorError> {
        let replay_target = self.cursor.replay_target();
        if let Some(invocation_id) = claim.expected_invocation_id() {
            return Ok(self
                .cursor
                .start_invocation_ids
                .lock()
                .unwrap()
                .get(&invocation_id)
                .and_then(|starts| {
                    starts
                        .range(..=replay_target)
                        .rev()
                        .find(|(index, recorded)| {
                            self.st.skipped_regions.is_in_deleted_region(**index)
                                && claim.matches_recorded(recorded)
                        })
                        .map(|(index, _)| *index)
                }));
        }
        let regions = self
            .st
            .skipped_regions
            .regions()
            .cloned()
            .collect::<Vec<_>>();
        let mut latest_match = None;
        for region in regions {
            if region.start > replay_target {
                break;
            }
            let end = region.end.min(replay_target);
            let mut next = region.start;
            while next <= end {
                let available = u64::from(end) - u64::from(next) + 1;
                let entries = self
                    .cursor
                    .oplog
                    .read_exact(next, CHUNK_SIZE.min(available))
                    .await?;
                let last_read = *entries.last_key_value().unwrap().0;
                for (index, entry) in entries {
                    if claim.matches_start_identity(&entry) {
                        latest_match = Some(index);
                    }
                }
                next = last_read.next();
            }
        }
        Ok(latest_match)
    }

    /// Claims the `Start` entry described by `claim` through the shared claim core
    /// ([`Self::claim_start_matching`]). Returns the typed claim attempt so callers can handle a
    /// genuine missing match separately from storage failures.
    pub(in crate::durable_host::replay_state) async fn claim_start(
        &mut self,
        claim: &StartClaim,
    ) -> Result<StartClaimAttempt, WorkerExecutorError> {
        // A reconstruction claim does not consume the entity invocation `Start` at the head. It
        // claims the `Start` through the scan, and the cursor consumes it when it reaches it.
        let attempt = self
            .claim_start_matching(claim, !claim.is_reconstruction_claim())
            .await?;
        let (mut handle, entry) = match attempt {
            StartClaimAttempt::Claimed(handle, entry) => (handle, entry),
            other => return Ok(other),
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
                && cancelled_before_delivery(self.cursor, &self.st, handle.start_idx()).await?)
        {
            self.register_replay_inert_subtree_root(handle.start_idx());
        }
        // Every `Start` claim registers a resolver awaiter atomically with the consume/claim, so
        // its terminal is always a resolver-routed *awaited terminal* — never an orphan a parked
        // awaiter behind it could sleep on until `switch_to_live`. The only un-drained terminals
        // the cursor may leave at its head are the dedicated-positional-consumer pairs
        // (`GolemApiFork`). A retained `Start` whose terminal was already committed
        // (and attached to it) is the one claim resolved before this returns.
        debug_assert!(
            self.st.concurrent_resolver.has_claim(handle.start_idx())
                || self.resolved_retained_claim == Some(handle.start_idx()),
            "Start claim at {} must leave a registered awaiter",
            handle.start_idx()
        );
        Ok(StartClaimAttempt::Claimed(handle, entry))
    }
}
