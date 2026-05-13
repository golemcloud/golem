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

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, NotCancellable};
use crate::durable_host::durability::{HostFailureKind, InFunctionRetryHost};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::preview2::golem::agent::host::{
    CancellationToken, DataValue, FutureInvokeResult, HostCancellationToken, HostFutureInvokeResult,
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
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{
    AgentMethod, AgentType, DataSchema, LegacyParsedAgentId, UntypedDataValue,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::invocation_context::{AttributeValue, InvocationContextSpan, SpanId};
use golem_common::model::oplog::host_functions::{
    GolemRpcCancellationTokenCancel, GolemRpcFutureInvokeResultCancel, GolemRpcWasmRpcInvoke,
    GolemRpcWasmRpcInvokeAndAwaitResult, GolemRpcWasmRpcNew, GolemRpcWasmRpcScheduleInvocation,
};
use golem_common::model::oplog::types::{SerializableInvokeResult, SerializableScheduleId};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemRpcInvoke, HostRequestGolemRpcScheduledInvocation,
    HostRequestGolemRpcScheduledInvocationCancellation, HostResponseGolemRpcCreate,
    HostResponseGolemRpcInvokeAndAwait, HostResponseGolemRpcScheduledInvocation,
    HostResponseGolemRpcUnit, HostResponseGolemRpcUnitOrFailure,
};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, IdempotencyKey, NamedRetryPolicy, OplogIndex,
    OwnedAgentId, PredicateValue, RetryContext, RetryProperties, ScheduleId, ScheduledAction,
};
use golem_common::schema::TypedSchemaValue;
use golem_common::schema::adapters::{
    typed_input_to_untyped_data_value, typed_schema_value_to_untyped_data_value,
    untyped_data_value_to_typed_input, untyped_data_value_to_typed_schema_output,
};
use golem_common::schema::agent::InputSchema;
use golem_common::schema::schema_value::SchemaValue;
use golem_common::serialization::{deserialize, serialize};

use golem_wasm::{
    CancellationTokenEntry, FutureInvokeResultEntry, SubscribeAny, ValueAndType, WasmRpcEntry,
};
use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use tracing::{Instrument, error};
use wasmtime::component::{Accessor, HasSelf, Resource, ResourceTableError};
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;

