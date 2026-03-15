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

use super::LogEventEmitBehaviour;
use crate::durable_host::{DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState};
use crate::metrics::wasm::record_allocated_memory;
use crate::model::{AgentConfig, ExecutionStatus, LastError, ReadFileResult, TrapType};
use crate::preview2::golem::agent::host::{
    CancellationToken, FutureInvokeResult, Host as AgentHost, HostCancellationToken,
    HostFutureInvokeResult, HostWasmRpc, RpcError, WasmRpc,
};
use crate::services::active_workers::ActiveWorkers;
use crate::services::agent_types::AgentTypesService;
use crate::services::agent_webhooks::AgentWebhooksService;
use crate::services::blob_store::BlobStoreService;
use crate::services::component::ComponentService;
use crate::services::environment_state::EnvironmentStateService;
use crate::services::file_loader::FileLoader;
use crate::services::golem_config::GolemConfig;
use crate::services::key_value::KeyValueService;
use crate::services::oplog::{Oplog, OplogService};
use crate::services::promise::PromiseService;
use crate::services::rdbms::RdbmsService;
use crate::services::resource_limits::{AtomicResourceEntry, ResourceLimits};
use crate::services::rpc::Rpc;
use crate::services::scheduler::SchedulerService;
use crate::services::shard::ShardService;
use crate::services::worker::WorkerService;
use crate::services::worker_event::WorkerEventService;
use crate::services::worker_fork::WorkerForkService;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{worker_enumeration, HasAll, NoAdditionalDeps};
use crate::worker::{RetryDecision, Worker};
use crate::workerctx::{
    ExternalOperations, FileSystemReading, FuelManagement, InvocationContextManagement,
    InvocationHooks, InvocationManagement, StatusManagement, UpdateManagement, WorkerCtx,
};
use anyhow::Error;
use async_trait::async_trait;
use golem_common::base_model::OplogIndex;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentMode, ParsedAgentId};
use golem_common::model::component::{ComponentFilePath, ComponentRevision, PluginPriority};
use golem_common::model::invocation_context::{
    self, AttributeValue, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::TimestampedUpdateDescription;
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationOutput, AgentStatusRecord, IdempotencyKey,
    OwnedAgentId,
};
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use golem_service_base::model::component::Component;
use golem_service_base::model::GetFileSystemNodeResult;
use golem_wasm::wasmtime::{ResourceStore, ResourceTypeId};
use golem_wasm::{Uri, WitType};
use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Weak};
use tracing::debug;
use uuid::Uuid;
use wasmtime::component::{Instance, Resource, ResourceAny};
use wasmtime::{AsContextMut, ResourceLimiterAsync};
use wasmtime_wasi::WasiView;
use wasmtime_wasi_http::WasiHttpView;

/// Tracks the wasmtime fuel gauge state for a single worker store.
///
/// Wasmtime fuel counts down from `u64::MAX`. Workers borrow a fixed amount of
/// fuel from the shared [`AtomicResourceEntry`] each epoch tick, and return any
/// unused portion at invocation end.
///
/// The key invariant for correct billing:
///
///   unused = total_borrowed_since_last_return - total_consumed_since_last_return
///
/// `total_consumed` is derived directly from the wasmtime gauge:
///   `gauge_at_last_return - current_gauge`
///
/// `total_borrowed` is accumulated explicitly across all borrows since the last
/// `on_return` call. This ensures that when `unused_to_return` is called at
/// invocation end, it refunds the full over-borrow across all epoch ticks — not
/// just the tail of the most recent borrow.
struct FuelTracker {
    /// The wasmtime gauge reading the last time `on_return` was called.
    /// Starts at `u64::MAX` (the gauge's initial value set at store creation).
    pub(self) gauge_at_last_return: u64,
    /// The gauge level down to which we have pre-paid via the account pool.
    /// Updated to `current_gauge - amount` after each successful borrow.
    /// Used only to determine the next `amount_to_borrow` — not for `unused_to_return`.
    pub(self) prepaid_gauge_floor: u64,
    /// Cumulative fuel borrowed from the account pool since the last `on_return`.
    /// Reset to 0 on every `on_return` call.
    pub(self) total_borrowed_since_last_return: u64,
    /// Minimum fuel units borrowed from the account pool per epoch tick.
    pub(self) fuel_to_borrow: u64,
}

impl FuelTracker {
    pub(self) fn new(fuel_to_borrow: u64) -> Self {
        Self {
            gauge_at_last_return: u64::MAX,
            prepaid_gauge_floor: u64::MAX,
            total_borrowed_since_last_return: 0,
            fuel_to_borrow,
        }
    }

