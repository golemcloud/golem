use super::*;

#[derive(Debug, Clone)]
pub(crate) enum RequestClaimIdentity {
    Exact(HostRequest),
    EntityInvocation(EntityInvocationRequestIdentity),
    ToolInvocation(Box<ToolInvocationClaimIdentity>),
}

/// Typed descriptor of the recorded `Start` entry a concurrent-replay claim is looking for.
/// Every identity-based claim variant — top-level call, owned call, durable scope, dynamic
/// "any call", with or without request-payload matching — is a variant of this descriptor driven
/// through the single core [`CursorTx::claim_start`]; each variant carries exactly the identity
/// the write side records in the `Start` entry for that kind of claim, so no invalid combination
/// (e.g. a scope claim without a function name) is representable.
#[derive(Debug, Clone)]
pub(crate) enum StartClaim {
    /// A top-level (unowned) durable-call `Start`. "Unowned" means the caller did not open its
    /// own durable scope; the expected recorded `parent_start_index` is still the scope encoded
    /// in the durable function type when there is one (batched / transaction
    /// `Some(begin_index)`), mirroring how the write side derives it — see
    /// [`Self::expected_parent_start_index`].
    Unowned {
        function_name: HostFunctionName,
        function_type: DurableFunctionType,
        /// When `Some`, the recorded request payload must additionally match this value by
        /// value; see [`recorded_request_payload_matches`].
        matching_request: Option<RequestClaimIdentity>,
    },
    /// A durable-call `Start` owned by another durable record (`parent_start_index` points at
    /// the owning scope/call `Start`).
    Owned {
        function_name: HostFunctionName,
        function_type: DurableFunctionType,
        parent_start_index: OplogIndex,
        /// When `Some`, the recorded request payload must additionally match this value by
        /// value; see [`recorded_request_payload_matches`].
        matching_request: Option<RequestClaimIdentity>,
    },
    /// Atomically claims either the accepted generic entity Start or the deterministic
    /// predispatch-rejection Start for one tool invocation attempt, whichever occurs first in the
    /// owner oplog.
    OwnedToolInvocation {
        accepted_function_name: HostFunctionName,
        rejected_function_name: HostFunctionName,
        function_type: DurableFunctionType,
        parent_start_index: OplogIndex,
        matching_request: RequestClaimIdentity,
    },
    /// A durable-*scope* `Start`: request-less and optionally owned by an entity invocation.
    /// Primary scopes remain unowned; entity scope Starts point at the entity invocation Start.
    Scope {
        function_name: HostFunctionName,
        function_type: DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
    },
    /// Any top-level durable-call `Start`, whatever its function name and durable function type
    /// (the dynamic guest-facing durability read learns the identity from the claimed entry
    /// itself). The `Start` must carry a request (durable host calls always do; a request-less
    /// `Start` is a scope `Start`) and must not be owned by another durable record.
    AnyUnownedCall,
}

impl StartClaim {
    /// See [`StartClaim::Unowned`].
    pub(crate) fn unowned(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
    ) -> Self {
        Self::Unowned {
            function_name: function_name.clone(),
            function_type: function_type.clone(),
            matching_request: None,
        }
    }

    /// [`StartClaim::Unowned`] additionally requiring the recorded request payload to match
    /// `request` by value; see [`recorded_request_payload_matches`].
    pub(crate) fn unowned_matching_request(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        request: &HostRequest,
    ) -> Self {
        Self::Unowned {
            function_name: function_name.clone(),
            function_type: function_type.clone(),
            matching_request: Some(RequestClaimIdentity::Exact(request.clone())),
        }
    }

    pub(crate) fn unowned_matching_entity_invocation(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        request: &EntityInvocationRequestIdentity,
    ) -> Self {
        Self::Unowned {
            function_name: function_name.clone(),
            function_type: function_type.clone(),
            matching_request: Some(RequestClaimIdentity::EntityInvocation(request.clone())),
        }
    }

    /// See [`StartClaim::Owned`].
    pub(crate) fn owned(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
    ) -> Self {
        Self::Owned {
            function_name: function_name.clone(),
            function_type: function_type.clone(),
            parent_start_index,
            matching_request: None,
        }
    }

