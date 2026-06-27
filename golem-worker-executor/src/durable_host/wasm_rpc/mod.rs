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

use crate::durable_host::concurrent::{
    CallHandle, CallReplayOutcome, Cancellable, DropEvent, NotCancellable,
    end_durable_function_access,
};
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
    GolemRpcCancellationTokenCancel, GolemRpcFutureInvokeResultCancel,
    GolemRpcFutureInvokeResultGet, GolemRpcWasmRpcInvoke, GolemRpcWasmRpcInvokeAndAwaitResult,
    GolemRpcWasmRpcNew, GolemRpcWasmRpcScheduleInvocation,
};
use golem_common::model::oplog::types::{SerializableInvokeResult, SerializableScheduleId};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemRpcInvoke, HostRequestGolemRpcScheduledInvocation,
    HostRequestGolemRpcScheduledInvocationCancellation, HostResponseGolemRpcCreate,
    HostResponseGolemRpcInvokeAndAwait, HostResponseGolemRpcInvokeGet,
    HostResponseGolemRpcScheduledInvocation, HostResponseGolemRpcUnit,
    HostResponseGolemRpcUnitOrFailure, OplogEntry,
};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, IdempotencyKey, NamedRetryPolicy, OplogIndex,
    OwnedAgentId, PredicateValue, RetryContext, RetryProperties, ScheduleId, ScheduledAction,
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
use tokio::sync::mpsc::UnboundedSender;
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

        // The generic read-only side-effect trap (see
        // `DurabilityHost::begin_durable_function`) refuses this call up front for
        // read-only agent methods.
        let begin_index = self
            .begin_durable_function(
                &DurableFunctionType::WriteRemote,
                "golem::rpc::wasm-rpc::async-invoke-and-await",
            )
            .await?;

        let oplog_index = self.state.oplog.current_oplog_index().await;
        let idempotency_key = self.derive_idempotency_key(oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &method_name, &idempotency_key)
                .await?;

        // Resolve the method and lift the input. Failures here are
        // deterministic functions of the cached remote agent type and the
        // guest payload, so they are reported as the future's baked-in result
        // rather than as wasmtime traps. The future surfaces the error on the
        // first `get`.
        let input_value =
            match resolve_method_and_lift_input(&remote_agent_type, &method_name, input) {
                Ok(parts) => parts,
                Err(rpc_error) => {
                    // The method/input could not be resolved. The recorded
                    // request `input` is only informational here — the future
                    // is baked with the error result and `get` never re-issues
                    // the call — so an empty placeholder is used. Live and
                    // replay agree because `get` surfaces the persisted result,
                    // not this input.
                    let request = HostRequestGolemRpcInvoke {
                        remote_agent_id: remote_agent_id.agent_id(),
                        idempotency_key: idempotency_key.clone(),
                        method_name: method_name.clone(),
                        input: SchemaValue::Tuple {
                            elements: Vec::new(),
                        },
                        remote_agent_type: None,
                        remote_agent_parameters: None,
                    };
                    let fut = self.table().push(FutureInvokeResultEntry {
                        payload: Box::new(FutureInvokeResultState::Completed {
                            request,
                            result: Ok(Err(rpc_error)),
                            span_id: span.span_id().clone(),
                            begin_index,
                        }),
                        child_pollables: Vec::new(),
                        drop_pending: false,
                    })?;
                    return Ok(fut);
                }
            };

        let agent_id = self.agent_id().clone();
        let created_by = self.created_by();
        let request = HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: method_name.clone(),
            input: input_value.clone(),
            remote_agent_type: None,
            remote_agent_parameters: None,
        };

        let result = if self.state.is_live() {
            let rpc = self.rpc();
            let stack = self.clone_as_inherited_stack(span.span_id());

            let in_atomic_region = self.in_atomic_region();
            let allow_retry = !in_atomic_region;
            let environment_state_service = self.state.environment_state_service.clone();
            let environment_id = self.state.owned_agent_id.environment_id;
            let default_retry_policy =
                NamedRetryPolicy::default_from_config(&self.state.config.retry);
            let agent_config_retry_policies = self.state.agent_config_retry_policies();
            let runtime_retry_policy_mutations = self.state.runtime_retry_policy_mutations.clone();
            let mut retry_properties =
                RetryContext::rpc("invoke-and-await", &remote_agent_id, &method_name);
            self.state.enrich_retry_properties(&mut retry_properties);
            let max_delay = self.durable_execution_state().max_in_function_retry_delay;
            let worker = self.public_state.worker();

            let retry_params = if allow_retry {
                Some(TaskRetryParams {
                    environment_state_service,
                    environment_id,
                    default_retry_policy,
                    agent_config_retry_policies,
                    runtime_retry_policy_mutations,
                    retry_properties,
                    max_in_function_retry_delay: max_delay,
                    worker,
                    retry_point: begin_index,
                    execution_status: self.execution_status.clone(),
                })
            } else {
                None
            };

            let handle = spawn_rpc_task_with_retry(
                rpc,
                remote_agent_id,
                idempotency_key,
                method_name,
                input_value.clone(),
                created_by,
                agent_id,
                env,
                stack,
                retry_params,
                self.agent_auth_ctx(),
            );

            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Pending {
                    handle: Arc::new(tokio::sync::Mutex::new(handle)),
                    request,
                    span_id: span.span_id().clone(),
                    begin_index,
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            Ok(fut)
        } else {
            let auth_ctx = self.agent_auth_ctx();
            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Deferred {
                    remote_agent_id,
                    self_agent_id: agent_id,
                    self_created_by: created_by,
                    env,
                    method_name,
                    method_parameters: input_value,
                    idempotency_key,
                    span_id: span.span_id().clone(),
                    begin_index,
                    auth_ctx,
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            Ok(fut)
        };

        if result.is_err() {
            self.end_function(&DurableFunctionType::WriteRemote, begin_index)
                .await?;
        }

        result
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
        let scheduled_at =
            chrono::DateTime::from_timestamp(datetime.seconds as i64, datetime.nanoseconds)
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

struct FutureInvokeGetSnapshot {
    request: HostRequestGolemRpcInvoke,
    begin_index: OplogIndex,
    span_id: SpanId,
    cancelled: bool,
}

struct FutureInvokeDropSnapshot {
    request: Option<HostRequestGolemRpcInvoke>,
    begin_index: OplogIndex,
    span_id: SpanId,
}

struct FutureInvokeParentScopeGuard {
    sink: Option<UnboundedSender<DropEvent>>,
    begin_index: OplogIndex,
    span_id: SpanId,
    armed: bool,
}

impl FutureInvokeParentScopeGuard {
    fn armed(
        sink: Option<UnboundedSender<DropEvent>>,
        begin_index: OplogIndex,
        span_id: SpanId,
    ) -> Self {
        Self {
            sink,
            begin_index,
            span_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for FutureInvokeParentScopeGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(sink) = &self.sink {
            let _ = sink.send(DropEvent::CloseDurableScope {
                function_type: DurableFunctionType::WriteRemote,
                begin_index: self.begin_index,
                span_id: Some(self.span_id.clone()),
            });
        }
    }
}

struct DeferredFutureInvoke {
    remote_agent_id: OwnedAgentId,
    self_agent_id: AgentId,
    self_created_by: AccountId,
    env: Vec<(String, String)>,
    method_name: String,
    method_parameters: SchemaValue,
    idempotency_key: IdempotencyKey,
    span_id: SpanId,
    begin_index: OplogIndex,
    auth_ctx: AuthCtx,
}

enum FutureInvokeGetActionPlan {
    Ready(FutureInvokeGetResult),
    Await(FutureInvokeTaskHandle),
    Deferred(DeferredFutureInvoke),
}

enum FutureInvokeGetAction {
    Ready(FutureInvokeGetResult),
    Await(FutureInvokeTaskHandle),
}

enum FutureInvokeGetActionResult {
    Ready(FutureInvokeGetResult),
    Awaited(FutureInvokeTaskResult),
}

fn future_invoke_request_from_deferred(
    deferred: &DeferredFutureInvoke,
) -> HostRequestGolemRpcInvoke {
    HostRequestGolemRpcInvoke {
        remote_agent_id: deferred.remote_agent_id.agent_id(),
        idempotency_key: deferred.idempotency_key.clone(),
        method_name: deferred.method_name.clone(),
        input: deferred.method_parameters.clone(),
        remote_agent_type: None,
        remote_agent_parameters: None,
    }
}

fn snapshot_future_invoke_get<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    this: Resource<FutureInvokeResult>,
) -> anyhow::Result<FutureInvokeGetSnapshot> {
    let entry = ctx.table().get_mut(&this)?;
    let state = entry
        .payload
        .as_any_mut()
        .downcast_mut::<FutureInvokeResultState>()
        .unwrap();

    let begin_index = state.begin_index();
    let span_id = state.span_id().clone();
    let cancelled = matches!(state, FutureInvokeResultState::Cancelled { .. });
    let request = match state {
        FutureInvokeResultState::Pending { request, .. }
        | FutureInvokeResultState::Completed { request, .. }
        | FutureInvokeResultState::Cancelled { request, .. }
        | FutureInvokeResultState::Consumed { request, .. } => request.clone(),
        FutureInvokeResultState::Deferred {
            remote_agent_id,
            method_name,
            method_parameters,
            idempotency_key,
            ..
        } => HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: method_name.clone(),
            input: method_parameters.clone(),
            remote_agent_type: None,
            remote_agent_parameters: None,
        },
    };

    Ok(FutureInvokeGetSnapshot {
        request,
        begin_index,
        span_id,
        cancelled,
    })
}

fn snapshot_future_invoke_drop<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    this: Resource<FutureInvokeResult>,
) -> anyhow::Result<Option<FutureInvokeDropSnapshot>> {
    let entry = ctx.table().get_mut(&this)?;
    let state = entry
        .payload
        .as_any_mut()
        .downcast_mut::<FutureInvokeResultState>()
        .unwrap();

    Ok(match state {
        FutureInvokeResultState::Pending {
            request,
            span_id,
            begin_index,
            ..
        }
        | FutureInvokeResultState::Completed {
            request,
            span_id,
            begin_index,
            ..
        } => Some(FutureInvokeDropSnapshot {
            request: Some(request.clone()),
            begin_index: *begin_index,
            span_id: span_id.clone(),
        }),
        FutureInvokeResultState::Deferred {
            remote_agent_id,
            method_name,
            method_parameters,
            idempotency_key,
            span_id,
            begin_index,
            ..
        } => Some(FutureInvokeDropSnapshot {
            request: Some(HostRequestGolemRpcInvoke {
                remote_agent_id: remote_agent_id.agent_id(),
                idempotency_key: idempotency_key.clone(),
                method_name: method_name.clone(),
                input: method_parameters.clone(),
                remote_agent_type: None,
                remote_agent_parameters: None,
            }),
            begin_index: *begin_index,
            span_id: span_id.clone(),
        }),
        FutureInvokeResultState::Cancelled {
            span_id,
            begin_index,
            ..
        } => Some(FutureInvokeDropSnapshot {
            request: None,
            begin_index: *begin_index,
            span_id: span_id.clone(),
        }),
        FutureInvokeResultState::Consumed { .. } => None,
    })
}

