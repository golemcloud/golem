use super::*;

impl ReplayState {
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
        let expected_function_name = expected_function_name.clone();
        let expected_function_type = expected_function_type.clone();
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            let result = tx
                .claim_unowned_start(&expected_function_name, &expected_function_type)
                .await;
            cursor.finish_tx(tx);
            result
        })
        .await
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
    /// see [`CursorTx::claim_any_concurrent_start`].
    pub async fn claim_any_concurrent_start(
        &self,
    ) -> Result<ClaimedConcurrentStart, WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            let result = tx.claim_any_concurrent_start().await;
            cursor.finish_tx(tx);
            result
        })
        .await
    }

    /// Claims the `Start` of a durable call owned by another durable record (its
    /// `parent_start_index`) by identity instead of position, scan-ahead-claiming a matching
    /// `Start` ahead of the cursor when concurrent host tasks interleaved the live append order.
    /// See [`CursorTx::claim_owned_start`].
    pub async fn claim_owned_concurrent_start(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let expected_function_name = expected_function_name.clone();
        let expected_function_type = expected_function_type.clone();
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            let result = tx
                .claim_owned_start(
                    &expected_function_name,
                    &expected_function_type,
                    parent_start_index,
                )
                .await;
            cursor.finish_tx(tx);
            result
        })
        .await
    }

    /// Claims the next durable-scope `Start` matching exactly the expected name and registers a
    /// resolver awaiter for it, so its matching scope `End` is consumed through
    /// [`Self::await_resolution_outcome`] rather than a positional read. See
    /// [`CursorTx::claim_scope_start`].
    pub async fn claim_scope_start(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
    ) -> Result<(OplogIndex, ReplayCallHandle), WorkerExecutorError> {
        let expected_function_name = expected_function_name.clone();
        let expected_function_type = expected_function_type.clone();
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            let result = tx
                .claim_scope_start(&expected_function_name, &expected_function_type)
                .await;
            cursor.finish_tx(tx);
            result
        })
        .await
    }

    /// Claims the next top-level durable-call `Start` matching identity **and recorded request
    /// payload**; see [`CursorTx::claim_unowned_start_matching_request`].
    pub async fn claim_concurrent_start_matching_request(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        expected_request: &HostRequest,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let expected_function_name = expected_function_name.clone();
        let expected_function_type = expected_function_type.clone();
        let expected_request = expected_request.clone();
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            let result = tx
                .claim_unowned_start_matching_request(
                    &expected_function_name,
                    &expected_function_type,
                    &expected_request,
                )
                .await;
            cursor.finish_tx(tx);
            result
        })
        .await
    }

    /// Claims the `Start` of a durable call owned by another durable record, matching identity
    /// **and recorded request payload**; see [`CursorTx::claim_owned_start_matching_request`].
    pub async fn claim_owned_concurrent_start_matching_request(
        &self,
        expected_function_name: &HostFunctionName,
        expected_function_type: &DurableFunctionType,
        parent_start_index: OplogIndex,
        expected_request: &HostRequest,
    ) -> Result<ReplayCallHandle, WorkerExecutorError> {
        let expected_function_name = expected_function_name.clone();
        let expected_function_type = expected_function_type.clone();
        let expected_request = expected_request.clone();
        self.run_owned_cursor_op(move |state| async move {
            let cursor = &*state.cursor;
            let mut tx = cursor.tx().await;
            let result = tx
                .claim_owned_start_matching_request(
                    &expected_function_name,
                    &expected_function_type,
                    parent_start_index,
                    &expected_request,
                )
                .await;
            cursor.finish_tx(tx);
            result
        })
        .await
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
