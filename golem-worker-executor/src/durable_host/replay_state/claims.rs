use super::*;

/// Whether a claimed `Start` records a durable call or opens a durable scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartKind {
    /// A durable call `Start`, which carries a request.
    Call,
    /// A durable scope `Start`, which carries no request.
    Scope,
}

/// Typed descriptor of the recorded `Start` entry a replay claim is looking for. Every claim is
/// driven through the single core [`CursorTx::claim_start`].
///
/// A `Start` is identified by the invocation id of the guest call that recorded it (see
/// [`DurableInvocationId`]) together with its function name and kind. One guest call can record
/// several `Start`s with the same function name, such as the retries of one request or the chunks
/// of one stream; they are recorded one after another, so they are claimed in oplog order. The
/// durable function type, the parent `Start` and the observational owner must match as well.
#[derive(Debug, Clone)]
pub(crate) struct StartClaim {
    function_name: HostFunctionName,
    /// The function of the `Start` a tool invocation records when it rejects the call before
    /// dispatch. The claim takes either kind of `Start`, whichever the call recorded.
    tool_rejection_function_name: Option<HostFunctionName>,
    function_type: DurableFunctionType,
    kind: StartKind,
    parent_start_index: Option<OplogIndex>,
    observational_owner: Option<OplogIndex>,
    invocation_id: Option<DurableInvocationId>,
    /// The `Start` opens a custom durable invocation. Replay returns its persisted result without
    /// executing its body, so the records under it are drained instead of claimed.
    custom_invocation: bool,
}

impl StartClaim {
    /// A durable call `Start` that is not owned by another record. The expected recorded
    /// `parent_start_index` is still the scope encoded in the durable function type when there is
    /// one (batched / transaction `Some(begin_index)`), mirroring how the write side derives it.
    pub(crate) fn unowned(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
    ) -> Self {
        Self {
            function_name: function_name.clone(),
            tool_rejection_function_name: None,
            function_type: function_type.clone(),
            kind: StartKind::Call,
            parent_start_index: parent_start_index_of(function_type),
            observational_owner: None,
            invocation_id: None,
            custom_invocation: false,
        }
    }

    /// A durable call `Start` owned by another durable record (`parent_start_index` points at the
    /// owning scope, call or entity invocation `Start`).
    pub(crate) fn owned(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
    ) -> Self {
        Self {
            parent_start_index: Some(parent_start_index),
            ..Self::unowned(function_name, function_type)
        }
    }

    /// The `Start` of a tool invocation under its owner: either the accepted entity invocation
    /// `Start` or the `Start` of the rejection recorded before dispatch.
    pub(crate) fn tool_invocation(
        accepted_function_name: &HostFunctionName,
        rejected_function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
    ) -> Self {
        Self {
            tool_rejection_function_name: Some(rejected_function_name.clone()),
            ..Self::owned(accepted_function_name, function_type, parent_start_index)
        }
    }

    /// A durable scope `Start`: request-less and optionally owned by an entity invocation.
    /// Primary scopes remain unowned; entity scope Starts point at the entity invocation Start.
    pub(crate) fn scope(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
    ) -> Self {
        Self {
            kind: StartKind::Scope,
            parent_start_index,
            ..Self::unowned(function_name, function_type)
        }
    }

    /// The `Start` of a custom durable invocation with `invocation_id`.
    pub(crate) fn custom_invocation(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
        invocation_id: DurableInvocationId,
    ) -> Self {
        Self {
            parent_start_index,
            invocation_id: Some(invocation_id),
            custom_invocation: true,
            ..Self::unowned(function_name, function_type)
        }
    }

    pub(crate) fn with_observational_owner(
        mut self,
        observational_owner: Option<OplogIndex>,
    ) -> Self {
        self.observational_owner = observational_owner;
        self
    }

    /// Requires the recorded `Start` to carry `invocation_id`, the invocation id of the guest call
    /// that is claiming it.
    pub(crate) fn with_invocation_id(mut self, invocation_id: Option<DurableInvocationId>) -> Self {
        self.invocation_id = invocation_id;
        self
    }

