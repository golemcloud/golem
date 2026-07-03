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

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, Cancellable, NotCancellable};
use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind, InFunctionRetryHost};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::preview2::golem::agent::host::{
    CancellationToken, FutureInvokeResult, HostCancellationToken, HostFutureInvokeResult,
    HostFutureInvokeResultWithStore, HostWasmRpc, RpcError,
};
use crate::services::HasWorker;
use crate::services::environment_state::EnvironmentStateService;
use crate::services::rpc::{Rpc, RpcDemand, RpcError as InternalRpcError};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use anyhow::Error;
use async_trait::async_trait;
use futures::future::Either;
use golem_common::base_model::agent::Principal;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::invocation_context::{AttributeValue, InvocationContextSpan, SpanId};
use golem_common::model::oplog::host_functions::{
    GolemRpcCancellationTokenCancel, GolemRpcWasmRpcInvoke, GolemRpcWasmRpcInvokeAndAwaitResult,
    GolemRpcWasmRpcNew, GolemRpcWasmRpcScheduleInvocation,
};
use golem_common::model::oplog::types::{SerializableRpcError, SerializableScheduleId};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemRpcInvoke, HostRequestGolemRpcScheduledInvocation,
    HostRequestGolemRpcScheduledInvocationCancellation, HostResponseGolemRpcCreate,
    HostResponseGolemRpcInvokeAndAwait, HostResponseGolemRpcScheduledInvocation,
    HostResponseGolemRpcUnit, HostResponseGolemRpcUnitOrFailure, OplogEntry,
};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, IdempotencyKey, NamedRetryPolicy, OplogIndex,
    OwnedAgentId, RetryContext, RetryProperties, ScheduleId, ScheduledAction,
};
use golem_common::schema::agent::{AgentMethodSchema, AgentTypeSchema};
use golem_common::schema::schema_value::SchemaValue;
use golem_common::serialization::{deserialize, serialize};
use golem_schema::schema::wit::{decode_typed, decode_value, encode_value};

use crate::durable_host::golem::agent::schema_value_tree_to_typed_constructor_parameters;
use golem_schema::schema::wit::wire as core_wire;
use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use tracing::{Instrument, error};
use wasmtime::component::{Accessor, HasSelf, Resource, ResourceTableError};
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;

use golem_common::model::oplog::payload::HostRequestGolemRpcCreate;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;

/// Host-side resource table entry backing the `golem:agent/host.wasm-rpc` resource.
pub struct WasmRpcEntry {
    pub payload: Box<dyn std::any::Any + Send + Sync>,
}

