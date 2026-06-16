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

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::Resolution;
use crate::metrics::wasm::{record_host_function_call, record_in_function_retry};
// `TrapType` was used for the legacy retry-config fallback; no longer needed
// here after the refactor that funnels every host-trap retry decision through
// named-policy resolution.
// use crate::model::TrapType;
use crate::preview2::golem::durability::durability;
use crate::services::environment_state::EnvironmentStateService;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::services::{HasOplog, HasWorker};
// `RetryDecision` was used by the legacy retry-config fallback removed from
// `try_trigger_retry`.
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequest, HostResponse, OplogEntry, OplogIndex, PersistenceLevel,
};
use golem_common::model::{
    NamedRetryPolicy, PredicateValue, RetryEvaluationError, RetryPolicyState, RetryProperties,
    RetryVerdict, ThreadRng, Timestamp,
};
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use golem_wasm::{FromValue, IntoValueAndType};
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Display};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};
use wasmtime::component::Resource;
use wasmtime_wasi::{DynPollable, DynamicPollable, Pollable, dynamic_subscribe};

/// Classification of host function failures for semantic retry decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFailureKind {
    /// Transient failure (network timeout, connection refused, etc.) - should be retried
    Transient,
    /// Permanent failure (invalid input, not found, etc.) - should NOT be retried
    Permanent,
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
/// trap because inline retry is not eligible (atomic region, non-idempotent
/// write, batched write, delay > threshold), this verdict travels with the
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

/// Result of `try_trigger_retry_or_loop`: tells the host function caller what to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalRetryResult {
    /// The operation result (success or permanent failure) should be persisted in the oplog.
    Persist,
    /// The host function should re-execute the operation (an internal retry sleep has completed).
    RetryInternally,
}

/// Result of `InFunctionRetryState::decide_async_retry`: tells the async RPC caller what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsyncRetryDecision {
    /// The caller should wait for the given duration, then retry the operation.
    RetryAfterDelay(Duration),
    /// Max retry attempts exhausted — persist the failure permanently.
    Exhausted,
    /// The computed delay exceeds the threshold — fall back to trap+replay.
    FallBackToTrap,
}

#[derive(Debug)]
pub struct DurableExecutionState {
    pub is_live: bool,
    pub persistence_level: PersistenceLevel,
    pub snapshotting_mode: Option<PersistenceLevel>,
    /// Whether the executor assumes idempotence for remote writes.
    pub assume_idempotence: bool,
    /// Maximum delay for in-function retries. Delays exceeding this fall back to trap+replay.
    pub max_in_function_retry_delay: Duration,
}

/// Subset of `DurabilityHost` needed by `InFunctionRetryState::decide_retry`.
///
/// This trait is implemented both for `DurableWorkerCtx` (in-context host calls) and
/// for `TaskRetryContext` (spawned background tasks like async RPC/HTTP), allowing
/// the retry decision logic to be shared without duplicating delay calculation,
/// metric emission, and oplog error writing.
#[async_trait]
pub trait InFunctionRetryHost {
    /// Returns true if the worker is currently inside a user-defined atomic region.
    fn in_atomic_region(&self) -> bool;

    /// Returns the oplog index that `OplogEntry::Error` entries should reference as `retry_from`.
    fn current_retry_point(&self) -> OplogIndex;

    /// Returns available semantic retry policies for the current worker context.
    async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy>;

    /// Returns the current semantic retry policy state for the given retry point, if available.
    async fn current_retry_state_for(&self, retry_from: OplogIndex) -> Option<RetryPolicyState>;

    /// Returns the current durable execution state.
    fn durable_execution_state(&self) -> DurableExecutionState;

    /// Writes an `OplogEntry::Error` entry for an in-function retry attempt, and commits.
    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        retry_policy_state: Option<RetryPolicyState>,
    );
}

pub(crate) fn collect_named_retry_policies(
    config: &HashMap<Vec<String>, golem_wasm::ValueAndType>,
) -> Vec<NamedRetryPolicy> {
    let mut policies = Vec::new();

    for value_and_type in config.values() {
        if let Ok(mut parsed) = Vec::<NamedRetryPolicy>::from_value(value_and_type.value.clone()) {
            policies.append(&mut parsed);
            continue;
        }

        if let Ok(parsed) = NamedRetryPolicy::from_value(value_and_type.value.clone()) {
            policies.push(parsed);
        }
    }

    policies
}

pub(crate) fn evaluate_named_policy_step(
    named_policy: &NamedRetryPolicy,
    properties: &RetryProperties,
    current_state: Option<&RetryPolicyState>,
) -> Result<(RetryPolicyState, RetryVerdict), RetryEvaluationError> {
    let mut rng = ThreadRng;
    let state = current_state
        .cloned()
        .unwrap_or_else(|| named_policy.policy.initial_state());

    let (new_state, verdict) =
        named_policy
            .policy
            .step(&state, Duration::ZERO, properties, &mut rng);

    Ok((new_state, verdict))
}

/// Like [`evaluate_named_policy_step`], but if the evaluation reports
/// `InvalidState` against a non-empty `current_state` (typically because
/// the persisted state was written by a *different* policy at the same
/// retry point — e.g. the in-function HTTP status retry path leaves a
/// `CountBox{Counter}` state at the request's begin index, and a
/// subsequent trap-retry path then evaluates a `Jitter/Exp` policy
/// against that slot), automatically retry the evaluation as if no
/// prior state existed.
///
/// This is a defensive guard for the case where a single retry-state
/// slot is shared between multiple distinct policies. Without it, the
/// shape mismatch falls through to the legacy retry config and silently
/// disables retrying altogether.
///
/// We only do this fallback for `InvalidState`; other evaluation errors
/// (e.g. `PropertyNotFound`, `CoercionFailed`) are real errors and must
/// surface as before.
pub(crate) fn evaluate_named_policy_step_resetting_on_invalid_state(
    named_policy: &NamedRetryPolicy,
    properties: &RetryProperties,
    current_state: Option<&RetryPolicyState>,
) -> Result<(RetryPolicyState, RetryVerdict), RetryEvaluationError> {
    let result = evaluate_named_policy_step(named_policy, properties, current_state)?;
    match (&result.1, current_state) {
        (RetryVerdict::Error(RetryEvaluationError::InvalidState { details }), Some(_)) => {
            tracing::debug!(
                policy = %named_policy.name,
                details,
                "Retry-state shape mismatch detected; retrying evaluation with empty state"
            );
            evaluate_named_policy_step(named_policy, properties, None)
        }
        _ => Ok(result),
    }
}

/// Encapsulates in-function retry state for a single durable function invocation.
///
/// Tracks the retry count accumulated within the current host function call
/// (as opposed to oplog-level retries which restart the entire worker).
#[derive(Debug)]
pub struct InFunctionRetryState {
    /// Number of in-function retries performed so far in this invocation.
    retry_count: u32,
}

impl Default for InFunctionRetryState {
    fn default() -> Self {
        Self::new()
    }
}

impl InFunctionRetryState {
    pub fn new() -> Self {
        Self { retry_count: 0 }
    }

    /// Returns the number of in-function retries performed so far.
    pub fn retry_count(&self) -> u32 {
        self.retry_count
    }

    /// Decides whether an in-function retry should happen against a specific named
    /// policy (skipping the normal resolver).
    ///
    /// Use this when the caller has already selected the matching policy and must
    /// not re-enter the resolver — for example, code paths whose entire behavior
    /// depends on whether a *user-defined* policy matches (the synthesized default
    /// must not silently take over). All other semantics — atomic region check,
    /// retry-state tracking, oplog error entry, retry metric — are identical to
    /// `decide_retry_with_properties`.
    pub async fn decide_retry_for_named_policy(
        &mut self,
        ctx: &mut (impl InFunctionRetryHost + Sync),
        function_label: &str,
        properties: &RetryProperties,
        named_policy: &NamedRetryPolicy,
    ) -> AsyncRetryDecision {
        if ctx.in_atomic_region() {
            return AsyncRetryDecision::FallBackToTrap;
        }

        let retry_point = ctx.current_retry_point();
        let current_state = ctx.current_retry_state_for(retry_point).await;
        let oplog_retry_count = current_state.as_ref().map(|s| s.retry_count()).unwrap_or(0);
        let total_attempts = self.retry_count + oplog_retry_count;

        self.apply_policy_step(
            ctx,
            function_label,
            properties,
            named_policy,
            current_state.as_ref(),
            total_attempts,
            retry_point,
        )
        .await
    }

    /// Decides whether an in-function retry should happen based on the current retry budget,
    /// atomic region status, and delay threshold.
    ///
    /// On `RetryAfterDelay`, increments the retry counter, writes an oplog error entry,
    /// emits a debug log, and records the metric.
    pub async fn decide_retry_with_properties(
        &mut self,
        ctx: &mut (impl InFunctionRetryHost + Sync),
        function_label: &str,
        properties: &RetryProperties,
    ) -> AsyncRetryDecision {
        if ctx.in_atomic_region() {
            return AsyncRetryDecision::FallBackToTrap;
        }

        let retry_point = ctx.current_retry_point();
        let current_state = ctx.current_retry_state_for(retry_point).await;
        let oplog_retry_count = current_state.as_ref().map(|s| s.retry_count()).unwrap_or(0);
        let total_attempts = self.retry_count + oplog_retry_count;

        let policies = ctx.named_retry_policies().await;
        let named_policy = match NamedRetryPolicy::resolve(&policies, properties) {
            Ok(Some(policy)) => policy,
            Ok(None) => return AsyncRetryDecision::Exhausted,
            Err(error) => {
                warn!(
                    function = function_label,
                    total_attempts,
                    ?error,
                    "Failed resolving semantic retry policy"
                );
                return AsyncRetryDecision::Exhausted;
            }
        };

        self.apply_policy_step(
            ctx,
            function_label,
            properties,
            named_policy,
            current_state.as_ref(),
            total_attempts,
            retry_point,
        )
        .await
    }

