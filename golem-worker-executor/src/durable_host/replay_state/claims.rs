use super::*;

/// Typed descriptor of the recorded `Start` entry a concurrent-replay claim is looking for.
/// Every identity-based claim variant — top-level call, owned call, durable scope, dynamic
/// "any call", with or without request-payload matching — is a variant of this descriptor driven
/// through the single core [`CursorTx::claim_start`]; each variant carries exactly the identity
/// the write side records in the `Start` entry for that kind of claim, so no invalid combination
/// (e.g. a scope claim without a function name) is representable.
#[derive(Debug, Clone)]
pub(super) enum StartClaim {
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
        matching_request: Option<HostRequest>,
    },
    /// A durable-call `Start` owned by another durable record (`parent_start_index` points at
    /// the owning scope/call `Start`).
    Owned {
        function_name: HostFunctionName,
        function_type: DurableFunctionType,
        parent_start_index: OplogIndex,
        /// When `Some`, the recorded request payload must additionally match this value by
        /// value; see [`recorded_request_payload_matches`].
        matching_request: Option<HostRequest>,
    },
    /// A durable-*scope* `Start`: request-less and unowned (e.g. `<scope:batched-write>` /
    /// `<scope:transaction>`).
    Scope {
        function_name: HostFunctionName,
        function_type: DurableFunctionType,
    },
    /// Any top-level durable-call `Start`, whatever its function name and durable function type
    /// (the dynamic guest-facing durability read learns the identity from the claimed entry
    /// itself). The `Start` must carry a request (durable host calls always do; a request-less
    /// `Start` is a scope `Start`) and must not be owned by another durable record.
    AnyUnownedCall,
}

impl StartClaim {
    /// See [`StartClaim::Unowned`].
    pub(super) fn unowned(
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
    pub(super) fn unowned_matching_request(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        request: &HostRequest,
    ) -> Self {
        Self::Unowned {
            function_name: function_name.clone(),
            function_type: function_type.clone(),
            matching_request: Some(request.clone()),
        }
    }

    /// See [`StartClaim::Owned`].
    pub(super) fn owned(
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
    pub(super) fn owned_matching_request(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
        request: &HostRequest,
    ) -> Self {
        Self::Owned {
            function_name: function_name.clone(),
            function_type: function_type.clone(),
            parent_start_index,
            matching_request: Some(request.clone()),
        }
    }

    /// See [`StartClaim::Scope`].
    pub(super) fn scope(
        function_name: &HostFunctionName,
        function_type: &DurableFunctionType,
    ) -> Self {
        Self::Scope {
            function_name: function_name.clone(),
            function_type: function_type.clone(),
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
            Self::AnyUnownedCall => None,
        }
    }

    /// The expected recorded durable function type; `None` claims any type.
    pub(super) fn expected_function_type(&self) -> Option<&DurableFunctionType> {
        match self {
            Self::Unowned { function_type, .. }
            | Self::Owned { function_type, .. }
            | Self::Scope { function_type, .. } => Some(function_type),
            Self::AnyUnownedCall => None,
        }
    }

    /// Whether the `Start` must carry a request payload: `true` for durable host calls, `false`
    /// for durable-scope `Start`s.
    pub(super) fn carries_request(&self) -> bool {
        match self {
            Self::Unowned { .. } | Self::Owned { .. } | Self::AnyUnownedCall => true,
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
            Self::Scope { .. } | Self::AnyUnownedCall => None,
        }
    }

    /// The request payload the recorded `Start` must additionally match by value, when the
    /// claim pins one; see [`recorded_request_payload_matches`].
    pub(super) fn matching_request(&self) -> Option<&HostRequest> {
        match self {
            Self::Unowned {
                matching_request, ..
            }
            | Self::Owned {
                matching_request, ..
            } => matching_request.as_ref(),
            Self::Scope { .. } | Self::AnyUnownedCall => None,
        }
    }

    /// Human-readable description of the expected `Start`, used as the "expected" side of an
    /// `unexpected_oplog_entry` claim error. Worded per claim variant, matching exactly what each
    /// variant has always reported.
    pub(super) fn expected_description(&self) -> String {
        match self {
            Self::AnyUnownedCall => {
                "Start { request: Some(..), parent_start_index: None }".to_string()
            }
            Self::Scope {
                function_name,
                function_type,
            } => {
                format!(
                    "Start {{ {function_name}, {function_type:?}, request: None, parent_start_index: None }}"
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
        }
    }
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
        self.run_owned_cursor_op(move |state| async move {
            state.with_tx(async |tx| tx.claim_start(&claim).await).await
        })
        .await
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
    ) -> Result<(OplogIndex, ReplayCallHandle), WorkerExecutorError> {
        let (handle, _) = self
            .claim_start(StartClaim::scope(
                expected_function_name,
                expected_function_type,
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
    pub async fn claim_concurrent_start_matching_request(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        expected_request: &HostRequest,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let (handle, _) = self
            .claim_start(StartClaim::unowned_matching_request(
                expected_function_name,
                expected_function_type,
                expected_request,
            ))
            .await?;
        Ok(handle)
    }

    /// Claims the `Start` of a durable call owned by another durable record, matching identity
    /// **and recorded request payload** — the owned counterpart of
    /// [`Self::claim_concurrent_start_matching_request`]. With a claim-safe parent (a
    /// discriminated scope) the parent index already pins the call, so the payload match acts as
    /// a cheap validation that the claimed record really belongs to this call.
    pub async fn claim_owned_concurrent_start_matching_request(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
        expected_request: &HostRequest,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let (handle, _) = self
            .claim_start(StartClaim::owned_matching_request(
                expected_function_name,
                expected_function_type,
                parent_start_index,
                expected_request,
            ))
            .await?;
        Ok(handle)
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
    expected: &HostRequest,
) -> Result<bool, String> {
    match recorded {
        OplogPayload::Inline(value) => Ok(value.as_ref() == expected),
        OplogPayload::SerializedInline {
            cached: Some(cached),
            ..
        }
        | OplogPayload::External {
            cached: Some(cached),
            ..
        } => Ok(cached.as_ref() == expected),
        OplogPayload::SerializedInline {
            bytes,
            cached: None,
        } => golem_common::serialization::deserialize::<HostRequest>(bytes)
            .map(|value| &value == expected)
            .map_err(|err| format!("failed to deserialize inline request payload: {err}")),
        OplogPayload::External { cached: None, .. } => oplog
            .download_payload(recorded.clone())
            .await
            .map(|value| &value == expected),
    }
}
