// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::get_oplog_entry;
use crate::preview2::golem::agent::host::{
    CancellationToken, FutureInvokeResult, HostCancellationToken, HostFutureInvokeResult,
    HostWasmRpc, RpcError,
};
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::services::rpc::{RpcDemand, RpcError as InternalRpcError};
use crate::services::HasWorker;
use crate::workerctx::{
    HasConfigVars, InvocationContextManagement, InvocationManagement, WorkerCtx,
};
use anyhow::Error;
use async_trait::async_trait;
use futures::future::Either;
use golem_common::base_model::agent::Principal;
use golem_common::model::account::AccountId;
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::UntypedDataValue;
use golem_common::model::invocation_context::{AttributeValue, InvocationContextSpan, SpanId};
use golem_common::model::oplog::host_functions::{
    GolemRpcCancellationTokenCancel, GolemRpcFutureInvokeResultGet, GolemRpcWasmRpcInvoke,
    GolemRpcWasmRpcInvokeAndAwaitResult, GolemRpcWasmRpcScheduleInvocation,
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
    AgentInvocation, IdempotencyKey, OplogIndex, OwnedWorkerId, ScheduledAction, WorkerId,
};
use golem_common::serialization::{deserialize, serialize};
use golem_wasm::{CancellationTokenEntry, FutureInvokeResultEntry, SubscribeAny, WasmRpcEntry};
use std::any::Any;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use tracing::{error, Instrument};
use wasmtime::component::Resource;
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;

use golem_service_base::error::worker_executor::WorkerExecutorError;