    /// Shared body of the in-function retry decision: evaluates the selected named
    /// policy against current retry state, updates retry-budget bookkeeping, and
    /// produces the resulting `AsyncRetryDecision`.
    #[allow(clippy::too_many_arguments)]
    async fn apply_policy_step(
        &mut self,
        ctx: &mut (impl InFunctionRetryHost + Sync),
        function_label: &str,
        properties: &RetryProperties,
        named_policy: &NamedRetryPolicy,
        current_state: Option<&RetryPolicyState>,
        total_attempts: u32,
        retry_point: OplogIndex,
    ) -> AsyncRetryDecision {
        let retry_policy_state: Option<RetryPolicyState>;

        let delay = match evaluate_named_policy_step_resetting_on_invalid_state(
            named_policy,
            properties,
            current_state,
        ) {
            Ok((new_state, RetryVerdict::Retry(delay))) => {
                retry_policy_state = Some(new_state);
                delay
            }
            Ok((_new_state, RetryVerdict::GiveUp)) => return AsyncRetryDecision::Exhausted,
            Ok((_new_state, RetryVerdict::Error(error))) => {
                warn!(
                    function = function_label,
                    retry_policy = %named_policy.name,
                    total_attempts,
                    ?error,
                    "Semantic retry policy evaluation returned error verdict"
                );
                return AsyncRetryDecision::Exhausted;
            }
            Err(error) => {
                warn!(
                    function = function_label,
                    retry_policy = %named_policy.name,
                    total_attempts,
                    ?error,
                    "Failed evaluating semantic retry policy"
                );
                return AsyncRetryDecision::Exhausted;
            }
        };

        let state = ctx.durable_execution_state();
        if delay > state.max_in_function_retry_delay {
            return AsyncRetryDecision::FallBackToTrap;
        }

        ctx.append_retry_error_entry(retry_point, retry_policy_state)
            .await;
        self.retry_count += 1;

        debug!(
            retry_count = self.retry_count,
            total_attempt = total_attempts + 1,
            function = function_label,
            ?delay,
            "In-function retry",
        );
        record_in_function_retry();

        AsyncRetryDecision::RetryAfterDelay(delay)
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct PersistedDurableFunctionInvocation {
    timestamp: Timestamp,
    function_name: String,
    response: HostResponse,
    function_type: DurableFunctionType,
    oplog_entry_version: OplogEntryVersion,
}

#[async_trait]
pub trait DurabilityHost: InFunctionRetryHost {
    /// Observes a function call (produces logs and metrics)
    fn observe_function_call(&self, interface: &str, function: &str);

    /// Marks the beginning of a durable function.
    ///
    /// There must be a corresponding call to `end_durable_function` after the function has
    /// performed its work (it can be ended in a different context, for example, after an async
    /// pollable operation has been completed).
    ///
    /// `host_function` is the fully qualified name of the host function that is being started.
    /// Implementations use it to surface diagnostic information when the call is refused —
    /// notably for the read-only side-effect guard that fires on every `Write*` function type
    /// when the worker is executing a read-only agent method. This is the single, central
    /// place where that guard lives: every host call that wants to perform a write side
    /// effect funnels through `begin_durable_function` (either directly or via the concurrent
    /// durability `CallHandle`), so the read-only trap is uniformly enforced here without any
    /// per-callsite checks.
    async fn begin_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        host_function: &str,
    ) -> Result<OplogIndex, WorkerExecutorError>;

    /// Marks the end of a durable function
    ///
    /// This is a pair of `begin_durable_function` and should be called after the durable function
    /// has performed and persisted or replayed its work. The `begin_index` should be the index
    /// returned by `begin_durable_function`.
    async fn end_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
        forced_commit: bool,
    ) -> Result<(), WorkerExecutorError>;

    /// Writes a record to the worker's oplog representing a durable function invocation
    async fn persist_durable_function_invocation(
        &self,
        function_name: HostFunctionName,
        request: &HostRequest,
        response: &HostResponse,
        function_type: DurableFunctionType,
    );

    /// Reads the next persisted durable function invocation from the oplog during replay
    async fn read_persisted_durable_function_invocation(
        &mut self,
    ) -> Result<PersistedDurableFunctionInvocation, WorkerExecutorError>;

    /// Checks if the current retry policy allows more retries, and if yes, then returns
    /// with `Err(failure)`. This error should be directly returned from host function
    /// implementations, triggering a retry.
    ///
    /// If retrying is not possible, the function returns Ok(()) and the host function
    /// can continue persisting the failed result permanently.
    ///
    /// The `properties` bag is used for semantic policy resolution when named retry
    /// policies are available. When empty or when no semantic policy matches, the
    /// legacy `RetryConfig` fallback is used.
    async fn try_trigger_retry(
        &mut self,
        failure: Error,
        properties: RetryProperties,
    ) -> anyhow::Result<()>;

    /// Marks the outermost active atomic region (if any) as having produced side effects.
    /// This is called when a non-hint oplog entry is persisted during live execution.
    fn mark_atomic_region_side_effect(&mut self);

    /// Creates an interrupt signal future that resolves when the worker is interrupted/suspended/etc.
    fn create_interrupt_signal(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>>;

    /// Returns `Ok(())` if the worker is in normal (write-capable) invocation strictness mode,
    /// or `Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation)` if the worker is currently
    /// executing a read-only agent method. This is invoked from `begin_durable_function` for every
    /// `Write*` durable function type so that any host call carrying a side effect is rejected
    /// before any oplog entry is written.
    fn check_read_only_allows(&self, host_function: &str) -> Result<(), GolemSpecificWasmTrap>;
}

/// Returns `true` when the given durable function type represents a write side-effect of any
/// kind (local or remote, single, batched, or transactional). These are exactly the function
/// types that must be refused when the current invocation is in read-only strictness mode.
pub(crate) fn is_write_side_effect(function_type: &DurableFunctionType) -> bool {
    matches!(
        function_type,
        DurableFunctionType::WriteLocal
            | DurableFunctionType::WriteRemote
            | DurableFunctionType::WriteRemoteBatched(_)
            | DurableFunctionType::WriteRemoteTransaction(_)
    )
}

impl From<durability::DurableFunctionType> for DurableFunctionType {
    fn from(value: durability::DurableFunctionType) -> Self {
        match value {
            durability::DurableFunctionType::WriteRemote => DurableFunctionType::WriteRemote,
            durability::DurableFunctionType::WriteLocal => DurableFunctionType::WriteLocal,
            durability::DurableFunctionType::WriteRemoteBatched(oplog_index) => {
                DurableFunctionType::WriteRemoteBatched(oplog_index.map(OplogIndex::from_u64))
            }
            durability::DurableFunctionType::ReadRemote => DurableFunctionType::ReadRemote,
            durability::DurableFunctionType::ReadLocal => DurableFunctionType::ReadLocal,
            durability::DurableFunctionType::WriteRemoteTransaction(oplog_index) => {
                DurableFunctionType::WriteRemoteTransaction(oplog_index.map(OplogIndex::from_u64))
            }
        }
    }
}

impl From<DurableFunctionType> for durability::DurableFunctionType {
    fn from(value: DurableFunctionType) -> Self {
        match value {
            DurableFunctionType::WriteRemote => durability::DurableFunctionType::WriteRemote,
            DurableFunctionType::WriteLocal => durability::DurableFunctionType::WriteLocal,
            DurableFunctionType::WriteRemoteBatched(oplog_index) => {
                durability::DurableFunctionType::WriteRemoteBatched(
                    oplog_index.map(|idx| idx.into()),
                )
            }
            DurableFunctionType::ReadRemote => durability::DurableFunctionType::ReadRemote,
            DurableFunctionType::ReadLocal => durability::DurableFunctionType::ReadLocal,
            DurableFunctionType::WriteRemoteTransaction(oplog_index) => {
                durability::DurableFunctionType::WriteRemoteTransaction(
                    oplog_index.map(|idx| idx.into()),
                )
            }
        }
    }
}

impl From<OplogEntryVersion> for durability::OplogEntryVersion {
    fn from(value: OplogEntryVersion) -> Self {
        match value {
            OplogEntryVersion::V1 => durability::OplogEntryVersion::V1,
            OplogEntryVersion::V2 => durability::OplogEntryVersion::V2,
        }
    }
}

impl From<PersistedDurableFunctionInvocation> for durability::PersistedDurableFunctionInvocation {
    fn from(value: PersistedDurableFunctionInvocation) -> Self {
        durability::PersistedDurableFunctionInvocation {
            timestamp: value.timestamp.into(),
            function_name: value.function_name,
            response: value.response.into_value_and_type().into(),
            function_type: value.function_type.into(),
            entry_version: value.oplog_entry_version.into(),
        }
    }
}

impl<Ctx: WorkerCtx> durability::HostLazyInitializedPollable for DurableWorkerCtx<Ctx> {
    async fn new(&mut self) -> anyhow::Result<Resource<LazyInitializedPollableEntry>> {
        DurabilityHost::observe_function_call(self, "durability::lazy_initialized_pollable", "new");
        let lazy_pollable = self.table().push(LazyInitializedPollableEntry::Empty)?;
        Ok(lazy_pollable)
    }

    async fn set(
        &mut self,
        self_: Resource<LazyInitializedPollableEntry>,
        pollable: Resource<DynPollable>,
    ) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, "durability::lazy_initialized_pollable", "set");
        let entry = self.table().get_mut(&self_)?;
        *entry = LazyInitializedPollableEntry::Subscribed { pollable };
        Ok(())
    }

    async fn subscribe(
        &mut self,
        self_: Resource<LazyInitializedPollableEntry>,
    ) -> anyhow::Result<Resource<DynPollable>> {
        DurabilityHost::observe_function_call(
            self,
            "durability::lazy_initialized_pollable",
            "subscribe",
        );

        Ok(dynamic_subscribe(self.table(), self_, None)?)
    }

    async fn drop(&mut self, rep: Resource<LazyInitializedPollableEntry>) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(
            self,
            "durability::lazy_initialized_pollable",
            "drop",
        );

        let entry = self.table().delete(rep)?;
        if let LazyInitializedPollableEntry::Subscribed { pollable } = entry {
            let _ = self.table().delete(pollable)?;
        }

        Ok(())
    }
}