/// Type-erased payload of a [`FutureInvokeResultEntry`] that can be polled for readiness.
#[async_trait::async_trait]
pub trait SubscribeAny: std::any::Any {
    async fn ready(&mut self);
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// Host-side resource table entry backing the `golem:agent/host.future-invoke-result` resource.
pub struct FutureInvokeResultEntry {
    pub payload: Box<dyn SubscribeAny + Send + Sync>,
    /// Tracks child Pollable rep indices created by `subscribe()`.
    /// Used to defer parent deletion until all children are dropped,
    /// because JS GC does not guarantee LIFO drop order.
    pub child_pollables: Vec<u32>,
    /// Set to `true` when the guest drops the parent while children still exist.
    /// The parent entry stays alive until the last child pollable is dropped.
    pub drop_pending: bool,
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for FutureInvokeResultEntry {
    async fn ready(&mut self) {
        self.payload.ready().await
    }
}

impl wasmtime_wasi::DynamicPollable for FutureInvokeResultEntry {
    fn override_index(&self) -> Option<u32> {
        None
    }
}

/// Host-side resource table entry backing the `golem:agent/host.cancellation-token` resource.
pub struct CancellationTokenEntry {
    pub schedule_id: Vec<u8>, // ScheduleId is defined locally in the worker-executor, so store a serialized version here
}

fn classify_rpc_error(err: &InternalRpcError) -> HostFailureKind {
    match err {
        InternalRpcError::ProtocolError { .. }
        | InternalRpcError::Denied { .. }
        | InternalRpcError::NotFound { .. } => HostFailureKind::Permanent,
        InternalRpcError::RemoteInternalError { .. } => HostFailureKind::Transient,
    }
}

impl<Ctx: WorkerCtx> HostWasmRpc for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        agent_type_name: String,
        constructor: core_wire::SchemaValueTree,
        phantom_id: Option<core_wire::Uuid>,
        config: Vec<
            golem_common::schema::agent::bindings::golem::agent::common::TypedAgentConfigValue,
        >,
    ) -> anyhow::Result<Resource<WasmRpcEntry>> {
        let mut env =
            wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(self).await?;
        crate::model::AgentConfig::remove_dynamic_vars(&mut env);

        let registered_agent_type = self
            .get_agent_type_schema_model(golem_common::model::agent::AgentTypeName(
                agent_type_name.clone(),
            ))
            .await?
            .ok_or_else(|| anyhow::anyhow!("Agent type '{}' not found", agent_type_name))?;

        let input = schema_value_tree_to_typed_constructor_parameters(
            &constructor,
            &registered_agent_type.agent_type,
        )
        .map_err(|err| anyhow::anyhow!("Invalid constructor input: {err}"))?;

        let component_id: golem_common::model::component::ComponentId =
            registered_agent_type.implemented_by.component_id;

        // Share the canonical agent type through `WasmRpcEntryPayload`. Every
        // subsequent RPC entry resolves the per-method input/output schema from
        // this cached value to drive the typed flow. `registered_agent_type` is
        // owned and no longer used, so move its agent type into the `Arc` rather
        // than cloning the whole schema graph.
        let remote_agent_type: Arc<AgentTypeSchema> = Arc::new(registered_agent_type.agent_type);

        let agent_id = golem_common::model::agent::ParsedAgentId::try_new(
            golem_common::model::agent::AgentTypeName(agent_type_name),
            input,
            phantom_id.map(|id| id.into()),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        let remote_agent_id = golem_common::model::AgentId::from_agent_id(component_id, &agent_id)
            .map_err(|err| anyhow::anyhow!("{err}"))?;

        let config = config
            .into_iter()
            .map(|c| {
                // The config value travels as a self-contained
                // `golem:core@2.0.0` typed-schema-value. Decode it and render
                // the inner `SchemaValue` as plain (schema-guided) JSON,
                // matching the `AgentConfigEntryDto` service-boundary contract:
                // the DTO carries plain user JSON which
                // `parse_worker_creation_agent_config` decodes with the schema
                // graph (`from_json_value`).
                let typed = decode_typed(&c.value)
                    .map_err(|err| anyhow::anyhow!("Invalid agent config value: {err}"))?;
                let encoded = golem_common::schema::render::to_json_value(
                    typed.graph(),
                    typed.root_type(),
                    typed.value(),
                )
                .map_err(|err| anyhow::anyhow!("Failed serializing agent config: {err}"))?;

                Ok::<_, anyhow::Error>(AgentConfigEntryDto {
                    path: c.path,
                    value: encoded.into(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let span = create_rpc_connection_span(self, &remote_agent_id).await?;

        let mut handle = CallHandle::<GolemRpcWasmRpcNew, NotCancellable>::start(
            self,
            HostRequestGolemRpcCreate {
                remote_agent_id: remote_agent_id.clone(),
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        if !handle.is_live() {
            match handle.replay(self).await? {
                CallReplayOutcome::Replayed(response) => {
                    return reconstruct_wasm_rpc_resource(
                        self,
                        remote_agent_id,
                        response.target_environment_id,
                        response.target_fingerprint,
                        span,
                        remote_agent_type,
                    )
                    .await;
                }
                CallReplayOutcome::Incomplete(live) => handle = live,
            }
        }

        construct_wasm_rpc_resource(
            self,
            handle,
            remote_agent_id,
            &env,
            config,
            span,
            remote_agent_type,
        )
        .await
    }

    async fn invoke_and_await(
        &mut self,
        self_: Resource<WasmRpcEntry>,
        method_name: String,
        input: core_wire::SchemaValueTree,
    ) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::wasm-rpc::invoke-and-await")
            .map_err(wasmtime::Error::from)?;

        let mut env =
            wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(self).await?;
        crate::model::AgentConfig::remove_dynamic_vars(&mut env);

        let own_agent_id = self.owned_agent_id().clone();

        let entry = self.table().get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_agent_id = payload.remote_agent_id.clone();
        let connection_span_id = payload.span_id.clone();
        let remote_agent_type = payload.remote_agent_type.clone();

        if remote_agent_id == own_agent_id {
            return Err(anyhow::anyhow!(
                "RPC calls to the same agent are not supported"
            ));
        }

        // Check the per-invocation RPC call limit before initiating the call.
        // Only counted in live mode; replay is a no-op.
        self.state
            .check_and_increment_rpc_call_count()
            .map_err(wasmtime::Error::from)?;

        // Returns Err(WorkerMonthlyRpcCallBudgetExhausted) when exhausted,
        // which maps to RetryDecision::TryStop — suspending the worker.
        self.record_monthly_rpc_call()?;

        // Resolve per-method schemas and lift the input. Both checks
        // are deterministic functions of the cached remote agent type
        // and the guest payload, so failures return `RpcError::*`
        // directly without opening durability or recording an oplog
        // entry — replay reaches the same outcome via the same code
        // path.
        let input_value =
            match resolve_method_and_lift_input(&remote_agent_type, &method_name, input) {
                Ok(parts) => parts,
                Err(rpc_error) => return Ok(Err(rpc_error.into())),
            };

        let oplog_index = self.state.oplog.current_oplog_index().await;
        let idempotency_key = self.derive_idempotency_key(oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &method_name, &idempotency_key)
                .await?;

        let request = HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: method_name.clone(),
            input: input_value.clone(),
            remote_agent_type: None,
            remote_agent_parameters: None,
        };

        let mut handle = CallHandle::<GolemRpcWasmRpcInvokeAndAwaitResult, NotCancellable>::start(
            self,
            request,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result: Result<SchemaValue, InternalRpcError> = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(persisted) => {
                        break 'result persisted.result.map_err(Into::into);
                    }
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties =
                RetryContext::rpc("invoke-and-await", &remote_agent_id, &method_name);
            let result: Result<SchemaValue, InternalRpcError> = loop {
                let stack = self.clone_as_inherited_stack(span.span_id());

                let interrupt_signal = self
                    .execution_status
                    .read()
                    .unwrap()
                    .create_await_interrupt_signal();
                let rpc = self.rpc();
                let created_by = self.created_by();
                let agent_id = self.agent_id().clone();
                let auth_ctx = self.agent_auth_ctx();

                let either_result = futures::future::select(
                    rpc.invoke_and_await(
                        &remote_agent_id,
                        Some(idempotency_key.clone()),
                        method_name.clone(),
                        input_value.clone(),
                        created_by,
                        &agent_id,
                        &env,
                        stack,
                        &auth_ctx,
                    ),
                    interrupt_signal,
                )
                .await;
                let result: Result<SchemaValue, InternalRpcError> = match either_result {
                    Either::Left((result, _)) => result,
                    Either::Right((interrupt_kind, _)) => {
                        tracing::info!("Interrupted while waiting for RPC result");
                        handle.abandon_for_trap();
                        return Err(interrupt_kind.into());
                    }
                };
                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &result,
                        classify_rpc_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };

            handle
                .complete(
                    self,
                    HostResponseGolemRpcInvokeAndAwait {
                        result: result.clone().map_err(Into::into),
                    },
                )
                .await?;
            result
        };

        self.finish_span(span.span_id()).await?;

        match result {
            Ok(value) => {
                // Project the schema-native reply to the WIT
                // `option<schema-value-tree>` shape (`none` for a `unit`
                // output) at the guest-facing boundary.
                Ok(Ok(schema_value_to_wire_output(&value)))
            }
            Err(err) => {
                error!("RPC error: {err}");
                Ok(Err(err.into()))
            }
        }
    }

    async fn invoke(
        &mut self,
        self_: Resource<WasmRpcEntry>,
        method_name: String,
        input: core_wire::SchemaValueTree,
    ) -> anyhow::Result<Result<(), RpcError>> {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::wasm-rpc::invoke")
            .map_err(wasmtime::Error::from)?;

        let mut env =
            wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(self).await?;
        crate::model::AgentConfig::remove_dynamic_vars(&mut env);

        let own_agent_id = self.owned_agent_id().clone();

        let entry = self.table().get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_agent_id = payload.remote_agent_id.clone();
        let connection_span_id = payload.span_id.clone();
        let remote_agent_type = payload.remote_agent_type.clone();

        if remote_agent_id == own_agent_id {
            return Err(anyhow::anyhow!(
                "RPC calls to the same agent are not supported"
            ));
        }

        // Check the per-invocation RPC call limit before initiating the call.
        self.state
            .check_and_increment_rpc_call_count()
            .map_err(wasmtime::Error::from)?;

        // Record against the monthly account-level RPC call quota.
        // Returns Err(WorkerMonthlyRpcCallBudgetExhausted) when exhausted,
        // which maps to RetryDecision::TryStop — suspending the worker.
        self.record_monthly_rpc_call()?;

        // Resolve the method and lift the input before opening durability
        // (see `invoke_and_await` for the rationale).
        let input_value =
            match resolve_method_and_lift_input(&remote_agent_type, &method_name, input) {
                Ok(parts) => parts,
                Err(rpc_error) => return Ok(Err(rpc_error.into())),
            };

        let oplog_index = self.state.oplog.current_oplog_index().await;
        let idempotency_key = self.derive_idempotency_key(oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &method_name, &idempotency_key)
                .await?;

        let request = HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: method_name.clone(),
            input: input_value.clone(),
            remote_agent_type: None,
            remote_agent_parameters: None,
        };

        let mut handle = CallHandle::<GolemRpcWasmRpcInvoke, NotCancellable>::start(
            self,
            request,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await {
                    Ok(CallReplayOutcome::Replayed(replayed)) => break 'result Ok(replayed),
                    Ok(CallReplayOutcome::Incomplete(live)) => handle = live,
                    Err(err) => break 'result Err(err.into()),
                }
            }

            let retry_properties = RetryContext::rpc("invoke", &remote_agent_id, &method_name);
            let result = loop {
                let stack = self.clone_as_inherited_stack(span.span_id());
                let result = self
                    .rpc()
                    .invoke(
                        &remote_agent_id,
                        Some(idempotency_key.clone()),
                        method_name.clone(),
                        input_value.clone(),
                        self.created_by(),
                        self.agent_id(),
                        &env,
                        stack,
                        &self.agent_auth_ctx(),
                    )
                    .await;
                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &result,
                        classify_rpc_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };

            let result = result.map_err(|err| err.into());
            handle
                .complete(self, HostResponseGolemRpcUnitOrFailure { result })
                .await
                .map_err(anyhow::Error::from)
        };

        self.finish_span(span.span_id()).await?;

        match result?.result {
            Ok(_) => Ok(Ok(())),
            Err(err) => {
                let rpc_error: crate::services::rpc::RpcError = err.into();
                error!("RPC error: {rpc_error}");
                Ok(Err(rpc_error.into()))
            }
        }
    }

    async fn async_invoke_and_await(
        &mut self,
        this: Resource<WasmRpcEntry>,
        method_name: String,
        input: core_wire::SchemaValueTree,
    ) -> anyhow::Result<Resource<FutureInvokeResult>> {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::wasm-rpc::async-invoke-and-await")
            .map_err(wasmtime::Error::from)?;

        let mut env =
            wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(self).await?;
        crate::model::AgentConfig::remove_dynamic_vars(&mut env);

        let own_agent_id = self.owned_agent_id().clone();

        let entry = self.table().get(&this)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_agent_id = payload.remote_agent_id.clone();
        let connection_span_id = payload.span_id.clone();
        let remote_agent_type = payload.remote_agent_type.clone();

        if remote_agent_id == own_agent_id {
            return Err(anyhow::anyhow!(
                "RPC calls to the same agent are not supported"
            ));
        }

        // Check the per-invocation RPC call limit before initiating the call.
        self.state
            .check_and_increment_rpc_call_count()
            .map_err(wasmtime::Error::from)?;

        // Returns Err(WorkerMonthlyRpcCallBudgetExhausted) when exhausted,
        // which maps to RetryDecision::TryStop — suspending the worker.
        self.record_monthly_rpc_call()?;

        // Resolve the method and lift the input before opening any durability. Failures here are
        // deterministic functions of the cached remote agent type and the guest payload, so they
        // are baked into the future's result and surfaced on the first `get` — without opening a
        // durable host call. Live and replay agree because the resolution is pure and no oplog
        // entry (beyond the invocation span) is written for it.
        let input_value =
            match resolve_method_and_lift_input(&remote_agent_type, &method_name, input) {
                Ok(parts) => parts,
                Err(rpc_error) => {
                    // The method/input could not be resolved, so no remote call is dispatched. The
                    // idempotency key is informational only and is derived from the current oplog
                    // index; it exists solely to label the invocation span.
                    let oplog_index = self.state.oplog.current_oplog_index().await;
                    let idempotency_key = self.derive_idempotency_key(oplog_index);
                    let span = create_invocation_span(
                        self,
                        &connection_span_id,
                        &method_name,
                        &idempotency_key,
                    )
                    .await?;
                    let fut = self.table().push(FutureInvokeResultEntry {
                        payload: Box::new(FutureInvokeResultState::Baked {
                            result: Ok(Err(rpc_error)),
                            span_id: span.span_id().clone(),
                        }),
                        child_pollables: Vec::new(),
                        drop_pending: false,
                    })?;
                    return Ok(fut);
                }
            };

        // Open the single durable host call for this async RPC as a `WriteRemote` — the same
        // durable function type as the synchronous `invoke_and_await`. It is a two-step call:
        // `begin` yields the begin index and `start_live` then appends the eager host-call `Start`
        // with the built request. The remote idempotency key is derived from the begin index.
        // `start_live` appends the host-call `Start` unconditionally (even under
        // `assume_idempotence`), and the accessor terminals (`complete_access` / `cancel_access` /
        // `replay_access`) all support `WriteRemote`, so no separate durable scope is needed to make
        // the key unique: each concurrently-created future advances the oplog by its own `Start` and
        // therefore derives a distinct key. Under `assume_idempotence` `begin` opens no scope, so
        // the begin index equals the host-call `Start` index; otherwise `begin` opens the durable
        // scope that the terminal later closes. The read-only side-effect guard was already applied
        // at the top of this function and is re-applied by `begin`.
        let begun = CallHandle::<GolemRpcWasmRpcInvokeAndAwaitResult, Cancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let begin_index = begun.begin_index();
        let idempotency_key = self.derive_idempotency_key(begin_index);

        let span =
            create_invocation_span(self, &connection_span_id, &method_name, &idempotency_key)
                .await?;

        let request = HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: method_name.clone(),
            input: input_value.clone(),
            remote_agent_type: None,
            remote_agent_parameters: None,
        };

        if begun.is_live() {
            let handle = match begun.start_live(self, request.clone()).await {
                Ok(handle) => handle,
                Err(err) => {
                    // The eager `Start` could not be written; close any durable scope opened by
                    // `begin` and finish the span so no half-open call is left behind.
                    self.end_function(&DurableFunctionType::WriteRemote, begin_index)
                        .await?;
                    self.finish_span(span.span_id()).await?;
                    return Err(err.into());
                }
            };

            let task = spawn_invoke_and_await_task(
                self,
                &remote_agent_id,
                idempotency_key,
                method_name,
                input_value,
                env.clone(),
                span.span_id(),
                begin_index,
            );

            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Active {
                    handle: Some(handle),
                    task: Some(Arc::new(tokio::sync::Mutex::new(task))),
                    request,
                    remote_agent_id,
                    env,
                    span_id: span.span_id().clone(),
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            Ok(fut)
        } else {
            // Replay: claim the eager `Start` from the oplog now. The RPC is not re-dispatched here.
            // On a normal replay `get` replays the matching `End`, and `cancel` / `drop` consume the
            // matching `Cancelled`. If the worker crashed after this `Start` but before its terminal,
            // `get`'s `replay_access` returns `Incomplete` (a read-only `WriteRemote` call is safe to
            // re-execute) and `get` re-dispatches the RPC there to complete the existing `Start`.
            let handle = begun.start_replay(self).await?;
            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Active {
                    handle: Some(handle),
                    task: None,
                    request,
                    remote_agent_id,
                    env,
                    span_id: span.span_id().clone(),
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            Ok(fut)
        }
    }

    async fn schedule_invocation(
        &mut self,
        this: Resource<WasmRpcEntry>,
        scheduled_time: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
        method_name: String,
        input: core_wire::SchemaValueTree,
    ) -> anyhow::Result<()> {
        let token = self
            .schedule_cancelable_invocation(this, scheduled_time, method_name, input)
            .await?;
        let _ = self.table().delete(token)?;
        Ok(())
    }

    async fn schedule_cancelable_invocation(
        &mut self,
        this: Resource<WasmRpcEntry>,
        datetime: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
        method_name: String,
        input: core_wire::SchemaValueTree,
    ) -> anyhow::Result<Resource<CancellationToken>> {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::wasm-rpc::schedule-cancelable-invocation")
            .map_err(wasmtime::Error::from)?;

        // Deterministic local validation must happen before opening
        // durability so a guest bug (unknown method, input incompatible
        // with the declared schema, or invalid datetime) does not leave
        // an open durable function. `schedule_cancelable_invocation`
        // has no `RpcError` return channel, so these are surfaced as
        // wasmtime traps.
        let (remote_agent_id, target_worker_fingerprint, input_value) = {
            let entry = self.table().get(&this)?;
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
            let remote_agent_id = payload.remote_agent_id.clone();
            let target_worker_fingerprint = payload.target_fingerprint;
            let remote_agent_type = payload.remote_agent_type.clone();

            // Validate the method exists, then transport the input as a
            // schema-free `SchemaValue` (the callee validates against its own
            // schema when it lowers the scheduled invocation).
            find_agent_method(&remote_agent_type, &method_name)?;
            let input_value =
                decode_value(&input).map_err(|err| anyhow::anyhow!("Invalid RPC input: {err}"))?;

            (remote_agent_id, target_worker_fingerprint, input_value)
        };
        let scheduled_at = chrono::DateTime::from_timestamp(datetime.seconds, datetime.nanoseconds)
            .ok_or_else(|| {
                anyhow::Error::from(WorkerExecutorError::runtime(format!(
                    "Received invalid datetime from wasi: seconds={}, nanoseconds={}",
                    datetime.seconds, datetime.nanoseconds
                )))
            })?;

        // The persisted request embeds an idempotency key derived from the durable-scope begin
        // index, so this is a two-step call: open the scope first to learn the index, then build
        // the request from it.
        let begun = CallHandle::<GolemRpcWasmRpcScheduleInvocation, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let begin_index = begun.begin_index();

        // Obtain a live handle to complete — either freshly (writing the eager `Start`) or by
        // recovering an incomplete `Start` from a previous run — or short-circuit on a fully
        // replayed call. The idempotency key is derived once per execution from `begin_index`
        // (which is stable across an incomplete-replay re-execution), so both live paths reproduce
        // the same key and `ScheduleId`.
        let idempotency_key;
        let handle;
        if begun.is_live() {
            idempotency_key = self.derive_idempotency_key(begin_index);

            let request = HostRequestGolemRpcScheduledInvocation {
                remote_agent_id: remote_agent_id.agent_id(),
                idempotency_key: idempotency_key.clone(),
                method_name: method_name.clone(),
                input: input_value.clone(),
                datetime: datetime.into(),
                remote_agent_type: None,
                remote_agent_parameters: None,
            };
            handle = begun.start_live(self, request).await?;
        } else {
            match begun.start_replay(self).await?.replay(self).await? {
                CallReplayOutcome::Replayed(result) => {
                    let serialized_result = serialize(&result.schedule_id).map_err(|err| {
                        anyhow::Error::from(WorkerExecutorError::runtime(format!(
                            "Failed to serialize schedule id: {err}"
                        )))
                    })?;
                    let resource = self.table().push(CancellationTokenEntry {
                        schedule_id: serialized_result,
                    })?;
                    return Ok(resource);
                }
                CallReplayOutcome::Incomplete(live) => {
                    idempotency_key = self.derive_idempotency_key(begin_index);
                    handle = live;
                }
            }
        }

        let schedule_id = ScheduleId::from_idempotency_key(&idempotency_key);

        let stack = InvocationContextStack::new(
            self.state.invocation_context.trace_id.clone(),
            InvocationContextSpan::external_parent(self.state.current_span_id.clone()),
            self.state.invocation_context.trace_states.clone(),
        );

        let action = ScheduledAction::Invoke {
            account_id: self.created_by(),
            owned_agent_id: remote_agent_id,
            invocation: Box::new(AgentInvocation::AgentMethod {
                idempotency_key,
                method_name,
                input: input_value,
                invocation_context: stack,
                principal: Principal::anonymous(),
            }),
            target_worker_fingerprint,
        };

        let result = self
            .state
            .scheduler_service
            .schedule_with_id(schedule_id, scheduled_at, action)
            .await;

        let schedule_id = SerializableScheduleId::from_domain(result);

        let result = handle
            .complete(
                self,
                HostResponseGolemRpcScheduledInvocation { schedule_id },
            )
            .await?;

        let serialized_result = serialize(&result.schedule_id).map_err(|err| {
            anyhow::Error::from(WorkerExecutorError::runtime(format!(
                "Failed to serialize schedule id: {err}"
            )))
        })?;
        let cancellation_token = CancellationTokenEntry {
            schedule_id: serialized_result,
        };

        let resource = self.table().push(cancellation_token)?;
        Ok(resource)
    }

    async fn drop(&mut self, rep: Resource<WasmRpcEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::wasm-rpc", "drop");

        let entry = self.table().delete(rep)?;
        let payload = entry.payload.downcast::<WasmRpcEntryPayload>();
        if let Ok(payload) = payload {
            self.finish_span(&payload.span_id).await?;
        }

        Ok(())
    }
}

type FutureInvokeTaskResult = Result<Result<SchemaValue, InternalRpcError>, Error>;
type FutureInvokeTaskHandle =
    Arc<tokio::sync::Mutex<AbortOnDropJoinHandle<FutureInvokeTaskResult>>>;
type FutureInvokeGetResult = Result<Result<SchemaValue, RpcError>, Error>;

/// The single durable host call representing one `async-invoke-and-await` operation. Its eager
/// `Start` is written when the future is created (`async_invoke_and_await`) and its `End` /
/// `Cancelled` when the future is awaited via `get`, cancelled, or dropped — so an async RPC is
/// recorded like any other durable host call, with its two halves possibly non-adjacent in the
/// oplog. Writing the `Start` eagerly at creation advances the oplog even under `assume_idempotence`,
/// which is what gives each concurrently-created future a distinct begin index and therefore a
/// distinct derived remote idempotency key. `Cancellable`: a future dropped before completion
/// records a `Cancelled`.
type FutureInvokeCallHandle = CallHandle<GolemRpcWasmRpcInvokeAndAwaitResult, Cancellable>;

/// Projects a background RPC task result (as produced for the [`FutureInvokeResultState::Baked`]
/// path) into the wire result shape returned by `future-invoke-result.get`. A hard task failure
/// (`Err`) is surfaced as a `get` trap.
fn future_invoke_get_result_to_wire(
    result: FutureInvokeGetResult,
) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
    match result {
        Ok(Ok(value)) => Ok(Ok(schema_value_to_wire_output(&value))),
        Ok(Err(rpc_error)) => Ok(Err(rpc_error)),
        Err(err) => Err(err),
    }
}

/// Projects a completed `invoke-and-await` durable response (the payload of the call's `End`) into
/// the wire result shape returned by `future-invoke-result.get`.
fn invoke_and_await_response_to_wire(
    result: Result<SchemaValue, SerializableRpcError>,
) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
    match result {
        Ok(value) => Ok(Ok(schema_value_to_wire_output(&value))),
        Err(error) => {
            let rpc_error: InternalRpcError = error.into();
            Ok(Err(rpc_error.into()))
        }
    }
}

fn future_invoke_task_result_to_get_result(
    result: &FutureInvokeTaskResult,
) -> FutureInvokeGetResult {
    match result {
        Ok(Ok(value)) => Ok(Ok(value.clone())),
        Ok(Err(error)) => Ok(Err(error.clone().into())),
        Err(err) => Err(anyhow::anyhow!(err.to_string())),
    }
}

async fn finish_span_access<T, Ctx: WorkerCtx>(
    accessor: &Accessor<T, HasSelf<DurableWorkerCtx<Ctx>>>,
    span_id: &SpanId,
) -> Result<(), WorkerExecutorError> {
    let (is_live, worker, replay_state) = accessor.with(|mut access| {
        let ctx = access.get();
        (
            ctx.state.is_live(),
            ctx.public_state.worker(),
            ctx.state.replay_state.clone(),
        )
    });

    if is_live {
        worker
            .add_to_oplog(OplogEntry::finish_span(span_id.clone()))
            .await;
    } else {
        crate::get_oplog_entry!(replay_state, OplogEntry::FinishSpan)?;
    }

    accessor.with(|mut access| {
        let ctx = access.get();
        if &ctx.state.current_span_id == span_id {
            let span = ctx.state.invocation_context.get(span_id).map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "span {span_id} missing during finish_span replay: {err}"
                ))
            })?;
            ctx.state.current_span_id = span
                .parent()
                .map(|p| p.span_id().clone())
                .unwrap_or_else(|| ctx.state.invocation_context.root.span_id().clone());
        }
        let _ = ctx
            .state
            .invocation_context
            .finish_span(span_id)
            .map_err(WorkerExecutorError::runtime);
        Ok(())
    })
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostFutureInvokeResultWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn get(
        accessor: &Accessor<U, Self>,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
        let this_rep = this.rep();

        // Decide what `get` must do while holding the state lock, then run the async terminal
        // outside it. The durable call handle (and, on the live path, the background task) are taken
        // out of the state here so the terminal runs while nothing in the resource table still owns
        // them: a live handle left behind and later dropped would spuriously enqueue a `Cancelled`.
        #[allow(clippy::large_enum_variant)]
        enum GetPlan {
            /// The result is already known — a baked deterministic failure or a prior cancellation.
            Ready {
                result: anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>>,
                span_id: SpanId,
                finish_span: bool,
            },
            /// The single durable call is still open; drive it to its `End`. `request`,
            /// `remote_agent_id`, and `env` are carried so a replay that finds an incomplete `Start`
            /// (crash-after-`Start`) can re-dispatch the read-only RPC and complete it.
            Active {
                handle: FutureInvokeCallHandle,
                task: Option<FutureInvokeTaskHandle>,
                request: HostRequestGolemRpcInvoke,
                remote_agent_id: OwnedAgentId,
                env: Vec<(String, String)>,
                span_id: SpanId,
            },
        }

        let plan = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("golem::rpc::future-invoke-result", "get");
            let entry = ctx
                .table()
                .get_mut(&Resource::<FutureInvokeResult>::new_borrow(this_rep))?;
            let state = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            Ok::<_, anyhow::Error>(match state {
                FutureInvokeResultState::Consumed { span_id, .. } => GetPlan::Ready {
                    result: Err(anyhow::Error::new(ClassifiedHostError {
                        kind: HostFailureKind::Permanent,
                        message: "future-invoke-result already consumed".to_string(),
                    })),
                    span_id: span_id.clone(),
                    finish_span: false,
                },
                FutureInvokeResultState::Baked { result, span_id } => {
                    let wire = future_invoke_get_result_to_wire(
                        future_invoke_task_result_to_get_result(result),
                    );
                    let span_id = span_id.clone();
                    *state = FutureInvokeResultState::Consumed {
                        span_id: span_id.clone(),
                    };
                    GetPlan::Ready {
                        result: wire,
                        span_id,
                        finish_span: true,
                    }
                }
                FutureInvokeResultState::Cancelled { span_id } => {
                    let rpc_error = InternalRpcError::ProtocolError {
                        details: "Invocation cancelled".to_string(),
                    };
                    let span_id = span_id.clone();
                    *state = FutureInvokeResultState::Consumed {
                        span_id: span_id.clone(),
                    };
                    GetPlan::Ready {
                        result: Ok(Err(rpc_error.into())),
                        span_id,
                        // The span was already finished by `cancel`.
                        finish_span: false,
                    }
                }
                FutureInvokeResultState::Active {
                    handle,
                    task,
                    request,
                    remote_agent_id,
                    env,
                    span_id,
                } => {
                    let handle = handle
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("future-invoke-result already consumed"))?;
                    GetPlan::Active {
                        handle,
                        task: task.take(),
                        request: request.clone(),
                        remote_agent_id: remote_agent_id.clone(),
                        env: env.clone(),
                        span_id: span_id.clone(),
                    }
                }
            })
        })?;

        match plan {
            GetPlan::Ready {
                result,
                span_id,
                finish_span,
            } => {
                if finish_span {
                    finish_span_access(accessor, &span_id).await?;
                }
                result
            }
            GetPlan::Active {
                mut handle,
                task,
                request,
                remote_agent_id,
                env,
                span_id,
            } => {
                let response = if handle.is_live() {
                    let task =
                        task.expect("a live future-invoke-result must own its background task");
                    let task_result = {
                        let mut task = task.lock().await;
                        (&mut *task).await
                    };
                    match task_result {
                        Ok(rpc_result) => handle
                            .complete_access(
                                accessor,
                                accessor.getter(),
                                HostResponseGolemRpcInvokeAndAwait {
                                    result: rpc_result.map_err(Into::into),
                                },
                            )
                            .await
                            .map_err(anyhow::Error::from)?,
                        Err(err) => {
                            // The background RPC failed hard after its in-task retries. This is a
                            // trap, not a durable result: abandon the call, leaving its `Start`
                            // incomplete for durable-scope recovery, instead of recording an `End`.
                            return Err(handle.trap(anyhow::anyhow!(err.to_string())));
                        }
                    }
                } else {
                    match handle
                        .replay_access(accessor, accessor.getter())
                        .await
                        .map_err(anyhow::Error::from)?
                    {
                        CallReplayOutcome::Replayed(response) => response,
                        CallReplayOutcome::Incomplete(mut live) => {
                            // Crash-after-`Start` recovery: the eager `Start` is committed but its
                            // terminal was never written. A read-only `WriteRemote` call is safe to
                            // re-execute, so re-dispatch the RPC now and complete the existing `Start`
                            // (mirrors the synchronous path's `Incomplete` -> re-run).
                            let retry_point = live.begin_index();
                            let mut task = accessor.with(|mut access| {
                                let ctx = access.get();
                                spawn_invoke_and_await_task(
                                    ctx,
                                    &remote_agent_id,
                                    request.idempotency_key.clone(),
                                    request.method_name.clone(),
                                    request.input.clone(),
                                    env.clone(),
                                    &span_id,
                                    retry_point,
                                )
                            });
                            match (&mut task).await {
                                Ok(rpc_result) => live
                                    .complete_access(
                                        accessor,
                                        accessor.getter(),
                                        HostResponseGolemRpcInvokeAndAwait {
                                            result: rpc_result.map_err(Into::into),
                                        },
                                    )
                                    .await
                                    .map_err(anyhow::Error::from)?,
                                Err(err) => {
                                    return Err(live.trap(anyhow::anyhow!(err.to_string())));
                                }
                            }
                        }
                    }
                };

                finish_span_access(accessor, &span_id).await?;
                accessor.with(|mut access| {
                    let ctx = access.get();
                    let entry = ctx
                        .table()
                        .get_mut(&Resource::<FutureInvokeResult>::new_borrow(this_rep))?;
                    let state = entry
                        .payload
                        .as_any_mut()
                        .downcast_mut::<FutureInvokeResultState>()
                        .unwrap();
                    *state = FutureInvokeResultState::Consumed {
                        span_id: span_id.clone(),
                    };
                    Ok::<_, anyhow::Error>(())
                })?;

                invoke_and_await_response_to_wire(response.result)
            }
        }
    }

    async fn drop(
        accessor: &Accessor<U, Self>,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<()> {
        let future_rep = this.rep();

        #[allow(clippy::large_enum_variant)]
        enum DropPlan {
            /// An open durable call must be cancelled and its span finished.
            Cancel {
                handle: FutureInvokeCallHandle,
                span_id: SpanId,
            },
            /// No durable call, but the invocation span of a baked failure is still open.
            FinishSpan { span_id: SpanId },
            /// Nothing to finish — already consumed or cancelled.
            Nothing,
        }

        let plan = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("golem::rpc::future-invoke-result", "drop");
            let entry = ctx
                .table()
                .get_mut(&Resource::<FutureInvokeResult>::new_borrow(future_rep))?;
            let state = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            Ok::<_, anyhow::Error>(match state {
                FutureInvokeResultState::Active {
                    handle, span_id, ..
                } => match handle.take() {
                    Some(handle) => DropPlan::Cancel {
                        handle,
                        span_id: span_id.clone(),
                    },
                    None => DropPlan::Nothing,
                },
                FutureInvokeResultState::Baked { span_id, .. } => DropPlan::FinishSpan {
                    span_id: span_id.clone(),
                },
                FutureInvokeResultState::Cancelled { .. }
                | FutureInvokeResultState::Consumed { .. } => DropPlan::Nothing,
            })
        })?;

        match plan {
            DropPlan::Cancel { handle, span_id } => {
                handle
                    .cancel_access(accessor, accessor.getter(), None)
                    .await
                    .map_err(anyhow::Error::from)?;
                finish_span_access(accessor, &span_id).await?;
            }
            DropPlan::FinishSpan { span_id } => {
                finish_span_access(accessor, &span_id).await?;
            }
            DropPlan::Nothing => {}
        }

        accessor.with(|mut access| {
            let ctx = access.get();
            match ctx.table().delete(this) {
                Ok(entry) => {
                    for child_rep in &entry.child_pollables {
                        ctx.state.rpc_pollable_to_parent.remove(child_rep);
                    }
                }
                Err(ResourceTableError::HasChildren) => {
                    let parent: Resource<FutureInvokeResult> = Resource::new_borrow(future_rep);
                    ctx.table().get_mut(&parent)?.drop_pending = true;
                }
                Err(err) => return Err(err.into()),
            }

            Ok(())
        })
    }
}