    /// [`StartClaim::Owned`] additionally requiring the recorded request payload to match
    /// `request` by value; see [`recorded_request_payload_matches`].
    pub(crate) fn owned_matching_request(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
        request: &HostRequest,
    ) -> Self {
        Self::Owned {
            function_name: function_name.clone(),
            function_type: function_type.clone(),
            parent_start_index,
            matching_request: Some(RequestClaimIdentity::Exact(request.clone())),
        }
    }

    pub(crate) fn owned_matching_entity_invocation(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
        request: &EntityInvocationRequestIdentity,
    ) -> Self {
        Self::Owned {
            function_name: function_name.clone(),
            function_type: function_type.clone(),
            parent_start_index,
            matching_request: Some(RequestClaimIdentity::EntityInvocation(request.clone())),
        }
    }

    pub(crate) fn owned_tool_invocation(
        accepted_function_name: &HostFunctionName,
        rejected_function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
        request: &ToolInvocationClaimIdentity,
    ) -> Self {
        Self::OwnedToolInvocation {
            accepted_function_name: accepted_function_name.clone(),
            rejected_function_name: rejected_function_name.clone(),
            function_type: function_type.clone(),
            parent_start_index,
            matching_request: RequestClaimIdentity::ToolInvocation(Box::new(request.clone())),
        }
    }

    /// See [`StartClaim::Scope`].
    pub(crate) fn scope(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
    ) -> Self {
        Self::Scope {
            function_name: function_name.clone(),
            function_type: function_type.clone(),
            parent_start_index,
        }
    }

    /// See [`StartClaim::AnyUnownedCall`].
    pub(super) fn any_unowned_call() -> Self {
        Self::AnyUnownedCall
    }

    /// The expected recorded host function name; `None` claims any name.
    pub(super) fn expected_function_name(&self) -> Option<&HostFunctionName> {
        match self {
            Self::Unowned { function_name, .. }
            | Self::Owned { function_name, .. }
            | Self::Scope { function_name, .. } => Some(function_name),
            Self::OwnedToolInvocation { .. } | Self::AnyUnownedCall => None,
        }
    }

    pub(super) fn matches_function_name(&self, actual: &HostFunctionName) -> bool {
        match self {
            Self::OwnedToolInvocation {
                accepted_function_name,
                rejected_function_name,
                ..
            } => actual == accepted_function_name || actual == rejected_function_name,
            _ => self
                .expected_function_name()
                .is_none_or(|expected| actual == expected),
        }
    }

    /// The expected recorded durable function type; `None` claims any type.
    pub(super) fn expected_function_type(&self) -> Option<&DurableFunctionType> {
        match self {
            Self::Unowned { function_type, .. }
            | Self::Owned { function_type, .. }
            | Self::Scope { function_type, .. }
            | Self::OwnedToolInvocation { function_type, .. } => Some(function_type),
            Self::AnyUnownedCall => None,
        }
    }

    /// Whether the `Start` must carry a request payload: `true` for durable host calls, `false`
    /// for durable-scope `Start`s.
    pub(super) fn carries_request(&self) -> bool {
        match self {
            Self::Unowned { .. }
            | Self::Owned { .. }
            | Self::OwnedToolInvocation { .. }
            | Self::AnyUnownedCall => true,
            Self::Scope { .. } => false,
        }
    }

    /// The expected recorded `parent_start_index`: the explicit owner for owned claims, the
    /// scope encoded in the durable function type for unowned calls (batched / transaction
    /// `Some(begin_index)`, mirroring how the write side derives it), and `None` for scopes and
    /// dynamic claims.
    pub(super) fn expected_parent_start_index(&self) -> Option<OplogIndex> {
        match self {
            Self::Unowned { function_type, .. } => parent_start_index_of(function_type),
            Self::Owned {
                parent_start_index, ..
            } => Some(*parent_start_index),
            Self::OwnedToolInvocation {
                parent_start_index, ..
            } => Some(*parent_start_index),
            Self::Scope {
                parent_start_index, ..
            } => *parent_start_index,
            Self::AnyUnownedCall => None,
        }
    }

    /// The request payload the recorded `Start` must additionally match by value, when the
    /// claim pins one; see [`recorded_request_payload_matches`].
    pub(super) fn matching_request(&self) -> Option<&RequestClaimIdentity> {
        match self {
            Self::Unowned {
                matching_request, ..
            }
            | Self::Owned {
                matching_request, ..
            } => matching_request.as_ref(),
            Self::OwnedToolInvocation {
                matching_request, ..
            } => Some(matching_request),
            Self::Scope { .. } | Self::AnyUnownedCall => None,
        }
    }

