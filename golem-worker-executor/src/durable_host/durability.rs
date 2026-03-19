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
use crate::metrics::wasm::{record_host_function_call, record_in_function_retry};
use crate::model::TrapType;
use crate::preview2::golem::durability::durability;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::services::{HasOplog, HasWorker};
use crate::worker::RetryDecision;
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostResponse, OplogEntry, OplogIndex,
    PersistenceLevel,
};
use golem_common::model::{RetryConfig, Timestamp};
use golem_common::retries::get_delay;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_wasm::IntoValueAndType;
use std::fmt::{Debug, Display};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tracing::{debug, error};
use wasmtime::component::Resource;
use wasmtime_wasi::{dynamic_subscribe, DynPollable, DynamicPollable, Pollable};

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

    /// Decides whether an in-function retry should happen based on the current retry budget,
    /// atomic region status, and delay threshold.
    ///
    /// On `RetryAfterDelay`, increments the retry counter, writes an oplog error entry,
    /// emits a debug log, and records the metric.
    pub async fn decide_retry(
        &mut self,
        ctx: &mut impl DurabilityHost,
        function_label: &str,
    ) -> AsyncRetryDecision {
        if ctx.in_atomic_region() {
            return AsyncRetryDecision::FallBackToTrap;
        }

        let retry_point = ctx.current_retry_point();
        let retry_config = ctx.retry_config();
        let oplog_retry_count = ctx.current_retry_count_for(retry_point).await;
        let total_attempts = self.retry_count + oplog_retry_count;

        let delay = match get_delay(&retry_config, total_attempts) {
            Some(delay) => delay,
            None => {
                return AsyncRetryDecision::Exhausted;
            }
        };

        let state = ctx.durable_execution_state();
        if delay > state.max_in_function_retry_delay {
            return AsyncRetryDecision::FallBackToTrap;
        }

        ctx.append_retry_error_entry(retry_point).await;
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
pub trait DurabilityHost {
    /// Observes a function call (produces logs and metrics)
    fn observe_function_call(&self, interface: &str, function: &str);

    /// Marks the beginning of a durable function.
    ///
    /// There must be a corresponding call to `end_durable_function` after the function has
    /// performed its work (it can be ended in a different context, for example, after an async
    /// pollable operation has been completed)
    async fn begin_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
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

    /// Gets the current durable execution state
    fn durable_execution_state(&self) -> DurableExecutionState;

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
    async fn try_trigger_retry(&mut self, failure: Error) -> anyhow::Result<()>;

    /// Marks the outermost active atomic region (if any) as having produced side effects.
    /// This is called when a non-hint oplog entry is persisted during live execution.
    fn mark_atomic_region_side_effect(&mut self);

    /// Returns true if the worker is currently inside a user-defined atomic region.
    fn in_atomic_region(&self) -> bool;

    /// Creates an interrupt signal future that resolves when the worker is interrupted/suspended/etc.
    fn create_interrupt_signal(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>>;

    /// Writes an `OplogEntry::Error` entry for an in-function retry attempt, and commits.
    async fn append_retry_error_entry(&mut self, retry_from: OplogIndex);

    /// Returns the current retry count for a given oplog index from the worker's status record.
    async fn current_retry_count_for(&self, retry_from: OplogIndex) -> u32;

    /// Returns the current retry point — the oplog index that Error entries should reference
    /// as `retry_from`. This is stable across trap+replay cycles for the same host function
    /// invocation, unlike `begin_index` which may advance past Error entries after replay.
    fn current_retry_point(&self) -> OplogIndex;

    /// Returns the current retry configuration (overridden or default).
    fn retry_config(&self) -> RetryConfig;
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
        let oplog_idx = DurabilityHost::begin_durable_function(self, &function_type.into()).await?;
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
        let state = DurabilityHost::durable_execution_state(self);
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
impl<Ctx: WorkerCtx> DurabilityHost for DurableWorkerCtx<Ctx> {
    fn observe_function_call(&self, interface: &str, function: &str) {
        record_host_function_call(interface, function);
    }

    async fn begin_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
    ) -> Result<OplogIndex, WorkerExecutorError> {
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
        }
        Ok(())
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

    async fn persist_durable_function_invocation(
        &self,
        function_name: HostFunctionName,
        request: &HostRequest,
        response: &HostResponse,
        function_type: DurableFunctionType,
    ) {
        self.public_state
            .worker()
            .oplog()
            .add_host_call(function_name, request, response, function_type)
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
            let (_, oplog_entry) =
                crate::get_oplog_entry!(self.state.replay_state, OplogEntry::HostCall)?;
            match oplog_entry {
                OplogEntry::HostCall {
                    timestamp,
                    function_name,
                    durable_function_type,
                    response,
                    ..
                } => {
                    let response = self
                        .public_state
                        .worker()
                        .oplog()
                        .download_payload(response)
                        .await
                        .map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "HostCall payload cannot be downloaded: {err}"
                            ))
                        })?;
                    Ok(PersistedDurableFunctionInvocation {
                        timestamp,
                        function_name: function_name.to_string(),
                        response,
                        function_type: durable_function_type,
                        oplog_entry_version: OplogEntryVersion::V2,
                    })
                }
                _ => Err(WorkerExecutorError::unexpected_oplog_entry(
                    "HostCall",
                    format!("{oplog_entry:?}"),
                )),
            }
        }
    }

    async fn try_trigger_retry(&mut self, failure: Error) -> anyhow::Result<()> {
        let latest_status = self
            .public_state
            .worker()
            .get_non_detached_last_known_status()
            .await;
        let current_retry_point = if let Some(region) = self.state.active_atomic_regions.last() {
            region.begin_index
        } else {
            self.state.current_retry_point
        };

        let default_retry_config = &self.state.config.retry;
        let retry_config = self
            .state
            .overridden_retry_policy
            .as_ref()
            .unwrap_or(default_retry_config)
            .clone();
        let in_atomic_region = !self.state.active_atomic_regions.is_empty();
        let trap_type = TrapType::from_error::<Ctx>(&failure, current_retry_point);
        let decision = Self::get_recovery_decision_on_trap(
            &retry_config,
            &latest_status.current_retry_count,
            &trap_type,
            in_atomic_region,
        );

        match decision {
            RetryDecision::Immediate
            | RetryDecision::Delayed(_)
            | RetryDecision::ReacquirePermits => Err(failure),
            RetryDecision::None | RetryDecision::TryStop(_) => Ok(()),
        }
    }

    fn mark_atomic_region_side_effect(&mut self) {
        self.state.mark_atomic_region_has_side_effects();
    }

    fn in_atomic_region(&self) -> bool {
        !self.state.active_atomic_regions.is_empty()
    }

    fn create_interrupt_signal(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
        self.execution_status
            .read()
            .unwrap()
            .create_await_interrupt_signal()
    }

    async fn append_retry_error_entry(&mut self, retry_from: OplogIndex) {
        use golem_common::model::oplog::AgentError;
        let inside_atomic_region = self.state.outermost_atomic_region_has_side_effects();
        let entry = OplogEntry::error(
            AgentError::TransientError("in-function retry".to_string()),
            retry_from,
            inside_atomic_region,
        );
        self.public_state.worker().add_and_commit_oplog(entry).await;
    }

    async fn current_retry_count_for(&self, retry_from: OplogIndex) -> u32 {
        let latest_status = self
            .public_state
            .worker()
            .get_non_detached_last_known_status()
            .await;
        latest_status
            .current_retry_count
            .get(&retry_from)
            .copied()
            .unwrap_or(0)
    }

    fn current_retry_point(&self) -> OplogIndex {
        self.state.current_retry_point
    }

    fn retry_config(&self) -> RetryConfig {
        self.state
            .overridden_retry_policy
            .as_ref()
            .unwrap_or(&self.state.config.retry)
            .clone()
    }
}