fn future_invoke_parent_scope<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    this: Resource<FutureInvokeResult>,
) -> anyhow::Result<Option<(OplogIndex, SpanId)>> {
    let entry = ctx.table().get_mut(&this)?;
    let state = entry
        .payload
        .as_any_mut()
        .downcast_mut::<FutureInvokeResultState>()
        .unwrap();

    Ok(match state {
        FutureInvokeResultState::Consumed { .. } => None,
        state => Some((state.begin_index(), state.span_id().clone())),
    })
}

fn take_future_invoke_get_action_plan<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    this: Resource<FutureInvokeResult>,
) -> anyhow::Result<FutureInvokeGetActionPlan> {
    let entry = ctx.table().get_mut(&this)?;
    let state = entry
        .payload
        .as_any_mut()
        .downcast_mut::<FutureInvokeResultState>()
        .unwrap();

    match state {
        FutureInvokeResultState::Consumed { .. } => {
            let message = "future-invoke-result already consumed";
            Ok(FutureInvokeGetActionPlan::Ready(Err(anyhow::Error::new(
                ClassifiedHostError {
                    kind: HostFailureKind::Permanent,
                    message: message.to_string(),
                },
            ))))
        }
        FutureInvokeResultState::Pending {
            request,
            handle,
            span_id,
            begin_index,
        } => {
            let action = FutureInvokeGetActionPlan::Await(handle.clone());
            let request = request.clone();
            let span_id = span_id.clone();
            let begin_index = *begin_index;
            *state = FutureInvokeResultState::Consumed {
                request,
                span_id,
                begin_index,
            };
            Ok(action)
        }
        FutureInvokeResultState::Completed {
            request,
            result,
            span_id,
            begin_index,
        } => {
            let result = future_invoke_task_result_to_get_result(result);
            let request = request.clone();
            let span_id = span_id.clone();
            let begin_index = *begin_index;
            *state = FutureInvokeResultState::Consumed {
                request,
                span_id,
                begin_index,
            };
            Ok(FutureInvokeGetActionPlan::Ready(result))
        }
        FutureInvokeResultState::Cancelled {
            request,
            span_id,
            begin_index,
        } => {
            let rpc_error = InternalRpcError::ProtocolError {
                details: "Invocation cancelled".to_string(),
            };
            let request = request.clone();
            let span_id = span_id.clone();
            let begin_index = *begin_index;
            *state = FutureInvokeResultState::Consumed {
                request,
                span_id,
                begin_index,
            };
            Ok(FutureInvokeGetActionPlan::Ready(Ok(Err(rpc_error.into()))))
        }
        FutureInvokeResultState::Deferred {
            remote_agent_id,
            self_agent_id,
            self_created_by,
            env,
            method_name,
            method_parameters,
            idempotency_key,
            span_id,
            begin_index,
            auth_ctx,
        } => {
            let deferred = DeferredFutureInvoke {
                remote_agent_id: remote_agent_id.clone(),
                self_agent_id: self_agent_id.clone(),
                self_created_by: *self_created_by,
                env: env.clone(),
                method_name: method_name.clone(),
                method_parameters: method_parameters.clone(),
                idempotency_key: idempotency_key.clone(),
                span_id: span_id.clone(),
                begin_index: *begin_index,
                auth_ctx: auth_ctx.clone(),
            };
            Ok(FutureInvokeGetActionPlan::Deferred(deferred))
        }
    }
}