    pub(super) fn matches_start_identity(&self, entry: &OplogEntry) -> bool {
        matches!(entry, OplogEntry::Start {
            function_name,
            invocation_id,
            observational_owner,
            request,
            durable_function_type,
            parent_start_index,
            ..
        } if self.matches_function_name(function_name)
            && self
                .expected_function_type()
                .is_none_or(|expected| durable_function_type == expected)
            && invocation_id.is_none()
            && observational_owner.is_none()
            && request.is_some() == self.carries_request()
            && *parent_start_index == self.expected_parent_start_index())
    }

    /// Human-readable description of the expected `Start`, used as the "expected" side of an
    /// `unexpected_oplog_entry` claim error. Worded per claim variant, matching exactly what each
    /// variant has always reported.
    pub(crate) fn expected_description(&self) -> String {
        match self {
            Self::AnyUnownedCall => {
                "Start { request: Some(..), parent_start_index: None }".to_string()
            }
            Self::Scope {
                function_name,
                function_type,
                parent_start_index,
            } => {
                format!(
                    "Start {{ {function_name}, {function_type:?}, request: None, parent_start_index: {parent_start_index:?} }}"
                )
            }
            Self::Unowned {
                function_name,
                function_type,
                matching_request,
            } => {
                let parent = parent_start_index_of(function_type);
                if matching_request.is_some() {
                    format!(
                        "Start {{ {function_name}, {function_type:?}, request: Some(<matching payload>), parent_start_index: {parent:?} }}"
                    )
                } else {
                    format!(
                        "Start {{ {function_name}, {function_type:?}, request: Some(..), parent_start_index: {parent:?} }}"
                    )
                }
            }
            Self::Owned {
                function_name,
                function_type,
                parent_start_index,
                matching_request,
            } => {
                if matching_request.is_some() {
                    format!(
                        "Start {{ {function_name}, {function_type:?}, request: Some(<matching payload>), parent_start_index: Some({parent_start_index}) }}"
                    )
                } else {
                    format!(
                        "Start {{ {function_name}, {function_type:?}, parent_start_index: Some({parent_start_index}) }}"
                    )
                }
            }
            Self::OwnedToolInvocation {
                accepted_function_name,
                rejected_function_name,
                function_type,
                parent_start_index,
                ..
            } => format!(
                "Start {{ {accepted_function_name} or {rejected_function_name}, {function_type:?}, request: Some(<matching tool invocation>), parent_start_index: Some({parent_start_index}) }}"
            ),
        }
    }
}

pub(crate) enum ReplayStartClaimOutcome {
    Claimed {
        handle: ReplayCallHandle,
        entry: Box<OplogEntry>,
    },
    ReplayEnded,
    DeletedRegion,
}