    /// How much fuel to request from the account pool this epoch tick.
    ///
    /// Always at least `fuel_to_borrow`. If the gauge has already dropped below
    /// `prepaid_gauge_floor` (the worker burned through the whole pre-paid amount
    /// in one epoch), the full deficit is topped up instead.
    pub(self) fn determine_amount_to_borrow(&self, current_gauge: u64) -> u64 {
        Ord::max(
            self.fuel_to_borrow,
            self.prepaid_gauge_floor.saturating_sub(current_gauge),
        )
    }

    /// Called after a successful borrow to advance the pre-paid floor and
    /// accumulate the total borrowed since the last return.
    pub(self) fn on_borrow_success(&mut self, current_gauge: u64, amount: u64) {
        self.prepaid_gauge_floor = current_gauge.saturating_sub(amount);
        self.total_borrowed_since_last_return =
            self.total_borrowed_since_last_return.saturating_add(amount);
    }

    /// How much unused pre-paid fuel to return to the account pool at invocation end.
    ///
    /// Returns the full over-borrow across all epoch ticks since the last `on_return`:
    ///   unused = total_borrowed - total_consumed
    ///          = total_borrowed - (gauge_at_last_return - current_gauge)
    ///
    /// This correctly refunds all pre-borrowed fuel that wasmtime did not actually
    /// consume, regardless of how many epoch ticks or batch syncs occurred.
    pub(self) fn unused_to_return(&self, current_gauge: u64) -> u64 {
        let total_consumed = self.gauge_at_last_return.saturating_sub(current_gauge);
        self.total_borrowed_since_last_return
            .saturating_sub(total_consumed)
    }

    /// Records the current gauge reading, resets the borrow accumulator, and
    /// returns wasmtime instructions burned since the previous call.
    ///
    /// # Panics
    ///
    /// Panics if `current_gauge > gauge_at_last_return` (fuel can only decrease).
    pub(self) fn on_return(&mut self, current_gauge: u64) -> u64 {
        assert!(
            self.gauge_at_last_return >= current_gauge,
            "fuel gauge increased: previous={} current={}",
            self.gauge_at_last_return,
            current_gauge
        );
        let consumed = self.gauge_at_last_return - current_gauge;
        self.gauge_at_last_return = current_gauge;
        self.total_borrowed_since_last_return = 0;
        consumed
    }
}

pub struct Context {
    pub durable_ctx: DurableWorkerCtx<Context>,
    resource_limit_entry: Arc<AtomicResourceEntry>,
    fuel_tracker: FuelTracker,
}

impl Context {
    pub fn new(
        golem_ctx: DurableWorkerCtx<Context>,
        config: Arc<GolemConfig>,
        resource_limit_entry: Arc<AtomicResourceEntry>,
    ) -> Self {
        Self {
            durable_ctx: golem_ctx,
            resource_limit_entry,
            fuel_tracker: FuelTracker::new(config.limits.fuel_to_borrow),
        }
    }

    pub fn get_max_memory(&self) -> usize {
        self.resource_limit_entry.max_memory_limit()
    }
}

impl DurableWorkerCtxView<Context> for Context {
    fn durable_ctx(&self) -> &DurableWorkerCtx<Context> {
        &self.durable_ctx
    }

    fn durable_ctx_mut(&mut self) -> &mut DurableWorkerCtx<Context> {
        &mut self.durable_ctx
    }
}

#[async_trait]
impl FuelManagement for Context {
    fn borrow_fuel(&mut self, current_level: u64) -> bool {
        let amount_to_borrow = self.fuel_tracker.determine_amount_to_borrow(current_level);
        let success = self.resource_limit_entry.borrow_fuel(amount_to_borrow);
        if success {
            self.fuel_tracker
                .on_borrow_success(current_level, amount_to_borrow);
            debug!("borrowed {amount_to_borrow} fuel");
        }
        success
    }

    fn return_fuel(&mut self, current_level: u64) -> u64 {
        let unused = self.fuel_tracker.unused_to_return(current_level);
        if unused > 0 {
            self.resource_limit_entry.return_fuel(unused);
            debug!("returned {} fuel", unused);
        }

        let consumed = self.fuel_tracker.on_return(current_level);
        debug!("reset fuel mark to {}", current_level);
        consumed
    }
}