use golem_common::model::oplog::payload::HostRequestGolemRpcCreate;
use golem_common::model::worker::AgentConfigEntryDto;
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
        constructor: golem_common::model::agent::bindings::golem::agent::common::DataValue,
        phantom_id: Option<golem_wasm::Uuid>,
        config: Vec<
            golem_common::model::agent::bindings::golem::agent::common::TypedAgentConfigValue,
        >,
    ) -> anyhow::Result<Resource<WasmRpcEntry>> {
        let mut env =
            self.get_environment()?;
        crate::model::AgentConfig::remove_dynamic_vars(&mut env);

        let agent_type = crate::preview2::golem::agent::host::Host::get_agent_type(
            self,
            agent_type_name.clone(),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("Agent type '{}' not found", agent_type_name))?;

        let input = golem_common::model::agent::DataValue::try_from_bindings(
            constructor,
            agent_type.agent_type.constructor.input_schema.clone(),
        )
        .map_err(|err| anyhow::anyhow!("Invalid constructor input: {err}"))?;

        // Convert the bindings-side agent type into the common-model
        // form once and share it through `WasmRpcEntryPayload`. Every
        // subsequent RPC entry resolves the per-method input/output
        // `DataSchema` from this cached value to drive the typed flow.
        let remote_agent_type: Arc<AgentType> =
            Arc::new(AgentType::from(agent_type.agent_type.clone()));

        let agent_id = golem_common::model::agent::LegacyParsedAgentId::new(
            golem_common::model::agent::AgentTypeName(agent_type_name),
            input,
            phantom_id.map(|id| id.into()),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;

        let component_id: golem_common::model::component::ComponentId =
            agent_type.implemented_by.into();
        let remote_agent_id = golem_common::model::AgentId::from_agent_id(component_id, &agent_id)
            .map_err(|err| anyhow::anyhow!("{err}"))?;

        let config = config
            .into_iter()
            .map(|c| {
                let value_and_type = ValueAndType::from(c.value);
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
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    ) -> anyhow::Result<
        Result<golem_common::model::agent::bindings::golem::agent::common::DataValue, RpcError>,
    > {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::wasm-rpc::invoke-and-await")
            .map_err(wasmtime::Error::from)?;

        let mut env =
            self.get_environment()?;
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
        let (input_typed, output_schema) =
            match resolve_method_and_lift_input(&remote_agent_type, &method_name, input) {
                Ok(parts) => parts,
                Err(rpc_error) => return Ok(Err(rpc_error.into())),
            };
        let input_untyped = typed_rpc_input_to_untyped(&input_typed)?;

        let oplog_index = self.state.oplog.current_oplog_index().await;
        let idempotency_key = self.derive_idempotency_key(oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &method_name, &idempotency_key)
                .await?;

        let request = HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: method_name.clone(),
            input: input_untyped.clone(),
            remote_agent_type: None,
            remote_agent_parameters: None,
        };

        let mut handle = CallHandle::<GolemRpcWasmRpcInvokeAndAwaitResult, NotCancellable>::start(
            self,
            request,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result_untyped: Result<UntypedDataValue, InternalRpcError> = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(persisted) => {
                        break 'result match persisted.result {
                            // Re-validate the persisted reply against the current
                            // declared output schema. A mismatch is a permanent
                            // `ProtocolError`; it never emits a new oplog entry.
                            Ok(untyped) => {
                                match output_untyped_to_typed(untyped.clone(), &output_schema) {
                                    Ok(_) => Ok(untyped),
                                    Err(err) => Err(InternalRpcError::ProtocolError {
                                        details: format!("invalid RPC output: {err}"),
                                    }),
                                }
                            }
                            Err(err) => Err(err.into()),
                        };
                    }
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties =
                RetryContext::rpc("invoke-and-await", &remote_agent_id, &method_name);
            let result_typed: Result<TypedSchemaValue, InternalRpcError> =
                loop {
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
                            input_untyped.clone(),
                            created_by,
                            &created_by_email,
                            &agent_id,
                            &env,
                            stack,
                        ),
                        interrupt_signal,
                    )
                    .await;
                    let result_untyped = match either_result {
                        Either::Left((result, _)) => result,
                        Either::Right((interrupt_kind, _)) => {
                            tracing::info!("Interrupted while waiting for RPC result");
                            handle.abandon_for_trap();
                            return Err(interrupt_kind.into());
                        }
                    };

                    // Lift the reply against the declared output schema
                    // before the retry classifier sees it: a schema
                    // mismatch is a protocol-level fault, classified as
                    // permanent so it is persisted into the oplog instead
                    // of triggering a transient retry. On replay, the same
                    // permanent error is reconstructed below from the
                    // persisted untyped payload.
                    let result_typed: Result<TypedSchemaValue, InternalRpcError> =
                        match result_untyped {
                            Ok(untyped) => output_untyped_to_typed(untyped, &output_schema)
                                .map_err(|err| InternalRpcError::ProtocolError {
                                    details: format!("invalid RPC output: {err}"),
                                }),
                            Err(err) => Err(err),
                        };
                    match handle
                        .try_trigger_retry_or_loop_with_properties(
                            self,
                            &result_typed,
                            classify_rpc_error,
                            retry_properties.clone(),
                        )
                        .await?
                    {
                        InternalRetryResult::Persist => break result_typed,
                        InternalRetryResult::RetryInternally => continue,
                    }
                };

            // Project typed → untyped for the oplog payload. A
            // projection failure here is a permanent protocol fault
            // (the typed value's shape does not match a legal
            // [`UntypedDataValue`] layout) and is persisted as such, so
            // replay reproduces the same outcome from the persisted
            // payload instead of re-running the projection.
            let result_untyped: Result<UntypedDataValue, InternalRpcError> = match result_typed {
                Ok(typed) => typed_schema_value_to_untyped_data_value(&typed).map_err(|err| {
                    InternalRpcError::ProtocolError {
                        details: format!(
                            "Failed to convert typed RPC result to legacy form: {err}"
                        ),
                    }
                }),
                Err(err) => Err(err),
            };
            handle
                .complete(
                    self,
                    HostResponseGolemRpcInvokeAndAwait {
                        result: result_untyped.clone().map_err(Into::into),
                    },
                )
                .await?;
            result_untyped
        };

        self.finish_span(span.span_id()).await?;

        match result_untyped {
            Ok(untyped) => {
                let data_value: golem_common::model::agent::bindings::golem::agent::common::DataValue = untyped.into();
                Ok(Ok(data_value))
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
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    ) -> anyhow::Result<Result<(), RpcError>> {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::wasm-rpc::invoke")
            .map_err(wasmtime::Error::from)?;

        let mut env =
            self.get_environment()?;
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

        // Resolve per-method schemas and lift the input before opening
        // durability (see `invoke_and_await` for the rationale). The
        // method's output schema is not needed because `invoke`
        // discards the remote reply.
        let (input_typed, _) =
            match resolve_method_and_lift_input(&remote_agent_type, &method_name, input) {
                Ok(parts) => parts,
                Err(rpc_error) => return Ok(Err(rpc_error.into())),
            };
        let input_untyped = typed_rpc_input_to_untyped(&input_typed)?;

        let oplog_index = self.state.oplog.current_oplog_index().await;
        let idempotency_key = self.derive_idempotency_key(oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &method_name, &idempotency_key)
                .await?;

        let request = HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: method_name.clone(),
            input: input_untyped.clone(),
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
                    Err(err) => break 'result Err(err),
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
                        input_untyped.clone(),
                        self.created_by(),
                        self.created_by_email(),
                        self.agent_id(),
                        &env,
                        stack,
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
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    ) -> anyhow::Result<Resource<FutureInvokeResult>> {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::wasm-rpc::async-invoke-and-await")
            .map_err(wasmtime::Error::from)?;

        let mut env =
            self.get_environment()?;
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

        // Resolve per-method schemas and lift the input. Failures here
        // are deterministic functions of the cached remote agent type
        // and the guest payload, so they are reported as the future's
        // baked-in result rather than as wasmtime traps. The future
        // surfaces the error on the first `get`.
        let (input_typed, output_schema) =
            match resolve_method_and_lift_input(&remote_agent_type, &method_name, input.clone()) {
                Ok(parts) => parts,
                Err(rpc_error) => {
                    let input_untyped: UntypedDataValue = input.into();
                    let request = HostRequestGolemRpcInvoke {
                        remote_agent_id: remote_agent_id.agent_id(),
                        idempotency_key: idempotency_key.clone(),
                        method_name: method_name.clone(),
                        input: input_untyped,
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
        let input_untyped = typed_rpc_input_to_untyped(&input_typed)?;

        let agent_id = self.agent_id().clone();
        let created_by = self.created_by();
        let created_by_email = self.created_by_email().clone();
        let request = HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: method_name.clone(),
            input: input_untyped,
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
                input_typed.clone(),
                output_schema.clone(),
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
                    method_parameters: input_typed,
                    output_schema,
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
        scheduled_time: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
        method_name: String,
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
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
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
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
        let (remote_agent_id, target_worker_fingerprint, input_untyped) = {
            let entry = self.table().get(&this)?;
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
            let remote_agent_id = payload.remote_agent_id.clone();
            let target_worker_fingerprint = payload.target_fingerprint;
            let remote_agent_type = payload.remote_agent_type.clone();

            let method = find_agent_method(&remote_agent_type, &method_name)?;
            let input_typed = input_data_value_to_typed_input(input, &method.input_schema)?;
            let input_untyped = typed_rpc_input_to_untyped(&input_typed)?;

            (remote_agent_id, target_worker_fingerprint, input_untyped)
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
                input: input_untyped.clone(),
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
                input: input_untyped,
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

// TODO(p3): port `HostFutureInvokeResult::get` (the previous `&mut self` body
// removed below) to the `Accessor`-based `HostFutureInvokeResultWithStore::get`
// pattern; it cannot be wrapped trivially because the existing logic awaits on
// `&mut self` across many steps (`Durability::new`, `try_trigger_retry`,
// `commit_oplog_and_update_state`, replay reads, etc.) which the `Accessor`
// API cannot express directly.
impl<Ctx: WorkerCtx> HostFutureInvokeResultWithStore for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn get<T: Send>(
        _accessor: &Accessor<T, Self>,
        _this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Result<DataValue, RpcError>> {
        unimplemented!("HostFutureInvokeResultWithStore::get (p3 migration)")
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
                } => {
                    // Project the in-memory typed input back to the
                    // legacy form for the persisted invoke request.
                    let input_untyped = typed_rpc_input_to_untyped(method_parameters)?;
                    (
                        true,
                        remote_agent_id.agent_id(),
                        idempotency_key.clone(),
                        HostRequestGolemRpcInvoke {
                            remote_agent_id: remote_agent_id.agent_id(),
                            idempotency_key: idempotency_key.clone(),
                            method_name: method_name.clone(),
                            input: input_untyped,
                            remote_agent_type: None,
                            remote_agent_parameters: None,
                        },
                    )
                }
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
                    // The persisted cancelled state stores the request
                    // in legacy form; project the typed input back.
                    let input_untyped = typed_rpc_input_to_untyped(method_parameters)?;
                    *state = FutureInvokeResultState::Cancelled {
                        request: HostRequestGolemRpcInvoke {
                            remote_agent_id: remote_agent_id.agent_id(),
                            idempotency_key: idempotency_key.clone(),
                            method_name: method_name.clone(),
                            input: input_untyped,
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

        // This only releases resource-table bookkeeping for the future and its child pollables (or
        // defers the delete while children are still live). It deliberately does not close the
        // invocation's durable scope: the `WriteRemote` scope opened at `begin_index` is ended by
        // `get()` once it observes a terminal (non-pending) result — including the cancelled result
        // produced after `future-invoke-result.cancel`. A future dropped before `get()` observes a
        // terminal result leaves its scope `Start` open.
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
    mut handle: CallHandle<GolemRpcWasmRpcNew, NotCancellable>,
    remote_agent_id: AgentId,
    env: &[(String, String)],
    config: Vec<AgentConfigEntryDto>,
    span: Arc<InvocationContextSpan>,
    remote_agent_type: Arc<AgentType>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let stack = ctx.clone_as_inherited_stack(span.span_id());

    let target_component = match ctx
        .component_service()
        .get_metadata(remote_agent_id.component_id, None)
        .await
    {
        Ok(target_component) => target_component,
        Err(err) => {
            handle.abandon_for_trap();
            return Err(err.into());
        }
    };
    let target_environment_id = target_component.environment_id;
    let remote_agent_id = OwnedAgentId::new(target_environment_id, &remote_agent_id);
    let demand = match ctx
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
        .await
    {
        Ok(demand) => demand,
        Err(err) => {
            handle.abandon_for_trap();
            return Err(err.into());
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
    input: TypedRpcInput,
    output_schema: DataSchema,
    created_by: AccountId,
    created_by_email: AccountEmail,
    agent_id: AgentId,
    env: Vec<(String, String)>,
    stack: InvocationContextStack,
    retry_params: Option<TaskRetryParams<Ctx>>,
) -> AbortOnDropJoinHandle<Result<Result<TypedSchemaValue, InternalRpcError>, Error>> {
    let invoke = move || {
        let rpc = rpc.clone();
        let remote_agent_id = remote_agent_id.clone();
        let idempotency_key = idempotency_key.clone();
        let method_name = method_name.clone();
        let input = input.clone();
        let output_schema = output_schema.clone();
        let created_by = created_by;
        let created_by_email = created_by_email.clone();
        let agent_id = agent_id.clone();
        let env = env.clone();
        let stack = stack.clone();
        async move {
            // Convert typed → untyped only at the legacy `Rpc::*`
            // boundary.
            let input_untyped = typed_input_to_untyped_data_value(&input.schema, &input.values)
                .map_err(|err| InternalRpcError::ProtocolError {
                    details: format!("failed to convert typed RPC input to legacy form: {err}"),
                })?;
            let result = rpc
                .invoke_and_await(
                    &remote_agent_id,
                    Some(idempotency_key),
                    method_name,
                    input_untyped,
                    created_by,
                    &created_by_email,
                    &agent_id,
                    &env,
                    stack,
                )
                .await?;
            // Re-type the legacy reply against the declared output
            // schema; a schema mismatch is a permanent protocol error.
            untyped_data_value_to_typed_schema_output(result, &output_schema).map_err(|err| {
                InternalRpcError::ProtocolError {
                    details: format!("invalid RPC output: {err}"),
                }
            })
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
        Result<Option<Result<TypedSchemaValue, RpcError>>, anyhow::Error>,
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
    // Borrow-check the persisted result and project typed → untyped
    // *before* swapping the state to `Consumed`. A projection failure
    // is permanent (the typed result's shape does not match a legal
    // `UntypedDataValue` layout) and is reported as
    // `InternalRpcError::ProtocolError`, so the caller can emit the
    // oplog record, end the durable function, finish the span, and
    // return `Ok(Err(RpcError))` to the guest along the normal path.
    if let FutureInvokeResultState::Completed {
        result: Ok(Ok(typed)),
        ..
    } = entry
        && let Err(err) = typed_schema_value_to_untyped_data_value(typed)
    {
        let rpc_error = InternalRpcError::ProtocolError {
            details: format!("Failed to convert typed RPC result to legacy form: {err}"),
        };
        *entry = FutureInvokeResultState::Completed {
            request: request.clone(),
            result: Ok(Err(rpc_error)),
            span_id: span_id.clone(),
            begin_index,
        };
    }
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
            Ok(Ok(typed)) => {
                // Pre-check above guarantees projection succeeds.
                let untyped = typed_schema_value_to_untyped_data_value(&typed)
                    .expect("typed → untyped projection pre-validated above");
                (
                    Ok(Some(Ok(typed))),
                    request,
                    SerializableInvokeResult::Completed(Ok(untyped)),
                    begin_index,
                )
            }
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
    Result<Option<Result<TypedSchemaValue, RpcError>>, anyhow::Error>,
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
        output_schema,
        idempotency_key,
        span_id,
        ..
    } = &*entry
    else {
        return Err(anyhow::anyhow!("unexpected state entry"));
    };

    let input_untyped = typed_rpc_input_to_untyped(method_parameters)?;
    let request = HostRequestGolemRpcInvoke {
        remote_agent_id: remote_agent_id.agent_id(),
        idempotency_key: idempotency_key.clone(),
        method_name: method_name.clone(),
        input: input_untyped,
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
        output_schema.clone(),
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
    /// schemas when bridging the WIT-bindgen / oplog legacy
    /// [`UntypedDataValue`] payload to the in-process
    /// [`TypedSchemaValue`] flow. Sourced from the durable
    /// `get_agent_type` lookup performed in [`HostWasmRpc::new`], so it
    /// is consistent across live execution and replay.
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

/// Typed in-process representation of an RPC call's input parameters.
///
/// Mirrors the design's `InputSchema = Parameters(Vec<NamedField>)` (§4.7
/// of the value-type-refactor doc): an ordered list of named parameters
/// plus a positionally aligned vector of [`SchemaValue`]s. This is the
/// natural typed shape for inputs and avoids the single-root constraint of
/// [`TypedSchemaValue`] (which only fits a single-rooted output value).
#[derive(Clone)]
struct TypedRpcInput {
    schema: InputSchema,
    values: Vec<SchemaValue>,
}

/// Resolve the per-method input/output schemas from the cached remote
/// agent type and lift the guest-side [`bindings::DataValue`] into a
/// [`TypedRpcInput`] for the in-process typed flow.
///
/// All failures are deterministic functions of (agent type, method
/// name, guest input) — replay reproduces them — so they are returned
/// as [`InternalRpcError`] for the caller to surface as `Err(RpcError)`
/// to the guest:
/// - unknown method → [`InternalRpcError::NotFound`]
/// - input that does not match the declared input schema →
///   [`InternalRpcError::ProtocolError`]
fn resolve_method_and_lift_input(
    agent_type: &AgentType,
    method_name: &str,
    input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
) -> Result<(TypedRpcInput, DataSchema), InternalRpcError> {
    let method = agent_type
        .methods
        .iter()
        .find(|m| m.name == method_name)
        .ok_or_else(|| InternalRpcError::NotFound {
            details: format!(
                "Method '{method_name}' not found on agent type '{}'",
                agent_type.type_name
            ),
        })?;
    let input_schema = method.input_schema.clone();
    let output_schema = method.output_schema.clone();
    let untyped: UntypedDataValue = input.into();
    let (schema, values) =
        untyped_data_value_to_typed_input(untyped, &input_schema).map_err(|err| {
            InternalRpcError::ProtocolError {
                details: format!("Invalid RPC input for method '{method_name}': {err}"),
            }
        })?;
    Ok((TypedRpcInput { schema, values }, output_schema))
}

/// Convert a guest-side [`bindings::DataValue`] (already lowered to
/// [`UntypedDataValue`]) into a schema-driven [`TypedRpcInput`] using the
/// method's input schema. Used on the schedule path where the failure is
/// surfaced as a `wasmtime::Error` trap.
fn input_data_value_to_typed_input(
    input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    input_schema: &DataSchema,
) -> anyhow::Result<TypedRpcInput> {
    let untyped: UntypedDataValue = input.into();
    let (schema, values) = untyped_data_value_to_typed_input(untyped, input_schema)
        .map_err(|err| anyhow::anyhow!("Invalid RPC input: {err}"))?;
    Ok(TypedRpcInput { schema, values })
}

/// Project a [`TypedRpcInput`] back into the legacy [`UntypedDataValue`]
/// for crossing the oplog / `Rpc::*` boundaries.
fn typed_rpc_input_to_untyped(input: &TypedRpcInput) -> anyhow::Result<UntypedDataValue> {
    typed_input_to_untyped_data_value(&input.schema, &input.values)
        .map_err(|err| anyhow::anyhow!("Failed to convert typed RPC input to legacy form: {err}"))
}

/// Project a [`TypedSchemaValue`] (an RPC output) back into the legacy
/// [`UntypedDataValue`] for crossing the oplog / `Rpc::*` boundaries.
/// Failures here would mean the typed value's root shape does not match
/// any of the canonical output layouts (empty tuple, multimodal list, or
/// any other single-rooted value).
fn typed_rpc_output_to_untyped(typed: &TypedSchemaValue) -> anyhow::Result<UntypedDataValue> {
    typed_schema_value_to_untyped_data_value(typed)
        .map_err(|err| anyhow::anyhow!("Failed to convert typed RPC output to legacy form: {err}"))
}

/// Convert an [`UntypedDataValue`] returned by the legacy `Rpc::*`
/// boundary into a [`TypedSchemaValue`] using the method's output
/// schema. A failure here indicates a protocol-level mismatch between
/// the remote agent and its declared schema (treated as permanent).
fn output_untyped_to_typed(
    output: UntypedDataValue,
    output_schema: &DataSchema,
) -> anyhow::Result<TypedSchemaValue> {
    untyped_data_value_to_typed_schema_output(output, output_schema)
        .map_err(|err| anyhow::anyhow!("Invalid RPC output: {err}"))
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
        handle: AbortOnDropJoinHandle<Result<Result<TypedSchemaValue, InternalRpcError>, Error>>,
        span_id: SpanId,
        begin_index: OplogIndex,
    },
    Completed {
        request: HostRequestGolemRpcInvoke,
        result: Result<Result<TypedSchemaValue, InternalRpcError>, Error>,
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
        method_parameters: TypedRpcInput,
        /// Needed when the deferred state is materialised into a live
        /// invocation (see [`handle_deferred_rpc_dispatch`]), so the
        /// spawned task can re-type the legacy `Rpc::*` reply.
        output_schema: DataSchema,
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