    /// The invocation id the recorded `Start` must carry.
    pub(super) fn expected_invocation_id(&self) -> Option<DurableInvocationId> {
        self.invocation_id
    }

    /// The expected recorded host function name of the accepted `Start`.
    pub(super) fn expected_function_name(&self) -> &HostFunctionName {
        &self.function_name
    }

    pub(super) fn matches_function_name(&self, actual: &HostFunctionName) -> bool {
        *actual == self.function_name || self.tool_rejection_function_name.as_ref() == Some(actual)
    }

    /// Whether the `Start` must carry a request payload: `true` for durable host calls, `false`
    /// for durable-scope `Start`s.
    pub(super) fn carries_request(&self) -> bool {
        self.kind == StartKind::Call
    }

    /// The expected recorded `parent_start_index`.
    pub(super) fn expected_parent_start_index(&self) -> Option<OplogIndex> {
        self.parent_start_index
    }

    /// Whether the claimed `Start` is an entity invocation, whose replayed body is a historical
    /// reconstruction.
    pub(super) fn is_reconstruction_claim(&self) -> bool {
        self.function_name == HostFunctionName::GolemEntityInvoke
    }

    pub(super) fn is_custom_invocation(&self) -> bool {
        self.custom_invocation
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
            && *durable_function_type == self.function_type
            && *invocation_id == self.invocation_id
            && *observational_owner == self.observational_owner
            && request.is_some() == self.carries_request()
            && *parent_start_index == self.parent_start_index)
    }

    /// Human-readable description of the expected `Start`, used as the "expected" side of an
    /// `unexpected_oplog_entry` claim error.
    pub(crate) fn expected_description(&self) -> String {
        let function_name = match &self.tool_rejection_function_name {
            Some(rejected) => format!("{} or {rejected}", self.function_name),
            None => self.function_name.to_string(),
        };
        let request = match self.kind {
            StartKind::Call => "Some(..)",
            StartKind::Scope => "None",
        };
        let description = format!(
            "Start {{ {function_name}, {:?}, request: {request}, parent_start_index: {:?} }}",
            self.function_type, self.parent_start_index
        );
        match self.invocation_id {
            Some(invocation_id) => format!("{description} with invocation id {invocation_id}"),
            None => description,
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
    /// No matching `Start` remains, but the issuing Store already continued live locally (an
    /// incomplete entity past a deleted region or a replay target that grew after it went live)
    /// while the shared cursor still replays other owners' records. The call is a new live call of
    /// that Store; its local live continuation is idempotent to re-enter.
    StoreAlreadyLive,
}

impl ReplayState {
    /// Waits until a blocked claim may run again.
    async fn wait_while_blocked(
        &self,
        blocked: BlockedOn,
        progress: std::pin::Pin<&mut tokio::sync::futures::Notified<'_>>,
    ) {
        #[cfg(test)]
        if matches!(blocked, BlockedOn::ActiveBody(_)) {
            self.cursor
                .active_body_waits
                .send_modify(|waits| *waits += 1);
        }
        blocked.wait(progress).await
    }

    /// Runs a [`StartClaim`] as an owned cursor operation: acquire a cursor transaction, claim
    /// the described `Start` (consuming it and registering a resolver receiver atomically), and
    /// return the registered handle together with the claimed entry. Shared frame of every
    /// public claim wrapper below.
    #[cfg(test)]
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
            ReplayStartClaimOutcome::StoreAlreadyLive => {
                unreachable!("strict claims are never issued on behalf of a live Store")
            }
        }
    }

    /// Claims a recorded `Start`, reports that the same cursor transaction observed replay already
    /// ended, or identifies a matching `Start` removed by a replay jump. A missing match while an
    /// active entity body owns the cursor head waits for that body or for cursor progress (see
    /// [`CursorTx::active_body_owning_head`]); every other missing match while replay is active
    /// remains strict divergence.
    pub(crate) async fn claim_start_or_replay_end(
        &self,
        claim: StartClaim,
    ) -> Result<ReplayStartClaimOutcome, WorkerExecutorError> {
        self.claim_start_for_store_observed(claim, false, |_, _| {})
            .await
    }

    /// [`Self::claim_start_or_replay_end`] issued on behalf of a Store whose own liveness is
    /// `store_live`. Replay admission of a Store that already continued live locally still claims
    /// while unclaimed retained `Start`s exist, so that a late owner adopts its recorded `Start`
    /// instead of appending a duplicate; when no `Start` matches before the replay target, the
    /// call is reported as [`ReplayStartClaimOutcome::StoreAlreadyLive`] rather than divergence.
    pub(crate) async fn claim_start_for_store(
        &self,
        claim: StartClaim,
        store_live: bool,
    ) -> Result<ReplayStartClaimOutcome, WorkerExecutorError> {
        self.claim_start_for_store_observed(claim, store_live, |_, _| {})
            .await
    }

    /// Shared claim loop of the wrappers above: claims on behalf of a Store whose own liveness is
    /// `store_live` and notifies `on_claim` of every accepted claim inside the owned cursor
    /// operation.
    pub(crate) async fn claim_start_for_store_observed(
        &self,
        claim: StartClaim,
        store_live: bool,
        on_claim: impl Fn(OplogIndex, &OplogEntry) + Clone + Send + Sync + 'static,
    ) -> Result<ReplayStartClaimOutcome, WorkerExecutorError> {
        enum Missing {
            ReplayEnded,
            DeletedRegion,
            StoreAlreadyLive,
        }
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();

            let owned_claim = claim.clone();
            let on_claim = on_claim.clone();
            let (claimed, blocked, missing) = self
                .run_owned_cursor_op(move |state| async move {
                    let gated = async |tx: &mut super::cursor::CursorTx<'_>| {
                        match tx.claim_start(&owned_claim).await {
                            Ok(StartClaimAttempt::Claimed(handle, entry)) => {
                                on_claim(handle.start_idx(), &entry);
                                Ok((Some((handle, entry)), None, None))
                            }
                            Ok(StartClaimAttempt::Blocked(on)) => Ok((None, Some(on), None)),
                            Ok(StartClaimAttempt::Missing) if tx.cursor.is_live() => {
                                Ok((None, None, Some(Missing::ReplayEnded)))
                            }
                            Ok(StartClaimAttempt::Missing) if store_live => {
                                Ok((None, None, Some(Missing::StoreAlreadyLive)))
                            }
                            Ok(StartClaimAttempt::Missing) => {
                                if let Some(bodies) = tx.active_body_owning_head(
                                    tx.buffered_head_owner(),
                                    owned_claim.expected_parent_start_index(),
                                ) {
                                    return Ok((None, Some(BlockedOn::ActiveBody(bodies)), None));
                                }
                                match tx.deleted_region_contains_start(&owned_claim).await? {
                                    Some(index) if index > tx.cursor.last_replayed_index() => {
                                        // The entity's atomic mask is installed before its body
                                        // starts. A host subtask must not publish local liveness
                                        // before the guest consumes the retained atomic Begin.
                                        Ok((None, Some(BlockedOn::CursorProgress), None))
                                    }
                                    Some(_) => Ok((None, None, Some(Missing::DeletedRegion))),
                                    None => Err(WorkerExecutorError::unexpected_oplog_entry(
                                        owned_claim.expected_description(),
                                        "no matching Start between the replay cursor and the replay target"
                                            .to_string(),
                                    )),
                                }
                            }
                            Ok(StartClaimAttempt::MissingSettling { .. }) => unreachable!(
                                "ordinary Start claims never enter missing-scope settlement"
                            ),
                            Err(error) => Err(error),
                        }
                    };
                    state
                        .with_claim_tx(
                            &owned_claim,
                            |handle, entry| {
                                on_claim(handle.start_idx(), &entry);
                                (Some((handle, entry)), None, None)
                            },
                            gated,
                        )
                        .await
                })
                .await?;
            if let Some(claimed) = claimed {
                return Ok(ReplayStartClaimOutcome::Claimed {
                    handle: claimed.0,
                    entry: claimed.1,
                });
            }
            match missing {
                Some(Missing::ReplayEnded) => return Ok(ReplayStartClaimOutcome::ReplayEnded),
                Some(Missing::DeletedRegion) => return Ok(ReplayStartClaimOutcome::DeletedRegion),
                Some(Missing::StoreAlreadyLive) => {
                    return Ok(ReplayStartClaimOutcome::StoreAlreadyLive);
                }
                None => {}
            }
            self.wait_while_blocked(
                blocked.expect("an undecided Start claim is blocked"),
                progress,
            )
            .await;
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
    #[cfg(test)]
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

    /// Claims the `Start` of a durable call owned by another durable record (its
    /// `parent_start_index`) by identity instead of position, scan-ahead-claiming a matching
    /// `Start` ahead of the cursor when concurrent host tasks interleaved the live append order.
    /// Matching `Start`s that share the same full identity (several chunks under one parent) are
    /// claimed in oplog order, preserving the deterministic per-parent chain order.
    #[cfg(test)]
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
    /// Folding scope `End`s into the resolver is what lets a scope `End` be auto-drained by any
    /// cursor driver (so a positional reader never steals a concurrently-replaying sibling call's
    /// terminal, and the scope close never steals a sibling's), at the cost of nothing on the serial
    /// path: when the scope `End` is the entry at the cursor head, awaiting it resolves immediately.
    ///
    /// Every durable scope `Start` consumed during replay leaves a registered awaiter, so its
    /// `End` is always a resolver-routed *awaited terminal* and never an orphan that a parked
    /// awaiter behind it could sleep on until `switch_to_live`. The only un-drained terminals the
    /// cursor may leave at its head are then the dedicated-positional-consumer pairs
    /// (`GolemApiFork`).
    #[cfg(test)]
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

    /// Claims an exact synthetic scope `Start`. If no such entry was committed, atomically proves
    /// that the remaining replay suffix has no competing claim, scope `Start` with the same
    /// invocation id, delivery boundary, or concurrent side effect before entering replay
    /// settlement.
    pub(crate) async fn claim_scope_start_or_recover_missing(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
        invocation_id: Option<DurableInvocationId>,
    ) -> Result<ScopeStartClaimOutcome, WorkerExecutorError> {
        self.claim_scope_start_with_missing_recovery(
            expected_function_name,
            expected_function_type,
            parent_start_index,
            invocation_id,
            true,
        )
        .await
    }

    /// Claims an exact synthetic scope `Start`, reporting absence without switching replay to live.
    pub(crate) async fn claim_scope_start_if_present(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
        invocation_id: Option<DurableInvocationId>,
    ) -> Result<ScopeStartClaimOutcome, WorkerExecutorError> {
        self.claim_scope_start_with_missing_recovery(
            expected_function_name,
            expected_function_type,
            parent_start_index,
            invocation_id,
            false,
        )
        .await
    }

    /// Claims an exact synthetic scope `Start` without polling `readiness` when the scope exists.
    /// If it is missing, waits for the caller's recovery precondition and then atomically repeats
    /// the exact claim and missing-scope safety checks before switching replay to live.
    pub(crate) async fn claim_scope_start_or_recover_missing_when_ready(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
        invocation_id: Option<DurableInvocationId>,
        readiness: impl Future<Output = Result<(), WorkerExecutorError>>,
    ) -> Result<ScopeStartClaimOutcome, WorkerExecutorError> {
        match self
            .claim_scope_start_if_present(
                expected_function_name,
                expected_function_type,
                parent_start_index,
                invocation_id,
            )
            .await?
        {
            claimed @ ScopeStartClaimOutcome::Claimed { .. } => Ok(claimed),
            ScopeStartClaimOutcome::Missing => {
                readiness.await?;
                self.claim_scope_start_or_recover_missing(
                    expected_function_name,
                    expected_function_type,
                    parent_start_index,
                    invocation_id,
                )
                .await
            }
            ScopeStartClaimOutcome::MissingSettling { .. } => {
                unreachable!("presence-only scope claim never enters replay settlement")
            }
        }
    }

    async fn claim_scope_start_with_missing_recovery(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
        invocation_id: Option<DurableInvocationId>,
        recover_missing: bool,
    ) -> Result<ScopeStartClaimOutcome, WorkerExecutorError> {
        let claim = StartClaim::scope(
            expected_function_name,
            expected_function_type,
            parent_start_index,
        )
        .with_invocation_id(invocation_id);
        loop {
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();

            let owned_claim = claim.clone();
            let outcome = self
                .run_owned_cursor_op(move |state| async move {
                    let gated = async |tx: &mut super::cursor::CursorTx<'_>| {
                        tx.claim_scope_start_with_missing_recovery(&owned_claim, recover_missing)
                            .await
                    };
                    state
                        .with_claim_tx(&owned_claim, StartClaimAttempt::Claimed, gated)
                        .await
                })
                .await?;

            match outcome {
                StartClaimAttempt::Claimed(handle, _) => {
                    return Ok(ScopeStartClaimOutcome::Claimed {
                        begin_index: handle.start_idx(),
                        handle,
                    });
                }
                StartClaimAttempt::Missing => {
                    debug_assert!(!recover_missing);
                    return Ok(ScopeStartClaimOutcome::Missing);
                }
                StartClaimAttempt::MissingSettling { replay_target } => {
                    debug_assert!(recover_missing);
                    return Ok(ScopeStartClaimOutcome::MissingSettling { replay_target });
                }
                StartClaimAttempt::Blocked(on) => self.wait_while_blocked(on, progress).await,
            }
        }
    }

    /// Claims the `Start` of a custom durable invocation by its invocation id and marks it as a
    /// replay-inert subtree root: replay returns the root's persisted result without executing its
    /// body, so the records under it are drained while its resolution is awaited.
    ///
    /// A custom invocation admitted after the live transition while unclaimed retained `Start`s
    /// exist (see `WorkerState::durable_call_is_live`) uses this to adopt its retained `Start` or
    /// fall back to recording a new one.
    pub(crate) async fn claim_custom_start_for_store(
        &self,
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
        invocation_id: DurableInvocationId,
        store_live: bool,
    ) -> Result<CustomStartClaimOutcome, WorkerExecutorError> {
        let claim = StartClaim::custom_invocation(
            function_name,
            function_type,
            parent_start_index,
            invocation_id,
        );
        match self.claim_start_for_store(claim, store_live).await? {
            ReplayStartClaimOutcome::Claimed { handle, entry } => {
                let OplogEntry::Start {
                    timestamp,
                    function_name,
                    durable_function_type,
                    ..
                } = *entry
                else {
                    unreachable!("claim_start only claims Start entries");
                };
                Ok(CustomStartClaimOutcome::Claimed(ClaimedConcurrentStart {
                    handle,
                    function_name,
                    durable_function_type,
                    timestamp,
                }))
            }
            ReplayStartClaimOutcome::ReplayEnded => Ok(CustomStartClaimOutcome::ReplayEnded),
            ReplayStartClaimOutcome::DeletedRegion | ReplayStartClaimOutcome::StoreAlreadyLive => {
                Ok(CustomStartClaimOutcome::StoreAlreadyLive)
            }
        }
    }
}

/// Outcome of [`ReplayState::claim_custom_start_for_store`].
pub(crate) enum CustomStartClaimOutcome {
    Claimed(ClaimedConcurrentStart),
    /// No `Start` carries the requested invocation id and the cursor is at the replay target: the
    /// invocation is new and must be recorded live.
    ReplayEnded,
    /// No `Start` carries the requested invocation id before the replay target, but the claiming
    /// Store already continued live locally, or the `Start` belongs to a deleted replay region, so
    /// the invocation is new for that Store.
    StoreAlreadyLive,
}

/// The `parent_start_index` a durable call's `Start` entry is recorded with when the caller does
/// not open its own durable scope: the scope explicitly encoded in the durable function type
/// (batched / transaction `Some(begin_index)`), or `None` for top-level calls. This mirrors the
/// derivation on the write side, so claims can reproduce the recorded value.
pub(super) fn parent_start_index_of(function_type: &DurableFunctionType) -> Option<OplogIndex> {
    match function_type {
        DurableFunctionType::WriteRemoteBatched(Some(idx))
        | DurableFunctionType::WriteRemoteTransaction(Some(idx)) => Some(*idx),
        _ => None,
    }
}