#[async_trait]
impl InvocationManagement for Context {
    async fn set_current_idempotency_key(&mut self, key: IdempotencyKey) {
        self.durable_ctx.set_current_idempotency_key(key).await
    }

    async fn get_current_idempotency_key(&self) -> Option<IdempotencyKey> {
        self.durable_ctx.get_current_idempotency_key().await
    }

    async fn set_current_invocation_context(
        &mut self,
        stack: InvocationContextStack,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx.set_current_invocation_context(stack).await
    }

    async fn get_current_invocation_context(&self) -> InvocationContextStack {
        self.durable_ctx.get_current_invocation_context().await
    }

    fn is_live(&self) -> bool {
        self.durable_ctx.is_live()
    }

    fn is_replay(&self) -> bool {
        self.durable_ctx.is_replay()
    }
}

#[async_trait]
impl StatusManagement for Context {
    fn check_interrupt(&self) -> Option<InterruptKind> {
        self.durable_ctx.check_interrupt()
    }

    fn set_suspended(&self) {
        self.durable_ctx.set_suspended()
    }

    fn set_running(&self) {
        self.durable_ctx.set_running()
    }
}

#[async_trait]
impl InvocationHooks for Context {
    async fn on_agent_invocation_started(
        &mut self,
        invocation: AgentInvocation,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx
            .on_agent_invocation_started(invocation)
            .await
    }

    async fn on_invocation_failure(
        &mut self,
        full_function_name: &str,
        trap_type: &TrapType,
    ) -> RetryDecision {
        self.durable_ctx
            .on_invocation_failure(full_function_name, trap_type)
            .await
    }

    async fn on_agent_invocation_success(
        &mut self,
        full_function_name: &str,
        consumed_fuel: u64,
        output: &AgentInvocationOutput,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx
            .on_agent_invocation_success(full_function_name, consumed_fuel, output)
            .await
    }

    async fn get_current_retry_point(&self) -> OplogIndex {
        self.durable_ctx.get_current_retry_point().await
    }
}

#[async_trait]
impl ResourceLimiterAsync for Context {
    async fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let limit = self.get_max_memory();
        debug!(
            "memory_growing: current={}, desired={}, maximum={:?}, account limit={}",
            current, desired, maximum, limit
        );

        if desired > limit || maximum.map(|m| desired > m).unwrap_or_default() {
            Err(GolemSpecificWasmTrap::WorkerExceededMemoryLimit)?;
        };

        let current_known = self.durable_ctx.total_linear_memory_size();
        let delta = (desired as u64).saturating_sub(current_known);

        if delta > 0 {
            // Get more permits from the host. If this is not allowed the worker will fail immediately and will retry with more permits.
            self.durable_ctx
                .increase_memory(delta)
                .await
                .map_err(wasmtime::Error::from_anyhow)?;
            record_allocated_memory(desired);
        }

        Ok(true)
    }

    async fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        debug!(
            "table_growing: current={}, desired={}, maximum={:?}",
            current, desired, maximum
        );
        Ok(true)
    }
}

#[async_trait]
impl ExternalOperations<Context> for Context {
    type ExtraDeps = NoAdditionalDeps;

    async fn get_last_error_and_retry_count<T: HasAll<Context> + Send + Sync>(
        this: &T,
        agent_id: &OwnedAgentId,
        worker_status_record: &AgentStatusRecord,
    ) -> Option<LastError> {
        DurableWorkerCtx::<Context>::get_last_error_and_retry_count(
            this,
            agent_id,
            worker_status_record,
        )
        .await
    }

    async fn resume_replay(
        store: &mut (impl AsContextMut<Data = Context> + Send),
        instance: &Instance,
        refresh_replay_target: bool,
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        DurableWorkerCtx::<Context>::resume_replay(store, instance, refresh_replay_target).await
    }

    async fn prepare_instance(
        agent_id: &AgentId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = Self> + Send),
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        DurableWorkerCtx::<Context>::prepare_instance(agent_id, instance, store).await
    }

    async fn on_shard_assignment_changed<T: HasAll<Context> + Send + Sync + 'static>(
        this: &T,
    ) -> Result<(), Error> {
        DurableWorkerCtx::<Context>::on_shard_assignment_changed(this).await
    }
}

#[async_trait]
impl ResourceStore for Context {
    fn self_uri(&self) -> Uri {
        self.durable_ctx.self_uri()
    }

    async fn add(&mut self, resource: ResourceAny, name: ResourceTypeId) -> u64 {
        self.durable_ctx.add(resource, name).await
    }