impl<Ctx: WorkerCtx> HostFutureInvokeResult for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, this: Resource<FutureInvokeResult>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::future-invoke-result", "cancel");

        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::future-invoke-result::cancel")
            .map_err(wasmtime::Error::from)?;

        // Decide what to cancel while holding the table borrow, taking the durable call handle out
        // of the state (a live handle must never drop while still owned here). The background task,
        // if any, is dropped together with the old `Active` state, aborting the in-flight RPC.
        #[allow(clippy::large_enum_variant)]
        enum CancelPlan {
            /// The single durable call is still open; record its `Cancelled` and, for a live call,
            /// best-effort cancel the remote invocation.
            Cancel {
                handle: FutureInvokeCallHandle,
                remote_agent_id: AgentId,
                idempotency_key: IdempotencyKey,
                span_id: SpanId,
            },
            /// Nothing to cancel: a baked failure, or already cancelled / consumed.
            Nothing,
        }

        let plan = {
            let entry = self.table().get_mut(&this)?;
            let state = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            match state {
                FutureInvokeResultState::Active {
                    handle,
                    request,
                    span_id,
                    ..
                } => match handle.take() {
                    Some(handle) => {
                        let remote_agent_id = request.remote_agent_id.clone();
                        let idempotency_key = request.idempotency_key.clone();
                        let span_id = span_id.clone();
                        *state = FutureInvokeResultState::Cancelled {
                            span_id: span_id.clone(),
                        };
                        CancelPlan::Cancel {
                            handle,
                            remote_agent_id,
                            idempotency_key,
                            span_id,
                        }
                    }
                    None => CancelPlan::Nothing,
                },
                FutureInvokeResultState::Baked { .. }
                | FutureInvokeResultState::Cancelled { .. }
                | FutureInvokeResultState::Consumed { .. } => CancelPlan::Nothing,
            }
        };

        if let CancelPlan::Cancel {
            handle,
            remote_agent_id,
            idempotency_key,
            span_id,
        } = plan
        {
            // Best-effort remote cancellation, only meaningful for a live call — on replay the
            // recorded `Cancelled` is re-applied without re-issuing the side effect.
            if handle.is_live()
                && let Err(err) = self
                    .worker_proxy()
                    .cancel_invocation(&remote_agent_id, idempotency_key, &self.agent_auth_ctx())
                    .await
            {
                tracing::info!(err=%err, "Best-effort cancel_invocation failed");
            }

            handle
                .cancel(self, None)
                .await
                .map_err(anyhow::Error::from)?;
            self.finish_span(&span_id).await?;
        }

        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostCancellationToken for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, this: Resource<CancellationToken>) -> anyhow::Result<()> {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::cancellation-token::cancel")
            .map_err(wasmtime::Error::from)?;

        let entry = self.table().get(&this)?;
        let serialized_schedule_id: SerializableScheduleId = deserialize(&entry.schedule_id)
            .map_err(|err| {
                anyhow::Error::from(WorkerExecutorError::runtime(format!(
                    "Failed to deserialize cancellation token: {err}"
                )))
            })?;

        let mut handle = CallHandle::<GolemRpcCancellationTokenCancel, NotCancellable>::start(
            self,
            HostRequestGolemRpcScheduledInvocationCancellation {
                schedule_id: serialized_schedule_id.clone(),
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        'cancel: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(_) => break 'cancel,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            self.scheduler_service()
                .cancel(serialized_schedule_id.into_domain())
                .await;

            handle.complete(self, HostResponseGolemRpcUnit {}).await?;
        }

        Ok(())
    }

    async fn drop(&mut self, this: Resource<CancellationToken>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::cancellation-token", "drop");
        let _ = self.table().delete(this)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> core_wire::Host for DurableWorkerCtx<Ctx> {
    async fn parse_uuid(
        &mut self,
        uuid: String,
    ) -> anyhow::Result<Result<core_wire::Uuid, String>> {
        Ok(uuid::Uuid::parse_str(&uuid)
            .map(|uuid| uuid.into())
            .map_err(|e| e.to_string()))
    }

    async fn uuid_to_string(&mut self, uuid: core_wire::Uuid) -> anyhow::Result<String> {
        let uuid: uuid::Uuid = uuid.into();
        Ok(uuid.to_string())
    }
}

pub async fn construct_wasm_rpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    mut handle: CallHandle<GolemRpcWasmRpcNew, NotCancellable>,
    remote_agent_id: AgentId,
    env: &[(String, String)],
    config: Vec<AgentConfigEntryDto>,
    span: Arc<InvocationContextSpan>,
    remote_agent_type: Arc<AgentTypeSchema>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let stack = ctx.clone_as_inherited_stack(span.span_id());

    let target_component = match ctx
        .component_service()
        .get_metadata(remote_agent_id.component_id, None)
        .await
    {
        Ok(target_component) => target_component,
        Err(err) => {
            return Err(handle.trap(err));
        }
    };
    let target_environment_id = target_component.environment_id;
    let remote_agent_id = OwnedAgentId::new(target_environment_id, &remote_agent_id);
    let demand = match ctx
        .rpc()
        .create_demand(
            &remote_agent_id,
            ctx.created_by(),
            ctx.agent_id(),
            env,
            stack,
            config,
            &ctx.agent_auth_ctx(),
        )
        .await
    {
        Ok(demand) => demand,
        Err(err) => {
            return Err(handle.trap(err));
        }
    };
    let target_fingerprint = demand.fingerprint();

    handle
        .complete(
            ctx,
            HostResponseGolemRpcCreate {
                target_fingerprint,
                target_environment_id,
            },
        )
        .await?;

    let entry = ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            demand,
            remote_agent_id,
            span_id: span.span_id().clone(),
            target_fingerprint,
            remote_agent_type,
        }),
    })?;
    Ok(entry)
}