impl ReplayState {
    /// Runs a [`StartClaim`] as an owned cursor operation: acquire a cursor transaction, claim
    /// the described `Start` (consuming it and registering a resolver receiver atomically), and
    /// return the registered handle together with the claimed entry. Shared frame of every
    /// public claim wrapper below.
    async fn claim_start(
        &self,
        claim: StartClaim,
    ) -> Result<(ReplayCallHandle, Box<OplogEntry>), WorkerExecutorError> {
        let expected = claim.expected_description();
        match self.claim_start_or_replay_end(claim).await? {
            ReplayStartClaimOutcome::Claimed { handle, entry } => Ok((handle, entry)),
            ReplayStartClaimOutcome::ReplayEnded => {
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    expected,
                    format!(
                        "end of replay at {}; no recorded Start remains",
                        self.last_replayed_index()
                    ),
                ))
            }
            ReplayStartClaimOutcome::DeletedRegion => {
                Err(WorkerExecutorError::unexpected_oplog_entry(
                    expected,
                    "matching Start belongs to a deleted replay region".to_string(),
                ))
            }
        }
    }

    /// Claims a recorded `Start`, reports that the same cursor transaction observed replay already
    /// ended, or identifies a matching `Start` removed by a replay jump. Every other missing match
    /// while replay is active remains strict divergence.
    pub(crate) async fn claim_start_or_replay_end(
        &self,
        claim: StartClaim,
    ) -> Result<ReplayStartClaimOutcome, WorkerExecutorError> {
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();

            let owned_claim = claim.clone();
            let (claimed, blocked_on_completion_delivery, replay_ended, deleted_region) = self
                .run_owned_cursor_op(move |state| async move {
                    state
                        .with_tx(async |tx| match tx.claim_start(&owned_claim).await {
                            Ok(claimed) => {
                                Ok((claimed, tx.blocked_on_completion_delivery, false, false))
                            }
                            Err(_) if tx.cursor.is_live() => Ok((None, false, true, false)),
                            Err(_) if tx.deleted_region_contains_start(&owned_claim).await? => {
                                Ok((None, false, false, true))
                            }
                            Err(error) => Err(error),
                        })
                        .await
                })
                .await?;
            if let Some(claimed) = claimed {
                return Ok(ReplayStartClaimOutcome::Claimed {
                    handle: claimed.0,
                    entry: claimed.1,
                });
            }
            if replay_ended {
                return Ok(ReplayStartClaimOutcome::ReplayEnded);
            }
            if deleted_region {
                return Ok(ReplayStartClaimOutcome::DeletedRegion);
            }
            debug_assert!(blocked_on_completion_delivery);
            progress.await;
        }
    }

    /// Claims the next top-level (unowned) durable-call `Start` matching the expected identity
    /// (function name, durable function type, request presence) and registers a resolver receiver
    /// keyed by the `Start`'s index. See [`CursorTx::claim_start_matching`].
    ///
    /// The claim is identity-based rather than strictly positional because top-level durable calls
    /// may be issued from concurrently running host tasks (e.g. parallel P3 HTTP sends), whose
    /// `Start` entries land in the oplog in network/scheduling order that replay does not
    /// reproduce. The head fast path keeps the serial case positional and free; otherwise the
    /// first not-yet-claimed matching `Start` ahead of the cursor is scan-ahead-claimed.
    /// `Start`s sharing the same identity are claimed in oplog order, preserving the deterministic
    /// per-task initiation order.
    ///
    /// `End` entries carry no function identity, so identity matching must happen here, at claim
    /// time. The request payload is not decoded: `function_name` already pins the request type
    /// (and the `Req` associated type has no `TryFrom<HostRequest>` to decode it generically); the
    /// response is fully type-checked on the `End` side during replay.
    pub async fn claim_concurrent_start(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let (handle, _) = self
            .claim_start(StartClaim::unowned(
                expected_function_name,
                expected_function_type,
            ))
            .await?;
        Ok(handle)
    }

    /// Positionally claims the next `Start` entry for a durable call **without** validating its
    /// function name or durable function type, registering a resolver receiver keyed by the
    /// `Start`'s index and returning the claimed entry's identity for the caller to inspect.
    ///
    /// This is the dynamic counterpart of [`Self::claim_concurrent_start`]: it is used by callers
    /// that learn the call identity from the claimed entry itself rather than knowing it up front —
    /// notably the guest-facing `golem::durability` read, which returns the persisted invocation's
    /// function name to the guest and therefore has no expected name to validate against.
    ///
    /// The `Start` consume and the resolver registration happen atomically under the cursor lock;
    /// see [`CursorTx::claim_start_matching`].
    pub async fn claim_any_concurrent_start(
        &self,
    ) -> Result<ClaimedConcurrentStart, WorkerExecutorError> {
        let (handle, entry) = self.claim_start(StartClaim::any_unowned_call()).await?;
        let OplogEntry::Start {
            timestamp,
            function_name,
            durable_function_type,
            ..
        } = *entry
        else {
            unreachable!("claim_start only claims Start entries");
        };
        Ok(ClaimedConcurrentStart {
            handle,
            function_name,
            durable_function_type,
            timestamp,
        })
    }

    /// Claims the `Start` of a durable call owned by another durable record (its
    /// `parent_start_index`) by identity instead of position, scan-ahead-claiming a matching
    /// `Start` ahead of the cursor when concurrent host tasks interleaved the live append order.
    /// Matching `Start`s that share the same full identity (several chunks under one parent) are
    /// claimed in oplog order, preserving the deterministic per-parent chain order.
    pub async fn claim_owned_concurrent_start(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let (handle, _) = self
            .claim_start(StartClaim::owned(
                expected_function_name,
                expected_function_type,
                parent_start_index,
            ))
            .await?;
        Ok(handle)
    }

    /// Claims the next durable-scope `Start` matching exactly the expected name and registers a
    /// resolver awaiter for it, so its matching scope `End` is consumed through
    /// [`Self::await_resolution_outcome`] rather than a positional read. Returns the scope's
    /// begin index and the handle its `end_function` / transaction-terminal awaits.
    ///
    /// The expected name must be exactly the name the live path recorded, including any
    /// discriminator suffix (a caller-supplied suffix that makes a concurrent scope claim-safe,
    /// e.g. `<scope:batched-write:req:HASH>`). There is no plain-name fallback: a discriminated
    /// claim must never match a plain scope `Start` (P3 deploys on a clean database, so every
    /// replayed oplog was recorded with the same naming scheme).
    ///
    /// Folding scope `End`s into the resolver is what lets a scope `End` be auto-drained by any
    /// cursor driver (so a positional reader never steals a concurrently-replaying sibling call's
    /// terminal, and the scope close never steals a sibling's), at the cost of nothing on the serial
    /// path: when the scope `End` is the entry at the cursor head, awaiting it resolves immediately.
    ///
    /// Every durable scope `Start` consumed during replay leaves a registered awaiter, so its
    /// `End` is always a resolver-routed *awaited terminal* and never an orphan that a parked
    /// awaiter behind it could sleep on until `switch_to_live`. The only un-drained terminals the
    /// cursor may leave at its head are then the dedicated-positional-consumer pairs (manual
    /// durability, `GolemApiFork`).
    pub async fn claim_scope_start(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
    ) -> Result<(OplogIndex, ReplayCallHandle), WorkerExecutorError> {
        let (handle, _) = self
            .claim_start(StartClaim::scope(
                expected_function_name,
                expected_function_type,
                parent_start_index,
            ))
            .await?;
        Ok((handle.start_idx(), handle))
    }

    /// Claims the next top-level (unowned) durable-call `Start` whose identity **and recorded
    /// request payload** match. Payload matching is what disambiguates concurrent durable calls
    /// that share the same function name and durable function type but were issued with different
    /// requests (e.g. parallel P3 HTTP sends): their `Start` entries land in the oplog in
    /// scheduling order, so identity alone would pair a replayed call with another call's record —
    /// and consequently deliver another call's recorded response. Calls with equal requests are
    /// still claimed in oplog order among the matches.
    ///
    /// `expected_request` must be the [`HostRequest`] value the live path would have persisted in
    /// the `Start` entry; see [`recorded_request_payload_matches`] for the value-based comparison.
    #[cfg(test)]
    pub async fn claim_concurrent_start_matching_request(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        expected_request: &HostRequest,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        Ok(self
            .claim_concurrent_start_matching_request_with_identity(
                expected_function_name,
                expected_function_type,
                expected_request,
            )
            .await?
            .handle)
    }

    /// Claims by exact identity and also returns the recorded Start metadata.
    #[cfg(test)]
    pub async fn claim_concurrent_start_matching_request_with_identity(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        expected_request: &HostRequest,
    ) -> Result<ClaimedConcurrentStart, WorkerExecutorError> {
        let (handle, entry) = self
            .claim_start(StartClaim::unowned_matching_request(
                expected_function_name,
                expected_function_type,
                expected_request,
            ))
            .await?;
        let OplogEntry::Start {
            timestamp,
            function_name,
            durable_function_type,
            ..
        } = *entry
        else {
            unreachable!("claim_start only claims Start entries");
        };
        Ok(ClaimedConcurrentStart {
            handle,
            function_name,
            durable_function_type,
            timestamp,
        })
    }

    /// Claims a custom durable invocation root and marks it as a logical subtree. Descendant
    /// custom invocations recorded under this owner are drained while the root resolution is
    /// awaited, because replay returns the root's persisted result without executing its body.
    pub async fn claim_custom_start_matching_invocation_id(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        expected_parent_start_index: Option<OplogIndex>,
        expected_invocation_id: uuid::Uuid,
        expected_request: &HostRequest,
    ) -> Result<ClaimedConcurrentStart, WorkerExecutorError> {
        let (handle, entry) = loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();

            let expected_function_name = expected_function_name.clone();
            let expected_function_type = expected_function_type.clone();
            let expected_request = expected_request.clone();
            let (claimed, blocked_on_completion_delivery) = self
                .run_owned_cursor_op(move |state| async move {
                    state
                        .with_tx(async |tx| {
                            if tx
                                .st
                                .claimed_custom_invocation_ids
                                .contains(&expected_invocation_id)
                            {
                            return Err(WorkerExecutorError::unexpected_oplog_entry(
                                format!(
                                    "unused custom durable invocation id {expected_invocation_id}"
                                ),
                                "custom durable invocation IDs are single-use".to_string(),
                            ));
                        }

                        let replay_target = tx.cursor.replay_target();
                        let exact = tx
                            .cursor
                            .scan_oplog(
                                OplogIndex::INITIAL,
                                replay_target.next(),
                                &tx.st.skipped_regions,
                                tx.st
                                    .skipped_regions
                                    .find_next_deleted_region(OplogIndex::INITIAL),
                                OplogIndex::NONE,
                                |entry, _begin_idx, index: &Option<OplogIndex>| {
                                    index.is_some_and(|idx| idx <= replay_target)
                                        && matches!(entry, OplogEntry::Start {
                                            invocation_id: Some(invocation_id),
                                            observational_owner: None,
                                            ..
                                        } if *invocation_id == expected_invocation_id)
                                },
                                |_, _, _| true,
                                None,
                                |_, idx, index: &mut Option<OplogIndex>| *index = Some(idx),
                            )
                            .await;

                        let result = match exact {
                            OplogEntryLookupResult::Found {
                                index: candidate_index,
                                entry: candidate,
                                ..
                            } => {
                                let duplicate = tx
                                    .cursor
                                    .scan_oplog(
                                        candidate_index.next(),
                                        replay_target.next(),
                                        &tx.st.skipped_regions,
                                        tx.st
                                            .skipped_regions
                                            .find_next_deleted_region(candidate_index.next()),
                                        OplogIndex::NONE,
                                        |entry, _begin_idx, index: &Option<OplogIndex>| {
                                            index.is_some_and(|idx| idx <= replay_target)
                                                && matches!(entry, OplogEntry::Start {
                                                    invocation_id: Some(invocation_id),
                                                    observational_owner: None,
                                                    ..
                                                } if *invocation_id == expected_invocation_id)
                                        },
                                        |_, _, _| true,
                                        None,
                                        |_, idx, index: &mut Option<OplogIndex>| {
                                            *index = Some(idx)
                                        },
                                    )
                                    .await;
                                if let OplogEntryLookupResult::Found {
                                    index: duplicate_index,
                                    ..
                                } = duplicate
                                {
                                    return Err(WorkerExecutorError::runtime(format!(
                                        "custom durable invocation ID {expected_invocation_id} is reused by Starts {candidate_index} and {duplicate_index}"
                                    )));
                                }
                                if candidate_index <= tx.cursor.last_replayed_index()
                                    || tx.st.claimed_starts.contains(&candidate_index)
                                {
                                    return Err(WorkerExecutorError::runtime(format!(
                                        "custom durable invocation ID {expected_invocation_id} at Start {candidate_index} was already consumed or claimed"
                                    )));
                                }

                                let OplogEntry::Start {
                                    function_name,
                                    request,
                                    durable_function_type,
                                    parent_start_index,
                                    ..
                                } = candidate.as_ref()
                                else {
                                    unreachable!(
                                        "the exact custom candidate scan only accepts Start entries"
                                    );
                                };
                                if function_name != &expected_function_name
                                    || durable_function_type != &expected_function_type
                                    || *parent_start_index != expected_parent_start_index
                                {
                                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                                        format!(
                                            "custom durable Start at {candidate_index} matching function {expected_function_name}, type {expected_function_type:?}, parent {expected_parent_start_index:?}, and invocation id {expected_invocation_id}"
                                        ),
                                        format!("{candidate:?}"),
                                    ));
                                }
                                let Some(recorded_request) = request else {
                                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                                        format!(
                                            "custom durable Start at {candidate_index} with a request"
                                        ),
                                        format!("{candidate:?}"),
                                    ));
                                };
                                let request_matches = recorded_request_payload_matches(
                                    tx.cursor.oplog.as_ref(),
                                    recorded_request,
                                    &RequestClaimIdentity::Exact(expected_request.clone()),
                                )
                                .await
                                .map_err(|err| {
                                    WorkerExecutorError::runtime(format!(
                                        "failed to load custom durable request payload at Start {candidate_index}: {err}"
                                    ))
                                })?;
                                if !request_matches {
                                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                                        format!(
                                            "custom durable Start at {candidate_index} with the current request payload"
                                        ),
                                        "recorded request payload differs".to_string(),
                                    ));
                                }

                                tx.claim_start_matching(
                                    |entry| matches!(entry, OplogEntry::Start {
                                        invocation_id: Some(invocation_id),
                                        observational_owner: None,
                                        ..
                                    } if *invocation_id == expected_invocation_id),
                                    || {
                                        format!(
                                            "custom durable Start {{ invocation_id: {expected_invocation_id} }}"
                                        )
                                    },
                                )
                                .await?
                            }
                            OplogEntryLookupResult::NotFound { .. } => {
                                return Err(WorkerExecutorError::unexpected_oplog_entry(
                                    format!(
                                        "custom durable Start {{ invocation_id: {expected_invocation_id} }}"
                                    ),
                                    "no Start with the required custom invocation ID before the replay target"
                                        .to_string(),
                                ));
                            }
                        };
                        if let Some(result) = &result {
                            tx.st
                                .claimed_custom_invocation_ids
                                .insert(expected_invocation_id);
                            let root = result.0.start_idx();
                            tx.register_custom_subtree_root(root);
                        }
                        Ok((result, tx.blocked_on_completion_delivery))
                    })
                    .await
            })
            .await?;
            if let Some(claimed) = claimed {
                break claimed;
            }
            debug_assert!(blocked_on_completion_delivery);
            progress.await;
        };
        let OplogEntry::Start {
            timestamp,
            function_name,
            durable_function_type,
            ..
        } = *entry
        else {
            unreachable!("claim_start only claims Start entries");
        };
        Ok(ClaimedConcurrentStart {
            handle,
            function_name,
            durable_function_type,
            timestamp,
        })
    }
}