    async fn get(&mut self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        ResourceStore::get(&mut self.durable_ctx, resource_id).await
    }

    async fn borrow(&self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        self.durable_ctx.borrow(resource_id).await
    }
}

#[async_trait]
impl UpdateManagement for Context {
    fn begin_call_snapshotting_function(&mut self) {
        self.durable_ctx.begin_call_snapshotting_function()
    }

    fn end_call_snapshotting_function(&mut self) {
        self.durable_ctx.end_call_snapshotting_function()
    }

    async fn on_worker_update_failed(
        &self,
        target_revision: ComponentRevision,
        details: Option<String>,
    ) {
        self.durable_ctx
            .on_worker_update_failed(target_revision, details)
            .await
    }

    async fn on_worker_update_succeeded(
        &self,
        target_revision: ComponentRevision,
        new_component_size: u64,
        new_active_plugins: HashSet<PluginPriority>,
    ) {
        self.durable_ctx
            .on_worker_update_succeeded(target_revision, new_component_size, new_active_plugins)
            .await
    }
}

#[async_trait]
impl FileSystemReading for Context {
    async fn get_file_system_node(
        &self,
        path: &ComponentFilePath,
    ) -> Result<GetFileSystemNodeResult, WorkerExecutorError> {
        self.durable_ctx.get_file_system_node(path).await
    }

    async fn read_file(
        &self,
        path: &ComponentFilePath,
    ) -> Result<ReadFileResult, WorkerExecutorError> {
        self.durable_ctx.read_file(path).await
    }
}

impl HostWasmRpc for Context {
    async fn new(
        &mut self,
        agent_type_name: String,
        constructor: golem_common::model::agent::bindings::golem::agent::common::DataValue,
        phantom_id: Option<golem_wasm::Uuid>,
    ) -> anyhow::Result<Resource<WasmRpc>> {
        self.durable_ctx
            .new(agent_type_name, constructor, phantom_id)
            .await
    }

    async fn invoke_and_await(
        &mut self,
        self_: Resource<WasmRpc>,
        method_name: String,
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    ) -> anyhow::Result<
        Result<golem_common::model::agent::bindings::golem::agent::common::DataValue, RpcError>,
    > {
        self.durable_ctx
            .invoke_and_await(self_, method_name, input)
            .await
    }

    async fn invoke(
        &mut self,
        self_: Resource<WasmRpc>,
        method_name: String,
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    ) -> anyhow::Result<Result<(), RpcError>> {
        self.durable_ctx.invoke(self_, method_name, input).await
    }

    async fn async_invoke_and_await(
        &mut self,
        self_: Resource<WasmRpc>,
        method_name: String,
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    ) -> anyhow::Result<Resource<FutureInvokeResult>> {
        self.durable_ctx
            .async_invoke_and_await(self_, method_name, input)
            .await
    }

    async fn schedule_invocation(
        &mut self,
        self_: Resource<WasmRpc>,
        scheduled_time: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime,
        method_name: String,
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    ) -> anyhow::Result<()> {
        self.durable_ctx
            .schedule_invocation(self_, scheduled_time, method_name, input)
            .await
    }

    async fn schedule_cancelable_invocation(
        &mut self,
        self_: Resource<WasmRpc>,
        scheduled_time: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime,
        method_name: String,
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    ) -> anyhow::Result<Resource<CancellationToken>> {
        self.durable_ctx
            .schedule_cancelable_invocation(self_, scheduled_time, method_name, input)
            .await
    }

    async fn drop(&mut self, rep: Resource<WasmRpc>) -> anyhow::Result<()> {
        HostWasmRpc::drop(&mut self.durable_ctx, rep).await
    }
}