async fn reconstruct_wasm_rpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: AgentId,
    target_environment_id: EnvironmentId,
    target_fingerprint: AgentFingerprint,
    span: Arc<InvocationContextSpan>,
    remote_agent_type: Arc<AgentTypeSchema>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let remote_agent_id = OwnedAgentId::new(target_environment_id, &remote_agent_id);
    let entry = ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            demand: Box::new(crate::services::rpc::ReplayedDemand::new(
                target_fingerprint,
            )),
            remote_agent_id,
            span_id: span.span_id().clone(),
            target_fingerprint,
            remote_agent_type,
        }),
    })?;
    Ok(entry)
}

struct TaskRetryParams<Ctx: WorkerCtx> {
    environment_state_service: Arc<dyn EnvironmentStateService>,
    environment_id: EnvironmentId,
    default_retry_policy: NamedRetryPolicy,
    agent_config_retry_policies: Vec<NamedRetryPolicy>,
    runtime_retry_policy_mutations: std::collections::BTreeMap<String, Option<NamedRetryPolicy>>,
    retry_properties: RetryProperties,
    max_in_function_retry_delay: Duration,
    worker: Arc<crate::worker::Worker<Ctx>>,
    retry_point: OplogIndex,
    execution_status: Arc<std::sync::RwLock<crate::model::ExecutionStatus>>,
}