impl<Ctx: WorkerCtx> durability::Host for DurableWorkerCtx<Ctx> {
    async fn observe_function_call(
        &mut self,
        iface: String,
        function: String,
    ) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, &iface, &function);
        Ok(())
    }

    async fn begin_durable_function(
        &mut self,
        function_type: durability::DurableFunctionType,
    ) -> anyhow::Result<durability::OplogIndex> {
        // Called from guest code via the durability WIT binding; the guest is the "host
        // function" identity for diagnostic purposes here.
        let oplog_idx = DurabilityHost::begin_durable_function(
            self,
            &function_type.into(),
            "golem::durability::begin-durable-function",
        )
        .await?;
        Ok(oplog_idx.into())
    }

    async fn end_durable_function(
        &mut self,
        function_type: durability::DurableFunctionType,
        begin_index: durability::OplogIndex,
        forced_commit: bool,
    ) -> anyhow::Result<()> {
        DurabilityHost::end_durable_function(
            self,
            &function_type.into(),
            OplogIndex::from_u64(begin_index),
            forced_commit,
        )
        .await?;
        Ok(())
    }

    async fn current_durable_execution_state(
        &mut self,
    ) -> anyhow::Result<durability::DurableExecutionState> {
        let state = InFunctionRetryHost::durable_execution_state(self);
        Ok(durability::DurableExecutionState {
            is_live: state.is_live,
            persistence_level: match state.persistence_level {
                PersistenceLevel::PersistNothing => durability::PersistenceLevel::PersistNothing,
                PersistenceLevel::PersistRemoteSideEffects => {
                    durability::PersistenceLevel::PersistRemoteSideEffects
                }
                PersistenceLevel::Smart => durability::PersistenceLevel::Smart,
            },
        })
    }

    async fn persist_durable_function_invocation(
        &mut self,
        function_name: String,
        request: durability::ValueAndType,
        response: durability::ValueAndType,
        function_type: durability::DurableFunctionType,
    ) -> anyhow::Result<()> {
        DurabilityHost::persist_durable_function_invocation(
            self,
            HostFunctionName::Custom(function_name),
            &HostRequest::Custom(request.into()),
            &HostResponse::Custom(response.into()),
            function_type.into(),
        )
        .await;
        Ok(())
    }

    async fn read_persisted_durable_function_invocation(
        &mut self,
    ) -> anyhow::Result<durability::PersistedDurableFunctionInvocation> {
        let invocation = DurabilityHost::read_persisted_durable_function_invocation(self).await?;
        Ok(invocation.into())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> InFunctionRetryHost for DurableWorkerCtx<Ctx> {
    fn in_atomic_region(&self) -> bool {
        !self.state.active_atomic_regions.is_empty()
    }

    fn current_retry_point(&self) -> OplogIndex {
        self.state.effective_retry_point()
    }

    async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy> {
        self.state.named_retry_policies().await
    }

    async fn current_retry_state_for(&self, retry_from: OplogIndex) -> Option<RetryPolicyState> {
        let latest_status = self
            .public_state
            .worker()
            .get_non_detached_last_known_status()
            .await;
        latest_status.current_retry_state.get(&retry_from).cloned()
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        DurableExecutionState {
            is_live: self.state.is_live(),
            persistence_level: self.state.persistence_level,
            snapshotting_mode: self.state.snapshotting_mode,
            assume_idempotence: self.state.assume_idempotence,
            max_in_function_retry_delay: self.state.config.max_in_function_retry_delay,
        }
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        retry_policy_state: Option<RetryPolicyState>,
    ) {
        use golem_common::model::oplog::AgentError;
        let inside_atomic_region = self.state.outermost_atomic_region_has_side_effects();
        let entry = OplogEntry::error(
            AgentError::TransientError("in-function retry".to_string()),
            retry_from,
            inside_atomic_region,
            retry_policy_state,
        );
        self.public_state.worker().add_and_commit_oplog(entry).await;
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> DurabilityHost for DurableWorkerCtx<Ctx> {
    fn observe_function_call(&self, interface: &str, function: &str) {
        record_host_function_call(interface, function);
    }

    async fn begin_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        host_function: &str,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        // Generic read-only side-effect trap. For any `Write*` durable function type, refuse
        // the call up front when the worker is executing a read-only agent method. This is the
        // single, central place that enforces the read-only contract — outgoing HTTP, RPC,
        // KV/blobstore writes, RDBMS queries/transactions, worker-management calls, websocket
        // writes, etc. all funnel through `begin_durable_function` (directly or via the
        // concurrent durability `CallHandle`) and are uniformly rejected here before any oplog
        // entry is appended. The `GolemSpecificWasmTrap` is folded into
        // `WorkerExecutorError::ReadOnlyViolation` so the trap survives the conversion chain
        // (anyhow → wasmtime::Error → StreamError / SocketError) and can later be recognised
        // by `TrapType::from_error` as `AgentError::ReadOnlyViolation`.
        if is_write_side_effect(function_type)
            && let Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation {
                method,
                host_function,
            }) = DurableWorkerCtx::check_read_only_allows(self, host_function)
        {
            return Err(WorkerExecutorError::ReadOnlyViolation {
                method,
                host_function,
            });
        }

        self.process_pending_replay_events().await?;
        let oplog_index = self.begin_function(function_type).await?;
        Ok(oplog_index)
    }

    async fn end_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
        forced_commit: bool,
    ) -> Result<(), WorkerExecutorError> {
        self.end_function(function_type, begin_index).await?;
        if function_type == &DurableFunctionType::WriteRemote
            || matches!(function_type, DurableFunctionType::WriteRemoteBatched(_))
            || matches!(
                function_type,
                DurableFunctionType::WriteRemoteTransaction(_)
            )
            || forced_commit
        {
            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                .await;
            // Just committed at a durable-op boundary: this is the single safe place to advance the
            // mid-invocation clean status checkpoint (status now reflects the committed tip).
            self.maybe_mid_invocation_checkpoint().await;
        }
        Ok(())
    }

    async fn persist_durable_function_invocation(
        &self,
        function_name: HostFunctionName,
        request: &HostRequest,
        response: &HostResponse,
        function_type: DurableFunctionType,
    ) {
        // The guest manages its own scope via separate `begin_durable_function` /
        // `end_durable_function` calls, so this completed call does not open a scope here. Its
        // parent is therefore only the scope explicitly encoded in the function type (batched /
        // transaction `Some(begin_index)`); otherwise it is top-level. It must not be inferred from
        // the set of temporally-open scopes, which may belong to unrelated concurrent operations.
        let parent_start_index = match &function_type {
            DurableFunctionType::WriteRemoteBatched(Some(idx))
            | DurableFunctionType::WriteRemoteTransaction(Some(idx)) => Some(*idx),
            _ => None,
        };
        self.public_state
            .worker()
            .oplog()
            .add_completed_host_call(
                function_name,
                request,
                response,
                function_type,
                parent_start_index,
            )
            .await
            .unwrap_or_else(|err| {
                panic!("failed to serialize and store durable function invocation: {err}")
            });
    }

    async fn read_persisted_durable_function_invocation(
        &mut self,
    ) -> Result<PersistedDurableFunctionInvocation, WorkerExecutorError> {
        if self.state.persistence_level == PersistenceLevel::PersistNothing {
            Err(WorkerExecutorError::runtime(
                "Trying to replay an durable invocation in a PersistNothing block",
            ))
        } else {
            // A completed durable function invocation is persisted as a `Start` + `End` pair.
            // Unlike the typed host-call readers, the guest learns the call's identity (function
            // name, type, timestamp) from the persisted `Start` itself, so claim the next `Start`
            // without an expected identity and await its matching `End` through the concurrent
            // resolver. `Cancelled` is unexpected: durable invocations are always persisted as a
            // completed pair.
            let claimed = self.state.replay_state.claim_any_concurrent_start().await?;
            let timestamp = claimed.timestamp;
            let function_name = claimed.function_name.to_string();
            let function_type = claimed.durable_function_type.clone();
            let resolution = self
                .state
                .replay_state
                .await_resolution(claimed.handle)
                .await?;
            match resolution {
                Resolution::Completed { response, .. } => {
                    let response_payload = response.ok_or_else(|| {
                        WorkerExecutorError::unexpected_oplog_entry(
                            "End { response: Some(..) }",
                            "End { response: None }".to_string(),
                        )
                    })?;
                    let response = self
                        .public_state
                        .worker()
                        .oplog()
                        .download_payload(response_payload)
                        .await
                        .map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "End payload cannot be downloaded: {err}"
                            ))
                        })?;
                    Ok(PersistedDurableFunctionInvocation {
                        timestamp,
                        function_name,
                        response,
                        function_type,
                        oplog_entry_version: OplogEntryVersion::V2,
                    })
                }
                Resolution::Cancelled { cancelled_idx, .. } => {
                    Err(WorkerExecutorError::unexpected_oplog_entry(
                        "End",
                        format!("Cancelled at {cancelled_idx}"),
                    ))
                }
            }
        }
    }

    async fn try_trigger_retry(
        &mut self,
        failure: Error,
        properties: RetryProperties,
    ) -> anyhow::Result<()> {
        let latest_status = self
            .public_state
            .worker()
            .get_non_detached_last_known_status()
            .await;
        let current_retry_point = self.state.effective_retry_point();

        // Resolve the matching named retry policy. The synthesized
        // default-from-config policy (with `Predicate::True`) is always
        // present in `named_retry_policies()`, so resolution finds at least
        // one match for every property set; there is no legacy `RetryConfig`
        // path any more.
        let policies = self.state.named_retry_policies().await;
        let named_policy =
            match NamedRetryPolicy::resolve_applicable_treating_missing_properties_as_no_match(
                &policies,
                &properties,
            ) {
                Ok(Some(named_policy)) => named_policy,
                Ok(None) => {
                    warn!(
                        retry_path = "host-trap",
                        "No named retry policy matched (including the synthesized default); giving up"
                    );
                    return Ok(());
                }
                Err(error) => {
                    warn!(
                        ?error,
                        retry_path = "host-trap",
                        "Failed resolving semantic host-trap retry policy; giving up"
                    );
                    return Ok(());
                }
            };

        let current_state = latest_status
            .current_retry_state
            .get(&current_retry_point)
            .cloned();
        let total_attempts = current_state.as_ref().map(|s| s.retry_count()).unwrap_or(0);

        match evaluate_named_policy_step_resetting_on_invalid_state(
            named_policy,
            &properties,
            current_state.as_ref(),
        ) {
            Ok((new_state, RetryVerdict::Retry(delay))) => {
                debug!(
                    retry_policy = %named_policy.name,
                    retry_path = "host-trap",
                    retry_policy_source = "worker-local",
                    retry_decision = "retry",
                    attempt = total_attempts + 1,
                    delay_ms = delay.as_millis() as u64,
                    "Semantic host-trap retry: triggering retry"
                );
                // Attach the resolved verdict to the failure so the
                // post-trap recovery path can honour it directly without
                // re-resolving from an impoverished trap context (e.g.
                // missing HTTP `status-code`).
                let payload = SemanticTrapRetryOverride {
                    policy_name: named_policy.name.clone(),
                    verdict: SemanticTrapRetryVerdict::Retry(delay),
                    retry_policy_state: new_state,
                };
                Err(anyhow::Error::new(SemanticTrapRetryOverrideMarker {
                    payload,
                    inner: failure,
                }))
            }
            Ok((_new_state, RetryVerdict::GiveUp)) => {
                debug!(
                    retry_policy = %named_policy.name,
                    retry_path = "host-trap",
                    retry_policy_source = "worker-local",
                    retry_decision = "give-up",
                    attempt = total_attempts + 1,
                    "Semantic host-trap retry: exhausted"
                );
                Ok(())
            }
            Ok((_new_state, RetryVerdict::Error(error))) => {
                warn!(
                    retry_policy = %named_policy.name,
                    ?error,
                    retry_path = "host-trap",
                    fallback_reason = "eval-error",
                    "Semantic host-trap retry evaluation returned an error verdict; giving up"
                );
                Ok(())
            }
            Err(error) => {
                warn!(
                    retry_policy = %named_policy.name,
                    ?error,
                    retry_path = "host-trap",
                    fallback_reason = "eval-error",
                    "Failed evaluating semantic host-trap retry policy; giving up"
                );
                Ok(())
            }
        }
    }

    fn mark_atomic_region_side_effect(&mut self) {
        self.state.mark_atomic_region_has_side_effects();
    }

    fn create_interrupt_signal(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
        self.execution_status
            .read()
            .unwrap()
            .create_await_interrupt_signal()
    }

    fn check_read_only_allows(&self, host_function: &str) -> Result<(), GolemSpecificWasmTrap> {
        DurableWorkerCtx::check_read_only_allows(self, host_function)
    }
}

