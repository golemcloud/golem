use super::*;
use crate::durable_host::tool::operation::OwnerToolOperation;
use golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal;

pub(super) type AdmissionRefusal = (OwnerToolOperation, String);

pub(super) fn matching_admission_refusal(
    admission: Option<&AdmissionRefusal>,
    claim: &StartClaim,
) -> Result<Option<(SerializableToolAdmissionRefusal, OplogIndex)>, WorkerExecutorError> {
    let Some((operation, initiating_function)) = admission else {
        return Ok(None);
    };
    let Some((expected, terminal_index)) = operation.expected_admission_refusal() else {
        return Ok(None);
    };
    let matches = match &expected {
        SerializableToolAdmissionRefusal::BeforeStart {
            initiating_function: function,
            claim: encoded,
        } => {
            function == initiating_function
                && desert_rust::deserialize::<StartClaim>(encoded).map_err(|error| {
                    WorkerExecutorError::runtime(format!(
                        "invalid recorded admission claim: {error}"
                    ))
                })? == *claim
        }
        SerializableToolAdmissionRefusal::AtRecordedOplogEntry { .. }
        | SerializableToolAdmissionRefusal::BeforePositional { .. }
        | SerializableToolAdmissionRefusal::BeforeInputRead { .. }
        | SerializableToolAdmissionRefusal::BeforeDurableInputDisposal { .. }
        | SerializableToolAdmissionRefusal::BeforeAttachmentDisposal { .. } => false,
    };
    Ok(
        (matches || operation.observed_admission_refusal().is_some())
            .then_some((expected, terminal_index)),
    )
}

/// Whether a claimed `Start` records a durable call or opens a durable scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, desert_rust::BinaryCodec)]
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
#[derive(Debug, Clone, PartialEq, desert_rust::BinaryCodec)]
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
    /// The call makes host-internal calls under its own `Start` (see
    /// `DurableCallSession::invoke_host_internal_access`). When its recorded terminal is a
    /// `Cancelled` without a partial result, the guest never observed the call, and the records
    /// of those host-internal calls are drained instead of claimed.
    host_internal_calls: bool,
}