fn spawn_rpc_task_with_retry<Ctx: WorkerCtx>(
    rpc: Arc<dyn Rpc>,
    remote_agent_id: OwnedAgentId,
    idempotency_key: IdempotencyKey,
    method_name: String,
    input: SchemaValue,
    created_by: AccountId,
    agent_id: AgentId,
    env: Vec<(String, String)>,
    stack: InvocationContextStack,
    retry_params: Option<TaskRetryParams<Ctx>>,
    auth_ctx: AuthCtx,
) -> AbortOnDropJoinHandle<Result<Result<SchemaValue, InternalRpcError>, Error>> {
    let invoke = move || {
        let rpc = rpc.clone();
        let remote_agent_id = remote_agent_id.clone();
        let idempotency_key = idempotency_key.clone();
        let method_name = method_name.clone();
        let input = input.clone();
        let created_by = created_by;
        let agent_id = agent_id.clone();
        let env = env.clone();
        let stack = stack.clone();
        let auth_ctx = auth_ctx.clone();
        async move {
            let result = rpc
                .invoke_and_await(
                    &remote_agent_id,
                    Some(idempotency_key),
                    method_name,
                    input,
                    created_by,
                    &agent_id,
                    &env,
                    stack,
                    &auth_ctx,
                )
                .await?;
            Ok(result)
        }
    };

    wasmtime_wasi::runtime::spawn(
        async move {
            let result = if let Some(retry_params) = retry_params {
                let execution_status = retry_params.execution_status;
                let current_retry_policy_state = retry_params
                    .worker
                    .get_non_detached_last_known_status()
                    .await
                    .current_retry_state
                    .get(&retry_params.retry_point)
                    .cloned();
                let task_ctx = crate::durable_host::durability::TaskRetryContext {
                    retry_point: retry_params.retry_point,
                    environment_state_service: retry_params.environment_state_service,
                    environment_id: retry_params.environment_id,
                    default_retry_policy: retry_params.default_retry_policy,
                    agent_config_retry_policies: retry_params.agent_config_retry_policies,
                    runtime_retry_policy_mutations: retry_params.runtime_retry_policy_mutations,
                    max_in_function_retry_delay: retry_params.max_in_function_retry_delay,
                    current_retry_policy_state,
                    retry_properties: retry_params.retry_properties,
                    worker: retry_params.worker,
                };
                crate::durable_host::durability::in_task_retry_loop(
                    task_ctx,
                    classify_rpc_error,
                    invoke,
                    || {
                        execution_status
                            .read()
                            .unwrap()
                            .create_await_interrupt_signal()
                    },
                )
                .await
            } else {
                invoke().await
            };
            Ok(result)
        }
        .in_current_span(),
    )
}