impl<Ctx: WorkerCtx> HostWasmRpc for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        agent_type_name: String,
        constructor: golem_common::model::agent::bindings::golem::agent::common::DataValue,
        phantom_id: Option<golem_wasm::Uuid>,
    ) -> anyhow::Result<Resource<WasmRpcEntry>> {
        self.observe_function_call("golem::rpc::wasm-rpc", "new");

        let mut env =
            wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(self).await?;
        crate::model::WorkerConfig::remove_dynamic_vars(&mut env);

        let config_vars = self.config_vars();

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

        let agent_id = golem_common::model::agent::AgentId::new(
            golem_common::model::agent::AgentTypeName(agent_type_name).to_wit_naming(),
            input,
            phantom_id.map(|id| id.into()),
        );

        let component_id: golem_common::model::component::ComponentId =
            agent_type.implemented_by.into();
        let remote_worker_id =
            golem_common::model::WorkerId::from_agent_id(component_id, &agent_id)
                .map_err(|err| anyhow::anyhow!("{err}"))?;

        construct_wasm_rpc_resource(self, remote_worker_id, &env, config_vars).await
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
        crate::model::WorkerConfig::remove_dynamic_vars(&mut env);

        let config_vars = self.config_vars();
        let own_worker_id = self.owned_worker_id().clone();

        let entry = self.table().get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id.clone();
        let connection_span_id = payload.span_id.clone();

        if remote_worker_id == own_worker_id {
            return Err(anyhow::anyhow!(
                "RPC calls to the same agent are not supported"
            ));
        }

        let current_idempotency_key = self
            .get_current_idempotency_key()
            .await
            .unwrap_or(IdempotencyKey::fresh());
        let oplog_index = self.state.oplog.current_oplog_index().await;
        let idempotency_key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &method_name, &idempotency_key)
                .await?;

        let durability = Durability::<GolemRpcWasmRpcInvokeAndAwaitResult>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let input_untyped: UntypedDataValue = input.into();

        let result = if durability.is_live() {
            let request = HostRequestGolemRpcInvoke {
                remote_worker_id: remote_worker_id.worker_id(),
                idempotency_key: idempotency_key.clone(),
                method_name: method_name.clone(),
                input: input_untyped.clone(),
                remote_agent_type: None,
                remote_agent_parameters: None,
            };
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
            let worker_id = self.worker_id().clone();

            let either_result = futures::future::select(
                rpc.invoke_and_await(
                    &remote_worker_id,
                    Some(idempotency_key),
                    method_name,
                    input_untyped,
                    created_by,
                    &worker_id,
                    &env,
                    config_vars,
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
            durability.try_trigger_retry(self, &result).await?;

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
        crate::model::WorkerConfig::remove_dynamic_vars(&mut env);

        let config_vars = self.config_vars();
        let own_worker_id = self.owned_worker_id().clone();

        let entry = self.table().get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id.clone();
        let connection_span_id = payload.span_id.clone();

        if remote_worker_id == own_worker_id {
            return Err(anyhow::anyhow!(
                "RPC calls to the same agent are not supported"
            ));
        }

        let current_idempotency_key = self
            .get_current_idempotency_key()
            .await
            .unwrap_or(IdempotencyKey::fresh());
        let oplog_index = self.state.oplog.current_oplog_index().await;
        let idempotency_key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &method_name, &idempotency_key)
                .await?;

        let durability =
            Durability::<GolemRpcWasmRpcInvoke>::new(self, DurableFunctionType::WriteRemote)
                .await?;

        let input_untyped: UntypedDataValue = input.into();

        let result = if durability.is_live() {
            let request = HostRequestGolemRpcInvoke {
                remote_worker_id: remote_worker_id.worker_id(),
                idempotency_key: idempotency_key.clone(),
                method_name: method_name.clone(),
                input: input_untyped.clone(),
                remote_agent_type: None,
                remote_agent_parameters: None,
            };
            let stack = self
                .state
                .invocation_context
                .clone_as_inherited_stack(span.span_id());
            let result = self
                .rpc()
                .invoke(
                    &remote_worker_id,
                    Some(idempotency_key),
                    method_name,
                    input_untyped,
                    self.created_by(),
                    self.worker_id(),
                    &env,
                    config_vars,
                    stack,
                )
                .await;
            durability.try_trigger_retry(self, &result).await?;

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
        crate::model::WorkerConfig::remove_dynamic_vars(&mut env);

        let config_vars = self.config_vars();
        let own_worker_id = self.owned_worker_id().clone();

        let begin_index = self
            .begin_function(&DurableFunctionType::WriteRemote)
            .await?;

        let entry = self.table().get(&this)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id.clone();
        let connection_span_id = payload.span_id.clone();

        if remote_worker_id == own_worker_id {
            return Err(anyhow::anyhow!(
                "RPC calls to the same agent are not supported"
            ));
        }

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

        let worker_id = self.worker_id().clone();
        let created_by = self.created_by();
        let request = HostRequestGolemRpcInvoke {
            remote_worker_id: remote_worker_id.worker_id(),
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
            let handle = wasmtime_wasi::runtime::spawn(
                async move {
                    Ok(rpc
                        .invoke_and_await(
                            &remote_worker_id,
                            Some(idempotency_key),
                            method_name,
                            input_untyped,
                            created_by,
                            &worker_id,
                            &env,
                            config_vars,
                            stack,
                        )
                        .await)
                }
                .in_current_span(),
            );

            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Pending {
                    handle,
                    request,
                    span_id: span.span_id().clone(),
                    begin_index,
                }),
            })?;
            Ok(fut)
        } else {
            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Deferred {
                    remote_worker_id,
                    self_worker_id: worker_id,
                    self_created_by: created_by,
                    env,
                    wasi_config_vars: config_vars,
                    method_name,
                    method_parameters: input_untyped,
                    idempotency_key,
                    span_id: span.span_id().clone(),
                    begin_index,
                }),
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
            let remote_worker_id = payload.remote_worker_id.clone();

            let input_untyped: UntypedDataValue = input.into();

            let current_idempotency_key = self
                .state
                .get_current_idempotency_key()
                .expect("Expected to get an idempotency key as we are inside an invocation");

            let current_oplog_index = self.state.oplog.current_oplog_index().await;

            let idempotency_key =
                IdempotencyKey::derived(&current_idempotency_key, current_oplog_index);

            let request = HostRequestGolemRpcScheduledInvocation {
                remote_worker_id: remote_worker_id.worker_id(),
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
                owned_worker_id: remote_worker_id,
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
        wasmtime_wasi::dynamic_subscribe(self.table(), this, None)
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
            let stack = self
                .state
                .invocation_context
                .clone_as_inherited_stack(&span_id);

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
                        Err(anyhow::anyhow!(message)),
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
                FutureInvokeResultState::Completed {
                    request,
                    begin_index,
                    ..
                } => {
                    let request = request.clone();
                    let begin_index = *begin_index;
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
                FutureInvokeResultState::Deferred { begin_index, .. } => {
                    let begin_index = *begin_index;

                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let handle = wasmtime_wasi::runtime::spawn(
                        async move {
                            let request = rx.await.map_err(|err| anyhow::anyhow!(err))?;
                            let FutureInvokeResultState::Deferred {
                                remote_worker_id,
                                self_worker_id,
                                self_created_by,
                                env,
                                wasi_config_vars: config_vars,
                                method_name,
                                method_parameters,
                                idempotency_key,
                                ..
                            } = request
                            else {
                                return Err(anyhow::anyhow!(
                                    "unexpected incoming response state".to_string()
                                ));
                            };
                            Ok(rpc
                                .invoke_and_await(
                                    &remote_worker_id,
                                    Some(idempotency_key),
                                    method_name,
                                    method_parameters,
                                    self_created_by,
                                    &self_worker_id,
                                    &env,
                                    config_vars,
                                    stack,
                                )
                                .await)
                        }
                        .in_current_span(),
                    );
                    let FutureInvokeResultState::Deferred {
                        remote_worker_id,
                        method_name,
                        method_parameters,
                        idempotency_key,
                        span_id,
                        ..
                    } = &*entry
                    else {
                        return Err(anyhow::anyhow!("unexpected state entry".to_string()));
                    };
                    let request = HostRequestGolemRpcInvoke {
                        remote_worker_id: remote_worker_id.worker_id(),
                        idempotency_key: idempotency_key.clone(),
                        method_name: method_name.clone(),
                        input: method_parameters.clone(),
                        remote_agent_type: None,
                        remote_agent_parameters: None,
                    };

                    tx.send(std::mem::replace(
                        entry,
                        FutureInvokeResultState::Pending {
                            handle,
                            request: request.clone(),
                            span_id: span_id.clone(),
                            begin_index,
                        },
                    ))
                    .map_err(|_| anyhow::anyhow!("failed to send request to handler"))?;
                    (
                        Ok(None),
                        request,
                        SerializableInvokeResult::Pending,
                        begin_index,
                    )
                }
            };

            let for_retry = match &result {
                Err(err) => Err(anyhow::anyhow!(err.to_string())),
                Ok(Some(Err(err))) => Err(anyhow::anyhow!(err.to_string())),
                _ => Ok(()),
            };

            if let Err(err) = for_retry {
                self.state.current_retry_point = begin_index;
                self.try_trigger_retry(err).await?;
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

    async fn drop(&mut self, this: Resource<FutureInvokeResult>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::future-invoke-result", "drop");
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
    remote_worker_id: WorkerId,
    env: &[(String, String)],
    config: std::collections::BTreeMap<String, String>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let span = create_rpc_connection_span(ctx, &remote_worker_id).await?;

    let stack = ctx
        .state
        .invocation_context
        .clone_as_inherited_stack(span.span_id());

    let remote_worker_id =
        OwnedWorkerId::new(ctx.owned_worker_id.environment_id, &remote_worker_id);
    let demand = ctx
        .rpc()
        .create_demand(
            &remote_worker_id,
            ctx.created_by(),
            ctx.worker_id(),
            env,
            config,
            stack,
        )
        .await?;
    let entry = ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            demand,
            remote_worker_id,
            span_id: span.span_id().clone(),
        }),
    })?;
    Ok(entry)
}

