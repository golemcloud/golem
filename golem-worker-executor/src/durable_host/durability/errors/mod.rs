// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_common::model::RetryPolicyState;
use golem_common::model::oplog::OplogIndex;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::time::Duration;

/// Classification of host function failures for semantic retry decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFailureKind {
    /// Transient failure (network timeout, connection refused, etc.) - should be retried
    Transient,
    /// Permanent failure (invalid input, not found, etc.) - should NOT be retried
    Permanent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableRecoveryFailureKind {
    Retryable,
    Permanent,
}

#[derive(Debug)]
pub struct DurableRecoveryFailure {
    pub kind: DurableRecoveryFailureKind,
    pub inner: anyhow::Error,
}

impl DurableRecoveryFailure {
    pub fn new(kind: DurableRecoveryFailureKind, inner: impl Into<anyhow::Error>) -> Self {
        Self {
            kind,
            inner: inner.into(),
        }
    }
}

impl std::fmt::Display for DurableRecoveryFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "durable runtime reconstruction failed: {}",
            self.inner
        )
    }
}

impl std::error::Error for DurableRecoveryFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(AsRef::<dyn std::error::Error + Send + Sync + 'static>::as_ref(&self.inner))
    }
}

/// A wrapper error that carries semantic classification of a host function failure.
/// This is detected during error chain traversal in TrapType::from_error to produce
/// the appropriate AgentError variant.
///
/// IMPORTANT: This type must NOT delegate `source()` to its inner error, so that
/// `downcast_ref::<ClassifiedHostError>()` on the anyhow chain finds this wrapper
/// directly, not the inner error.
#[derive(Debug)]
pub struct ClassifiedHostError {
    pub kind: HostFailureKind,
    pub message: String,
}

impl std::fmt::Display for ClassifiedHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ClassifiedHostError {
    // Deliberately not implementing source() — we don't want to chain through
    // to the original error, as that would prevent downcast_ref from finding us.
}

/// Verdict carried with a `SemanticTrapRetryOverride`.
///
/// Captures the outcome of a named-policy resolution that happened inside a
/// host call (with full retry properties). When the host call escalates to a
/// trap because inline retry is not eligible (batched write, transaction write,
/// delay > threshold), this verdict travels with the
/// failure into the trap-recovery path so the post-trap decision does not
/// have to re-resolve the policy from an impoverished trap context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticTrapRetryVerdict {
    /// Retry after the given delay.
    Retry(Duration),
    /// Give up; persist the failure.
    GiveUp,
}

/// Ephemeral retry decision attached to a trap-bound failure.
///
/// Lives only inside the in-flight `anyhow::Error` chain (via the
/// `SemanticTrapRetryOverrideMarker` wrapper). It is never persisted in the
/// oplog and never stored in long-lived worker state. The post-trap recovery
/// path consumes it once via `TrapType::semantic_trap_retry_override`.
#[derive(Debug, Clone)]
pub struct SemanticTrapRetryOverride {
    /// Retry point owned by the host call/scope that produced this trap. The
    /// outer invocation wrapper can use this instead of consulting the ambient
    /// worker fallback state.
    pub retry_from: OplogIndex,
    /// Name of the named retry policy that produced this decision.
    pub policy_name: String,
    /// Retry verdict computed from the policy.
    pub verdict: SemanticTrapRetryVerdict,
    /// Post-step retry policy state. This is what ends up persisted in
    /// `OplogEntry::Error.retry_policy_state` so the next attempt's
    /// resolution can pick up where this attempt left off.
    pub retry_policy_state: RetryPolicyState,
}

/// Marker error wrapping a `SemanticTrapRetryOverride` together with the
/// original failure. The marker is used as the *head* of the resulting
/// `anyhow::Error` chain (via `anyhow::Error::new(marker)`) and exposes the
/// original failure through `source()`, so `TrapType::from_error` can locate
/// it by walking the error chain with `.chain().find_map(downcast_ref)` and
/// any other downstream `downcast_ref::<...>()` calls (e.g. for
/// `ClassifiedHostError`) still see the inner error via `source()`.
///
/// We wrap the inner error directly rather than using `anyhow::Error::context`
/// because anyhow stores context attachments in a private `ContextError`
/// wrapper that is not equal to the marker type, so plain `chain()` +
/// `downcast_ref::<MarkerType>()` lookups would miss the context. Wrapping
/// directly keeps both lookups predictable.
#[derive(Debug)]
pub struct SemanticTrapRetryOverrideMarker {
    pub payload: SemanticTrapRetryOverride,
    pub inner: anyhow::Error,
}