/// Spawns the background RPC task for one `async-invoke-and-await` future, building the in-task
/// retry context from the current worker state. Used both when the future is first created (live)
/// and by `get` when replay finds the eager `Start` committed without a terminal (crash-after-`Start`
/// recovery), where the read-only call is re-dispatched to complete the existing `Start`. Retries
/// are enabled unless the call was initiated inside an atomic region.
fn spawn_invoke_and_await_task<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: &OwnedAgentId,
    idempotency_key: IdempotencyKey,
    method_name: String,
    input: SchemaValue,
    env: Vec<(String, String)>,
    span_id: &SpanId,
    retry_point: OplogIndex,
) -> AbortOnDropJoinHandle<FutureInvokeTaskResult> {
    let rpc = ctx.rpc();
    let created_by = ctx.created_by();
    let agent_id = ctx.agent_id().clone();
    let auth_ctx = ctx.agent_auth_ctx();
    let stack = ctx.clone_as_inherited_stack(span_id);

    let retry_params = if ctx.in_atomic_region() {
        None
    } else {
        let mut retry_properties =
            RetryContext::rpc("invoke-and-await", remote_agent_id, &method_name);
        ctx.state.enrich_retry_properties(&mut retry_properties);
        Some(TaskRetryParams {
            environment_state_service: ctx.state.environment_state_service.clone(),
            environment_id: ctx.state.owned_agent_id.environment_id,
            default_retry_policy: NamedRetryPolicy::default_from_config(&ctx.state.config.retry),
            agent_config_retry_policies: ctx.state.agent_config_retry_policies(),
            runtime_retry_policy_mutations: ctx.state.runtime_retry_policy_mutations.clone(),
            retry_properties,
            max_in_function_retry_delay: ctx.durable_execution_state().max_in_function_retry_delay,
            worker: ctx.public_state.worker(),
            retry_point,
            execution_status: ctx.execution_status.clone(),
        })
    };

    spawn_rpc_task_with_retry(
        rpc,
        remote_agent_id.clone(),
        idempotency_key,
        method_name,
        input,
        created_by,
        agent_id,
        env,
        stack,
        retry_params,
        auth_ctx,
    )
}

