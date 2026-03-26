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
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::services::rpc::{Rpc, RpcDemand, RpcError as InternalRpcError};
use crate::services::HasWorker;
use crate::workerctx::{InvocationContextManagement, InvocationManagement, WorkerCtx};
use anyhow::Error;
use async_trait::async_trait;
use futures::future::Either;
use golem_common::base_model::agent::Principal;
use golem_common::model::account::AccountId;
use golem_common::model::agent::UntypedDataValue;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::invocation_context::{AttributeValue, InvocationContextSpan, SpanId};
use golem_common::model::oplog::host_functions::{
    GolemRpcCancellationTokenCancel, GolemRpcFutureInvokeResultCancel,
    GolemRpcFutureInvokeResultGet, GolemRpcWasmRpcInvoke, GolemRpcWasmRpcInvokeAndAwaitResult,
    GolemRpcWasmRpcScheduleInvocation,
};
use golem_common::model::oplog::types::{
    SerializableInvokeResult, SerializableScheduledInvocation,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostRequestGolemRpcInvoke,
    HostRequestGolemRpcScheduledInvocation, HostRequestGolemRpcScheduledInvocationCancellation,
    HostResponse, HostResponseGolemRpcInvokeAndAwait, HostResponseGolemRpcInvokeGet,
    HostResponseGolemRpcScheduledInvocation, HostResponseGolemRpcUnit,
    HostResponseGolemRpcUnitOrFailure, OplogEntry, PersistenceLevel,
};
use golem_common::model::{
    AgentId, AgentInvocation, IdempotencyKey, NamedRetryPolicy, OplogIndex, OwnedAgentId,
    RetryConfig, RetryContext, RetryProperties, ScheduledAction,
};
use golem_common::serialization::{deserialize, serialize};

use golem_wasm::{
    CancellationTokenEntry, FutureInvokeResultEntry, SubscribeAny, ValueAndType, WasmRpcEntry,
};
use std::any::Any;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use tracing::{Instrument, error};
use wasmtime::component::Resource;
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;

