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

use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind, InFunctionRetryHost};
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::get_oplog_entry;
use crate::preview2::golem::agent::host::{
    CancellationToken, FutureInvokeResult, HostCancellationToken, HostFutureInvokeResult,
    HostWasmRpc, RpcError,
};
use crate::services::HasWorker;
use crate::services::environment_state::EnvironmentStateService;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::services::rpc::{Rpc, RpcDemand, RpcError as InternalRpcError};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use anyhow::Error;
use async_trait::async_trait;
use futures::future::Either;
use golem_common::base_model::agent::Principal;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{AgentMethod, AgentType, LegacyParsedAgentId};
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
    DurableFunctionType, HostPayloadPair, HostRequest, HostRequestGolemRpcInvoke,
    HostRequestGolemRpcScheduledInvocation, HostRequestGolemRpcScheduledInvocationCancellation,
    HostResponse, HostResponseGolemRpcCreate, HostResponseGolemRpcInvokeAndAwait,
    HostResponseGolemRpcInvokeGet, HostResponseGolemRpcScheduledInvocation,
    HostResponseGolemRpcUnit, HostResponseGolemRpcUnitOrFailure, OplogEntry, PersistenceLevel,
};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, IdempotencyKey, NamedRetryPolicy, OplogIndex,
    OwnedAgentId, PredicateValue, RetryContext, RetryProperties, ScheduleId, ScheduledAction,
};
use golem_common::schema::adapters::typed_schema_value_to_value_and_type;
use golem_common::schema::schema_value::SchemaValue;
use golem_common::schema::wit::{decode_typed, decode_value, encode_value};
use golem_common::serialization::{deserialize, serialize};

use crate::durable_host::golem::agent::schema_value_tree_to_data_value;
use golem_wasm::golem_core_2_0_x::types as core_wire;
use golem_wasm::{CancellationTokenEntry, FutureInvokeResultEntry, SubscribeAny, WasmRpcEntry};
use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use tracing::{Instrument, error};
use wasmtime::component::{Resource, ResourceTableError};
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;