pub struct WasmRpcEntryPayload {
    #[allow(dead_code)]
    pub demand: Box<dyn RpcDemand>,
    pub remote_agent_id: OwnedAgentId,
    pub span_id: SpanId,
    pub target_fingerprint: AgentFingerprint,
    /// Cached remote agent type, used to resolve per-method input/output
    /// schemas for the in-process [`SchemaValue`] / [`TypedSchemaValue`]
    /// flow. Sourced from the durable `get_agent_type` lookup performed in
    /// [`HostWasmRpc::new`], so it is consistent across live execution and
    /// replay.
    pub remote_agent_type: Arc<AgentTypeSchema>,
}

impl Debug for WasmRpcEntryPayload {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmRpcEntryPayload")
            .field("remote_agent_id", &self.remote_agent_id)
            .finish()
    }
}

/// Look up an [`AgentMethod`] by name from the cached remote agent type.
/// Used on the schedule path where the result is surfaced as a
/// `wasmtime::Error` trap, since `schedule_cancelable_invocation` has no
/// way to return `Err(RpcError)` to the guest.
fn find_agent_method<'a>(
    agent_type: &'a AgentTypeSchema,
    method_name: &str,
) -> anyhow::Result<&'a AgentMethodSchema> {
    agent_type
        .methods
        .iter()
        .find(|m| m.name == method_name)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Method '{method_name}' not found on agent type '{}'",
                agent_type.type_name
            )
        })
}