use golem_common::model::worker::WorkerAgentConfigEntry;
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
        constructor: golem_common::model::agent::bindings::golem::agent::common::DataValue,
        phantom_id: Option<golem_wasm::Uuid>,
        agent_config: Vec<
            golem_common::model::agent::bindings::golem::agent::common::TypedAgentConfigValue,
        >,
    ) -> anyhow::Result<Resource<WasmRpcEntry>> {
        self.observe_function_call("golem::rpc::wasm-rpc", "new");

        let mut env =
            wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(self).await?;
        crate::model::AgentConfig::remove_dynamic_vars(&mut env);

        let config_vars = self.state.config_vars.clone();

        let agent_type = crate::preview2::golem::agent::host::Host::get_agent_type(
            self,
            agent_type_name.clone(),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("Agent type '{}' not found", agent_type_name))?;

        let input = golem_common::model::agent::DataValue::try_from_bindings(
            constructor,
            agent_type.agent_type.constructor.input_schema,
        )
        .map_err(|err| anyhow::anyhow!("Invalid constructor input: {err}"))?;

        let agent_id = golem_common::model::agent::ParsedAgentId::new(
            golem_common::model::agent::AgentTypeName(agent_type_name),
            input,
            phantom_id.map(|id| id.into()),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;

        let component_id: golem_common::model::component::ComponentId =
            agent_type.implemented_by.into();
        let remote_agent_id = golem_common::model::AgentId::from_agent_id(component_id, &agent_id)
            .map_err(|err| anyhow::anyhow!("{err}"))?;

        let agent_config = agent_config
            .into_iter()
            .map(|c| {
                let value_and_type = ValueAndType::from(c.value);
                let encoded = value_and_type
                    .to_json_value()
                    .map_err(|err| anyhow::anyhow!("Failed serializing agent config: {err}"))?;

                Ok::<_, anyhow::Error>(WorkerAgentConfigEntry {
                    path: c.path,
                    value: encoded,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        construct_wasm_rpc_resource(self, remote_agent_id, &env, config_vars, agent_config).await
    }

    async fn invoke_and_await(
        &mut self,
        self_: Resource<WasmRpcEntry>,
        method_name: String,
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    ) -> anyhow::Result<
        Result<golem_common::model::agent::bindings::golem::agent::common::DataValue, RpcError>,
    > {
        let mut env =
            wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(self).await?;
        crate::model::AgentConfig::remove_dynamic_vars(&mut env);

        let config_vars = self.state.config_vars.clone();
        let own_agent_id = self.owned_agent_id().clone();

        let entry = self.table().get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_agent_id = payload.remote_agent_id.clone();
        let connection_span_id = payload.span_id.clone();

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

        let current_idempotency_key = self
            .get_current_idempotency_key()
            .await
            .unwrap_or(IdempotencyKey::fresh());
        let oplog_index = self.state.oplog.current_oplog_index().await;
        let idempotency_key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &method_name, &idempotency_key)
                .await?;

        let mut durability = Durability::<GolemRpcWasmRpcInvokeAndAwaitResult>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let input_untyped: UntypedDataValue = input.into();

        let result = if durability.is_live() {
            let request = HostRequestGolemRpcInvoke {
                remote_agent_id: remote_agent_id.agent_id(),
                idempotency_key: idempotency_key.clone(),
                method_name: method_name.clone(),
                input: input_untyped.clone(),
                remote_agent_type: None,
                remote_agent_parameters: None,
            };
            let retry_properties =
                RetryContext::rpc("invoke-and-await", &remote_agent_id, &method_name);
            let result = loop {
                let stack = self
                    .state
                    .invocation_context
                    .clone_as_inherited_stack(span.span_id());

                let interrupt_signal = self
                    .execution_status
                    .read()
                    .unwrap()
                    .create_await_interrupt_signal();
                let rpc = self.rpc();
                let created_by = self.created_by();
                let agent_id = self.agent_id().clone();

                let either_result = futures::future::select(
                    rpc.invoke_and_await(
                        &remote_agent_id,
                        Some(idempotency_key.clone()),
                        method_name.clone(),
                        input_untyped.clone(),
                        created_by,
                        &agent_id,
                        &env,
                        config_vars.clone(),
                        stack,
                    ),
                    interrupt_signal,
                )
                .await;
                let result = match either_result {
                    Either::Left((result, _)) => result,
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
                        result: result.map_err(|err| err.into()),
                    },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        self.finish_span(span.span_id()).await?;

        match result.result {
            Ok(untyped_data_value) => {
                let data_value: golem_common::model::agent::bindings::golem::agent::common::DataValue = untyped_data_value.into();
                Ok(Ok(data_value))
            }
            Err(err) => {
                let rpc_error: crate::services::rpc::RpcError = err.into();
                error!("RPC error: {rpc_error}");
                Ok(Err(rpc_error.into()))
            }
        }
    }

    async fn invoke(
        &mut self,
        self_: Resource<WasmRpcEntry>,
        method_name: String,
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    ) -> anyhow::Result<Result<(), RpcError>> {
        let mut env =
            wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(self).await?;
        crate::model::AgentConfig::remove_dynamic_vars(&mut env);

        let config_vars = self.state.config_vars.clone();
        let own_agent_id = self.owned_agent_id().clone();

        let entry = self.table().get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_agent_id = payload.remote_agent_id.clone();
        let connection_span_id = payload.span_id.clone();

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

        let current_idempotency_key = self
            .get_current_idempotency_key()
            .await
            .unwrap_or(IdempotencyKey::fresh());
        let oplog_index = self.state.oplog.current_oplog_index().await;
        let idempotency_key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &method_name, &idempotency_key)
                .await?;

        let mut durability =
            Durability::<GolemRpcWasmRpcInvoke>::new(self, DurableFunctionType::WriteRemote)
                .await?;

        let input_untyped: UntypedDataValue = input.into();

        let result = if durability.is_live() {
            let request = HostRequestGolemRpcInvoke {
                remote_agent_id: remote_agent_id.agent_id(),
                idempotency_key: idempotency_key.clone(),
                method_name: method_name.clone(),
                input: input_untyped.clone(),
                remote_agent_type: None,
                remote_agent_parameters: None,
            };
            let retry_properties = RetryContext::rpc("invoke", &remote_agent_id, &method_name);
            let result = loop {
                let stack = self
                    .state
                    .invocation_context
                    .clone_as_inherited_stack(span.span_id());
                let result = self
                    .rpc()
                    .invoke(
                        &remote_agent_id,
                        Some(idempotency_key.clone()),
                        method_name.clone(),
                        input_untyped.clone(),
                        self.created_by(),
                        self.agent_id(),
                        &env,
                        config_vars.clone(),
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
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    ) -> anyhow::Result<Resource<FutureInvokeResult>> {
        let mut env =
            wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(self).await?;
        crate::model::AgentConfig::remove_dynamic_vars(&mut env);

        let config_vars = self.state.config_vars.clone();
        let own_agent_id = self.owned_agent_id().clone();

        let entry = self.table().get(&this)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_agent_id = payload.remote_agent_id.clone();
        let connection_span_id = payload.span_id.clone();

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

        let begin_index = self
            .begin_function(&DurableFunctionType::WriteRemote)
            .await?;

        let current_idempotency_key = self
            .get_current_idempotency_key()
            .await
            .unwrap_or(IdempotencyKey::fresh());
        let oplog_index = self.state.oplog.current_oplog_index().await;
        let idempotency_key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &method_name, &idempotency_key)
                .await?;

        let input_untyped: UntypedDataValue = input.into();

        let agent_id = self.agent_id().clone();
        let created_by = self.created_by();
        let request = HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: method_name.clone(),
            input: input_untyped.clone(),
            remote_agent_type: None,
            remote_agent_parameters: None,
        };

        let result = if self.state.is_live() {
            let rpc = self.rpc();
            let stack = self
                .state
                .invocation_context
                .clone_as_inherited_stack(span.span_id());

            let in_atomic_region = self.in_atomic_region();
            let retry_config = if in_atomic_region {
                None
            } else {
                Some(self.retry_config())
            };
            let named_retry_policies = if in_atomic_region {
                None
            } else {
                let policies = self.state.named_retry_policies();
                (!policies.is_empty()).then_some(policies.to_vec())
            };
            let retry_properties =
                RetryContext::rpc("invoke-and-await", &remote_agent_id, &method_name);
            let max_delay = self.durable_execution_state().max_in_function_retry_delay;
            let worker = self.public_state.worker();

            let handle = spawn_rpc_task_with_retry(
                rpc,
                remote_agent_id,
                idempotency_key,
                method_name,
                input_untyped,
                created_by,
                agent_id,
                env,
                config_vars,
                stack,
                retry_config,
                named_retry_policies,
                retry_properties,
                max_delay,
                worker,
                begin_index,
                self.execution_status.clone(),
            );

            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Pending {
                    handle,
                    request,
                    span_id: span.span_id().clone(),
                    begin_index,
                }),
                child_pollables: Vec::new(),
            })?;
            Ok(fut)
        } else {
            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Deferred {
                    remote_agent_id,
                    self_agent_id: agent_id,
                    self_created_by: created_by,
                    env,
                    wasi_config_vars: config_vars,
                    method_name,
                    method_parameters: input_untyped,
                    idempotency_key,
                    span_id: span.span_id().clone(),
                    begin_index,
                }),
                child_pollables: Vec::new(),
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
        datetime: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime,
        method_name: String,
        input: golem_common::model::agent::bindings::golem::agent::common::DataValue,
    ) -> anyhow::Result<Resource<CancellationToken>> {
        let durability = Durability::<GolemRpcWasmRpcScheduleInvocation>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let entry = self.table().get(&this)?;
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
            let remote_agent_id = payload.remote_agent_id.clone();

            let input_untyped: UntypedDataValue = input.into();

            let current_idempotency_key = self
                .state
                .get_current_idempotency_key()
                .expect("Expected to get an idempotency key as we are inside an invocation");

            let current_oplog_index = self.state.oplog.current_oplog_index().await;

            let idempotency_key =
                IdempotencyKey::derived(&current_idempotency_key, current_oplog_index);

            let request = HostRequestGolemRpcScheduledInvocation {
                remote_agent_id: remote_agent_id.agent_id(),
                idempotency_key: idempotency_key.clone(),
                method_name: method_name.clone(),
                input: input_untyped.clone(),
                datetime: datetime.into(),
                remote_agent_type: None,
                remote_agent_parameters: None,
            };

            let stack = self
                .state
                .invocation_context
                .clone_as_inherited_stack(&self.state.current_span_id);

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
            };

            let result = self
                .state
                .scheduler_service
                .schedule(
                    chrono::DateTime::from_timestamp(datetime.seconds as i64, datetime.nanoseconds)
                        .expect("Received invalid datetime from wasi"),
                    action,
                )
                .await;

            let invocation = SerializableScheduledInvocation::from_domain(result)
                .map_err(|err| anyhow::anyhow!(err))?;

            durability
                .persist(
                    self,
                    request,
                    HostResponseGolemRpcScheduledInvocation { invocation },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        let serialized_result = serialize(&result.invocation).expect("Failed to serialize result");
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
        Ok(pollable)
    }

    async fn get(
        &mut self,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<
        Option<
            Result<golem_common::model::agent::bindings::golem::agent::common::DataValue, RpcError>,
        >,
    > {
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
            let stack = self
                .state
                .invocation_context
                .clone_as_inherited_stack(&span_id);

            let in_atomic_region = self.in_atomic_region();
            let retry_config = if in_atomic_region {
                None
            } else {
                Some(self.retry_config())
            };
            let named_retry_policies = if in_atomic_region {
                None
            } else {
                let policies = self.state.named_retry_policies();
                (!policies.is_empty()).then_some(policies.to_vec())
            };
            let max_delay = self.durable_execution_state().max_in_function_retry_delay;
            let worker = self.public_state.worker();
            let execution_status = self.execution_status.clone();

            let entry = self.table().get_mut(&this)?;
            let entry = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();

            #[allow(clippy::type_complexity)]
            let (result, serializable_invoke_request, serializable_invoke_result, begin_index): (
                Result<Option<Result<UntypedDataValue, RpcError>>, anyhow::Error>,
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
                    handle_completed_rpc_result(entry, &span_id)
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
                    handle_deferred_rpc_dispatch(
                        entry,
                        rpc,
                        stack,
                        retry_config,
                        named_retry_policies,
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
                self.try_trigger_retry(failure).await?;
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
                Ok(Some(Ok(untyped))) => {
                    let data_value: golem_common::model::agent::bindings::golem::agent::common::DataValue = untyped.into();
                    Ok(Some(Ok(data_value)))
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
            let (_, oplog_entry) = get_oplog_entry!(self.state.replay_state, OplogEntry::HostCall)
                .map_err(|golem_err| {
                    anyhow::anyhow!(
                    "failed to get golem::rpc::future-invoke-result::get oplog entry: {golem_err}"
                )
                })?;

            let serialized_invoke_result = match oplog_entry {
                OplogEntry::HostCall { response, .. } => {
                    let response =
                        self.state
                            .oplog
                            .download_payload(response)
                            .await
                            .map_err(|err| {
                                anyhow::anyhow!("Failed to download oplog payload: {err}")
                            })?;

                    match response {
                        HostResponse::GolemRpcInvokeGet(HostResponseGolemRpcInvokeGet {
                            result,
                        }) => result,
                        _ => panic!("unexpected oplog payload type"),
                    }
                }
                _ => panic!("unexpected oplog entry type"),
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
                    Ok(untyped) => {
                        let data_value: golem_common::model::agent::bindings::golem::agent::common::DataValue = untyped.into();
                        Ok(Some(Ok(data_value)))
                    }
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
                if let Err(err) = self
                    .worker_proxy()
                    .cancel_invocation(&remote_agent_id, idempotency_key, caller_account_id)
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

        let child_reps = {
            let entry = self.table().get_mut(&this)?;
            std::mem::take(&mut entry.child_pollables)
        };

        for rep in child_reps {
            let child: Resource<golem_wasm::DynPollable> = Resource::new_own(rep);
            if let Err(err) = self.table().delete(child) {
                tracing::debug!(rep, err=%err, "Child pollable already dropped by guest");
            }
        }

        let _ = self.table().delete(this)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostCancellationToken for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, this: Resource<CancellationToken>) -> anyhow::Result<()> {
        let entry = self.table().get(&this)?;
        let serialized_scheduled_invocation: SerializableScheduledInvocation =
            deserialize(&entry.schedule_id).expect("Failed to deserialize cancellation token");

        let durability = Durability::<GolemRpcCancellationTokenCancel>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        if durability.is_live() {
            self.scheduler_service()
                .cancel(serialized_scheduled_invocation.clone().into_domain())
                .await;

            durability
                .persist(
                    self,
                    HostRequestGolemRpcScheduledInvocationCancellation {
                        invocation: serialized_scheduled_invocation,
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
    config: std::collections::BTreeMap<String, String>,
    agent_config: Vec<WorkerAgentConfigEntry>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let span = create_rpc_connection_span(ctx, &remote_agent_id).await?;

    let stack = ctx
        .state
        .invocation_context
        .clone_as_inherited_stack(span.span_id());

    let remote_agent_id = OwnedAgentId::new(ctx.owned_agent_id.environment_id, &remote_agent_id);
    let demand = ctx
        .rpc()
        .create_demand(
            &remote_agent_id,
            ctx.created_by(),
            ctx.agent_id(),
            env,
            config,
            stack,
            agent_config,
        )
        .await?;
    let entry = ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            demand,
            remote_agent_id,
            span_id: span.span_id().clone(),
        }),
    })?;
    Ok(entry)
}

fn spawn_rpc_task_with_retry<Ctx: WorkerCtx>(
    rpc: Arc<dyn Rpc>,
    remote_agent_id: OwnedAgentId,
    idempotency_key: IdempotencyKey,
    method_name: String,
    input: UntypedDataValue,
    created_by: AccountId,
    agent_id: AgentId,
    env: Vec<(String, String)>,
    config_vars: BTreeMap<String, String>,
    stack: InvocationContextStack,
    retry_config: Option<RetryConfig>,
    named_retry_policies: Option<Vec<NamedRetryPolicy>>,
    retry_properties: RetryProperties,
    max_in_function_retry_delay: Duration,
    worker: Arc<crate::worker::Worker<Ctx>>,
    retry_point: OplogIndex,
    execution_status: Arc<std::sync::RwLock<crate::model::ExecutionStatus>>,
) -> AbortOnDropJoinHandle<Result<Result<UntypedDataValue, InternalRpcError>, Error>> {
    let invoke = move || {
        let rpc = rpc.clone();
        let remote_agent_id = remote_agent_id.clone();
        let idempotency_key = idempotency_key.clone();
        let method_name = method_name.clone();
        let input = input.clone();
        let created_by = created_by;
        let agent_id = agent_id.clone();
        let env = env.clone();
        let config_vars = config_vars.clone();
        let stack = stack.clone();
        async move {
            rpc.invoke_and_await(
                &remote_agent_id,
                Some(idempotency_key),
                method_name,
                input,
                created_by,
                &agent_id,
                &env,
                config_vars,
                stack,
            )
            .await
        }
    };

    wasmtime_wasi::runtime::spawn(
        async move {
            let result = match retry_config {
                Some(retry_config) => {
                    let current_retry_policy_state = worker
                        .get_non_detached_last_known_status()
                        .await
                        .current_retry_state
                        .get(&retry_point)
                        .cloned();
                    let task_ctx = crate::durable_host::durability::TaskRetryContext {
                        retry_point,
                        retry_config,
                        named_retry_policies,
                        max_in_function_retry_delay,
                        current_retry_policy_state,
                        retry_properties,
                        worker,
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
                }
                None => invoke().await,
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
) -> (
    Result<Option<Result<UntypedDataValue, RpcError>>, anyhow::Error>,
    HostRequestGolemRpcInvoke,
    SerializableInvokeResult,
    OplogIndex,
) {
    let request = match entry {
        FutureInvokeResultState::Completed { request, .. } => request.clone(),
        _ => panic!("unexpected state: not FutureInvokeResultState::Completed"),
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
        match result {
            Ok(Ok(untyped)) => (
                Ok(Some(Ok(untyped.clone()))),
                request,
                SerializableInvokeResult::Completed(Ok(untyped)),
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
        }
    } else {
        panic!("unexpected state: not FutureInvokeResultState::Completed")
    }
}

#[allow(clippy::type_complexity)]
fn handle_deferred_rpc_dispatch<Ctx: WorkerCtx>(
    entry: &mut FutureInvokeResultState,
    rpc: Arc<dyn Rpc>,
    stack: InvocationContextStack,
    retry_config: Option<RetryConfig>,
    named_retry_policies: Option<Vec<NamedRetryPolicy>>,
    max_in_function_retry_delay: Duration,
    worker: Arc<crate::worker::Worker<Ctx>>,
    execution_status: Arc<std::sync::RwLock<crate::model::ExecutionStatus>>,
) -> anyhow::Result<(
    Result<Option<Result<UntypedDataValue, RpcError>>, anyhow::Error>,
    HostRequestGolemRpcInvoke,
    SerializableInvokeResult,
    OplogIndex,
)> {
    let begin_index = entry.begin_index();

    let FutureInvokeResultState::Deferred {
        remote_agent_id,
        self_agent_id,
        self_created_by,
        env,
        wasi_config_vars: config_vars,
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
    let retry_properties = RetryContext::rpc("invoke-and-await", remote_agent_id, method_name);

    let handle = spawn_rpc_task_with_retry(
        rpc,
        remote_agent_id.clone(),
        idempotency_key.clone(),
        method_name.clone(),
        method_parameters.clone(),
        *self_created_by,
        self_agent_id.clone(),
        env.clone(),
        config_vars.clone(),
        stack,
        retry_config,
        named_retry_policies,
        retry_properties,
        max_in_function_retry_delay,
        worker,
        begin_index,
        execution_status,
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
}

impl Debug for WasmRpcEntryPayload {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmRpcEntryPayload")
            .field("remote_agent_id", &self.remote_agent_id)
            .finish()
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
        handle: AbortOnDropJoinHandle<Result<Result<UntypedDataValue, InternalRpcError>, Error>>,
        span_id: SpanId,
        begin_index: OplogIndex,
    },
    Completed {
        request: HostRequestGolemRpcInvoke,
        result: Result<Result<UntypedDataValue, InternalRpcError>, Error>,
        span_id: SpanId,
        begin_index: OplogIndex,
    },
    Deferred {
        remote_agent_id: OwnedAgentId,
        self_agent_id: AgentId,
        self_created_by: AccountId,
        env: Vec<(String, String)>,
        wasi_config_vars: BTreeMap<String, String>,
        method_name: String,
        method_parameters: UntypedDataValue,
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
