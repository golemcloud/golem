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
use crate::model::WorkerConfig;
use crate::services::component::ComponentService;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::services::rpc::{RpcDemand, RpcError};
use crate::services::HasWorker;
use crate::workerctx::{
    HasWasiConfigVars, InvocationContextManagement, InvocationManagement, WorkerCtx,
};
use anyhow::{anyhow, Error};
use async_trait::async_trait;
use futures::future::Either;
use golem_common::model::account::AccountId;
use golem_common::model::component::ComponentId;
use golem_common::model::invocation_context::{AttributeValue, InvocationContextSpan, SpanId};
use golem_common::model::oplog::host_functions::GolemRpcFutureInvokeResultGet;
use golem_common::model::oplog::host_functions::{
    GolemRpcCancellationTokenCancel, GolemRpcWasmRpcInvoke, GolemRpcWasmRpcInvokeAndAwaitResult,
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
use golem_common::model::{IdempotencyKey, OplogIndex, OwnedWorkerId, ScheduledAction, WorkerId};
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_wasm::analysis::analysed_type;
use golem_wasm::golem_rpc_0_2_x::types::{
    CancellationToken, FutureInvokeResult, HostCancellationToken, HostFutureInvokeResult, Pollable,
    Uri,
};
use golem_wasm::{
    CancellationTokenEntry, FutureInvokeResultEntry, HostWasmRpc, SubscribeAny, Value,
    ValueAndType, WasmRpcEntry, WitValue,
};
use std::any::Any;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use tracing::{error, Instrument};
use uuid::Uuid;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::cli::environment::Host;
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;
use wasmtime_wasi::subscribe;

impl<Ctx: WorkerCtx> HostWasmRpc for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        worker_id: golem_wasm::golem_rpc_0_2_x::types::AgentId,
    ) -> anyhow::Result<Resource<WasmRpcEntry>> {
        self.observe_function_call("golem::rpc::wasm-rpc", "new");

        let mut env = self.get_environment().await?;
        WorkerConfig::remove_dynamic_vars(&mut env);

        let wasi_config_vars = self.wasi_config_vars();

        let remote_worker_id: WorkerId = worker_id.into();

        construct_wasm_rpc_resource(self, remote_worker_id, &env, wasi_config_vars).await
    }

    async fn invoke_and_await(
        &mut self,
        self_: Resource<WasmRpcEntry>,
        function_name: String,
        mut function_params: Vec<WitValue>,
    ) -> anyhow::Result<Result<WitValue, golem_wasm::RpcError>> {
        let mut env = self.get_environment().await?;
        WorkerConfig::remove_dynamic_vars(&mut env);

        let wasi_config_vars = self.wasi_config_vars();
        let own_worker_id = self.owned_worker_id().clone();

        let entry = self.table().get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id().clone();

        let connection_span_id = payload.span_id().clone();

        Self::add_self_parameter_if_needed(&mut function_params, payload);

        let current_idempotency_key = self
            .get_current_idempotency_key()
            .await
            .unwrap_or(IdempotencyKey::fresh());
        let oplog_index = self.state.oplog.current_oplog_index().await;

        let idempotency_key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &function_name, &idempotency_key)
                .await?;

        let durability = Durability::<GolemRpcWasmRpcInvokeAndAwaitResult>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        if remote_worker_id == own_worker_id {
            return Err(anyhow!("RPC calls to the same agent are not supported"));
        }

        let result = if durability.is_live() {
            let request = HostRequestGolemRpcInvoke {
                remote_worker_id: remote_worker_id.worker_id(),
                idempotency_key: idempotency_key.clone(),
                function_name: function_name.clone(),
                function_params: try_get_typed_parameters(
                    self.state.component_service.clone(),
                    &remote_worker_id.worker_id.component_id,
                    &function_name,
                    &function_params,
                )
                .await,
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
            let created_by = *self.created_by();
            let worker_id = self.worker_id().clone();

            let either_result = futures::future::select(
                rpc.invoke_and_await(
                    &remote_worker_id,
                    Some(idempotency_key),
                    function_name,
                    function_params,
                    &created_by,
                    &worker_id,
                    &env,
                    wasi_config_vars,
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
            Ok(value_and_type) => {
                // Temporary wrapping of the WitValue in a tuple to keep the original WIT interface
                let wit_value = match value_and_type {
                    Some(value_and_type) => {
                        let value: Value = value_and_type.value;
                        let wrapped = Value::Tuple(vec![value]);
                        WitValue::from(wrapped)
                    }
                    None => WitValue::from(Value::Record(vec![])),
                };

                Ok(Ok(wit_value))
            }
            Err(err) => {
                let rpc_error: RpcError = err.into();
                error!("RPC error: {rpc_error}");
                Ok(Err(rpc_error.into()))
            }
        }
    }

    async fn invoke(
        &mut self,
        self_: Resource<WasmRpcEntry>,
        function_name: String,
        mut function_params: Vec<WitValue>,
    ) -> anyhow::Result<Result<(), golem_wasm::RpcError>> {
        let mut env = self.get_environment().await?;
        WorkerConfig::remove_dynamic_vars(&mut env);

        let wasi_config_vars = self.wasi_config_vars();
        let own_worker_id = self.owned_worker_id().clone();

        let entry = self.table().get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id().clone();

        let connection_span_id = payload.span_id().clone();

        Self::add_self_parameter_if_needed(&mut function_params, payload);

        let current_idempotency_key = self
            .get_current_idempotency_key()
            .await
            .unwrap_or(IdempotencyKey::fresh());
        let oplog_index = self.state.oplog.current_oplog_index().await;

        let idempotency_key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &function_name, &idempotency_key)
                .await?;

        let durability =
            Durability::<GolemRpcWasmRpcInvoke>::new(self, DurableFunctionType::WriteRemote)
                .await?;

        if remote_worker_id == own_worker_id {
            return Err(anyhow!("RPC calls to the same agent are not supported"));
        }

        let result = if durability.is_live() {
            let request = HostRequestGolemRpcInvoke {
                remote_worker_id: remote_worker_id.worker_id(),
                idempotency_key: idempotency_key.clone(),
                function_name: function_name.clone(),
                function_params: try_get_typed_parameters(
                    self.state.component_service.clone(),
                    &remote_worker_id.worker_id.component_id,
                    &function_name,
                    &function_params,
                )
                .await,
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
                    function_name,
                    function_params,
                    self.created_by(),
                    self.worker_id(),
                    &env,
                    wasi_config_vars,
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
                let rpc_error: RpcError = err.into();
                error!("RPC error for: {rpc_error}");
                Ok(Err(rpc_error.into()))
            }
        }
    }

    async fn async_invoke_and_await(
        &mut self,
        this: Resource<WasmRpcEntry>,
        function_name: String,
        mut function_params: Vec<WitValue>,
    ) -> anyhow::Result<Resource<FutureInvokeResult>> {
        let mut env = self.get_environment().await?;
        WorkerConfig::remove_dynamic_vars(&mut env);

        let wasi_config_vars = self.wasi_config_vars();
        let own_worker_id = self.owned_worker_id().clone();

        let begin_index = self
            .begin_function(&DurableFunctionType::WriteRemote)
            .await?;

        let entry = self.table().get(&this)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id().clone();

        let connection_span_id = payload.span_id().clone();

        Self::add_self_parameter_if_needed(&mut function_params, payload);

        if remote_worker_id == own_worker_id {
            return Err(anyhow!("RPC calls to the same agent are not supported"));
        }

        let current_idempotency_key = self
            .get_current_idempotency_key()
            .await
            .unwrap_or(IdempotencyKey::fresh());
        let oplog_index = self.state.oplog.current_oplog_index().await;

        let idempotency_key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);

        let span =
            create_invocation_span(self, &connection_span_id, &function_name, &idempotency_key)
                .await?;

        let worker_id = self.worker_id().clone();
        let created_by = *self.created_by();
        let request = HostRequestGolemRpcInvoke {
            remote_worker_id: remote_worker_id.worker_id(),
            idempotency_key: idempotency_key.clone(),
            function_name: function_name.clone(),
            function_params: try_get_typed_parameters(
                self.state.component_service.clone(),
                &remote_worker_id.worker_id.component_id,
                &function_name,
                &function_params,
            )
            .await,
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
                            function_name,
                            function_params,
                            &created_by,
                            &worker_id,
                            &env,
                            wasi_config_vars,
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
                    wasi_config_vars,
                    function_name,
                    function_params,
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
        datetime: golem_wasm::wasi::clocks::wall_clock::Datetime,
        full_function_name: String,
        function_input: Vec<golem_wasm::golem_rpc_0_2_x::types::WitValue>,
    ) -> anyhow::Result<()> {
        self.schedule_cancelable_invocation(this, datetime, full_function_name, function_input)
            .await?;

        Ok(())
    }

    async fn schedule_cancelable_invocation(
        &mut self,
        this: Resource<WasmRpcEntry>,
        datetime: golem_wasm::wasi::clocks::wall_clock::Datetime,
        function_name: String,
        mut function_params: Vec<golem_wasm::golem_rpc_0_2_x::types::WitValue>,
    ) -> anyhow::Result<Resource<CancellationToken>> {
        let durability = Durability::<GolemRpcWasmRpcScheduleInvocation>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let entry = self.table().get(&this)?;
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
            let remote_worker_id = payload.remote_worker_id().clone();

            Self::add_self_parameter_if_needed(&mut function_params, payload);

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
                function_name: function_name.clone(),
                function_params: try_get_typed_parameters(
                    self.state.component_service.clone(),
                    &remote_worker_id.worker_id.component_id,
                    &function_name,
                    &function_params,
                )
                .await,
                datetime: datetime.into(),
                remote_agent_type: None,
                remote_agent_parameters: None,
            };

            let stack = self
                .state
                .invocation_context
                .clone_as_inherited_stack(&self.state.current_span_id);
            let action = ScheduledAction::Invoke {
                account_id: *self.created_by(),
                owned_worker_id: remote_worker_id,
                idempotency_key,
                full_function_name: function_name,
                function_input: function_params.into_iter().map(|e| e.into()).collect(),
                invocation_context: stack,
            };

            let result = self
                .state
                .scheduler_service
                .schedule(datetime.into(), action)
                .await;

            let invocation =
                SerializableScheduledInvocation::from_domain(result).map_err(|err| anyhow!(err))?;

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
            self.finish_span(payload.span_id()).await?;
        }

        Ok(())
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    fn add_self_parameter_if_needed(
        function_params: &mut Vec<WitValue>,
        payload: &WasmRpcEntryPayload,
    ) {
        if let WasmRpcEntryPayload::Resource {
            resource_uri,
            resource_id,
            ..
        } = payload
        {
            function_params.insert(
                0,
                Value::Handle {
                    uri: resource_uri.value.to_string(),
                    resource_id: *resource_id,
                }
                .into(),
            );
        }
    }
}