#[derive(Debug)]
pub enum OplogEntryVersion {
    V1,
    V2,
}

/// Holds the in-function retry decision logic for a single durable host call, decoupled from how
/// the call's request/response is persisted or replayed. Both the legacy [`Durability`] and the
/// concurrent [`crate::durable_host::concurrent::CallHandle`] own one and route their
/// `try_trigger_retry*` methods through it, so the retry logic has a single home.
///
/// Every method takes `&mut impl DurabilityHost`, so the controller stays unit-testable against
/// `MockDurabilityHost` without a real `DurableWorkerCtx`.
pub struct InFunctionRetryController {
    function_type: DurableFunctionType,
    durable_execution_state: DurableExecutionState,
    retry_state: InFunctionRetryState,
    /// Fully-qualified host-function name used as the retry decision's function label.
    function_label: &'static str,
}

impl InFunctionRetryController {
    pub fn new(
        function_type: DurableFunctionType,
        durable_execution_state: DurableExecutionState,
        function_label: &'static str,
    ) -> Self {
        Self {
            function_type,
            durable_execution_state,
            retry_state: InFunctionRetryState::new(),
            function_label,
        }
    }

    pub fn function_type(&self) -> &DurableFunctionType {
        &self.function_type
    }

    pub fn durable_execution_state(&self) -> &DurableExecutionState {
        &self.durable_execution_state
    }

    pub fn is_live(&self) -> bool {
        self.durable_execution_state.is_live
    }

    /// Checks if the current retry policy allows more retries, and if yes, then returns
    /// with `Err(failure)`. This error should be directly returned from host function
    /// implementations, triggering a retry.
    ///
    /// If retrying is not possible, the function returns Ok(()) and the host function
    /// can continue persisting the failed result permanently.
    ///
    /// The `classify` closure inspects the error and returns a `HostFailureKind` which determines
    /// whether the error is wrapped as `AgentError::TransientError` or `AgentError::PermanentError`.
    ///
    /// When `Permanent`, the method returns `Ok(())` immediately (no retry, persist the failure).
    /// When `Transient`, the inner `try_trigger_retry` is called, and if it triggers a retry,
    /// the error is wrapped in a `ClassifiedHostError` so `TrapType::from_error` can detect it.
    pub async fn try_trigger_retry<Ok, Err: Display>(
        &self,
        ctx: &mut impl DurabilityHost,
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
    ) -> anyhow::Result<()> {
        self.try_trigger_retry_with_properties(ctx, result, classify, RetryProperties::new())
            .await
    }

    pub async fn try_trigger_retry_with_properties<Ok, Err: Display>(
        &self,
        ctx: &mut impl DurabilityHost,
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<()> {
        if let Err(err) = result {
            let kind = classify(err);
            match kind {
                HostFailureKind::Permanent => Ok(()),
                HostFailureKind::Transient => {
                    let message = err.to_string();
                    let failure = Error::new(ClassifiedHostError {
                        kind,
                        message: message.clone(),
                    });
                    let mut properties = properties;
                    properties.set("error-type", PredicateValue::Text("transient".to_string()));
                    ctx.try_trigger_retry(failure, properties).await
                }
            }
        } else {
            Ok(())
        }
    }

    /// Like `try_trigger_retry`, but supports in-function retries for eligible host functions.
    ///
    /// Returns:
    /// - `Ok(InternalRetryResult::Persist)` — persist the result (success or permanent/exhausted failure)
    /// - `Ok(InternalRetryResult::RetryInternally)` — caller should re-execute the operation
    /// - `Err(...)` — propagated trap (fallback to oplog replay, or interrupt)
    pub async fn try_trigger_retry_or_loop<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
    ) -> anyhow::Result<InternalRetryResult> {
        self.try_trigger_retry_or_loop_with_properties(
            ctx,
            result,
            classify,
            RetryProperties::new(),
        )
        .await
    }

    pub async fn try_trigger_retry_or_loop_with_properties<Ok, Err: Display>(
        &mut self,
        ctx: &mut (impl DurabilityHost + Sync),
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
        properties: RetryProperties,
    ) -> anyhow::Result<InternalRetryResult> {
        let err = match result {
            Ok(_) => return Ok(InternalRetryResult::Persist),
            Err(err) => err,
        };

        let kind = classify(err);

        let mut properties = properties;
        properties.set(
            "error-type",
            PredicateValue::Text(match kind {
                HostFailureKind::Transient => "transient".to_string(),
                HostFailureKind::Permanent => "permanent".to_string(),
            }),
        );

        if kind == HostFailureKind::Permanent {
            return Ok(InternalRetryResult::Persist);
        }

        if !self.is_eligible_for_internal_retry() {
            let message = err.to_string();
            let failure = Error::new(ClassifiedHostError { kind, message });
            ctx.try_trigger_retry(failure, properties).await?;
            return Ok(InternalRetryResult::Persist);
        }

        let decision = self
            .retry_state
            .decide_retry_with_properties(ctx, self.function_label, &properties)
            .await;

        match decision {
            AsyncRetryDecision::RetryAfterDelay(delay) => {
                // Interrupt-aware sleep
                let interrupt = ctx.create_interrupt_signal();
                let sleep = tokio::time::sleep(delay);
                tokio::pin!(sleep);

                match futures::future::select(sleep, interrupt).await {
                    futures::future::Either::Left((_done, _)) => {
                        Ok(InternalRetryResult::RetryInternally)
                    }
                    futures::future::Either::Right((interrupt_kind, _)) => {
                        Err(anyhow::Error::from(interrupt_kind))
                    }
                }
            }
            AsyncRetryDecision::Exhausted => Ok(InternalRetryResult::Persist),
            AsyncRetryDecision::FallBackToTrap => {
                let message = err.to_string();
                let failure = Error::new(ClassifiedHostError { kind, message });
                ctx.try_trigger_retry(failure, properties.clone()).await?;
                // If try_trigger_retry returned Ok, retries are exhausted — persist the failure
                Ok(InternalRetryResult::Persist)
            }
        }
    }

    /// Checks whether the current function type is eligible for in-function retry.
    fn is_eligible_for_internal_retry(&self) -> bool {
        match &self.function_type {
            DurableFunctionType::ReadRemote
            | DurableFunctionType::ReadLocal
            | DurableFunctionType::WriteLocal => true,
            DurableFunctionType::WriteRemote => self.durable_execution_state.assume_idempotence,
            DurableFunctionType::WriteRemoteBatched(_)
            | DurableFunctionType::WriteRemoteTransaction(_) => false,
        }
    }

    /// Whether a call of this function type may be safely re-executed when replay finds its
    /// host-call `Start` committed but its `End` missing (see
    /// [`crate::durable_host::concurrent::CallReplayOutcome::Incomplete`]). This is exactly the set
    /// that is eligible for in-function retry: reads and local/idempotent writes can be re-run
    /// without duplicating an external side effect; non-idempotent / batched / transaction writes
    /// cannot, and rely on durable-scope recovery instead.
    pub fn can_reexecute_on_incomplete_replay(&self) -> bool {
        self.is_eligible_for_internal_retry()
    }
}

/// Counts the number of `OplogEntry::Error` entries in the oplog whose `retry_from`
/// matches the given `retry_point`. This is used to initialize `base_retry_count`
/// when spawning background retry tasks, so that the retry budget accounts for
/// errors already recorded before the task was spawned.
pub async fn count_oplog_errors_for(
    oplog: &Arc<dyn crate::services::oplog::Oplog>,
    retry_point: OplogIndex,
) -> u32 {
    let len = oplog.length().await;
    if len == 0 {
        return 0;
    }
    let entries = oplog.read_many(OplogIndex::INITIAL, len).await;
    let mut count: u32 = 0;
    for entry in entries.values() {
        if let OplogEntry::Error { retry_from, .. } = entry
            && *retry_from == retry_point
        {
            count += 1;
        }
    }
    count
}