pub fn semantic_trap_retry_override_error(
    payload: SemanticTrapRetryOverride,
    kind: HostFailureKind,
    message: impl Into<String>,
) -> anyhow::Error {
    anyhow::Error::new(SemanticTrapRetryOverrideMarker {
        payload,
        inner: anyhow::Error::new(ClassifiedHostError {
            kind,
            message: message.into(),
        }),
    })
}

impl std::fmt::Display for SemanticTrapRetryOverrideMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.payload.verdict {
            SemanticTrapRetryVerdict::Retry(delay) => write!(
                f,
                "semantic-trap-retry-override(policy={}, verdict=retry, delay={:?})",
                self.payload.policy_name, delay
            ),
            SemanticTrapRetryVerdict::GiveUp => write!(
                f,
                "semantic-trap-retry-override(policy={}, verdict=give-up)",
                self.payload.policy_name
            ),
        }
    }
}

impl std::error::Error for SemanticTrapRetryOverrideMarker {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // Expose the inner failure as the source so chain() walks past
        // this marker into the original error (e.g. `ClassifiedHostError`),
        // and so error reporting still surfaces the underlying cause.
        Some(AsRef::<dyn std::error::Error + Send + Sync + 'static>::as_ref(&self.inner))
    }
}

/// Searches an `anyhow::Error` for a `SemanticTrapRetryOverrideMarker` and
/// returns a clone of its payload.
pub fn find_semantic_trap_retry_override(
    error: &anyhow::Error,
) -> Option<SemanticTrapRetryOverride> {
    error
        .chain()
        .find_map(|e| e.downcast_ref::<SemanticTrapRetryOverrideMarker>())
        .map(|m| m.payload.clone())
}

/// Call-owned trap classification carried with a hard (non-semantic) host-call trap.
///
/// When a durable call abandons for a trap, the call's own retry point
/// is attached to the escaping error (via the `DurableCallTrapContextMarker` wrapper). The
/// post-trap recovery path then groups the failure against the *call's* scope rather than the
/// ambient/global worker state, which an overlapping sibling call can clobber once concurrent
/// durable calls truly overlap.
///
/// The retry point is a *pure* function of the call's execution scope captured at initiation (no
/// ambient/`ctx` read), so the context can be built at the trap egress point with no early snapshot
/// and is immune to sibling activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurableCallTrapContext {
    /// The retry point owned by the trapping call: its custom invocation owner when observational,
    /// otherwise its enclosing durable
    /// scope `Start` or its own `Start`.
    pub retry_from: OplogIndex,
}

/// Marker error wrapping a [`DurableCallTrapContext`] together with the original failure. Mirrors
/// [`SemanticTrapRetryOverrideMarker`]: it is the *head* of the resulting `anyhow::Error` chain and
/// exposes the original failure through `source()`, so `TrapType::from_error` (and any nested
/// semantic-override / classification lookups) still walk past it via `.chain()`.
#[derive(Debug)]
pub struct DurableCallTrapContextMarker {
    pub payload: DurableCallTrapContext,
    pub inner: anyhow::Error,
}

impl std::fmt::Display for DurableCallTrapContextMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "durable-call-trap-context(retry_from={})",
            self.payload.retry_from
        )
    }
}

impl std::error::Error for DurableCallTrapContextMarker {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(AsRef::<dyn std::error::Error + Send + Sync + 'static>::as_ref(&self.inner))
    }
}

/// Wraps `inner` with a [`DurableCallTrapContextMarker`] carrying `payload`, returning the new head
/// of the error chain. Idempotent in spirit: re-wrapping is harmless because
/// [`find_durable_call_trap_context`] returns the *outermost* (most specific) marker it encounters.
pub fn mark_durable_call_trap_context(
    inner: anyhow::Error,
    payload: DurableCallTrapContext,
) -> anyhow::Error {
    anyhow::Error::new(DurableCallTrapContextMarker { payload, inner })
}

/// Searches an `anyhow::Error` for a `DurableCallTrapContextMarker` and returns a copy of its
/// payload.
pub fn find_durable_call_trap_context(error: &anyhow::Error) -> Option<DurableCallTrapContext> {
    error
        .chain()
        .find_map(|e| e.downcast_ref::<DurableCallTrapContextMarker>())
        .map(|m| m.payload)
}