impl HostFutureInvokeResult for Context {
    async fn subscribe(
        &mut self,
        self_: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Resource<golem_wasm::DynPollable>> {
        HostFutureInvokeResult::subscribe(&mut self.durable_ctx, self_).await
    }

    async fn get(
        &mut self,
        self_: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<
        Option<
            Result<golem_common::model::agent::bindings::golem::agent::common::DataValue, RpcError>,
        >,
    > {
        HostFutureInvokeResult::get(&mut self.durable_ctx, self_).await
    }

    async fn drop(&mut self, rep: Resource<FutureInvokeResult>) -> anyhow::Result<()> {
        HostFutureInvokeResult::drop(&mut self.durable_ctx, rep).await
    }
}

impl HostCancellationToken for Context {
    async fn cancel(&mut self, this: Resource<CancellationToken>) -> anyhow::Result<()> {
        HostCancellationToken::cancel(&mut self.durable_ctx, this).await
    }

    async fn drop(&mut self, this: Resource<CancellationToken>) -> anyhow::Result<()> {
        HostCancellationToken::drop(&mut self.durable_ctx, this).await
    }
}

impl AgentHost for Context {
    async fn get_all_agent_types(
        &mut self,
    ) -> anyhow::Result<
        Vec<golem_common::model::agent::bindings::golem::agent::common::RegisteredAgentType>,
    > {
        AgentHost::get_all_agent_types(&mut self.durable_ctx).await
    }

    async fn get_agent_type(
        &mut self,
        agent_type_name: String,
    ) -> anyhow::Result<
        Option<golem_common::model::agent::bindings::golem::agent::common::RegisteredAgentType>,
    > {
        AgentHost::get_agent_type(&mut self.durable_ctx, agent_type_name).await
    }

    async fn make_agent_id(
        &mut self,
        agent_type_name: String,
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
        phantom_id: Option<golem_wasm::Uuid>,
    ) -> anyhow::Result<
        Result<String, golem_common::model::agent::bindings::golem::agent::common::AgentError>,
    > {
        AgentHost::make_agent_id(&mut self.durable_ctx, agent_type_name, input, phantom_id).await
    }

    async fn parse_agent_id(
        &mut self,
        agent_id: String,
    ) -> anyhow::Result<
        Result<
            (
                String,
                golem_common::model::agent::bindings::golem::agent::common::DataValue,
                Option<golem_wasm::Uuid>,
            ),
            golem_common::model::agent::bindings::golem::agent::common::AgentError,
        >,
    > {
        AgentHost::parse_agent_id(&mut self.durable_ctx, agent_id).await
    }

    async fn create_webhook(
        &mut self,
        promise_id: crate::preview2::golem_api_1_x::host::PromiseId,
    ) -> anyhow::Result<String> {
        AgentHost::create_webhook(&mut self.durable_ctx, promise_id).await
    }

    async fn get_config_value(
        &mut self,
        key: Vec<String>,
        expected_type: WitType,
    ) -> anyhow::Result<golem_wasm::WitValue> {
        AgentHost::get_config_value(&mut self.durable_ctx, key, expected_type).await
    }
}

impl wasmtime_wasi::p2::bindings::cli::environment::Host for Context {
    fn get_environment(
        &mut self,
    ) -> impl Future<Output = wasmtime::Result<Vec<(String, String)>>> + Send {
        wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(&mut self.durable_ctx)
    }

    fn get_arguments(&mut self) -> impl Future<Output = wasmtime::Result<Vec<String>>> + Send {
        wasmtime_wasi::p2::bindings::cli::environment::Host::get_arguments(&mut self.durable_ctx)
    }

    fn initial_cwd(&mut self) -> impl Future<Output = wasmtime::Result<Option<String>>> + Send {
        wasmtime_wasi::p2::bindings::cli::environment::Host::initial_cwd(&mut self.durable_ctx)
    }
}

#[async_trait]
impl InvocationContextManagement for Context {
    async fn start_span(
        &mut self,
        initial_attributes: &[(String, AttributeValue)],
        activate: bool,
    ) -> Result<Arc<invocation_context::InvocationContextSpan>, WorkerExecutorError> {
        self.durable_ctx
            .start_span(initial_attributes, activate)
            .await
    }

    async fn start_child_span(
        &mut self,
        parent: &invocation_context::SpanId,
        initial_attributes: &[(String, invocation_context::AttributeValue)],
    ) -> Result<Arc<invocation_context::InvocationContextSpan>, WorkerExecutorError> {
        self.durable_ctx
            .start_child_span(parent, initial_attributes)
            .await
    }

    fn remove_span(
        &mut self,
        span_id: &invocation_context::SpanId,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx.remove_span(span_id)
    }

    async fn finish_span(
        &mut self,
        span_id: &invocation_context::SpanId,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx.finish_span(span_id).await
    }

    async fn set_span_attribute(
        &mut self,
        span_id: &SpanId,
        key: &str,
        value: AttributeValue,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx
            .set_span_attribute(span_id, key, value)
            .await
    }

    fn clone_as_inherited_stack(&self, current_span_id: &SpanId) -> InvocationContextStack {
        self.durable_ctx.clone_as_inherited_stack(current_span_id)
    }
}

#[async_trait]
impl WorkerCtx for Context {
    type PublicState = PublicDurableWorkerState<Context>;

    const LOG_EVENT_EMIT_BEHAVIOUR: LogEventEmitBehaviour = LogEventEmitBehaviour::LiveOnly;

    async fn create(
        account_id: AccountId,
        owned_agent_id: OwnedAgentId,
        agent_id: Option<ParsedAgentId>,
        promise_service: Arc<dyn PromiseService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn RdbmsService>,
        event_service: Arc<dyn WorkerEventService>,
        _active_workers: Arc<ActiveWorkers<Self>>,
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        invocation_queue: Weak<Worker<Self>>,
        scheduler_service: Arc<dyn SchedulerService>,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        component_service: Arc<dyn ComponentService>,
        _extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: AgentConfig,
        execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        worker_fork: Arc<dyn WorkerForkService>,
        resource_limits: Arc<dyn ResourceLimits>,
        agent_types_service: Arc<dyn AgentTypesService>,
        environment_state_service: Arc<dyn EnvironmentStateService>,
        agent_webhooks_service: Arc<AgentWebhooksService>,
        shard_service: Arc<dyn ShardService>,
        http_connection_pool: Option<wasmtime_wasi_http::HttpConnectionPool>,
        pending_update: Option<TimestampedUpdateDescription>,
        original_phantom_id: Option<Uuid>,
    ) -> Result<Self, WorkerExecutorError> {
        let golem_ctx = DurableWorkerCtx::create(
            owned_agent_id.clone(),
            agent_id,
            promise_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            event_service,
            oplog_service,
            oplog,
            invocation_queue,
            scheduler_service,
            rpc,
            worker_proxy,
            component_service,
            config.clone(),
            worker_config.clone(),
            execution_status,
            file_loader,
            worker_fork,
            agent_types_service,
            environment_state_service,
            agent_webhooks_service,
            shard_service,
            http_connection_pool,
            pending_update,
            original_phantom_id,
        )
        .await?;
        let account_resource_limits = resource_limits.initialize_account(account_id).await?;
        Ok(Self::new(golem_ctx, config, account_resource_limits))
    }

    fn as_wasi_view(&mut self) -> impl WasiView {
        self.durable_ctx.as_wasi_view()
    }

    fn as_wasi_http_view(&mut self) -> impl WasiHttpView {
        self.durable_ctx.as_wasi_http_view()
    }

    fn get_public_state(&self) -> &Self::PublicState {
        &self.durable_ctx.public_state
    }

    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync {
        self
    }

    fn agent_id(&self) -> &AgentId {
        self.durable_ctx.agent_id()
    }

    fn owned_agent_id(&self) -> &OwnedAgentId {
        self.durable_ctx.owned_agent_id()
    }

    fn parsed_agent_id(&self) -> Option<ParsedAgentId> {
        self.durable_ctx.parsed_agent_id()
    }

    fn agent_mode(&self) -> AgentMode {
        self.durable_ctx.agent_mode()
    }

    fn created_by(&self) -> AccountId {
        self.durable_ctx.created_by()
    }

    fn component_metadata(&self) -> &Component {
        self.durable_ctx.component_metadata()
    }

    fn is_exit(error: &Error) -> Option<i32> {
        DurableWorkerCtx::<Context>::is_exit(error)
    }

    fn rpc(&self) -> Arc<dyn Rpc> {
        self.durable_ctx.rpc()
    }

    fn worker_proxy(&self) -> Arc<dyn WorkerProxy> {
        self.durable_ctx.worker_proxy()
    }

    fn component_service(&self) -> Arc<dyn ComponentService> {
        self.durable_ctx.component_service()
    }

    fn worker_fork(&self) -> Arc<dyn WorkerForkService> {
        self.durable_ctx.worker_fork()
    }
}

#[cfg(test)]
mod tests {
    use super::FuelTracker;
    use test_r::test;

    // -------------------------------------------------------------------------
    // FuelTracker
    //
    // Wasmtime fuel counts DOWN from u64::MAX. `current_gauge` is always a
    // large number that decreases over time as WASM executes.
    //
    // Invariant after each successful borrow:
    //   prepaid_gauge_floor = current_gauge - amount_borrowed
    //
    // This means:
    //   - next determine_amount_to_borrow = max(fuel_to_borrow, prepaid_gauge_floor - new_gauge)
    //   - if the gauge has not dropped to prepaid_gauge_floor yet, the second
    //     term is zero and we borrow exactly fuel_to_borrow
    //   - if the gauge dropped below prepaid_gauge_floor (burned through the
    //     pre-paid amount), we top up the full deficit
    // -------------------------------------------------------------------------

    const FUEL_TO_BORROW: u64 = 10_000;
    const INITIAL: u64 = u64::MAX; // wasmtime gauge starting value

    fn fuel_tracker() -> FuelTracker {
        FuelTracker::new(FUEL_TO_BORROW)
    }

    // --- determine_amount_to_borrow ---

    #[test]
    fn first_borrow_uses_fuel_to_borrow() {
        // On the very first tick, gauge = INITIAL - 5000 (5000 burned).
        // prepaid_gauge_floor starts at INITIAL, so:
        //   floor.saturating_sub(gauge) = INITIAL - (INITIAL - 5000) = 5000
        //   max(10000, 5000) = 10000
        let ft = fuel_tracker();
        let gauge = INITIAL - 5_000;
        assert_eq!(ft.determine_amount_to_borrow(gauge), FUEL_TO_BORROW);
    }

    #[test]
    fn second_borrow_is_exactly_fuel_to_borrow_when_not_exhausted() {
        // After tick 1: floor = (INITIAL - 5000) - 10000 = INITIAL - 15000.
        // Tick 2: gauge = INITIAL - 9000 (still above floor).
        //   second term = (INITIAL - 15000) - (INITIAL - 9000) = saturates to 0
        //   → borrow exactly fuel_to_borrow
        let mut ft = fuel_tracker();
        let gauge1 = INITIAL - 5_000;
        let amount1 = ft.determine_amount_to_borrow(gauge1);
        ft.on_borrow_success(gauge1, amount1);

        let gauge2 = INITIAL - 9_000;
        assert_eq!(ft.determine_amount_to_borrow(gauge2), FUEL_TO_BORROW);
    }

    #[test]
    fn borrow_tops_up_full_deficit_when_gauge_drops_well_below_floor() {
        // After tick 1 with floor at INITIAL - 15000:
        //   gauge drops to INITIAL - 30000 (burned 15000 past the floor)
        //   deficit = (INITIAL - 15000) - (INITIAL - 30000) = 15000
        //   max(10000, 15000) = 15000 → deficit larger than fuel_to_borrow is topped up
        let mut ft = fuel_tracker();
        let gauge1 = INITIAL - 5_000;
        let amount1 = ft.determine_amount_to_borrow(gauge1);
        ft.on_borrow_success(gauge1, amount1);

        let gauge_deep = INITIAL - 30_000;
        assert_eq!(ft.determine_amount_to_borrow(gauge_deep), 15_000);
    }

    // --- on_borrow_success ---

    #[test]
    fn on_borrow_success_sets_floor_to_gauge_minus_amount() {
        let mut ft = fuel_tracker();
        let gauge = INITIAL - 5_000;
        let amount = 10_000;
        ft.on_borrow_success(gauge, amount);
        assert_eq!(ft.prepaid_gauge_floor, gauge - amount);
    }

    #[test]
    fn successive_borrows_each_set_correct_floor() {
        let mut ft = fuel_tracker();

        let gauge1 = INITIAL - 5_000;
        let a1 = ft.determine_amount_to_borrow(gauge1);
        ft.on_borrow_success(gauge1, a1);
        assert_eq!(ft.prepaid_gauge_floor, gauge1 - a1);

        let gauge2 = INITIAL - 9_000;
        let a2 = ft.determine_amount_to_borrow(gauge2);
        ft.on_borrow_success(gauge2, a2);
        assert_eq!(ft.prepaid_gauge_floor, gauge2 - a2);
    }

    #[test]
    fn unused_to_return_gives_back_total_overborrow() {
        // Tick 1: gauge = INITIAL - 5000 → borrow 10,000.
        //         total_borrowed = 10,000
        // Tick 2: gauge = INITIAL - 8000 → borrow 10,000.
        //         total_borrowed = 20,000
        // Invocation ends: gauge = INITIAL - 9000 (only 1000 more burned after tick 2).
        //   total_consumed = INITIAL - (INITIAL - 9000) = 9,000
        //   unused = total_borrowed - total_consumed = 20,000 - 9,000 = 11,000
        let mut ft = fuel_tracker();

        let gauge1 = INITIAL - 5_000;
        ft.on_borrow_success(gauge1, ft.determine_amount_to_borrow(gauge1));

        let gauge2 = INITIAL - 8_000;
        ft.on_borrow_success(gauge2, ft.determine_amount_to_borrow(gauge2));

        let end_gauge = INITIAL - 9_000;
        assert_eq!(ft.unused_to_return(end_gauge), 11_000);
    }

    #[test]
    fn unused_to_return_is_zero_when_all_borrowed_fuel_consumed() {
        // One tick: borrow 10,000. Worker burns exactly 10,000 instructions.
        // total_consumed = total_borrowed → unused = 0.
        let mut ft = fuel_tracker();
        let gauge = INITIAL; // borrow at the very start, before any instructions burned
        ft.on_borrow_success(gauge, ft.determine_amount_to_borrow(gauge));
        // 10,000 instructions burned since store start
        assert_eq!(ft.unused_to_return(INITIAL - 10_000), 0);
    }

    #[test]
    fn unused_to_return_saturates_to_zero_when_consumed_exceeds_borrowed() {
        // Worker burned more instructions than were borrowed (pre-borrow happened
        // after some instructions already ran). unused cannot be negative.
        let mut ft = fuel_tracker();
        let gauge = INITIAL - 5_000; // 5000 already burned before first borrow
        ft.on_borrow_success(gauge, ft.determine_amount_to_borrow(gauge));
        // 12,000 total burned (5,000 before borrow + 7,000 after) > 10,000 borrowed
        assert_eq!(ft.unused_to_return(INITIAL - 12_000), 0);
    }

    #[test]
    fn on_return_reports_correct_consumption() {
        let mut ft = fuel_tracker();
        let gauge = INITIAL - 10_000;
        let consumed = ft.on_return(gauge);
        assert_eq!(consumed, 10_000);
        assert_eq!(ft.gauge_at_last_return, gauge);
    }

    #[test]
    fn on_return_tracks_gauge_correctly_across_multiple_calls() {
        let mut ft = fuel_tracker();

        let c1 = ft.on_return(INITIAL - 5_000);
        assert_eq!(c1, 5_000);

        let c2 = ft.on_return(INITIAL - 12_000);
        assert_eq!(c2, 7_000); // only the delta since last call
    }

    #[test]
    fn net_delta_over_full_invocation_is_within_one_fuel_to_borrow_of_actual_consumption() {
        // Two epoch ticks of 5000 each (10000 total consumed).
        // Each tick borrows fuel_to_borrow = 10000. Unused is returned at end.
        //
        //   tick 1: +10000 borrowed
        //   tick 2: +10000 borrowed
        //   end:    -10000 returned (unused portion of last pre-paid amount)
        //   net:    +10000 == actual consumption

        let mut ft = fuel_tracker();
        let mut net_delta: i64 = 0;

        let gauge1 = INITIAL - 5_000;
        let a1 = ft.determine_amount_to_borrow(gauge1);
        ft.on_borrow_success(gauge1, a1);
        net_delta += a1 as i64;

        let gauge2 = INITIAL - 10_000;
        let a2 = ft.determine_amount_to_borrow(gauge2);
        ft.on_borrow_success(gauge2, a2);
        net_delta += a2 as i64;

        let unused = ft.unused_to_return(INITIAL - 10_000);
        net_delta -= unused as i64;

        let actual_consumed = 10_000i64;
        assert!(
            net_delta >= actual_consumed,
            "under-charged: net={net_delta} actual={actual_consumed}"
        );
        assert!(
            net_delta <= actual_consumed + FUEL_TO_BORROW as i64,
            "over-charged by more than fuel_to_borrow: net={net_delta} actual={actual_consumed}"
        );
    }

    #[test]
    fn net_charge_equals_actual_consumption_across_many_ticks() {
        // 1000 ticks (10 seconds), 3000 instructions per tick (30% utilisation).
        // Total consumed = 3,000,000. Total borrowed = 10,000,000.
        // unused_to_return must refund 7,000,000 so net billed = 3,000,000 = actual.
        let instructions_per_tick = 3_000u64;
        let ticks = 1_000usize;

        let mut ft = fuel_tracker();
        let mut total_billed: i64 = 0;
        let mut current_gauge = INITIAL;

        for _ in 1..=ticks {
            current_gauge -= instructions_per_tick;
            let amount = ft.determine_amount_to_borrow(current_gauge);
            ft.on_borrow_success(current_gauge, amount);
            total_billed += amount as i64;
        }

        total_billed -= ft.unused_to_return(current_gauge) as i64;
        let actual_consumed = (INITIAL - current_gauge) as i64;

        assert_eq!(
            total_billed,
            actual_consumed,
            "over-charged by {}: billed={total_billed} actual={actual_consumed}",
            total_billed - actual_consumed
        );
    }
}