#[derive(Debug)]
pub enum OplogEntryVersion {
    V1,
    V2,
}

pub struct Durability<Pair: HostPayloadPair> {
    function_type: DurableFunctionType,
    begin_index: OplogIndex,
    durable_execution_state: DurableExecutionState,
    retry_state: InFunctionRetryState,
    _phantom: std::marker::PhantomData<Pair>,
}

impl<Pair: HostPayloadPair> Durability<Pair> {
    pub async fn new(
        ctx: &mut impl DurabilityHost,
        function_type: DurableFunctionType,
    ) -> Result<Self, WorkerExecutorError> {
        ctx.observe_function_call(Pair::INTERFACE, Pair::FUNCTION);

        let begin_index = ctx.begin_durable_function(&function_type).await?;
        let durable_execution_state = ctx.durable_execution_state();

        Ok(Self {
            function_type,
            begin_index,
            durable_execution_state,
            retry_state: InFunctionRetryState::new(),
            _phantom: std::marker::PhantomData,
        })
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
                    ctx.try_trigger_retry(failure).await
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
        ctx: &mut impl DurabilityHost,
        result: &Result<Ok, Err>,
        classify: impl Fn(&Err) -> HostFailureKind,
    ) -> anyhow::Result<InternalRetryResult> {
        let err = match result {
            Ok(_) => return Ok(InternalRetryResult::Persist),
            Err(err) => err,
        };

        let kind = classify(err);
        if kind == HostFailureKind::Permanent {
            return Ok(InternalRetryResult::Persist);
        }

        if !self.is_eligible_for_internal_retry() {
            let message = err.to_string();
            let failure = Error::new(ClassifiedHostError { kind, message });
            ctx.try_trigger_retry(failure).await?;
            return Ok(InternalRetryResult::Persist);
        }

        let decision = self.retry_state.decide_retry(ctx, Pair::FQFN).await;

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
                ctx.try_trigger_retry(failure).await?;
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

    pub async fn persist(
        &self,
        ctx: &mut impl DurabilityHost,
        request: Pair::Req,
        response: Pair::Resp,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        let response = self
            .persist_raw(ctx, request.into(), response.into())
            .await?;
        Ok(response.try_into().unwrap()) // Assuming converting to HostResponse and back always succeeds
    }

    pub async fn persist_raw(
        &self,
        ctx: &mut impl DurabilityHost,
        request: HostRequest,
        response: HostResponse,
    ) -> Result<HostResponse, WorkerExecutorError> {
        if self.durable_execution_state.snapshotting_mode.is_none() {
            ctx.mark_atomic_region_side_effect();
            ctx.persist_durable_function_invocation(
                Pair::HOST_FUNCTION_NAME,
                &request,
                &response,
                self.function_type.clone(),
            )
            .await;
            ctx.end_durable_function(&self.function_type, self.begin_index, false)
                .await?;
        }
        Ok(response)
    }

    pub async fn replay(
        &self,
        ctx: &mut impl DurabilityHost,
    ) -> Result<Pair::Resp, WorkerExecutorError> {
        let response = self.replay_raw(ctx).await?;
        response
            .try_into()
            .map_err(|err| WorkerExecutorError::unexpected_oplog_entry("HostResponse", err))
    }

    pub async fn replay_raw(
        &self,
        ctx: &mut impl DurabilityHost,
    ) -> Result<HostResponse, WorkerExecutorError> {
        let oplog_entry = ctx.read_persisted_durable_function_invocation().await?;

        let function_name = Pair::FQFN;
        Self::validate_oplog_entry(&oplog_entry, function_name)?;

        ctx.end_durable_function(&self.function_type, self.begin_index, false)
            .await?;

        Ok(oplog_entry.response)
    }

    fn validate_oplog_entry(
        oplog_entry: &PersistedDurableFunctionInvocation,
        expected_function_name: &str,
    ) -> Result<(), WorkerExecutorError> {
        if oplog_entry.function_name != expected_function_name {
            error!(
                "Unexpected imported function call entry in oplog: expected {}, got {}",
                expected_function_name, oplog_entry.function_name
            );
            Err(WorkerExecutorError::unexpected_oplog_entry(
                expected_function_name,
                oplog_entry.function_name.clone(),
            ))
        } else {
            Ok(())
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
    use golem_common::model::oplog::host_functions::KeyvalueEventualGet;
    use golem_common::model::oplog::PersistenceLevel;
    use golem_common::model::RetryConfig;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use test_r::test;

    /// A mock DurabilityHost for testing `try_trigger_retry_or_loop` in isolation.
    struct MockDurabilityHost {
        in_atomic_region: bool,
        assume_idempotence: bool,
        max_in_function_retry_delay: Duration,
        retry_config: RetryConfig,
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
    }

    impl MockDurabilityHost {
        fn new() -> Self {
            Self {
                in_atomic_region: false,
                assume_idempotence: true,
                max_in_function_retry_delay: Duration::from_secs(20),
                retry_config: RetryConfig {
                    max_attempts: 5,
                    min_delay: Duration::from_millis(1),
                    max_delay: Duration::from_millis(100),
                    multiplier: 1.0,
                    max_jitter_factor: None,
                },
                oplog_retry_count: 0,
                retry_entries_appended: 0,
                trap_triggered_count: 0,
                interrupt_signal: None,
                interrupt_armed: Arc::new(AtomicBool::new(false)),
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
    impl DurabilityHost for MockDurabilityHost {
        fn observe_function_call(&self, _interface: &str, _function: &str) {}

        async fn begin_durable_function(
            &mut self,
            _function_type: &DurableFunctionType,
        ) -> Result<OplogIndex, WorkerExecutorError> {
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

        fn durable_execution_state(&self) -> DurableExecutionState {
            MockDurabilityHost::durable_execution_state(self)
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

        async fn try_trigger_retry(&mut self, _failure: Error) -> anyhow::Result<()> {
            self.trap_triggered_count += 1;
            // Simulate: retries exhausted, persist the failure
            Ok(())
        }

        fn mark_atomic_region_side_effect(&mut self) {}

        fn in_atomic_region(&self) -> bool {
            self.in_atomic_region
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

        async fn append_retry_error_entry(&mut self, _retry_from: OplogIndex) {
            self.retry_entries_appended += 1;
        }

        async fn current_retry_count_for(&self, _retry_from: OplogIndex) -> u32 {
            self.oplog_retry_count
        }

        fn current_retry_point(&self) -> OplogIndex {
            OplogIndex::INITIAL
        }

        fn retry_config(&self) -> RetryConfig {
            self.retry_config.clone()
        }
    }

    /// Helper: creates a Durability<KeyvalueEventualGet> with a given function type
    async fn make_durability(
        ctx: &mut MockDurabilityHost,
        function_type: DurableFunctionType,
    ) -> Durability<KeyvalueEventualGet> {
        Durability::<KeyvalueEventualGet>::new(ctx, function_type)
            .await
            .expect("Durability::new should succeed with mock")
    }

    // Test 1: In-function retry works for eligible operations
    #[test]
    async fn in_function_retry_returns_retry_internally_on_transient_error() {
        let mut ctx = MockDurabilityHost::new();
        // Allow up to 5 attempts, with 1ms delay — well within the 20s threshold
        let mut durability = make_durability(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("connection refused".to_string());
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::RetryInternally);
        assert_eq!(ctx.retry_entries_appended, 1);
        assert_eq!(durability.retry_state.retry_count(), 1);
        assert_eq!(ctx.trap_triggered_count, 0, "should NOT fall back to trap");
    }

    // Test 1b: Success returns Persist immediately
    #[test]
    async fn in_function_retry_returns_persist_on_success() {
        let mut ctx = MockDurabilityHost::new();
        let mut durability = make_durability(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Ok("value".to_string());
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.retry_entries_appended, 0);
        assert_eq!(durability.retry_state.retry_count(), 0);
    }

    // Test 1c: Permanent errors return Persist (no retry)
    #[test]
    async fn in_function_retry_returns_persist_on_permanent_error() {
        let mut ctx = MockDurabilityHost::new();
        let mut durability = make_durability(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("invalid key format".to_string());
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Permanent)
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
        ctx.retry_config.max_attempts = 3;
        let mut durability = make_durability(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());

        // Attempts 0, 1, 2 should return RetryInternally
        for i in 0..3 {
            let action = durability
                .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
                .await
                .expect("should not propagate error");
            assert_eq!(
                action,
                InternalRetryResult::RetryInternally,
                "attempt {i} should retry"
            );
        }

        // Attempt 3 should return Persist (budget exhausted)
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
            .await
            .expect("should not propagate error");
        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.retry_entries_appended, 3);
        assert_eq!(durability.retry_state.retry_count(), 3);
    }

    // Test 1e: WriteRemote with idempotence is eligible
    #[test]
    async fn in_function_retry_eligible_for_write_remote_with_idempotence() {
        let mut ctx = MockDurabilityHost::new();
        ctx.assume_idempotence = true;
        let mut durability = make_durability(&mut ctx, DurableFunctionType::WriteRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::RetryInternally);
        assert_eq!(ctx.trap_triggered_count, 0);
    }

    // Test 2: Fallback to trap when delay exceeds max_in_function_retry_delay
    #[test]
    async fn fallback_to_trap_when_delay_exceeds_threshold() {
        let mut ctx = MockDurabilityHost::new();
        // Set min_delay very high so it exceeds the threshold
        ctx.retry_config.min_delay = Duration::from_secs(30);
        ctx.retry_config.max_delay = Duration::from_secs(60);
        ctx.max_in_function_retry_delay = Duration::from_secs(20);
        let mut durability = make_durability(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
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
        let mut durability = make_durability(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("connection refused".to_string());
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
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
        let mut durability =
            make_durability(&mut ctx, DurableFunctionType::WriteRemoteBatched(None)).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.trap_triggered_count, 1);
    }

    // Test 3c: WriteRemoteTransaction is ineligible
    #[test]
    async fn transaction_write_disables_in_function_retry() {
        let mut ctx = MockDurabilityHost::new();
        let mut durability =
            make_durability(&mut ctx, DurableFunctionType::WriteRemoteTransaction(None)).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
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
        let mut durability = make_durability(&mut ctx, DurableFunctionType::WriteRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
            .await
            .expect("should not propagate error");

        assert_eq!(action, InternalRetryResult::Persist);
        assert_eq!(ctx.trap_triggered_count, 1);
    }

    // Test 4: Interrupt during in-function retry sleep
    #[test]
    async fn interrupt_during_in_function_retry_sleep_propagates_error() {
        let mut ctx = MockDurabilityHost::new();
        // Set a non-trivial sleep so the interrupt wins the race
        ctx.retry_config.min_delay = Duration::from_secs(10);
        ctx.retry_config.max_delay = Duration::from_secs(10);
        // But still within the threshold so we don't fall back to trap
        ctx.max_in_function_retry_delay = Duration::from_secs(20);
        // Interrupt fires immediately
        ctx.interrupt_signal = Some(InterruptKind::Interrupt(Timestamp::now_utc()));

        let mut durability = make_durability(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let err = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
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
        ctx.retry_config.min_delay = Duration::from_millis(500);
        ctx.retry_config.max_delay = Duration::from_millis(500);
        ctx.max_in_function_retry_delay = Duration::from_secs(20);
        // Arm the interrupt to fire via the polling loop
        ctx.interrupt_armed.store(true, Ordering::Release);

        let mut durability = make_durability(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());
        let err = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
            .await
            .expect_err("should propagate interrupt as error");

        assert!(err.downcast_ref::<InterruptKind>().is_some());
    }

    // Test: oplog retry count is combined with in-function retry count
    #[test]
    async fn oplog_retry_count_combined_with_in_function_retry_count() {
        let mut ctx = MockDurabilityHost::new();
        ctx.retry_config.max_attempts = 5;
        // Simulate 3 prior oplog-level retries
        ctx.oplog_retry_count = 3;
        let mut durability = make_durability(&mut ctx, DurableFunctionType::ReadRemote).await;

        let result: Result<String, String> = Err("timeout".to_string());

        // With 3 oplog retries, total=3. max_attempts=5.
        // Attempt at total=3 → ok (get_delay(config, 3) = Some)
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
            .await
            .expect("should not propagate error");
        assert_eq!(action, InternalRetryResult::RetryInternally);

        // Attempt at total=4 → ok (get_delay(config, 4) = Some)
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
            .await
            .expect("should not propagate error");
        assert_eq!(action, InternalRetryResult::RetryInternally);

        // Attempt at total=5 → exhausted (get_delay(config, 5) = None since 5 >= 5)
        let action = durability
            .try_trigger_retry_or_loop(&mut ctx, &result, |_| HostFailureKind::Transient)
            .await
            .expect("should not propagate error");
        assert_eq!(action, InternalRetryResult::Persist);
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
            let durability: Durability<KeyvalueEventualGet> = Durability {
                function_type: ft.clone(),
                begin_index: OplogIndex::from_u64(1),
                durable_execution_state: ctx.durable_execution_state(),
                retry_state: InFunctionRetryState::new(),
                _phantom: std::marker::PhantomData,
            };
            assert_eq!(
                durability.is_eligible_for_internal_retry(),
                expected,
                "ft={ft:?}, idempotent={idempotent}"
            );
        }
    }
}