fn prepare_future_invoke_get_action<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    this: Resource<FutureInvokeResult>,
) -> anyhow::Result<FutureInvokeGetAction> {
    let this_rep = this.rep();
    match take_future_invoke_get_action_plan(ctx, Resource::new_borrow(this_rep))? {
        FutureInvokeGetActionPlan::Ready(result) => Ok(FutureInvokeGetAction::Ready(result)),
        FutureInvokeGetActionPlan::Await(handle) => Ok(FutureInvokeGetAction::Await(handle)),
        FutureInvokeGetActionPlan::Deferred(deferred) => {
            let stack = ctx.clone_as_inherited_stack(&deferred.span_id);
            let in_atomic_region = ctx.in_atomic_region();
            let allow_retry = !in_atomic_region;
            let retry_params = if allow_retry {
                Some(TaskRetryParams {
                    environment_state_service: ctx.state.environment_state_service.clone(),
                    environment_id: ctx.state.owned_agent_id.environment_id,
                    default_retry_policy: NamedRetryPolicy::default_from_config(
                        &ctx.state.config.retry,
                    ),
                    agent_config_retry_policies: ctx.state.agent_config_retry_policies(),
                    runtime_retry_policy_mutations: ctx
                        .state
                        .runtime_retry_policy_mutations
                        .clone(),
                    retry_properties: {
                        let mut properties = RetryContext::rpc(
                            "invoke-and-await",
                            &deferred.remote_agent_id,
                            &deferred.method_name,
                        );
                        if let Some(agent_id) = ctx.state.agent_id.as_ref() {
                            properties.set(
                                "agent-type",
                                PredicateValue::Text(agent_id.agent_type.to_string()),
                            );
                            properties.set(
                                "is-idempotent",
                                PredicateValue::Boolean(ctx.state.assume_idempotence),
                            );
                        }
                        properties
                    },
                    max_in_function_retry_delay: ctx
                        .durable_execution_state()
                        .max_in_function_retry_delay,
                    worker: ctx.public_state.worker(),
                    retry_point: deferred.begin_index,
                    execution_status: ctx.execution_status.clone(),
                })
            } else {
                None
            };

            let request = future_invoke_request_from_deferred(&deferred);
            let span_id = deferred.span_id.clone();
            let begin_index = deferred.begin_index;
            let handle = spawn_rpc_task_with_retry(
                ctx.rpc(),
                deferred.remote_agent_id,
                deferred.idempotency_key,
                deferred.method_name,
                deferred.method_parameters,
                deferred.self_created_by,
                deferred.self_agent_id,
                deferred.env,
                stack,
                retry_params,
                deferred.auth_ctx,
            );
            let handle = Arc::new(tokio::sync::Mutex::new(handle));
            let this = Resource::<FutureInvokeResult>::new_borrow(this_rep);
            let entry = ctx.table().get_mut(&this)?;
            let state = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            if matches!(state, FutureInvokeResultState::Deferred { .. }) {
                *state = FutureInvokeResultState::Consumed {
                    request,
                    span_id,
                    begin_index,
                };
            }
            Ok(FutureInvokeGetAction::Await(handle))
        }
    }
}