/// The `parent_start_index` a durable call's `Start` entry is recorded with when the caller does
/// not open its own durable scope: the scope explicitly encoded in the durable function type
/// (batched / transaction `Some(begin_index)`), or `None` for top-level calls. This mirrors the
/// derivation on the write side (`persist_durable_function_invocation` and the accessor start
/// path), so identity-based claims can reproduce the recorded value.
pub(super) fn parent_start_index_of(function_type: &DurableFunctionType) -> Option<OplogIndex> {
    match function_type {
        DurableFunctionType::WriteRemoteBatched(Some(idx))
        | DurableFunctionType::WriteRemoteTransaction(Some(idx)) => Some(*idx),
        _ => None,
    }
}

/// Whether a recorded `Start` request payload equals the expected request *value*. The comparison
/// must be by value, never by serialized bytes: payload types can contain `HashMap`s (e.g. the
/// header map of a P3 HTTP request head), whose serialization order depends on the process-random
/// hasher seed, so bytes recorded before a restart do not reproduce. Uncached external payloads
/// are downloaded before comparison; falling back to oplog order could pair concurrent calls with
/// different requests and deliver the wrong recorded response.
pub(super) async fn recorded_request_payload_matches(
    oplog: &dyn Oplog,
    recorded: &OplogPayload<HostRequest>,
    expected: &RequestClaimIdentity,
) -> Result<bool, String> {
    match recorded {
        OplogPayload::Inline(value) => request_claim_identity_matches(value, expected),
        OplogPayload::SerializedInline {
            cached: Some(cached),
            ..
        }
        | OplogPayload::External {
            cached: Some(cached),
            ..
        } => request_claim_identity_matches(cached, expected),
        OplogPayload::SerializedInline {
            bytes,
            cached: None,
        } => golem_common::serialization::deserialize::<HostRequest>(bytes)
            .map_err(|err| format!("failed to deserialize inline request payload: {err}"))
            .and_then(|value| request_claim_identity_matches(&value, expected)),
        OplogPayload::External { cached: None, .. } => oplog
            .download_payload(recorded.clone())
            .await
            .and_then(|value| request_claim_identity_matches(&value, expected)),
    }
}