/// Implementation of [`InFunctionRetryHost`] for spawned background tasks (async RPC, HTTP).
///
/// Unlike `DurableWorkerCtx`, this does NOT read retry state from worker status
/// (which is invocation-scoped and can panic if detached). Instead it captures
/// the current retry policy state at spawn time from the worker's last known status.
///
/// Error entries are written through `Worker::add_and_commit_oplog` so that the
/// worker status tracker is properly updated (unlike raw `oplog.add_and_commit`).
pub struct TaskRetryContext<Ctx: WorkerCtx> {
    /// The oplog index that error entries reference as their `retry_from` point.
    pub retry_point: OplogIndex,
    /// Environment state service for lazy policy fetching
    pub environment_state_service: Arc<dyn EnvironmentStateService>,
    /// Environment ID for policy lookup
    pub environment_id: EnvironmentId,
    /// Default catch-all retry policy derived from GolemConfig (priority 0, Predicate::True)
    pub default_retry_policy: NamedRetryPolicy,
    /// Cached agent-config-derived retry policies (cheap, already computed)
    pub agent_config_retry_policies: Vec<NamedRetryPolicy>,
    /// Runtime overlay mutations (set/remove via guest API)
    pub runtime_retry_policy_mutations: BTreeMap<String, Option<NamedRetryPolicy>>,
    /// Maximum delay for in-function retries; delays exceeding this fall back to trap+replay.
    pub max_in_function_retry_delay: Duration,
    /// The current semantic retry policy state, if a named policy was previously selected.
    pub current_retry_policy_state: Option<RetryPolicyState>,
    /// Properties describing the error context (verb, URI, status code, etc.) for predicate evaluation.
    pub retry_properties: RetryProperties,
    /// Reference to the worker that owns this task.
    pub worker: Arc<crate::worker::Worker<Ctx>>,
}

#[async_trait]
impl<Ctx: WorkerCtx> InFunctionRetryHost for TaskRetryContext<Ctx> {
    fn in_atomic_region(&self) -> bool {
        // Spawned tasks are never inside atomic regions
        false
    }

    fn current_retry_point(&self) -> OplogIndex {
        self.retry_point
    }

    async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy> {
        // Fetch environment policies lazily (only called on error path)
        let environment_policies = self
            .environment_state_service
            .get_retry_policies(self.environment_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to fetch environment retry policies in task context: {e}");
                vec![]
            });

        let mut deduped = BTreeMap::new();
        deduped.insert(
            self.default_retry_policy.name.clone(),
            self.default_retry_policy.clone(),
        );
        for policy in &self.agent_config_retry_policies {
            deduped.insert(policy.name.clone(), policy.clone());
        }
        for policy in environment_policies {
            deduped.insert(policy.name.clone(), policy);
        }
        for (name, mutation) in &self.runtime_retry_policy_mutations {
            match mutation {
                Some(policy) => {
                    deduped.insert(name.clone(), policy.clone());
                }
                None => {
                    deduped.remove(name);
                }
            }
        }
        let mut policies: Vec<NamedRetryPolicy> = deduped.into_values().collect();
        policies.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.name.cmp(&b.name))
        });
        policies
    }

    async fn current_retry_state_for(&self, _retry_from: OplogIndex) -> Option<RetryPolicyState> {
        self.current_retry_policy_state.clone()
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        DurableExecutionState {
            is_live: true,
            persistence_level: PersistenceLevel::Smart,
            snapshotting_mode: None,
            assume_idempotence: true,
            max_in_function_retry_delay: self.max_in_function_retry_delay,
        }
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        retry_policy_state: Option<RetryPolicyState>,
    ) {
        use golem_common::model::oplog::AgentError;
        let entry = OplogEntry::error(
            AgentError::TransientError("in-function retry".to_string()),
            retry_from,
            false, // spawned tasks are never inside atomic regions
            retry_policy_state.clone(),
        );
        self.worker.add_and_commit_oplog(entry).await;

        self.current_retry_policy_state = retry_policy_state;
    }
}

/// Shared retry loop for spawned background tasks (async RPC, HTTP).
///
/// Runs `operation` in a loop, retrying on transient failures according to the
/// retry budget. Delegates retry decisions to `InFunctionRetryState::decide_retry`
/// via a `TaskRetryContext`, ensuring delay calculation, metric emission, and oplog
/// error writing are shared with the in-context host function retry path.
///
/// Returns:
/// - `Ok(value)` on success
/// - `Err(err)` when the error is permanent, retries are exhausted, or the
///   computed delay exceeds `max_delay`
///
/// For spawned tasks, `FallBackToTrap` from `decide_retry` means "stop inline
/// retry and return the error" — there is no WASM stack to trap.
pub async fn in_task_retry_loop<Ctx, T, E, C, Op, Fut, ISF, ISFut>(
    mut task_ctx: TaskRetryContext<Ctx>,
    classify: C,
    mut operation: Op,
    interrupt_signal_factory: ISF,
) -> Result<T, E>
where
    Ctx: WorkerCtx,
    E: Display,
    C: Fn(&E) -> HostFailureKind,
    Op: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    ISF: Fn() -> ISFut,
    ISFut: Future<Output = InterruptKind>,
{
    let mut retry_state = InFunctionRetryState::new();

    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                let error_kind = classify(&err);
                if error_kind == HostFailureKind::Permanent {
                    return Err(err);
                }

                let mut retry_properties = task_ctx.retry_properties.clone();
                retry_properties.set("error-type", PredicateValue::Text("transient".to_string()));

                let decision = retry_state
                    .decide_retry_with_properties(&mut task_ctx, "in-task", &retry_properties)
                    .await;

                match decision {
                    AsyncRetryDecision::RetryAfterDelay(delay) => {
                        let sleep = tokio::time::sleep(delay);
                        let interrupt = interrupt_signal_factory();
                        tokio::pin!(sleep);
                        tokio::pin!(interrupt);

                        match futures::future::select(sleep, interrupt).await {
                            futures::future::Either::Left((_done, _)) => {
                                // Sleep completed, continue retry loop
                            }
                            futures::future::Either::Right((_interrupt_kind, _)) => {
                                // Interrupted during backoff — return the last transient error
                                return Err(err);
                            }
                        }
                    }
                    AsyncRetryDecision::Exhausted | AsyncRetryDecision::FallBackToTrap => {
                        return Err(err);
                    }
                }
            }
        }
    }
}

pub enum LazyInitializedPollableEntry {
    Empty,
    Subscribed { pollable: Resource<DynPollable> },
}

#[async_trait]
impl Pollable for LazyInitializedPollableEntry {
    async fn ready(&mut self) {
        match self {
            LazyInitializedPollableEntry::Empty => {
                // Empty pollable is always ready
            }
            LazyInitializedPollableEntry::Subscribed { .. } => {
                unreachable!("The dynamic pollable override should prevent this from being called")
            }
        }
    }
}