impl StartClaim {
    /// A durable call `Start` with no parent record.
    pub(crate) fn unowned(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
    ) -> Self {
        Self {
            function_name: function_name.clone(),
            tool_rejection_function_name: None,
            function_type: function_type.clone(),
            kind: StartKind::Call,
            parent_start_index: None,
            observational_owner: None,
            invocation_id: None,
            custom_invocation: false,
            host_internal_calls: false,
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

    /// Marks the call as one that makes host-internal calls under its own `Start`.
    pub(crate) fn with_host_internal_calls(mut self, host_internal_calls: bool) -> Self {
        self.host_internal_calls = host_internal_calls;
        self
    }

    pub(super) fn makes_host_internal_calls(&self) -> bool {
        self.host_internal_calls
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

    pub(super) fn expected_invocation_id(&self) -> Option<DurableInvocationId> {
        self.invocation_id
    }

    pub(super) fn matches_start_identity(&self, entry: &OplogEntry) -> bool {
        RecordedStart::of(entry).is_some_and(|recorded| self.matches_recorded(&recorded))
    }

    pub(super) fn matches_recorded(&self, recorded: &RecordedStart) -> bool {
        self.matches_function_name(&recorded.function_name)
            && recorded.function_type == self.function_type
            && recorded.invocation_id == self.invocation_id
            && recorded.observational_owner == self.observational_owner
            && recorded.carries_request == self.carries_request()
            && recorded.parent_start_index == self.parent_start_index
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

/// The identity of a recorded `Start` entry: the fields a [`StartClaim`] matches.
#[derive(Debug, Clone)]
pub(super) struct RecordedStart {
    function_name: HostFunctionName,
    function_type: DurableFunctionType,
    parent_start_index: Option<OplogIndex>,
    observational_owner: Option<OplogIndex>,
    invocation_id: Option<DurableInvocationId>,
    carries_request: bool,
}

impl RecordedStart {
    pub(super) fn of(entry: &OplogEntry) -> Option<Self> {
        match entry {
            OplogEntry::Start {
                function_name,
                invocation_id,
                observational_owner,
                request,
                durable_function_type,
                parent_start_index,
                ..
            } => Some(Self {
                function_name: function_name.clone(),
                function_type: durable_function_type.clone(),
                parent_start_index: *parent_start_index,
                observational_owner: *observational_owner,
                invocation_id: *invocation_id,
                carries_request: request.is_some(),
            }),
            _ => None,
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
    /// Retires a claimed incomplete call without recording cancellation or advancing replay.
    pub(crate) async fn retire_incomplete_admission_claim(
        &self,
        start_index: OplogIndex,
    ) -> Result<bool, WorkerExecutorError> {
        self.run_owned_cursor_op(move |state| async move {
            if state.has_visible_terminal(start_index).await? {
                return Ok(false);
            }
            let mut st = state.cursor.state.lock().await;
            if st.skipped_regions.is_in_deleted_region(start_index)
                || !st.concurrent_resolver.has_claim(start_index)
            {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "claimed incomplete admission Start",
                    format!("Start {start_index} is deleted or has no pending resolver"),
                ));
            }
            st.concurrent_resolver.unregister_incomplete(start_index);
            drop(st);
            state.cursor.progress.notify_waiters();
            Ok(true)
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn has_unclaimed_start_before(
        &self,
        claim: StartClaim,
        terminal_index: OplogIndex,
    ) -> Result<bool, WorkerExecutorError> {
        self.run_owned_cursor_op(move |state| async move {
            state
                .cursor
                .state
                .lock()
                .await
                .has_unclaimed_start_before(&state.cursor, &claim, terminal_index)
                .await
        })
        .await
    }

    /// Waits until a blocked claim may run again.
    pub(super) async fn wait_while_blocked(
        &self,
        blocked: BlockedOn,
        progress: std::pin::Pin<&mut tokio::sync::futures::Notified<'_>>,
    ) -> Result<(), WorkerExecutorError> {
        #[cfg(test)]
        if matches!(blocked, BlockedOn::ActiveBody(_)) {
            self.cursor
                .active_body_waits
                .send_modify(|waits| *waits += 1);
        }
        tokio::select! {
            biased;
            failure = self.cursor.owner_tool_operations.wait_for_owner_failure() => {
                Err(super::cursor::historical_reconstruction_owner_failure(failure))
            }
            _ = blocked.wait(progress) => Ok(()),
        }
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
    /// [`CursorTx::active_body_owning_head`]); any other missing match is decided by
    /// [`CursorTx::decide_missing_start`].
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
        self.claim_start_for_store_with_admission(claim, store_live, on_claim, None)
            .await
    }

    pub(crate) async fn claim_start_for_store_with_admission(
        &self,
        claim: StartClaim,
        store_live: bool,
        on_claim: impl Fn(OplogIndex, &OplogEntry) + Clone + Send + Sync + 'static,
        admission: Option<AdmissionRefusal>,
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
            let admission = admission.clone();
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
                                        // A host subtask must not publish local liveness before
                                        // the guest consumes the surviving positional prefix.
                                        Ok((None, Some(BlockedOn::CursorProgress), None))
                                    }
                                    Some(_) => Ok((None, None, Some(Missing::DeletedRegion))),
                                    None => match tx.decide_missing_start(&owned_claim).await? {
                                        MissingStart::ReplayEnded => {
                                            Ok((None, None, Some(Missing::ReplayEnded)))
                                        }
                                        MissingStart::Blocked(on) => Ok((None, Some(on), None)),
                                    },
                                }
                            }
                            Err(error) => Err(error),
                        }
                    };
                    state
                        .with_claim_tx_admission(
                            &owned_claim,
                            admission.as_ref(),
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
            .await?;
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