use golem_common::model::oplog::payload::HostRequestGolemRpcCreate;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_wasm::json::ValueAndTypeJsonExtensions;

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

        // Resolve the canonical (legacy-model) registered agent type; the
        // schema-native WIT wire form is only produced at the host-import
        // boundary, never for internal agent-id / service / oplog use.
        let agent_type = self
            .get_agent_type_model(golem_common::model::agent::AgentTypeName(
                agent_type_name.clone(),
            ))
            .await?
            .ok_or_else(|| anyhow::anyhow!("Agent type '{}' not found", agent_type_name))?;

        // Lower the guest's `golem:core@2.0.0` constructor value tree to the
        // legacy canonical `DataValue` against the constructor's input schema.
        let input = schema_value_tree_to_data_value(
            &constructor,
            &agent_type.agent_type.constructor.input_schema,
        )
        .map_err(|err| anyhow::anyhow!("Invalid constructor input: {err}"))?;

        // Share the canonical agent type through `WasmRpcEntryPayload`. Every
        // subsequent RPC entry resolves the per-method input/output
        // `DataSchema` from this cached value to drive the typed flow.
        let remote_agent_type: Arc<AgentType> = Arc::new(agent_type.agent_type.clone());

        let agent_id = golem_common::model::agent::LegacyParsedAgentId::new(
            golem_common::model::agent::AgentTypeName(agent_type_name),
            input,
            phantom_id.map(|id| id.into()),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;

        let component_id: golem_common::model::component::ComponentId =
            agent_type.implemented_by.component_id;
        let remote_agent_id = golem_common::model::AgentId::from_agent_id(component_id, &agent_id)
            .map_err(|err| anyhow::anyhow!("{err}"))?;

        let config = config
            .into_iter()
            .map(|c| {
                // The config value travels as a self-contained
                // `golem:core@2.0.0` typed-schema-value; decode it and project
                // it down to the legacy typed-value JSON form the service
                // boundary (migrated in a later wave) still expects.
                let typed = decode_typed(&c.value)
                    .map_err(|err| anyhow::anyhow!("Invalid agent config value: {err}"))?;
                let value_and_type = typed_schema_value_to_value_and_type(&typed)
                    .map_err(|err| anyhow::anyhow!("Invalid agent config value: {err}"))?;
                let encoded = value_and_type
                    .to_json_value()
                    .map_err(|err| anyhow::anyhow!("Failed serializing agent config: {err}"))?;

                Ok::<_, anyhow::Error>(AgentConfigEntryDto {
                    path: c.path,
                    value: encoded.into(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let span = create_rpc_connection_span(self, &remote_agent_id).await?;

        let durability =
            Durability::<GolemRpcWasmRpcNew>::new(self, DurableFunctionType::WriteRemote).await?;

        if durability.is_live() {
            construct_wasm_rpc_resource(
                self,
                remote_agent_id,
                &env,
                config,
                &durability,
                span,
                remote_agent_type,
            )
            .await
        } else {
            let response = durability.replay(self).await?;
            reconstruct_wasm_rpc_resource(
                self,
                remote_agent_id,
                response.target_environment_id,
                response.target_fingerprint,
                span,
                remote_agent_type,
            )
            .await
        }
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

        let mut durability = Durability::<GolemRpcWasmRpcInvokeAndAwaitResult>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result: Result<SchemaValue, InternalRpcError> = if durability.is_live() {
            let request = HostRequestGolemRpcInvoke {
                remote_agent_id: remote_agent_id.agent_id(),
                idempotency_key: idempotency_key.clone(),
                method_name: method_name.clone(),
                input: input_value.clone(),
                remote_agent_type: None,
                remote_agent_parameters: None,
            };
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
                let created_by_email = self.created_by_email().clone();
                let agent_id = self.agent_id().clone();

                let either_result = futures::future::select(
                    rpc.invoke_and_await(
                        &remote_agent_id,
                        Some(idempotency_key.clone()),
                        method_name.clone(),
                        input_value.clone(),
                        created_by,
                        &created_by_email,
                        &agent_id,
                        &env,
                        stack,
                    ),
                    interrupt_signal,
                )
                .await;
                let result: Result<SchemaValue, InternalRpcError> = match either_result {
                    Either::Left((result, _)) => result.map_err(Into::into),
                    Either::Right((interrupt_kind, _)) => {
                        tracing::info!("Interrupted while waiting for RPC result");
                        return Err(interrupt_kind.into());
                    }
                };
                match durability
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

            durability
                .persist(
                    self,
                    request,
                    HostResponseGolemRpcInvokeAndAwait {
                        result: result.clone().map_err(Into::into),
                    },
                )
                .await?;
            result
        } else {
            let persisted = durability.replay(self).await?;
            persisted.result.map_err(Into::into)
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

        let mut durability =
            Durability::<GolemRpcWasmRpcInvoke>::new(self, DurableFunctionType::WriteRemote)
                .await?;

        let result = if durability.is_live() {
            let request = HostRequestGolemRpcInvoke {
                remote_agent_id: remote_agent_id.agent_id(),
                idempotency_key: idempotency_key.clone(),
                method_name: method_name.clone(),
                input: input_value.clone(),
                remote_agent_type: None,
                remote_agent_parameters: None,
            };
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
                        self.created_by_email(),
                        self.agent_id(),
                        &env,
                        stack,
                    )
                    .await;
                match durability
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
            durability
                .persist(self, request, HostResponseGolemRpcUnitOrFailure { result })
                .await
        } else {
            durability.replay(self).await
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
        let created_by_email = self.created_by_email().clone();
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
                created_by_email,
                agent_id,
                env,
                stack,
                retry_params,
            );

            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Pending {
                    handle,
                    request,
                    span_id: span.span_id().clone(),
                    begin_index,
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            Ok(fut)
        } else {
            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Deferred {
                    remote_agent_id,
                    self_agent_id: agent_id,
                    self_created_by: created_by,
                    self_created_by_email: created_by_email,
                    env,
                    method_name,
                    method_parameters: input_value,
                    idempotency_key,
                    span_id: span.span_id().clone(),
                    begin_index,
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
        scheduled_time: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime,
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
        datetime: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime,
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

        let durability = Durability::<GolemRpcWasmRpcScheduleInvocation>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let current_oplog_index = self.state.oplog.current_oplog_index().await;

            let idempotency_key = self.derive_idempotency_key(current_oplog_index);
            let schedule_id = ScheduleId::from_idempotency_key(&idempotency_key);

            let request = HostRequestGolemRpcScheduledInvocation {
                remote_agent_id: remote_agent_id.agent_id(),
                idempotency_key: idempotency_key.clone(),
                method_name: method_name.clone(),
                input: input_value.clone(),
                datetime: datetime.into(),
                remote_agent_type: None,
                remote_agent_parameters: None,
            };

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

            durability
                .persist(
                    self,
                    request,
                    HostResponseGolemRpcScheduledInvocation { schedule_id },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

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

impl<Ctx: WorkerCtx> HostFutureInvokeResult for DurableWorkerCtx<Ctx> {
    async fn subscribe(
        &mut self,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Resource<golem_wasm::DynPollable>> {
        self.observe_function_call("golem::rpc::future-invoke-result", "subscribe");
        let parent_rep = this.rep();
        let pollable = wasmtime_wasi::dynamic_subscribe(self.table(), this, None)?;
        let child_rep = pollable.rep();
        let parent: Resource<FutureInvokeResult> = Resource::new_borrow(parent_rep);
        let entry = self.table().get_mut(&parent)?;
        entry.child_pollables.push(child_rep);
        self.state
            .rpc_pollable_to_parent
            .insert(child_rep, parent_rep);
        Ok(pollable)
    }

    async fn get(
        &mut self,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Option<Result<Option<core_wire::SchemaValueTree>, RpcError>>> {
        self.observe_function_call("golem::rpc::future-invoke-result", "get");
        let rpc = self.rpc();

        let span_id = {
            let entry = self.table().get_mut(&this)?;
            let entry = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            entry.span_id().clone()
        };

        if self.state.is_live() || self.state.snapshotting_mode.is_some() {
            // Main state machine match
            let stack = self.clone_as_inherited_stack(&span_id);

            let in_atomic_region = self.in_atomic_region();
            let allow_retry = !in_atomic_region;
            let environment_state_service = self.state.environment_state_service.clone();
            let environment_id = self.state.owned_agent_id.environment_id;
            let default_retry_policy =
                NamedRetryPolicy::default_from_config(&self.state.config.retry);
            let agent_config_retry_policies = self.state.agent_config_retry_policies();
            let runtime_retry_policy_mutations = self.state.runtime_retry_policy_mutations.clone();
            let max_delay = self.durable_execution_state().max_in_function_retry_delay;
            let worker = self.public_state.worker();
            let execution_status = self.execution_status.clone();
            let enrichment_agent_id = self.state.agent_id.clone();
            let enrichment_idempotence = self.state.assume_idempotence;

            let entry = self.table().get_mut(&this)?;
            let entry = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();

            #[allow(clippy::type_complexity)]
            let (result, serializable_invoke_request, serializable_invoke_result, begin_index): (
                Result<Option<Result<SchemaValue, RpcError>>, anyhow::Error>,
                HostRequestGolemRpcInvoke,
                SerializableInvokeResult,
                OplogIndex,
            ) = match entry {
                FutureInvokeResultState::Consumed {
                    request,
                    begin_index,
                    ..
                } => {
                    let begin_index = *begin_index;
                    let message = "future-invoke-result already consumed";
                    (
                        Err(anyhow::Error::new(ClassifiedHostError {
                            kind: HostFailureKind::Permanent,
                            message: message.to_string(),
                        })),
                        request.clone(),
                        SerializableInvokeResult::Failed(message.to_string()),
                        begin_index,
                    )
                }
                FutureInvokeResultState::Pending {
                    request,
                    begin_index,
                    ..
                } => {
                    let begin_index = *begin_index;

                    (
                        Ok(None),
                        request.clone(),
                        SerializableInvokeResult::Pending,
                        begin_index,
                    )
                }
                FutureInvokeResultState::Completed { .. } => {
                    handle_completed_rpc_result(entry, &span_id)?
                }
                FutureInvokeResultState::Cancelled {
                    request,
                    span_id,
                    begin_index,
                } => {
                    let begin_index = *begin_index;
                    let request = request.clone();
                    let rpc_error = InternalRpcError::ProtocolError {
                        details: "Invocation cancelled".to_string(),
                    };
                    let serializable_result = SerializableInvokeResult::Completed(Err(
                        rpc_error.clone().into(),
                    ));
                    *entry = FutureInvokeResultState::Consumed {
                        request: request.clone(),
                        span_id: span_id.clone(),
                        begin_index,
                    };
                    (
                        Ok(Some(Err(rpc_error.into()))),
                        request,
                        serializable_result,
                        begin_index,
                    )
                }
                FutureInvokeResultState::Deferred { .. } => {
                    let enrichment = enrichment_agent_id
                        .as_ref()
                        .map(|id| (id, enrichment_idempotence));
                    handle_deferred_rpc_dispatch(
                        entry,
                        rpc,
                        stack,
                        allow_retry,
                        environment_state_service,
                        environment_id,
                        default_retry_policy,
                        agent_config_retry_policies,
                        runtime_retry_policy_mutations,
                        enrichment,
                        max_delay,
                        worker,
                        execution_status,
                    )?
                }
            };

            // For non-retried transient errors (e.g., from Err(anyhow::Error) path
            // or non-RPC transient errors), fall back to trap+replay
            let for_retry = match &result {
                Err(err) => {
                    let kind = err
                        .downcast_ref::<ClassifiedHostError>()
                        .map(|c| c.kind)
                        .unwrap_or(HostFailureKind::Transient);
                    if kind == HostFailureKind::Transient {
                        Some((err.to_string(), kind))
                    } else {
                        None
                    }
                }
                _ => None,
            };

            if let Some((message, kind)) = for_retry
                && kind == HostFailureKind::Transient
            {
                self.state.current_retry_point = begin_index;
                let failure = anyhow::Error::new(ClassifiedHostError { kind, message });
                let mut properties = RetryProperties::new();
                properties.set("error-type", PredicateValue::Text("transient".to_string()));
                self.try_trigger_retry(failure, properties).await?;
            }

            if self.state.snapshotting_mode.is_none() {
                let is_pending = matches!(
                    serializable_invoke_result,
                    SerializableInvokeResult::Pending
                );

                self.state
                    .oplog
                    .add_host_call(
                        GolemRpcFutureInvokeResultGet::HOST_FUNCTION_NAME,
                        &HostRequest::GolemRpcInvoke(serializable_invoke_request),
                        &HostResponse::GolemRpcInvokeGet(HostResponseGolemRpcInvokeGet {
                            result: serializable_invoke_result,
                        }),
                        DurableFunctionType::WriteRemote,
                    )
                    .await
                    .unwrap_or_else(|err| panic!("failed to serialize RPC response: {err}"));

                if !is_pending {
                    self.end_function(&DurableFunctionType::WriteRemote, begin_index)
                        .await?;

                    self.finish_span(&span_id).await?;
                }

                self.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await;
            }

            match result {
                Ok(Some(Ok(value))) => {
                    // Project the schema-native output to the WIT
                    // `option<schema-value-tree>` shape (`none` for `unit`)
                    // at the guest-facing boundary.
                    Ok(Some(Ok(schema_value_to_wire_output(&value))))
                }
                Ok(Some(Err(error))) => Ok(Some(Err(error))),
                Ok(None) => Ok(None),
                Err(err) => Err(err),
            }
        } else if self.state.persistence_level == PersistenceLevel::PersistNothing {
            Err(WorkerExecutorError::runtime(
                "Trying to replay an RPC call in a PersistNothing block",
            )
            .into())
        } else {
            // Propagate WorkerExecutorError via `?` (From) so the downcast
            // survives the anyhow::Error chain — TrapType::from_error
            // classifies UnexpectedOplogEntry as non-retriable.
            let (_, oplog_entry) = get_oplog_entry!(self.state.replay_state, OplogEntry::HostCall)?;

            let serialized_invoke_result = match oplog_entry {
                OplogEntry::HostCall { response, .. } => {
                    let response =
                        self.state
                            .oplog
                            .download_payload(response)
                            .await
                            .map_err(|err| {
                                WorkerExecutorError::runtime(format!(
                                    "Failed to download golem::rpc::future-invoke-result oplog payload: {err}"
                                ))
                            })?;

                    match response {
                        HostResponse::GolemRpcInvokeGet(HostResponseGolemRpcInvokeGet {
                            result,
                        }) => result,
                        other => {
                            return Err(anyhow::Error::from(
                                WorkerExecutorError::unexpected_oplog_entry(
                                    "HostResponse::GolemRpcInvokeGet",
                                    format!("{other:?}"),
                                ),
                            ));
                        }
                    }
                }
                // The macro above already guarantees `OplogEntry::HostCall`, so
                // this arm is structurally unreachable. We still return an
                // error rather than panicking to keep the function panic-free.
                other => {
                    return Err(anyhow::Error::from(
                        WorkerExecutorError::unexpected_oplog_entry(
                            "OplogEntry::HostCall",
                            format!("{other:?}"),
                        ),
                    ));
                }
            };

            let entry = self.table().get_mut(&this)?;
            let entry = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            let begin_index = entry.begin_index();

            if !matches!(serialized_invoke_result, SerializableInvokeResult::Pending) {
                self.end_function(&DurableFunctionType::WriteRemote, begin_index)
                    .await?;

                self.finish_span(&span_id).await?;
            }

            match serialized_invoke_result {
                SerializableInvokeResult::Pending => Ok(None),
                SerializableInvokeResult::Completed(result) => match result {
                    // The persisted reply is already schema-native; project it
                    // to the WIT `option<schema-value-tree>` shape directly.
                    Ok(value) => Ok(Some(Ok(schema_value_to_wire_output(&value)))),
                    Err(error) => {
                        let rpc_error: InternalRpcError = error.into();
                        let rpc_error: RpcError = rpc_error.into();
                        Ok(Some(Err(rpc_error)))
                    }
                },
                SerializableInvokeResult::Failed(error) => Err(anyhow::anyhow!(error)),
            }
        }
    }

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

        let durability = Durability::<GolemRpcFutureInvokeResultCancel>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        if durability.is_live() {
            if should_attempt_remote_cancel {
                let caller_account_id = self.created_by();
                let caller_account_email = self.created_by_email();
                if let Err(err) = self
                    .worker_proxy()
                    .cancel_invocation(
                        &remote_agent_id,
                        idempotency_key,
                        caller_account_id,
                        caller_account_email,
                    )
                    .await
                {
                    tracing::info!(err=%err, "Best-effort cancel_invocation failed");
                }
            }

            durability
                .persist(self, request, HostResponseGolemRpcUnit {})
                .await
        } else {
            durability.replay(self).await
        }?;

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

        Ok(())
    }

    async fn drop(&mut self, this: Resource<FutureInvokeResult>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::future-invoke-result", "drop");
        let future_rep = this.rep();

        match self.table().delete(this) {
            Ok(entry) => {
                for child_rep in &entry.child_pollables {
                    self.state.rpc_pollable_to_parent.remove(child_rep);
                }
            }
            Err(ResourceTableError::HasChildren) => {
                let parent: Resource<FutureInvokeResult> = Resource::new_borrow(future_rep);
                self.table().get_mut(&parent)?.drop_pending = true;
            }
            Err(err) => return Err(err.into()),
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

        let durability = Durability::<GolemRpcCancellationTokenCancel>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        if durability.is_live() {
            self.scheduler_service()
                .cancel(serialized_schedule_id.clone().into_domain())
                .await;

            durability
                .persist(
                    self,
                    HostRequestGolemRpcScheduledInvocationCancellation {
                        schedule_id: serialized_schedule_id,
                    },
                    HostResponseGolemRpcUnit {},
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(())
    }

    async fn drop(&mut self, this: Resource<CancellationToken>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::cancellation-token", "drop");
        let _ = self.table().delete(this)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> golem_wasm::Host for DurableWorkerCtx<Ctx> {
    async fn parse_uuid(
        &mut self,
        uuid: String,
    ) -> anyhow::Result<Result<golem_wasm::Uuid, String>> {
        Ok(uuid::Uuid::parse_str(&uuid)
            .map(|uuid| uuid.into())
            .map_err(|e| e.to_string()))
    }

    async fn uuid_to_string(&mut self, uuid: golem_wasm::Uuid) -> anyhow::Result<String> {
        let uuid: uuid::Uuid = uuid.into();
        Ok(uuid.to_string())
    }
}

pub async fn construct_wasm_rpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: AgentId,
    env: &[(String, String)],
    config: Vec<AgentConfigEntryDto>,
    durability: &Durability<GolemRpcWasmRpcNew>,
    span: Arc<InvocationContextSpan>,
    remote_agent_type: Arc<AgentType>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let stack = ctx.clone_as_inherited_stack(span.span_id());

    let target_component = ctx
        .component_service()
        .get_metadata(remote_agent_id.component_id, None)
        .await?;
    let target_environment_id = target_component.environment_id;
    let remote_agent_id = OwnedAgentId::new(target_environment_id, &remote_agent_id);
    let demand = ctx
        .rpc()
        .create_demand(
            &remote_agent_id,
            ctx.created_by(),
            ctx.created_by_email(),
            ctx.agent_id(),
            env,
            stack,
            config,
        )
        .await?;
    let target_fingerprint = demand.fingerprint();

    durability
        .persist(
            ctx,
            HostRequestGolemRpcCreate {
                remote_agent_id: remote_agent_id.agent_id(),
            },
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
    remote_agent_type: Arc<AgentType>,
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
    created_by_email: AccountEmail,
    agent_id: AgentId,
    env: Vec<(String, String)>,
    stack: InvocationContextStack,
    retry_params: Option<TaskRetryParams<Ctx>>,
) -> AbortOnDropJoinHandle<Result<Result<SchemaValue, InternalRpcError>, Error>> {
    let invoke = move || {
        let rpc = rpc.clone();
        let remote_agent_id = remote_agent_id.clone();
        let idempotency_key = idempotency_key.clone();
        let method_name = method_name.clone();
        let input = input.clone();
        let created_by = created_by;
        let created_by_email = created_by_email.clone();
        let agent_id = agent_id.clone();
        let env = env.clone();
        let stack = stack.clone();
        async move {
            let result = rpc
                .invoke_and_await(
                    &remote_agent_id,
                    Some(idempotency_key),
                    method_name,
                    input,
                    created_by,
                    &created_by_email,
                    &agent_id,
                    &env,
                    stack,
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

#[allow(clippy::type_complexity)]
fn handle_completed_rpc_result(
    entry: &mut FutureInvokeResultState,
    span_id: &SpanId,
) -> Result<
    (
        Result<Option<Result<SchemaValue, RpcError>>, anyhow::Error>,
        HostRequestGolemRpcInvoke,
        SerializableInvokeResult,
        OplogIndex,
    ),
    WorkerExecutorError,
> {
    // Validate the state *before* any mutation so a corrupt/unexpected state
    // does not leave a torn entry behind.
    if !matches!(entry, FutureInvokeResultState::Completed { .. }) {
        return Err(WorkerExecutorError::runtime(
            "handle_completed_rpc_result called with state != FutureInvokeResultState::Completed",
        ));
    }
    let request = match entry {
        FutureInvokeResultState::Completed { request, .. } => request.clone(),
        // Structurally excluded by the `matches!` check above, but we surface a runtime
        // error instead of panicking to keep the worker-executor process alive on any
        // unforeseen state-machine corruption.
        _ => {
            return Err(WorkerExecutorError::runtime(
                "handle_completed_rpc_result: unexpected non-completed state after precheck",
            ));
        }
    };
    let begin_index = entry.begin_index();
    let span_id = span_id.clone();
    let result = std::mem::replace(
        entry,
        FutureInvokeResultState::Consumed {
            request,
            span_id,
            begin_index,
        },
    );
    if let FutureInvokeResultState::Completed {
        request, result, ..
    } = result
    {
        Ok(match result {
            Ok(Ok(typed)) => (
                Ok(Some(Ok(typed.clone()))),
                request,
                SerializableInvokeResult::Completed(Ok(typed)),
                begin_index,
            ),
            Ok(Err(rpc_error)) => (
                Ok(Some(Err(rpc_error.clone().into()))),
                request,
                SerializableInvokeResult::Completed(Err(rpc_error.into())),
                begin_index,
            ),
            Err(err) => {
                let serializable_err = err.to_string();
                (
                    Err(err),
                    request,
                    SerializableInvokeResult::Failed(serializable_err),
                    begin_index,
                )
            }
        })
    } else {
        // Unreachable in practice (we validated `entry` above and only swapped
        // a different value out of the *same slot*), but kept for safety.
        Err(WorkerExecutorError::runtime(
            "handle_completed_rpc_result: extracted state was not FutureInvokeResultState::Completed",
        ))
    }
}

#[allow(clippy::type_complexity)]
fn handle_deferred_rpc_dispatch<Ctx: WorkerCtx>(
    entry: &mut FutureInvokeResultState,
    rpc: Arc<dyn Rpc>,
    stack: InvocationContextStack,
    allow_retry: bool,
    environment_state_service: Arc<dyn EnvironmentStateService>,
    environment_id: EnvironmentId,
    default_retry_policy: NamedRetryPolicy,
    agent_config_retry_policies: Vec<NamedRetryPolicy>,
    runtime_retry_policy_mutations: std::collections::BTreeMap<String, Option<NamedRetryPolicy>>,
    enrichment: Option<(&LegacyParsedAgentId, bool)>,
    max_in_function_retry_delay: Duration,
    worker: Arc<crate::worker::Worker<Ctx>>,
    execution_status: Arc<std::sync::RwLock<crate::model::ExecutionStatus>>,
) -> anyhow::Result<(
    Result<Option<Result<SchemaValue, RpcError>>, anyhow::Error>,
    HostRequestGolemRpcInvoke,
    SerializableInvokeResult,
    OplogIndex,
)> {
    let begin_index = entry.begin_index();

    let FutureInvokeResultState::Deferred {
        remote_agent_id,
        self_agent_id,
        self_created_by,
        self_created_by_email,
        env,
        method_name,
        method_parameters,
        idempotency_key,
        span_id,
        ..
    } = &*entry
    else {
        return Err(anyhow::anyhow!("unexpected state entry"));
    };

    let request = HostRequestGolemRpcInvoke {
        remote_agent_id: remote_agent_id.agent_id(),
        idempotency_key: idempotency_key.clone(),
        method_name: method_name.clone(),
        input: method_parameters.clone(),
        remote_agent_type: None,
        remote_agent_parameters: None,
    };
    let mut retry_properties = RetryContext::rpc("invoke-and-await", remote_agent_id, method_name);
    if let Some((agent_id, assume_idempotence)) = enrichment {
        retry_properties.set(
            "agent-type",
            PredicateValue::Text(agent_id.agent_type.to_string()),
        );
        retry_properties.set("is-idempotent", PredicateValue::Boolean(assume_idempotence));
    }

    let retry_params = if allow_retry {
        Some(TaskRetryParams {
            environment_state_service,
            environment_id,
            default_retry_policy,
            agent_config_retry_policies,
            runtime_retry_policy_mutations,
            retry_properties,
            max_in_function_retry_delay,
            worker,
            retry_point: begin_index,
            execution_status,
        })
    } else {
        None
    };

    let handle = spawn_rpc_task_with_retry(
        rpc,
        remote_agent_id.clone(),
        idempotency_key.clone(),
        method_name.clone(),
        method_parameters.clone(),
        *self_created_by,
        self_created_by_email.clone(),
        self_agent_id.clone(),
        env.clone(),
        stack,
        retry_params,
    );

    let span_id = span_id.clone();
    *entry = FutureInvokeResultState::Pending {
        handle,
        request: request.clone(),
        span_id,
        begin_index,
    };

    Ok((
        Ok(None),
        request,
        SerializableInvokeResult::Pending,
        begin_index,
    ))
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
    pub remote_agent_type: Arc<AgentType>,
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
    agent_type: &'a AgentType,
    method_name: &str,
) -> anyhow::Result<&'a AgentMethod> {
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
    agent_type: &AgentType,
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
        handle: AbortOnDropJoinHandle<Result<Result<SchemaValue, InternalRpcError>, Error>>,
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
        self_created_by_email: AccountEmail,
        env: Vec<(String, String)>,
        method_name: String,
        method_parameters: SchemaValue,
        idempotency_key: IdempotencyKey,
        span_id: SpanId,
        begin_index: OplogIndex,
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
            let result = handle.await;
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