impl DynamicPollable for LazyInitializedPollableEntry {
    fn override_index(&self) -> Option<u32> {
        match self {
            LazyInitializedPollableEntry::Empty => None,
            LazyInitializedPollableEntry::Subscribed { pollable } => Some(pollable.rep()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::HostPayloadPair;
    use golem_common::model::oplog::PersistenceLevel;
    use golem_common::model::oplog::host_functions::KeyvalueEventualGet;
    use golem_common::model::{NamedRetryPolicy, Predicate, PredicateValue, RetryPolicy};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_r::test;

    /// A mock DurabilityHost for testing `try_trigger_retry_or_loop` in isolation.
    struct MockDurabilityHost {
        in_atomic_region: bool,
        assume_idempotence: bool,
        max_in_function_retry_delay: Duration,
        named_retry_policies: Vec<NamedRetryPolicy>,
        oplog_retry_count: u32,
        /// Tracks how many times `append_retry_error_entry` was called.
        retry_entries_appended: u32,
        /// Tracks how many times `try_trigger_retry` was called (fallback path).
        trap_triggered_count: u32,
        /// If set, `create_interrupt_signal` resolves immediately with this kind.
        interrupt_signal: Option<InterruptKind>,
        /// Controls whether the interrupt fires before the sleep completes.
        /// When true, the interrupt resolves immediately.
        interrupt_armed: Arc<AtomicBool>,
        /// Tracks the latest retry policy state written via `append_retry_error_entry`.
        current_retry_policy_state: Option<RetryPolicyState>,
        /// When `Some`, `check_read_only_allows` fails with a `WorkerReadOnlyViolation`
        /// using the contained method name. Mirrors the real
        /// `DurableWorkerCtx::check_read_only_allows` behaviour.
        read_only_method: Option<String>,
    }

    impl MockDurabilityHost {
        fn new() -> Self {
            Self {
                in_atomic_region: false,
                assume_idempotence: true,
                max_in_function_retry_delay: Duration::from_secs(20),
                named_retry_policies: Vec::new(),
                oplog_retry_count: 0,
                retry_entries_appended: 0,
                trap_triggered_count: 0,
                interrupt_signal: None,
                interrupt_armed: Arc::new(AtomicBool::new(false)),
                current_retry_policy_state: None,
                read_only_method: None,
            }
        }

        fn durable_execution_state(&self) -> DurableExecutionState {
            DurableExecutionState {
                is_live: true,
                persistence_level: PersistenceLevel::Smart,
                snapshotting_mode: None,
                assume_idempotence: self.assume_idempotence,
                max_in_function_retry_delay: self.max_in_function_retry_delay,
            }
        }
    }

    #[async_trait]
    impl InFunctionRetryHost for MockDurabilityHost {
        fn in_atomic_region(&self) -> bool {
            self.in_atomic_region
        }

        fn current_retry_point(&self) -> OplogIndex {
            OplogIndex::INITIAL
        }

        async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy> {
            self.named_retry_policies.clone()
        }

        async fn current_retry_state_for(
            &self,
            _retry_from: OplogIndex,
        ) -> Option<RetryPolicyState> {
            if let Some(state) = &self.current_retry_policy_state {
                Some(state.clone())
            } else if self.oplog_retry_count > 0 {
                Some(RetryPolicyState::CountBox {
                    attempts: self.oplog_retry_count,
                    inner: Box::new(RetryPolicyState::Counter(self.oplog_retry_count)),
                })
            } else {
                None
            }
        }

        fn durable_execution_state(&self) -> DurableExecutionState {
            MockDurabilityHost::durable_execution_state(self)
        }

        async fn append_retry_error_entry(
            &mut self,
            _retry_from: OplogIndex,
            retry_policy_state: Option<RetryPolicyState>,
        ) {
            self.retry_entries_appended += 1;
            self.current_retry_policy_state = retry_policy_state;
        }
    }

    #[async_trait]
    impl DurabilityHost for MockDurabilityHost {
        fn observe_function_call(&self, _interface: &str, _function: &str) {}

        async fn begin_durable_function(
            &mut self,
            function_type: &DurableFunctionType,
            host_function: &str,
        ) -> Result<OplogIndex, WorkerExecutorError> {
            // Mirror the production read-only side-effect trap so tests can exercise the
            // single central enforcement point.
            if is_write_side_effect(function_type)
                && let Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation {
                    method,
                    host_function,
                }) = self.check_read_only_allows(host_function)
            {
                return Err(WorkerExecutorError::ReadOnlyViolation {
                    method,
                    host_function,
                });
            }
            Ok(OplogIndex::from_u64(1))
        }

        async fn end_durable_function(
            &mut self,
            _function_type: &DurableFunctionType,
            _begin_index: OplogIndex,
            _forced_commit: bool,
        ) -> Result<(), WorkerExecutorError> {
            Ok(())
        }

        async fn persist_durable_function_invocation(
            &self,
            _function_name: HostFunctionName,
            _request: &HostRequest,
            _response: &HostResponse,
            _function_type: DurableFunctionType,
        ) {
        }

        async fn read_persisted_durable_function_invocation(
            &mut self,
        ) -> Result<PersistedDurableFunctionInvocation, WorkerExecutorError> {
            Err(WorkerExecutorError::runtime("not implemented in mock"))
        }

        async fn try_trigger_retry(
            &mut self,
            _failure: Error,
            _properties: RetryProperties,
        ) -> anyhow::Result<()> {
            self.trap_triggered_count += 1;
            // Simulate: retries exhausted, persist the failure
            Ok(())
        }

        fn mark_atomic_region_side_effect(&mut self) {}

        fn check_read_only_allows(&self, host_function: &str) -> Result<(), GolemSpecificWasmTrap> {
            if let Some(method) = &self.read_only_method {
                Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation {
                    method: method.clone(),
                    host_function: host_function.to_string(),
                })
            } else {
                Ok(())
            }
        }

        fn create_interrupt_signal(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
            if let Some(kind) = self.interrupt_signal {
                // Resolve immediately
                Box::pin(async move { kind })
            } else {
                let armed = self.interrupt_armed.clone();
                Box::pin(async move {
                    loop {
                        if armed.load(Ordering::Acquire) {
                            return InterruptKind::Interrupt(Timestamp::now_utc());
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
            }
        }
    }

    /// Helper: creates an `InFunctionRetryController` for the given function type, using the
    /// fully-qualified host-function name of an arbitrary host-call pair as the retry label.
    async fn make_retry_controller(
        ctx: &mut MockDurabilityHost,
        function_type: DurableFunctionType,
    ) -> InFunctionRetryController {
        InFunctionRetryController::new(
            function_type,
            ctx.durable_execution_state(),
            <KeyvalueEventualGet as HostPayloadPair>::FQFN,
        )
    }

    /// Helper: opens a durable function the same way the host-call entry path does — observe the
    /// call, then run the central read-only side-effect check in `begin_durable_function`.
    async fn begin_durable_call(
        ctx: &mut MockDurabilityHost,
        function_type: DurableFunctionType,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        DurabilityHost::observe_function_call(
            ctx,
            <KeyvalueEventualGet as HostPayloadPair>::INTERFACE,
            <KeyvalueEventualGet as HostPayloadPair>::FUNCTION,
        );
        ctx.begin_durable_function(
            &function_type,
            <KeyvalueEventualGet as HostPayloadPair>::FQFN,
        )
        .await
    }

    // Test 1: In-function retry works for eligible operations
    #[test]
    async fn in_function_retry_returns_retry_internally_on_transient_error() {
        let mut ctx = MockDurabilityHost::new();
        ctx.named_retry_policies = vec![NamedRetryPolicy {
            name: "default".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::CountBox {
                max_retries: 5,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
            },
        }];
        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("connection refused".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::RetryInternally);
        assert_eq!(ctx.retry_entries_appended, 1);
        assert_eq!(controller.retry_state.retry_count(), 1);
        assert_eq!(ctx.trap_triggered_count, 0, "should NOT fall back to trap");
    }

    // Test 1b: Success returns Persist immediately
    #[test]
    async fn in_function_retry_returns_persist_on_success() {
        let mut ctx = MockDurabilityHost::new();
        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Ok("value".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.retry_entries_appended, 0);
        assert_eq!(controller.retry_state.retry_count(), 0);
    }

    // Test 1c: Permanent errors return Persist (no retry)
    #[test]
    async fn in_function_retry_returns_persist_on_permanent_error() {
        let mut ctx = MockDurabilityHost::new();
        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("invalid key format".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Permanent,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.retry_entries_appended, 0);
        assert_eq!(ctx.trap_triggered_count, 0);
    }

    // Test 1d: Multiple retries exhaust budget, then returns Persist
    #[test]
    async fn in_function_retry_exhausts_budget_then_persists() {
        let mut ctx = MockDurabilityHost::new();
        ctx.named_retry_policies = vec![NamedRetryPolicy {
            name: "default".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::CountBox {
                max_retries: 3,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
            },
        }];
        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());

        // Attempts 0, 1, 2 should return RetryInternally
        for i in 0..3 {
            let action = controller
                .try_trigger_retry_or_loop_with_properties(
                    &mut ctx,
                    &result,
                    |_| HostFailureKind::Transient,
                    RetryProperties::new(),
                )
                .await
                .expect("should not propagate error");
            assert_eq!(
                action,
                InternalRetryResult::RetryInternally,
                "attempt {i} should retry"
            );
        }

        // Attempt 3 should return Persist (budget exhausted)
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error");
        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.retry_entries_appended, 3);
        assert_eq!(controller.retry_state.retry_count(), 3);
    }

    #[test]
    async fn named_retry_policy_match_overrides_legacy_retry_budget() {
        let mut ctx = MockDurabilityHost::new();
        ctx.named_retry_policies = vec![NamedRetryPolicy {
            name: "rpc-transient".to_string(),
            priority: 100,
            predicate: Predicate::PropEq {
                property: "verb".to_string(),
                value: PredicateValue::Text("invoke".to_string()),
            },
            policy: RetryPolicy::CountBox {
                max_retries: 1,
                inner: Box::new(RetryPolicy::Immediate),
            },
        }];

        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;
        let mut props = RetryProperties::new();
        props.set("verb", PredicateValue::Text("invoke".to_string()));

        let result: Result<String, String> = Err("timeout".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                props,
            )
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::RetryInternally);
        assert_eq!(ctx.retry_entries_appended, 1);
    }

    #[test]
    async fn named_retry_policy_no_match_returns_exhausted() {
        let mut ctx = MockDurabilityHost::new();
        ctx.named_retry_policies = vec![NamedRetryPolicy {
            name: "rpc-transient".to_string(),
            priority: 100,
            predicate: Predicate::PropEq {
                property: "verb".to_string(),
                value: PredicateValue::Text("invoke".to_string()),
            },
            policy: RetryPolicy::Immediate,
        }];

        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;
        let mut props = RetryProperties::new();
        props.set("verb", PredicateValue::Text("query".to_string()));

        let result: Result<String, String> = Err("timeout".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                props,
            )
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.retry_entries_appended, 0);
    }

    #[test]
    async fn semantic_policy_resolution_error_returns_exhausted() {
        let mut ctx = MockDurabilityHost::new();
        ctx.named_retry_policies = vec![NamedRetryPolicy {
            name: "invalid-for-props".to_string(),
            priority: 100,
            predicate: Predicate::PropEq {
                property: "attempt".to_string(),
                value: PredicateValue::Integer(1),
            },
            policy: RetryPolicy::Never,
        }];

        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;
        let mut props = RetryProperties::new();
        // This type mismatch forces a coercion error during predicate evaluation.
        props.set("attempt", PredicateValue::Boolean(true));

        let result: Result<String, String> = Err("timeout".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                props,
            )
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.retry_entries_appended, 0);
    }