/// Resolve and lift the guest-side input value tree into the schema-native
/// [`SchemaValue`] carrier used across the executor↔executor RPC hop.
///
/// The wire value tree is transported as a schema-free [`SchemaValue`]; each
/// end validates it against its own declared schema (the callee when it lowers
/// the invocation, see [`lower_invocation`](crate::worker::invocation)). The
/// method is resolved only to fast-fail an unknown method before durability is
/// opened — a deterministic check that replay reproduces, surfaced as
/// [`InternalRpcError`] so the caller can return `Err(RpcError)` to the guest.
fn resolve_method_and_lift_input(
    agent_type: &AgentTypeSchema,
    method_name: &str,
    input: core_wire::SchemaValueTree,
) -> Result<SchemaValue, InternalRpcError> {
    agent_type
        .methods
        .iter()
        .find(|m| m.name == method_name)
        .ok_or_else(|| InternalRpcError::NotFound {
            details: format!(
                "Method '{method_name}' not found on agent type '{}'",
                agent_type.type_name
            ),
        })?;
    decode_value(&input).map_err(|err| InternalRpcError::ProtocolError {
        details: format!("Invalid RPC input for method '{method_name}': {err}"),
    })
}

/// Project an RPC output [`SchemaValue`] into the WIT
/// `option<schema-value-tree>` result shape used by `invoke-and-await` and
/// `future-invoke-result.get`.
///
/// Per the `golem:agent@2.0.0` contract a declared `unit` output (the
/// canonical empty tuple) maps to `none`, while a `single` output maps to
/// `some(value)`. A method that declares a single `()`/empty-tuple output is
/// structurally indistinguishable from `unit` here and is likewise reported as
/// `none`; both live and replay paths funnel through this helper, so the choice
/// is applied consistently.
fn schema_value_to_wire_output(value: &SchemaValue) -> Option<core_wire::SchemaValueTree> {
    match value {
        SchemaValue::Tuple { elements } if elements.is_empty() => None,
        value => Some(encode_value(value)),
    }
}

pub async fn create_rpc_connection_span<Ctx: InvocationContextManagement>(
    ctx: &mut Ctx,
    target_agent_id: &AgentId,
) -> anyhow::Result<Arc<InvocationContextSpan>> {
    Ok(ctx
        .start_span(
            &[
                (
                    "name".to_string(),
                    AttributeValue::String("rpc-connection".to_string()),
                ),
                (
                    "target_agent_id".to_string(),
                    AttributeValue::String(target_agent_id.to_string()),
                ),
            ],
            false,
        )
        .await?)
}

pub async fn create_invocation_span<Ctx: InvocationContextManagement>(
    ctx: &mut Ctx,
    connection_span_id: &SpanId,
    function_name: &str,
    idempotency_key: &IdempotencyKey,
) -> anyhow::Result<Arc<InvocationContextSpan>> {
    Ok(ctx
        .start_child_span(
            connection_span_id,
            &[
                (
                    "name".to_string(),
                    AttributeValue::String("rpc-invocation".to_string()),
                ),
                (
                    "function_name".to_string(),
                    AttributeValue::String(function_name.to_string()),
                ),
                (
                    "idempotency_key".to_string(),
                    AttributeValue::String(idempotency_key.to_string()),
                ),
            ],
        )
        .await?)
}

#[allow(clippy::large_enum_variant)]
enum FutureInvokeResultState {
    /// The eager host-call `Start` has been written (live) or claimed from the oplog (replay). The
    /// single [`FutureInvokeCallHandle`] owns both halves of the durable call: `get` finishes it
    /// with an `End`, `cancel` / `drop` with a `Cancelled`. It is always `take`n out of the state
    /// before a terminal runs, so a live handle is never dropped while still owned here (which would
    /// spuriously enqueue a `Cancelled`).
    ///
    /// `task` is `Some` only on the live path — the background RPC task whose result feeds the `End`.
    /// On the replay path it is `None`: `get` replays the recorded `End`. If replay instead finds an
    /// incomplete `Start` (the worker crashed after the eager `Start` but before its terminal), `get`
    /// re-dispatches the read-only RPC — using `remote_agent_id`, `request`, and `env` — and
    /// completes the existing `Start`.
    Active {
        handle: Option<FutureInvokeCallHandle>,
        task: Option<FutureInvokeTaskHandle>,
        request: HostRequestGolemRpcInvoke,
        remote_agent_id: OwnedAgentId,
        env: Vec<(String, String)>,
        span_id: SpanId,
    },
    /// Method resolution / input lifting failed deterministically before any host call was opened,
    /// so no `Start` / `End` is written for this future. `get` surfaces the baked error and finishes
    /// the span. Live and replay agree because the failure is a pure function of the cached remote
    /// agent type and the guest payload.
    Baked {
        result: FutureInvokeTaskResult,
        span_id: SpanId,
    },
    /// The future was cancelled: its host call recorded a `Cancelled` and its span was finished.
    /// `get` returns a cancellation error without touching the oplog.
    Cancelled { span_id: SpanId },
    /// `get` already produced the result and finished the span; a second `get` traps.
    Consumed { span_id: SpanId },
}

impl Debug for FutureInvokeResultState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active { .. } => write!(f, "Active"),
            Self::Baked { .. } => write!(f, "Baked"),
            Self::Cancelled { .. } => write!(f, "Cancelled"),
            Self::Consumed { .. } => write!(f, "Consumed"),
        }
    }
}

#[async_trait]
impl SubscribeAny for FutureInvokeResultState {
    async fn ready(&mut self) {
        // The p3 `future-invoke-result` resource exposes no `subscribe`, so this pollable-readiness
        // hook is never driven: `get` is the sole consumer and awaits the background task directly.
        // Kept only to satisfy the `SubscribeAny` bound of the resource-table payload.
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