async fn run_future_invoke_get_action(
    action: FutureInvokeGetAction,
) -> FutureInvokeGetActionResult {
    match action {
        FutureInvokeGetAction::Ready(result) => FutureInvokeGetActionResult::Ready(result),
        FutureInvokeGetAction::Await(handle) => {
            let result = {
                let mut handle = handle.lock().await;
                (&mut *handle).await
            };
            FutureInvokeGetActionResult::Awaited(result)
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

fn serializable_future_invoke_get_result(
    result: &FutureInvokeGetResult,
) -> SerializableInvokeResult {
    match result {
        Ok(Ok(value)) => SerializableInvokeResult::Completed(Ok(value.clone())),
        Ok(Err(error)) => {
            let error: InternalRpcError = error.clone().into();
            SerializableInvokeResult::Completed(Err(error.into()))
        }
        Err(err) => SerializableInvokeResult::Failed(err.to_string()),
    }
}

fn serializable_future_invoke_get_result_to_wire(
    result: SerializableInvokeResult,
) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
    match result {
        SerializableInvokeResult::Pending => Err(anyhow::anyhow!(
            "future-invoke-result.get replayed a pending result"
        )),
        SerializableInvokeResult::Completed(result) => match result {
            Ok(value) => Ok(Ok(schema_value_to_wire_output(&value))),
            Err(error) => {
                let rpc_error: InternalRpcError = error.into();
                Ok(Err(rpc_error.into()))
            }
        },
        SerializableInvokeResult::Failed(error) => Err(anyhow::anyhow!(error)),
    }
}

fn mark_future_invoke_get_consumed<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    this: Resource<FutureInvokeResult>,
    request: HostRequestGolemRpcInvoke,
    span_id: SpanId,
    begin_index: OplogIndex,
) -> anyhow::Result<()> {
    let entry = ctx.table().get_mut(&this)?;
    let state = entry
        .payload
        .as_any_mut()
        .downcast_mut::<FutureInvokeResultState>()
        .unwrap();
    *state = FutureInvokeResultState::Consumed {
        request,
        span_id,
        begin_index,
    };
    Ok(())
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

async fn finish_future_invoke_get<T, Ctx: WorkerCtx>(
    accessor: &Accessor<T, HasSelf<DurableWorkerCtx<Ctx>>>,
    begin_index: OplogIndex,
    span_id: &SpanId,
) -> anyhow::Result<()> {
    end_durable_function_access(
        accessor,
        accessor.getter(),
        DurableFunctionType::WriteRemote,
        begin_index,
        false,
    )
    .await
    .map_err(anyhow::Error::from)?;
    finish_span_access(accessor, span_id)
        .await
        .map_err(anyhow::Error::from)
}

async fn finish_future_invoke_if_open<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    begin_index: OplogIndex,
    span_id: &SpanId,
) -> anyhow::Result<()> {
    if ctx.state.is_durable_scope_open(begin_index) {
        ctx.end_function(&DurableFunctionType::WriteRemote, begin_index)
            .await?;
        ctx.finish_span(span_id).await?;
    }
    Ok(())
}

async fn finish_future_invoke_if_open_access<T, Ctx: WorkerCtx>(
    accessor: &Accessor<T, HasSelf<DurableWorkerCtx<Ctx>>>,
    begin_index: OplogIndex,
    span_id: &SpanId,
) -> anyhow::Result<()> {
    let is_open = accessor.with(|mut access| access.get().state.is_durable_scope_open(begin_index));

    if is_open {
        end_durable_function_access(
            accessor,
            accessor.getter(),
            DurableFunctionType::WriteRemote,
            begin_index,
            false,
        )
        .await
        .map_err(anyhow::Error::from)?;
        finish_span_access(accessor, span_id)
            .await
            .map_err(anyhow::Error::from)?;
    }

    Ok(())
}

impl<Ctx: WorkerCtx> HostFutureInvokeResultWithStore for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn get<T: Send>(
        accessor: &Accessor<T, Self>,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
        let this_rep = this.rep();
        let snapshot = accessor.with(|mut access| {
            snapshot_future_invoke_get(access.get(), Resource::new_borrow(this_rep))
        })?;

        if snapshot.cancelled {
            accessor.with(|mut access| {
                mark_future_invoke_get_consumed(
                    access.get(),
                    Resource::new_borrow(this_rep),
                    snapshot.request,
                    snapshot.span_id.clone(),
                    snapshot.begin_index,
                )
            })?;
            let rpc_error = InternalRpcError::ProtocolError {
                details: "Invocation cancelled".to_string(),
            };
            return Ok(Err(rpc_error.into()));
        }

        let function_type = DurableFunctionType::WriteRemoteBatched(Some(snapshot.begin_index));
        let mut parent_scope_guard = FutureInvokeParentScopeGuard::armed(
            accessor.with(|mut access| access.get().state.dropped_call_event_sender()),
            snapshot.begin_index,
            snapshot.span_id.clone(),
        );
        let mut call = match CallHandle::<GolemRpcFutureInvokeResultGet, Cancellable>::start_access(
            accessor,
            accessor.getter(),
            snapshot.request.clone(),
            function_type,
        )
        .await
        {
            Ok(call) => call,
            Err(err) => {
                parent_scope_guard.disarm();
                return Err(anyhow::Error::from(err));
            }
        };

        if !call.is_live() {
            match call
                .replay_access(accessor, accessor.getter())
                .await
                .map_err(anyhow::Error::from)?
            {
                CallReplayOutcome::Replayed(response) => {
                    finish_future_invoke_get(accessor, snapshot.begin_index, &snapshot.span_id)
                        .await?;
                    parent_scope_guard.disarm();
                    accessor.with(|mut access| {
                        mark_future_invoke_get_consumed(
                            access.get(),
                            Resource::new_borrow(this_rep),
                            snapshot.request,
                            snapshot.span_id.clone(),
                            snapshot.begin_index,
                        )
                    })?;
                    return serializable_future_invoke_get_result_to_wire(response.result);
                }
                CallReplayOutcome::Incomplete(live) => call = live,
            }
        }

        let action = accessor.with(|mut access| {
            prepare_future_invoke_get_action(access.get(), Resource::new_borrow(this_rep))
        })?;
        let result = match run_future_invoke_get_action(action).await {
            FutureInvokeGetActionResult::Ready(result) => result,
            FutureInvokeGetActionResult::Awaited(task_result) => {
                future_invoke_task_result_to_get_result(&task_result)
            }
        };
        let response = HostResponseGolemRpcInvokeGet {
            result: serializable_future_invoke_get_result(&result),
        };
        let response = call
            .complete_access(accessor, accessor.getter(), response)
            .await
            .map_err(anyhow::Error::from)?;
        finish_future_invoke_get(accessor, snapshot.begin_index, &snapshot.span_id).await?;
        parent_scope_guard.disarm();
        accessor.with(|mut access| {
            mark_future_invoke_get_consumed(
                access.get(),
                Resource::new_borrow(this_rep),
                snapshot.request,
                snapshot.span_id.clone(),
                snapshot.begin_index,
            )
        })?;

        serializable_future_invoke_get_result_to_wire(response.result)
    }

    async fn drop<T>(
        accessor: &Accessor<T, Self>,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<()> {
        let future_rep = this.rep();
        let drop_snapshot = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("golem::rpc::future-invoke-result", "drop");
            snapshot_future_invoke_drop(ctx, Resource::new_borrow(future_rep))
        })?;

        if let Some(snapshot) = &drop_snapshot {
            if let Some(request) = &snapshot.request {
                let handle =
                    CallHandle::<GolemRpcFutureInvokeResultGet, Cancellable>::start_access(
                        accessor,
                        accessor.getter(),
                        request.clone(),
                        DurableFunctionType::WriteRemoteBatched(Some(snapshot.begin_index)),
                    )
                    .await?;

                handle
                    .cancel_access(accessor, accessor.getter(), None)
                    .await?;
            }

            finish_future_invoke_if_open_access(accessor, snapshot.begin_index, &snapshot.span_id)
                .await?;
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

        let (should_attempt_remote_cancel, remote_agent_id, idempotency_key, request) = {
            let entry = self.table().get(&this)?;
            let state = entry
                .payload
                .as_any()
                .downcast_ref::<FutureInvokeResultState>()
                .unwrap();
            match state {
                FutureInvokeResultState::Pending { request, .. } => (
                    true,
                    request.remote_agent_id.clone(),
                    request.idempotency_key.clone(),
                    request.clone(),
                ),
                FutureInvokeResultState::Deferred {
                    remote_agent_id,
                    idempotency_key,
                    method_name,
                    method_parameters,
                    ..
                } => (
                    true,
                    remote_agent_id.agent_id(),
                    idempotency_key.clone(),
                    HostRequestGolemRpcInvoke {
                        remote_agent_id: remote_agent_id.agent_id(),
                        idempotency_key: idempotency_key.clone(),
                        method_name: method_name.clone(),
                        input: method_parameters.clone(),
                        remote_agent_type: None,
                        remote_agent_parameters: None,
                    },
                ),
                FutureInvokeResultState::Completed { request, .. }
                | FutureInvokeResultState::Cancelled { request, .. }
                | FutureInvokeResultState::Consumed { request, .. } => (
                    false,
                    request.remote_agent_id.clone(),
                    request.idempotency_key.clone(),
                    request.clone(),
                ),
            }
        };

        let mut handle = CallHandle::<GolemRpcFutureInvokeResultCancel, NotCancellable>::start(
            self,
            request,
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

            if should_attempt_remote_cancel
                && let Err(err) = self
                    .worker_proxy()
                    .cancel_invocation(&remote_agent_id, idempotency_key, &self.agent_auth_ctx())
                    .await
            {
                tracing::info!(err=%err, "Best-effort cancel_invocation failed");
            }

            handle.complete(self, HostResponseGolemRpcUnit {}).await?;
        }

        // Transition deferred/pending futures to Cancelled so they won't be initiated on recovery
        {
            let entry = self.table().get_mut(&this)?;
            let state = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            match state {
                FutureInvokeResultState::Deferred {
                    remote_agent_id,
                    method_name,
                    method_parameters,
                    idempotency_key,
                    span_id,
                    begin_index,
                    ..
                } => {
                    *state = FutureInvokeResultState::Cancelled {
                        request: HostRequestGolemRpcInvoke {
                            remote_agent_id: remote_agent_id.agent_id(),
                            idempotency_key: idempotency_key.clone(),
                            method_name: method_name.clone(),
                            input: method_parameters.clone(),
                            remote_agent_type: None,
                            remote_agent_parameters: None,
                        },
                        span_id: span_id.clone(),
                        begin_index: *begin_index,
                    };
                }
                FutureInvokeResultState::Pending {
                    request,
                    span_id,
                    begin_index,
                    ..
                } => {
                    *state = FutureInvokeResultState::Cancelled {
                        request: request.clone(),
                        span_id: span_id.clone(),
                        begin_index: *begin_index,
                    };
                }
                _ => {} // Completed/Consumed/already Cancelled - no-op
            }
        }

        if let Some((begin_index, span_id)) =
            future_invoke_parent_scope(self, Resource::new_borrow(this.rep()))?
        {
            finish_future_invoke_if_open(self, begin_index, &span_id).await?;
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
    Pending {
        request: HostRequestGolemRpcInvoke,
        handle: FutureInvokeTaskHandle,
        span_id: SpanId,
        begin_index: OplogIndex,
    },
    Completed {
        request: HostRequestGolemRpcInvoke,
        result: Result<Result<SchemaValue, InternalRpcError>, Error>,
        span_id: SpanId,
        begin_index: OplogIndex,
    },
    Deferred {
        remote_agent_id: OwnedAgentId,
        self_agent_id: AgentId,
        self_created_by: AccountId,
        env: Vec<(String, String)>,
        method_name: String,
        method_parameters: SchemaValue,
        idempotency_key: IdempotencyKey,
        span_id: SpanId,
        begin_index: OplogIndex,
        auth_ctx: AuthCtx,
    },
    Cancelled {
        request: HostRequestGolemRpcInvoke,
        span_id: SpanId,
        begin_index: OplogIndex,
    },
    Consumed {
        request: HostRequestGolemRpcInvoke,
        span_id: SpanId,
        begin_index: OplogIndex,
    },
}

impl Debug for FutureInvokeResultState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending { .. } => write!(f, "Pending"),
            Self::Completed { .. } => write!(f, "Completed"),
            Self::Deferred { .. } => write!(f, "Deferred"),
            Self::Cancelled { .. } => write!(f, "Cancelled"),
            Self::Consumed { .. } => write!(f, "Consumed"),
        }
    }
}

impl FutureInvokeResultState {
    pub fn span_id(&self) -> &SpanId {
        match self {
            Self::Pending { span_id, .. }
            | Self::Completed { span_id, .. }
            | Self::Deferred { span_id, .. }
            | Self::Cancelled { span_id, .. }
            | Self::Consumed { span_id, .. } => span_id,
        }
    }

    pub fn begin_index(&self) -> OplogIndex {
        match self {
            Self::Pending { begin_index, .. } => *begin_index,
            Self::Completed { begin_index, .. } => *begin_index,
            Self::Deferred { begin_index, .. } => *begin_index,
            Self::Cancelled { begin_index, .. } => *begin_index,
            Self::Consumed { begin_index, .. } => *begin_index,
        }
    }
}

#[async_trait]
impl SubscribeAny for FutureInvokeResultState {
    async fn ready(&mut self) {
        if let Self::Pending {
            handle,
            request,
            span_id,
            begin_index,
        } = self
        {
            let result = {
                let mut handle = handle.lock().await;
                (&mut *handle).await
            };
            let request = request.clone();
            let span_id = span_id.clone();
            let begin_index = *begin_index;
            *self = Self::Completed {
                result,
                request,
                span_id,
                begin_index,
            };
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