fn request_claim_identity_matches(
    value: &HostRequest,
    expected: &RequestClaimIdentity,
) -> Result<bool, String> {
    match expected {
        RequestClaimIdentity::Exact(expected) => Ok(value == expected),
        RequestClaimIdentity::EntityInvocation(expected) => {
            let HostRequest::EntityInvocation(request) = value else {
                return Ok(false);
            };
            let metadata = desert_rust::deserialize::<EntityInvocationRequest>(&request.metadata)
                .map_err(|error| {
                format!("failed to decode entity invocation request metadata: {error}")
            })?;
            Ok(expected.matches(&metadata, &request.input))
        }
        RequestClaimIdentity::ToolInvocation(expected) => match value {
            HostRequest::EntityInvocation(request) => {
                let Some(expected) = &expected.accepted else {
                    return Ok(false);
                };
                let metadata =
                    desert_rust::deserialize::<EntityInvocationRequest>(&request.metadata)
                        .map_err(|error| {
                            format!("failed to decode entity invocation request metadata: {error}")
                        })?;
                Ok(expected.matches(&metadata, &request.input))
            }
            HostRequest::GolemToolInvocationRejected(request) => Ok(request.tool_name
                == expected.rejected.tool_name.as_str()
                && request.command_path == expected.rejected.command_path
                && request.input == expected.rejected.input
                && request.input_decode_failure == expected.rejected.input_decode_failure
                && request.has_stdin == expected.rejected.has_stdin
                && request.has_stdout == expected.rejected.has_stdout
                && request.call_mode == expected.rejected.call_mode),
            _ => Ok(false),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::entity::{
        EntityCallMode, ToolInputDecodeFailure, ToolInvocationRejectedIdentity,
    };
    use golem_common::model::oplog::HostRequestGolemToolInvocationRejected;
    use golem_common::model::oplog::payload::types::SerializableToolRpcError;
    use golem_common::model::tool::ToolName;
    use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
    use test_r::test;

    fn input(value: &str) -> TypedSchemaValue {
        TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::string()),
            SchemaValue::String(value.to_string()),
        )
    }

    #[test]
    fn tool_rejection_claim_ignores_selected_error_but_matches_logical_attempt() {
        let tool_name = ToolName::try_from("grep").unwrap();
        let expected =
            RequestClaimIdentity::ToolInvocation(Box::new(ToolInvocationClaimIdentity {
                accepted: None,
                rejected: ToolInvocationRejectedIdentity {
                    tool_name: tool_name.clone(),
                    command_path: vec!["search".to_string()],
                    input: Some(input("needle")),
                    input_decode_failure: None,
                    has_stdin: true,
                    has_stdout: false,
                    call_mode: EntityCallMode::Asynchronous,
                },
            }));
        let request =
            HostRequest::GolemToolInvocationRejected(HostRequestGolemToolInvocationRejected {
                tool_name: tool_name.into_inner(),
                command_path: vec!["search".to_string()],
                input: Some(input("needle")),
                input_decode_failure: None,
                has_stdin: true,
                has_stdout: false,
                call_mode: EntityCallMode::Asynchronous,
                error: SerializableToolRpcError::Denied("recorded decision".to_string()),
            });

        assert!(request_claim_identity_matches(&request, &expected).unwrap());

        let HostRequest::GolemToolInvocationRejected(mut mismatched) = request else {
            unreachable!();
        };
        mismatched.has_stdout = true;
        assert!(
            !request_claim_identity_matches(
                &HostRequest::GolemToolInvocationRejected(mismatched),
                &expected,
            )
            .unwrap()
        );
    }

    #[test]
    fn malformed_tool_rejection_claim_distinguishes_decode_failure_class() {
        let expected =
            RequestClaimIdentity::ToolInvocation(Box::new(ToolInvocationClaimIdentity {
                accepted: None,
                rejected: ToolInvocationRejectedIdentity {
                    tool_name: ToolName::try_from("grep").unwrap(),
                    command_path: Vec::new(),
                    input: None,
                    input_decode_failure: Some(ToolInputDecodeFailure::InvalidSchemaGraph),
                    has_stdin: false,
                    has_stdout: false,
                    call_mode: EntityCallMode::Synchronous,
                },
            }));
        let request =
            HostRequest::GolemToolInvocationRejected(HostRequestGolemToolInvocationRejected {
                tool_name: "grep".to_string(),
                command_path: Vec::new(),
                input: None,
                input_decode_failure: Some(ToolInputDecodeFailure::InvalidSchemaValue),
                has_stdin: false,
                has_stdout: false,
                call_mode: EntityCallMode::Synchronous,
                error: SerializableToolRpcError::RemoteInternalError(
                    "selected error is not claim identity".to_string(),
                ),
            });

        assert!(!request_claim_identity_matches(&request, &expected).unwrap());
    }
}