impl From<RpcError> for golem_wasm::RpcError {
    fn from(value: RpcError) -> Self {
        match value {
            RpcError::ProtocolError { details } => golem_wasm::RpcError::ProtocolError(details),
            RpcError::Denied { details } => golem_wasm::RpcError::Denied(details),
            RpcError::NotFound { details } => golem_wasm::RpcError::NotFound(details),
            RpcError::RemoteInternalError { details } => {
                golem_wasm::RpcError::RemoteInternalError(details)
            }
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum FutureInvokeResultState {
    Pending {
        request: HostRequestGolemRpcInvoke,
        handle: AbortOnDropJoinHandle<Result<Result<Option<ValueAndType>, RpcError>, Error>>,
        span_id: SpanId,
        begin_index: OplogIndex,
    },
    Completed {
        request: HostRequestGolemRpcInvoke,
        result: Result<Result<Option<ValueAndType>, RpcError>, Error>,
        span_id: SpanId,
        begin_index: OplogIndex,
    },
    Deferred {
        remote_worker_id: OwnedWorkerId,
        self_worker_id: WorkerId,
        self_created_by: AccountId,
        env: Vec<(String, String)>,
        wasi_config_vars: BTreeMap<String, String>,
        function_name: String,
        function_params: Vec<WitValue>,
        idempotency_key: IdempotencyKey,
        span_id: SpanId,
        begin_index: OplogIndex,
    },
    Consumed {
        request: HostRequestGolemRpcInvoke,
        begin_index: OplogIndex,
    },
}

impl FutureInvokeResultState {
    pub fn span_id(&self) -> &SpanId {
        match self {
            Self::Pending { span_id, .. } => span_id,
            Self::Completed { span_id, .. } => span_id,
            Self::Deferred { span_id, .. } => span_id,
            Self::Consumed { .. } => panic!("unexpected state: Consumed"),
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

impl<Ctx: WorkerCtx> HostFutureInvokeResult for DurableWorkerCtx<Ctx> {
    async fn subscribe(
        &mut self,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        self.observe_function_call("golem::rpc::future-invoke-result", "subscribe");
        subscribe(self.table(), this, None)
    }

    async fn get(
        &mut self,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Option<Result<WitValue, golem_wasm::RpcError>>> {
        self.observe_function_call("golem::rpc::future-invoke-result", "get");
        let rpc = self.rpc();
        let component_service = self.state.component_service.clone();

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
                Result<Option<Result<Option<ValueAndType>, golem_wasm::RpcError>>, Error>,
                HostRequestGolemRpcInvoke,
                SerializableInvokeResult,
                OplogIndex,
            ) = match entry {
                FutureInvokeResultState::Consumed {
                    request,
                    begin_index,
                } => {
                    let begin_index = *begin_index;
                    let message = "future-invoke-result already consumed";
                    (
                        Err(anyhow!(message)),
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
                    let result = std::mem::replace(
                        entry,
                        FutureInvokeResultState::Consumed {
                            request,
                            begin_index,
                        },
                    );
                    if let FutureInvokeResultState::Completed {
                        request, result, ..
                    } = result
                    {
                        match result {
                            Ok(Ok(result)) => (
                                Ok(Some(Ok(result.clone()))),
                                request,
                                SerializableInvokeResult::Completed(Ok(result)),
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
                            let request = rx.await.map_err(|err| anyhow!(err))?;
                            let FutureInvokeResultState::Deferred {
                                remote_worker_id,
                                self_worker_id,
                                self_created_by,
                                env,
                                wasi_config_vars,
                                function_name,
                                function_params,
                                idempotency_key,
                                ..
                            } = request
                            else {
                                return Err(anyhow!(
                                    "unexpected incoming response state".to_string()
                                ));
                            };
                            Ok(rpc
                                .invoke_and_await(
                                    &remote_worker_id,
                                    Some(idempotency_key),
                                    function_name,
                                    function_params,
                                    &self_created_by,
                                    &self_worker_id,
                                    &env,
                                    wasi_config_vars,
                                    stack,
                                )
                                .await)
                        }
                        .in_current_span(),
                    );
                    let FutureInvokeResultState::Deferred {
                        remote_worker_id,
                        function_name,
                        function_params,
                        idempotency_key,
                        span_id,
                        ..
                    } = &entry
                    else {
                        return Err(anyhow!("unexpected state entry".to_string()));
                    };
                    let request = HostRequestGolemRpcInvoke {
                        remote_worker_id: remote_worker_id.worker_id(),
                        idempotency_key: idempotency_key.clone(),
                        function_name: function_name.clone(),
                        function_params: try_get_typed_parameters(
                            component_service,
                            &remote_worker_id.worker_id.component_id,
                            function_name,
                            function_params,
                        )
                        .await,
                        remote_agent_type: None,
                        remote_agent_parameters: None
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
                    .map_err(|_| anyhow!("failed to send request to handler"))?;
                    (
                        Ok(None),
                        request,
                        SerializableInvokeResult::Pending,
                        begin_index,
                    )
                }
            };

            let for_retry = match &result {
                Err(err) => Err(anyhow!(err.to_string())),
                Ok(Some(Err(err))) => Err(anyhow!(err.to_string())),
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
                    .add_imported_function_invoked(
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
                Ok(Some(Ok(value_and_type))) => {
                    // The wasm-rpc interface encodes unit result types as empty records and other result types as 1-tuples.
                    let wit_value = match value_and_type {
                        Some(value_and_type) => {
                            let wrapped = ValueAndType::new(
                                Value::Tuple(vec![value_and_type.value]),
                                analysed_type::tuple(vec![value_and_type.typ]),
                            );
                            wrapped.into()
                        }
                        None => {
                            ValueAndType::new(Value::Record(vec![]), analysed_type::record(vec![]))
                                .into()
                        }
                    };

                    Ok(Some(Ok(wit_value)))
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
            let (_, oplog_entry) =
                get_oplog_entry!(self.state.replay_state, OplogEntry::ImportedFunctionInvoked)
                    .map_err(|golem_err| {
                        anyhow!(
                    "failed to get golem::rpc::future-invoke-result::get oplog entry: {golem_err}"
                )
                    })?;

            let serialized_invoke_result = match oplog_entry {
                OplogEntry::ImportedFunctionInvoked { response, .. } => {
                    let response = self
                        .state
                        .oplog
                        .download_payload(response)
                        .await
                        .map_err(|err| anyhow!("Failed to download oplog payload: {err}"))?;

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
                    Ok(tav) => {
                        // The wasm-rpc interface encodes unit result types as empty records and other result types as 1-tuples.
                        let wit_value = match tav {
                            Some(value_and_type) => {
                                let wrapped = ValueAndType::new(
                                    Value::Tuple(vec![value_and_type.value]),
                                    analysed_type::tuple(vec![value_and_type.typ]),
                                );
                                wrapped.into()
                            }
                            None => ValueAndType::new(
                                Value::Record(vec![]),
                                analysed_type::record(vec![]),
                            )
                            .into(),
                        };
                        Ok(Some(Ok(wit_value)))
                    }
                    Err(error) => {
                        let rpc_error: RpcError = error.into();
                        Ok(Some(Err(rpc_error.into())))
                    }
                },
                SerializableInvokeResult::Failed(error) => {
                    Err(RpcError::RemoteInternalError { details: error }.into())
                }
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
        self.observe_function_call("golem::rpc::future-invoke-result", "drop");
        let _ = self.table().delete(this)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> golem_wasm::Host for DurableWorkerCtx<Ctx> {
    async fn parse_uuid(
        &mut self,
        uuid: String,
    ) -> anyhow::Result<Result<golem_wasm::Uuid, String>> {
        Ok(Uuid::parse_str(&uuid)
            .map(|uuid| uuid.into())
            .map_err(|e| e.to_string()))
    }

    async fn uuid_to_string(&mut self, uuid: golem_wasm::Uuid) -> anyhow::Result<String> {
        let uuid: Uuid = uuid.into();
        Ok(uuid.to_string())
    }
}

pub async fn construct_wasm_rpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_worker_id: WorkerId,
    env: &[(String, String)],
    config: BTreeMap<String, String>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let span = create_rpc_connection_span(ctx, &remote_worker_id).await?;

    let stack = ctx
        .state
        .invocation_context
        .clone_as_inherited_stack(span.span_id());

    let remote_worker_id =
        OwnedWorkerId::new(&ctx.owned_worker_id.environment_id, &remote_worker_id);
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
        payload: Box::new(WasmRpcEntryPayload::Interface {
            demand,
            remote_worker_id,
            span_id: span.span_id().clone(),
        }),
    })?;
    Ok(entry)
}

/// Tries to get a `ValueAndType` representation for the given `WitValue` parameters by querying the latest component metadata for the
/// target component.
/// If the query fails, or the expected function name is not in its metadata or the number of parameters does not match, then it returns an
/// empty vector.
///
/// This should only be used for generating "debug information" for the stored oplog entries.
async fn try_get_typed_parameters(
    components: Arc<dyn ComponentService>,
    component_id: &ComponentId,
    function_name: &str,
    params: &[WitValue],
) -> Vec<ValueAndType> {
    if let Ok(component) = components.get_metadata(component_id, None).await {
        if let Ok(Some(function)) = component.metadata.find_function(function_name) {
            if function.analysed_export.parameters.len() == params.len() {
                return params
                    .iter()
                    .zip(function.analysed_export.parameters)
                    .map(|(value, def)| ValueAndType::new(value.clone().into(), def.typ.clone()))
                    .collect();
            }
        }
    }

    Vec::new()
}

pub enum WasmRpcEntryPayload {
    Interface {
        #[allow(dead_code)]
        demand: Box<dyn RpcDemand>,
        remote_worker_id: OwnedWorkerId,
        span_id: SpanId,
    },
    Resource {
        #[allow(dead_code)]
        demand: Box<dyn RpcDemand>,
        remote_worker_id: OwnedWorkerId,
        resource_uri: Uri,
        resource_id: u64,
        span_id: SpanId,
    },
}

impl Debug for WasmRpcEntryPayload {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interface {
                remote_worker_id, ..
            } => f
                .debug_struct("Interface")
                .field("remote_worker_id", remote_worker_id)
                .finish(),
            Self::Resource {
                remote_worker_id,
                resource_uri,
                resource_id,
                ..
            } => f
                .debug_struct("Resource")
                .field("remote_worker_id", remote_worker_id)
                .field("resource_uri", resource_uri)
                .field("resource_id", resource_id)
                .finish(),
        }
    }
}

impl WasmRpcEntryPayload {
    pub fn remote_worker_id(&self) -> &OwnedWorkerId {
        match self {
            Self::Interface {
                remote_worker_id, ..
            } => remote_worker_id,
            Self::Resource {
                remote_worker_id, ..
            } => remote_worker_id,
        }
    }

    pub fn span_id(&self) -> &SpanId {
        match self {
            Self::Interface { span_id, .. } => span_id,
            Self::Resource { span_id, .. } => span_id,
        }
    }

    #[allow(clippy::borrowed_box)]
    pub fn demand(&self) -> &Box<dyn RpcDemand> {
        match self {
            Self::Interface { demand, .. } => demand,
            Self::Resource { demand, .. } => demand,
        }
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