    #[test]
    async fn elapsed_time_policy_returns_exhausted() {
        let mut ctx = MockDurabilityHost::new();
        ctx.named_retry_policies = vec![NamedRetryPolicy {
            name: "time-boxed".to_string(),
            priority: 100,
            predicate: Predicate::PropEq {
                property: "verb".to_string(),
                value: PredicateValue::Text("invoke".to_string()),
            },
            policy: RetryPolicy::TimeBox {
                limit: Duration::ZERO,
                inner: Box::new(RetryPolicy::Immediate),
            },
        }];

        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;
        let mut props = RetryProperties::new();
        props.set("verb", PredicateValue::Text("invoke".to_string()));

        let result: Result<String, String> = Err("timeout".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                props,
            )
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.retry_entries_appended, 0);
    }

    // Test 1e: WriteRemote with idempotence is eligible
    #[test]
    async fn in_function_retry_eligible_for_write_remote_with_idempotence() {
        let mut ctx = MockDurabilityHost::new();
        ctx.assume_idempotence = true;
        ctx.named_retry_policies = vec![NamedRetryPolicy {
            name: "default".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::CountBox {
                max_retries: 5,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
            },
        }];
        let mut controller =
            make_retry_controller(&mut ctx, DurableFunctionType::WriteRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::RetryInternally);
        assert_eq!(ctx.trap_triggered_count, 0);
    }

    // Test 2: Fallback to trap when delay exceeds max_in_function_retry_delay
    #[test]
    async fn fallback_to_trap_when_delay_exceeds_threshold() {
        let mut ctx = MockDurabilityHost::new();
        ctx.named_retry_policies = vec![NamedRetryPolicy {
            name: "default".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::Periodic(Duration::from_secs(30)),
        }];
        ctx.max_in_function_retry_delay = Duration::from_secs(20);
        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error (mock try_trigger_retry returns Ok)");

        // Should have fallen back to trap+replay path
        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(
            ctx.trap_triggered_count, 1,
            "should fall back to trap+replay"
        );
        assert_eq!(
            ctx.retry_entries_appended, 0,
            "should NOT append in-function retry entry"
        );
    }

    // Test 3: Atomic regions disable in-function retry
    #[test]
    async fn atomic_region_disables_in_function_retry() {
        let mut ctx = MockDurabilityHost::new();
        ctx.in_atomic_region = true;
        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("connection refused".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error (mock try_trigger_retry returns Ok)");

        // Should fall back to trap+replay because we're in an atomic region
        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(
            ctx.trap_triggered_count, 1,
            "should fall back to trap+replay"
        );
        assert_eq!(
            ctx.retry_entries_appended, 0,
            "should NOT append in-function retry entry"
        );
    }

    // Test 3b: WriteRemoteBatched is ineligible (always falls back to trap)
    #[test]
    async fn batched_write_disables_in_function_retry() {
        let mut ctx = MockDurabilityHost::new();
        let mut controller =
            make_retry_controller(&mut ctx, DurableFunctionType::WriteRemoteBatched(None)).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.trap_triggered_count, 1);
    }

    // Test 3c: WriteRemoteTransaction is ineligible
    #[test]
    async fn transaction_write_disables_in_function_retry() {
        let mut ctx = MockDurabilityHost::new();
        let mut controller =
            make_retry_controller(&mut ctx, DurableFunctionType::WriteRemoteTransaction(None))
                .await;

        let result: Result<String, String> = Err("timeout".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.trap_triggered_count, 1);
    }

    // Test 3d: WriteRemote without idempotence falls back to trap
    #[test]
    async fn write_remote_without_idempotence_disables_in_function_retry() {
        let mut ctx = MockDurabilityHost::new();
        ctx.assume_idempotence = false;
        let mut controller =
            make_retry_controller(&mut ctx, DurableFunctionType::WriteRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.trap_triggered_count, 1);
    }

    // Test 4: Interrupt during in-function retry sleep
    #[test]
    async fn interrupt_during_in_function_retry_sleep_propagates_error() {
        let mut ctx = MockDurabilityHost::new();
        ctx.named_retry_policies = vec![NamedRetryPolicy {
            name: "default".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::Periodic(Duration::from_secs(10)),
        }];
        // But still within the threshold so we don't fall back to trap
        ctx.max_in_function_retry_delay = Duration::from_secs(20);
        // Interrupt fires immediately
        ctx.interrupt_signal = Some(InterruptKind::Interrupt(Timestamp::now_utc()));

        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let err = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect_err("should propagate interrupt as error");

        // The error should be an InterruptKind
        assert!(
            err.downcast_ref::<InterruptKind>().is_some(),
            "error should be InterruptKind, got: {err}"
        );
        // The retry entry should still have been appended before the sleep
        assert_eq!(ctx.retry_entries_appended, 1);
    }

    // Test 4b: Interrupt via armed flag during sleep
    #[test]
    async fn interrupt_armed_during_sleep_propagates() {
        let mut ctx = MockDurabilityHost::new();
        ctx.named_retry_policies = vec![NamedRetryPolicy {
            name: "default".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::Periodic(Duration::from_millis(500)),
        }];
        ctx.max_in_function_retry_delay = Duration::from_secs(20);
        // Arm the interrupt to fire via the polling loop
        ctx.interrupt_armed.store(true, Ordering::Release);

        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let err = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect_err("should propagate interrupt as error");

        assert!(err.downcast_ref::<InterruptKind>().is_some());
    }

    // Test: oplog retry count is combined with in-function retry count
    #[test]
    async fn oplog_retry_count_combined_with_in_function_retry_count() {
        let mut ctx = MockDurabilityHost::new();
        ctx.named_retry_policies = vec![NamedRetryPolicy {
            name: "default".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::CountBox {
                max_retries: 5,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
            },
        }];
        // Simulate 3 prior oplog-level retries
        ctx.oplog_retry_count = 3;
        let mut controller = make_retry_controller(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());

        // With 3 oplog retries, total=3. max_retries=5.
        // Attempt at total=3 → ok (within budget)
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error");
        assert_eq!(action, InternalRetryResult::RetryInternally);

        // Attempt at total=4 → ok (within budget)
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error");
        assert_eq!(action, InternalRetryResult::RetryInternally);

        // Attempt at total=5 → exhausted (5 >= max_retries)
        let action = controller
            .try_trigger_retry_or_loop_with_properties(
                &mut ctx,
                &result,
                |_| HostFailureKind::Transient,
                RetryProperties::new(),
            )
            .await
            .expect("should not propagate error");
        assert_eq!(action, InternalRetryResult::Persist);
    }

    // Test: decide_retry_for_named_policy uses the supplied policy directly,
    // bypassing the resolver. Even if `named_retry_policies` would have selected a
    // different (e.g. synthesized default) policy for the same properties,
    // `decide_retry_for_named_policy` honors the explicit `named_policy`.
    #[test]
    async fn decide_retry_for_named_policy_uses_supplied_policy() {
        let mut ctx = MockDurabilityHost::new();
        // Resolver-visible policies include only a "would-be-default" that exhausts
        // immediately. If the helper re-resolved, no retry would happen.
        ctx.named_retry_policies = vec![NamedRetryPolicy {
            name: "default".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::CountBox {
                max_retries: 0,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
            },
        }];

        // The explicitly supplied policy permits one retry.
        let explicit = NamedRetryPolicy {
            name: "explicit".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::CountBox {
                max_retries: 1,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
            },
        };

        let mut state = InFunctionRetryState::new();
        let decision = state
            .decide_retry_for_named_policy(&mut ctx, "test-fn", &RetryProperties::new(), &explicit)
            .await;

        match decision {
            AsyncRetryDecision::RetryAfterDelay(_) => {}
            other => panic!("expected RetryAfterDelay, got {other:?}"),
        }
        assert_eq!(ctx.retry_entries_appended, 1);
        assert_eq!(state.retry_count(), 1);
    }

    // Test: decide_retry_for_named_policy honors atomic-region as FallBackToTrap.
    #[test]
    async fn decide_retry_for_named_policy_falls_back_in_atomic_region() {
        let mut ctx = MockDurabilityHost::new();
        ctx.in_atomic_region = true;

        let explicit = NamedRetryPolicy {
            name: "explicit".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::CountBox {
                max_retries: 5,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
            },
        };

        let mut state = InFunctionRetryState::new();
        let decision = state
            .decide_retry_for_named_policy(&mut ctx, "test-fn", &RetryProperties::new(), &explicit)
            .await;

        assert!(matches!(decision, AsyncRetryDecision::FallBackToTrap));
        assert_eq!(ctx.retry_entries_appended, 0);
    }

    // Test: decide_retry_for_named_policy returns Exhausted when the supplied
    // policy has no remaining attempts.
    #[test]
    async fn decide_retry_for_named_policy_exhausted_when_supplied_policy_gives_up() {
        let mut ctx = MockDurabilityHost::new();

        let explicit = NamedRetryPolicy {
            name: "explicit".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::CountBox {
                max_retries: 0,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
            },
        };

        let mut state = InFunctionRetryState::new();
        let decision = state
            .decide_retry_for_named_policy(&mut ctx, "test-fn", &RetryProperties::new(), &explicit)
            .await;

        assert!(matches!(decision, AsyncRetryDecision::Exhausted));
        assert_eq!(ctx.retry_entries_appended, 0);
    }

    // Regression reproducer: status-code retry decisions are keyed on the HTTP
    // request's begin index, which can already contain retry state written by a
    // different policy shape (for example a transport-error/default policy). The
    // explicit status-code policy must reset on that shape mismatch instead of
    // treating `InvalidState` as exhausted.
    #[test]
    async fn decide_retry_for_named_policy_resets_stale_state_shape() {
        let mut ctx = MockDurabilityHost::new();
        ctx.current_retry_policy_state = Some(RetryPolicyState::CountBox {
            attempts: 1,
            inner: Box::new(RetryPolicyState::Wrapper(Box::new(
                RetryPolicyState::Counter(1),
            ))),
        });

        let status_policy = NamedRetryPolicy {
            name: "http-5xx".to_string(),
            priority: 100,
            predicate: Predicate::True,
            policy: RetryPolicy::CountBox {
                max_retries: 5,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(1))),
            },
        };

        let mut state = InFunctionRetryState::new();
        let decision = state
            .decide_retry_for_named_policy(
                &mut ctx,
                "http-status-retry",
                &RetryProperties::new(),
                &status_policy,
            )
            .await;

        assert!(
            matches!(decision, AsyncRetryDecision::RetryAfterDelay(_)),
            "stale retry state from a different policy shape must not exhaust the status policy; got {decision:?}"
        );
        assert_eq!(ctx.retry_entries_appended, 1);
    }

    // Test: is_eligible_for_internal_retry covers all DurableFunctionType variants
    #[test]
    fn eligibility_matrix() {
        let cases = vec![
            (DurableFunctionType::ReadRemote, true, true),
            (DurableFunctionType::ReadRemote, false, true),
            (DurableFunctionType::ReadLocal, true, true),
            (DurableFunctionType::ReadLocal, false, true),
            (DurableFunctionType::WriteLocal, true, true),
            (DurableFunctionType::WriteLocal, false, true),
            (DurableFunctionType::WriteRemote, true, true),
            (DurableFunctionType::WriteRemote, false, false),
            (DurableFunctionType::WriteRemoteBatched(None), true, false),
            (DurableFunctionType::WriteRemoteBatched(None), false, false),
            (
                DurableFunctionType::WriteRemoteTransaction(None),
                true,
                false,
            ),
            (
                DurableFunctionType::WriteRemoteTransaction(None),
                false,
                false,
            ),
        ];

        for (ft, idempotent, expected) in cases {
            let ctx = MockDurabilityHost {
                assume_idempotence: idempotent,
                ..MockDurabilityHost::new()
            };
            let controller = InFunctionRetryController::new(
                ft.clone(),
                ctx.durable_execution_state(),
                KeyvalueEventualGet::FQFN,
            );
            assert_eq!(
                controller.is_eligible_for_internal_retry(),
                expected,
                "ft={ft:?}, idempotent={idempotent}"
            );
        }
    }

    /// Bug B reproducer: when the in-function HTTP status retry exhausts
    /// (so the guest finally sees the 500 and traps), the leftover
    /// `CountBox{Counter}` retry state at the request's begin index must
    /// not poison the trap-retry path's evaluation of a *different*
    /// policy (the catch-all `default` policy synthesised from
    /// `RetryConfig`, which has shape `CountBox{Jitter{Clamp{Exp}}}`).
    ///
    /// Without the `evaluate_named_policy_step_resetting_on_invalid_state`
    /// guard, evaluation produces `RetryVerdict::Error(InvalidState ..)`
    /// and the trap path silently falls back to the legacy retry config
    /// instead of using the user-configured (or default) named policy.
    #[test]
    fn evaluate_named_policy_step_resets_on_state_shape_mismatch() {
        use golem_common::model::RetryConfig;

        let default_policy = NamedRetryPolicy::default_from_config(&RetryConfig::max_attempts_5());

        // Simulate the leftover state from an exhausted inline status
        // retry: its inner state is a bare `Counter`, while the default
        // policy's inner is a `Wrapper` chain.
        let stale_state = RetryPolicyState::CountBox {
            attempts: 2,
            inner: Box::new(RetryPolicyState::Counter(2)),
        };
        let properties = RetryProperties::new();

        // Without the guard: evaluation surfaces InvalidState as a
        // verdict (which the trap-retry call site treats as a fall-back
        // signal to the legacy retry config).
        let baseline = evaluate_named_policy_step(&default_policy, &properties, Some(&stale_state))
            .expect("evaluate must not error at the Result layer");
        assert!(
            matches!(
                baseline.1,
                RetryVerdict::Error(RetryEvaluationError::InvalidState { .. })
            ),
            "baseline must produce InvalidState verdict, got {:?}",
            baseline.1
        );

        // With the guard: evaluation transparently re-evaluates from an
        // empty state, producing a usable Retry verdict.
        let guarded = evaluate_named_policy_step_resetting_on_invalid_state(
            &default_policy,
            &properties,
            Some(&stale_state),
        )
        .expect("evaluate must not error at the Result layer");
        assert!(
            matches!(guarded.1, RetryVerdict::Retry(_)),
            "guarded evaluation must produce a Retry verdict, got {:?}",
            guarded.1
        );

        // The guard MUST NOT change behaviour when the state shape
        // matches: the existing accumulator must be preserved (otherwise
        // it would be a regression in normal retry accounting).
        let valid_state = default_policy.policy.initial_state();
        let direct = evaluate_named_policy_step(&default_policy, &properties, Some(&valid_state))
            .expect("evaluate must not error");
        let via_guard = evaluate_named_policy_step_resetting_on_invalid_state(
            &default_policy,
            &properties,
            Some(&valid_state),
        )
        .expect("evaluate must not error");
        assert!(matches!(direct.1, RetryVerdict::Retry(_)));
        assert!(matches!(via_guard.1, RetryVerdict::Retry(_)));
        assert_eq!(
            format!("{:?}", direct.0),
            format!("{:?}", via_guard.0),
            "guard must not perturb state when shape matches"
        );
    }

    /// Round-trip the `SemanticTrapRetryOverrideMarker` through an
    /// `anyhow::Error`: the marker must be discoverable via
    /// `find_semantic_trap_retry_override`, and the inner error must still
    /// be discoverable via the chain (because the marker exposes it as
    /// `source()`). Also verifies that the carried verdict — including the
    /// already-jittered delay and the policy state's attempt count — is
    /// returned verbatim, with no re-rolling on the trap path.
    #[test]
    fn semantic_trap_retry_override_round_trips_through_anyhow_error() {
        // Use a non-round delay value to make accidental re-rolling
        // (which would clobber jitter) easy to spot.
        let jittered_delay = Duration::from_millis(1873);
        let payload = SemanticTrapRetryOverride {
            policy_name: "manifest-5xx-retry".to_string(),
            verdict: SemanticTrapRetryVerdict::Retry(jittered_delay),
            retry_policy_state: RetryPolicyState::CountBox {
                attempts: 4,
                inner: Box::new(RetryPolicyState::Counter(4)),
            },
        };

        let inner = anyhow::Error::new(ClassifiedHostError {
            kind: HostFailureKind::Transient,
            message: "HTTP response status 500 matched user-defined retry policy".to_string(),
        });

        let with_marker = anyhow::Error::new(SemanticTrapRetryOverrideMarker {
            payload: payload.clone(),
            inner,
        });

        // (1) Marker is found verbatim.
        let extracted = find_semantic_trap_retry_override(&with_marker)
            .expect("override marker must round-trip through anyhow chain");
        assert_eq!(extracted.policy_name, payload.policy_name);
        assert_eq!(extracted.verdict, payload.verdict);
        assert_eq!(
            format!("{:?}", extracted.retry_policy_state),
            format!("{:?}", payload.retry_policy_state),
            "retry policy state must round-trip verbatim"
        );

        // (2) Wrapping in marker MUST NOT hide the inner classified error
        //     — `TrapType::from_error` walks the same chain to classify
        //     the failure (transient vs. permanent), so this lookup must
        //     keep working.
        let inner_classified = with_marker
            .chain()
            .find_map(|e| e.downcast_ref::<ClassifiedHostError>())
            .expect("inner ClassifiedHostError must still be reachable via chain()");
        assert_eq!(inner_classified.kind, HostFailureKind::Transient);
        assert_eq!(
            inner_classified.message,
            "HTTP response status 500 matched user-defined retry policy"
        );
    }

    /// Adding an `anyhow::Error::context(...)` wrapper on top of the
    /// marker (e.g. by an error-handling layer between the host call and
    /// the trap path) must NOT hide the marker from
    /// `find_semantic_trap_retry_override`. Walking `.chain()` and
    /// downcasting on each `&dyn Error` must still find it.
    #[test]
    fn semantic_trap_retry_override_survives_anyhow_context_wrapper() {
        let payload = SemanticTrapRetryOverride {
            policy_name: "manifest-5xx-retry".to_string(),
            verdict: SemanticTrapRetryVerdict::Retry(Duration::from_secs(2)),
            retry_policy_state: RetryPolicyState::CountBox {
                attempts: 1,
                inner: Box::new(RetryPolicyState::Counter(1)),
            },
        };

        let inner = anyhow::Error::new(ClassifiedHostError {
            kind: HostFailureKind::Transient,
            message: "boom".to_string(),
        });

        let with_marker = anyhow::Error::new(SemanticTrapRetryOverrideMarker {
            payload: payload.clone(),
            inner,
        });

        // Add an arbitrary context layer on top.
        let wrapped = with_marker.context("escalated through outer layer");

        let extracted = find_semantic_trap_retry_override(&wrapped)
            .expect("override marker must survive an outer context wrapper");
        assert_eq!(extracted.policy_name, payload.policy_name);
        assert_eq!(extracted.verdict, payload.verdict);
    }

    /// `find_semantic_trap_retry_override` must return `None` for a plain
    /// failure that has no marker attached — this is the "guest-originated
    /// trap" path where the trap recovery must fall through to the
    /// rich-context resolver and not silently honour a stale override.
    #[test]
    fn find_semantic_trap_retry_override_returns_none_when_marker_absent() {
        let bare = anyhow::Error::new(ClassifiedHostError {
            kind: HostFailureKind::Transient,
            message: "generic failure".to_string(),
        });
        assert!(find_semantic_trap_retry_override(&bare).is_none());

        let with_context = bare.context("escalated to trap");
        assert!(find_semantic_trap_retry_override(&with_context).is_none());
    }

    // ---------------------------------------------------------------------
    // Generic read-only side-effect trap (begin_durable_function)
    // ---------------------------------------------------------------------

    /// Read function types must always succeed, regardless of whether the
    /// mock currently models a read-only agent method. Only `Write*`
    /// variants must be gated by `check_read_only_allows`.
    #[test]
    async fn read_only_check_does_not_trip_for_read_function_types() {
        for ft in [
            DurableFunctionType::ReadLocal,
            DurableFunctionType::ReadRemote,
        ] {
            let mut ctx = MockDurabilityHost::new();
            ctx.read_only_method = Some("read-only-method".to_string());
            let result = begin_durable_call(&mut ctx, ft.clone()).await;
            assert!(
                result.is_ok(),
                "read-only check must not trip for {ft:?}, got {:?}",
                result.err()
            );
        }
    }

    /// When the worker is NOT in read-only mode, every `Write*` function
    /// type must open the durable function successfully — the new generic
    /// check must be a strict precondition added on top of the existing
    /// behaviour, not a new constraint that always fires.
    #[test]
    async fn write_function_types_succeed_when_not_in_read_only_mode() {
        for ft in [
            DurableFunctionType::WriteLocal,
            DurableFunctionType::WriteRemote,
            DurableFunctionType::WriteRemoteBatched(None),
            DurableFunctionType::WriteRemoteTransaction(None),
        ] {
            let mut ctx = MockDurabilityHost::new();
            // read_only_method left as None.
            let result = begin_durable_call(&mut ctx, ft.clone()).await;
            assert!(
                result.is_ok(),
                "non-read-only worker must accept {ft:?}, got {:?}",
                result.err()
            );
        }
    }

    /// In read-only mode, every `Write*` function type must be refused via
    /// `WorkerExecutorError::ReadOnlyViolation`, carrying:
    ///   - the agent method name supplied by the host context, and
    ///   - the fully qualified host-function name (`Pair::FQFN`) of the
    ///     attempted host call.
    /// This is the core property the generic check is supposed to enforce
    /// uniformly across every side-effecting host function path.
    #[test]
    async fn write_function_types_trap_with_read_only_violation_in_read_only_mode() {
        for ft in [
            DurableFunctionType::WriteLocal,
            DurableFunctionType::WriteRemote,
            DurableFunctionType::WriteRemoteBatched(None),
            DurableFunctionType::WriteRemoteTransaction(None),
        ] {
            let mut ctx = MockDurabilityHost::new();
            ctx.read_only_method = Some("agent::read-only-method".to_string());

            let result = begin_durable_call(&mut ctx, ft.clone()).await;
            let err = match result {
                Ok(_) => panic!("read-only worker must refuse {ft:?} via begin_durable_function"),
                Err(e) => e,
            };

            match err {
                WorkerExecutorError::ReadOnlyViolation {
                    method,
                    host_function,
                } => {
                    assert_eq!(method, "agent::read-only-method");
                    assert_eq!(
                        host_function,
                        <KeyvalueEventualGet as HostPayloadPair>::FQFN,
                        "host_function must be Pair::FQFN (got {host_function})"
                    );
                }
                other => panic!(
                    "expected WorkerExecutorError::ReadOnlyViolation for {ft:?}, got {other:?}"
                ),
            }
        }
    }
}