/// An error type that can carry a [`DurableCallTrapContextMarker`] in its chain, used as the error
/// bound of the Shape-A `DurableCallSession::run` combinator.
///
/// Implemented only for `anyhow::Error` / `wasmtime::Error` (which preserve the marker chain). It is
/// deliberately **not** implemented for typed errors such as `WorkerExecutorError`: converting a
/// marked `anyhow::Error` into a typed error would silently drop the marker and reintroduce the
/// ambient-fallback misclassification under overlap. So a `run` call site that wants a typed error
/// must instead return `wasmtime::Result`/`anyhow::Result` and map the error at the boundary.
pub trait DurableCallTrapError: From<WorkerExecutorError> + Into<anyhow::Error> {
    /// Re-wraps a marked `anyhow::Error` (produced by [`DurableCallSession::trap`]) back into this error
    /// type, preserving the marker chain.
    fn from_durable_call_trap(error: anyhow::Error) -> Self;
}

impl DurableCallTrapError for anyhow::Error {
    fn from_durable_call_trap(error: anyhow::Error) -> Self {
        error
    }
}

impl DurableCallTrapError for wasmtime::Error {
    fn from_durable_call_trap(error: anyhow::Error) -> Self {
        wasmtime::Error::from_anyhow(error)
    }
}

/// A failure escaping a live *terminal* durable-call step — `DurableCallSession::complete` /
/// `complete_access` / `cancel` / `cancel_access` and the dropped-call cancellation drain — paired
/// with that call's own [`DurableCallTrapContext`].
///
/// It is a *conversion carrier*, deliberately **not** a `std::error::Error`: converting it into a
/// marker-preserving error type (`anyhow::Error` / `wasmtime::Error` /
/// `wasmtime_wasi::p2::StreamError`) attaches the call-owned trap marker via
/// [`mark_durable_call_trap_context`], so post-trap retry grouping stays owned by the failing call
/// rather than falling back to ambient worker state an overlapping sibling could have clobbered. If
/// it implemented `std::error::Error`, the upstream blanket `From<E: Error>` conversions for
/// `anyhow::Error` / `wasmtime::Error` would collide with (or silently bypass) the marker-adding
/// `From` impls below.
#[derive(Debug)]
pub struct TerminalCallError {
    pub source: WorkerExecutorError,
    pub context: DurableCallTrapContext,
}

impl TerminalCallError {
    pub fn new(source: WorkerExecutorError, context: DurableCallTrapContext) -> Self {
        Self { source, context }
    }

    /// Converts into an `anyhow::Error` carrying this call's trap marker, keeping the original
    /// `WorkerExecutorError` reachable as the root cause so `TrapType::from_error` can still
    /// downcast it (e.g. for `ReadOnlyViolation` / unexpected-oplog-entry classification).
    pub fn into_marked_anyhow(self) -> anyhow::Error {
        let inner = anyhow::Error::new(self.source);
        debug_assert!(
            inner
                .root_cause()
                .downcast_ref::<wasmtime::Trap>()
                .is_none(),
            "TerminalCallError must not wrap a deterministic wasm trap"
        );
        mark_durable_call_trap_context(inner, self.context)
    }
}

impl std::fmt::Display for TerminalCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.source, f)
    }
}

impl From<TerminalCallError> for anyhow::Error {
    fn from(error: TerminalCallError) -> Self {
        error.into_marked_anyhow()
    }
}

impl From<TerminalCallError> for wasmtime::Error {
    fn from(error: TerminalCallError) -> Self {
        wasmtime::Error::from_anyhow(error.into_marked_anyhow())
    }
}

impl From<TerminalCallError> for wasmtime_wasi::p2::StreamError {
    fn from(error: TerminalCallError) -> Self {
        wasmtime_wasi::p2::StreamError::Trap(wasmtime::Error::from_anyhow(
            error.into_marked_anyhow(),
        ))
    }
}

impl From<TerminalCallError> for wasmtime_wasi::p2::SocketError {
    fn from(error: TerminalCallError) -> Self {
        // `TrappableError::trap` takes `impl Into<wasmtime::Error>`, so the marker is attached via the
        // `From<TerminalCallError> for wasmtime::Error` impl above.
        wasmtime_wasi::p2::SocketError::trap(error)
    }
}

#[cfg(test)]
mod tests;
