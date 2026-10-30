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
use crate::durable_host::call_coordinator::{DurableCallAdmission, DurableCallCoordinator};
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::golem::durability::durability;
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::oplog::{DurableFunctionType, HostResponse};
use golem_common::model::{RetryProperties, Timestamp};
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use std::future::Future;
use std::pin::Pin;

mod custom;
mod errors;
mod interrupt;
mod invocation_id;
mod retry;

pub(crate) use custom::{
    ActiveCustomInvocation, CustomInvocationContext, CustomInvocationScope,
    OpenCustomInvocationScope,
};
pub use custom::{
    CustomBeginLifecycle, CustomInvocationContextSnapshot, LiveCustomDurableInvocation,
};
pub use errors::{
    ClassifiedHostError, DurableCallTrapContext, DurableCallTrapContextMarker,
    DurableCallTrapError, DurableRecoveryFailure, DurableRecoveryFailureKind, HostFailureKind,
    SemanticTrapRetryOverride, SemanticTrapRetryOverrideMarker, SemanticTrapRetryVerdict,
    TerminalCallError, find_durable_call_trap_context, find_semantic_trap_retry_override,
    mark_durable_call_trap_context, semantic_trap_retry_override_error,
};
pub(crate) use interrupt::InterruptSources;
pub(crate) use invocation_id::{
    GuestImportIdentity, GuestImportNumbering, ImportNumbering, UnidentifiedReason,
};
pub use retry::{
    AsyncRetryDecision, DurableExecutionState, InFunctionRetryController, InFunctionRetryHost,
    InFunctionRetryState, InternalRetryResult, TaskRetryContext, count_oplog_errors_for,
    in_task_retry_loop,
};
pub(crate) use retry::{
    collect_named_retry_policies, evaluate_named_policy_step_resetting_on_invalid_state,
    merge_named_retry_policy_tiers, try_trigger_host_trap_retry,
};

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
    /// durability `DurableCallSession`), so the read-only trap is uniformly enforced here without any
    /// per-callsite checks.
    async fn begin_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        host_function: &str,
    ) -> Result<(), WorkerExecutorError>;

    /// Marks the end of a durable function
    ///
    /// This is a pair of `begin_durable_function` and should be called after the durable function
    /// has performed and persisted or replayed its work.
    async fn end_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        forced_commit: bool,
    ) -> Result<(), WorkerExecutorError>;

    /// Classifies a failed host operation and delegates transient failures to the worker's retry
    /// policy. If that policy permits another attempt, the returned error should be propagated by
    /// the host function to trigger the retry.
    ///
    /// If retrying is not possible, the function returns Ok(()) and the host function
    /// can continue persisting the failed result permanently.
    ///
    /// The `properties` bag is used for semantic policy resolution. The available named policies
    /// include a synthesized default policy that matches any property set.
    async fn try_trigger_retry(
        &mut self,
        failure: Error,
        properties: RetryProperties,
    ) -> anyhow::Result<()>;

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
        DurableFunctionType::WriteLocal | DurableFunctionType::WriteRemote
    )
}

impl From<durability::DurableFunctionType> for DurableFunctionType {
    fn from(value: durability::DurableFunctionType) -> Self {
        match value {
            durability::DurableFunctionType::WriteRemote => DurableFunctionType::WriteRemote,
            durability::DurableFunctionType::WriteLocal => DurableFunctionType::WriteLocal,
            durability::DurableFunctionType::ReadRemote => DurableFunctionType::ReadRemote,
            durability::DurableFunctionType::ReadLocal => DurableFunctionType::ReadLocal,
        }
    }
}

impl From<DurableFunctionType> for durability::DurableFunctionType {
    fn from(value: DurableFunctionType) -> Self {
        match value {
            DurableFunctionType::WriteRemote => durability::DurableFunctionType::WriteRemote,
            DurableFunctionType::WriteLocal => durability::DurableFunctionType::WriteLocal,
            DurableFunctionType::ReadRemote => durability::DurableFunctionType::ReadRemote,
            DurableFunctionType::ReadLocal => durability::DurableFunctionType::ReadLocal,
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

impl TryFrom<PersistedDurableFunctionInvocation>
    for durability::PersistedDurableFunctionInvocation
{
    type Error = anyhow::Error;

    fn try_from(value: PersistedDurableFunctionInvocation) -> Result<Self, Self::Error> {
        let response = value.response.into_typed_schema_value().map_err(|e| {
            anyhow::anyhow!("Failed to render persisted durable function response: {e}")
        })?;
        let response = golem_common::schema::wit::encode_typed(&response).map_err(|e| {
            anyhow::anyhow!("Failed to encode persisted durable function response: {e}")
        })?;
        Ok(durability::PersistedDurableFunctionInvocation {
            timestamp: value.timestamp.into(),
            function_name: value.function_name,
            response,
            function_type: value.function_type.into(),
            entry_version: value.oplog_entry_version.into(),
        })
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
    ) -> Result<(), WorkerExecutorError> {
        let invocation_id = self.direct_call_invocation_id()?;
        DurableCallCoordinator::new(self)
            .admit(
                DurableCallAdmission::new(function_type, host_function)
                    .with_invocation_id(invocation_id),
            )
            .await
    }

    async fn end_durable_function(
        &mut self,
        function_type: &DurableFunctionType,
        forced_commit: bool,
    ) -> Result<(), WorkerExecutorError> {
        DurableCallCoordinator::new(self)
            .finish(function_type, forced_commit)
            .await
    }

    async fn try_trigger_retry(
        &mut self,
        failure: Error,
        properties: RetryProperties,
    ) -> anyhow::Result<()> {
        try_trigger_host_trap_retry(self, failure, properties).await
    }

    fn create_interrupt_signal(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
        self.interrupt_sources().subscribe()
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