pub struct WasmRpcEntryPayload {
    #[allow(dead_code)]
    pub demand: Box<dyn RpcDemand>,
    pub remote_worker_id: OwnedWorkerId,
    pub span_id: SpanId,
}

impl Debug for WasmRpcEntryPayload {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmRpcEntryPayload")
            .field("remote_worker_id", &self.remote_worker_id)
            .finish()
    }
}

pub async fn create_rpc_connection_span<Ctx: InvocationContextManagement>(
    ctx: &mut Ctx,
    target_worker_id: &WorkerId,
) -> anyhow::Result<Arc<InvocationContextSpan>> {
    Ok(ctx
        .start_span(
            &[
                (
                    "name".to_string(),
                    AttributeValue::String("rpc-connection".to_string()),
                ),
                (
                    "target_worker_id".to_string(),
                    AttributeValue::String(target_worker_id.to_string()),
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
        remote_worker_id: OwnedWorkerId,
        self_worker_id: WorkerId,
        self_created_by: AccountId,
        env: Vec<(String, String)>,
        wasi_config_vars: BTreeMap<String, String>,
        method_name: String,
        method_parameters: UntypedDataValue,
        idempotency_key: IdempotencyKey,
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
            | Self::Consumed { span_id, .. } => span_id,
        }
    }

    pub fn begin_index(&self) -> OplogIndex {
        match self {
            Self::Pending { begin_index, .. } => *begin_index,
            Self::Completed { begin_index, .. } => *begin_index,
            Self::Deferred { begin_index, .. } => *begin_index,
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
            *self = Self::Completed {
                result: handle.await,
                request: request.clone(),
                span_id: span_id.clone(),
                begin_index: *begin_index,
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
