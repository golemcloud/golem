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

pub mod serialized;

use self::serialized::{SerializableScheduleId, SerializableScheduleInvocationRequest};
use crate::durable_host::serialized::SerializableDateTime;
use crate::durable_host::serialized::SerializableError;
use crate::durable_host::wasm_rpc::serialized::{
    SerializableInvokeRequest, SerializableInvokeResult, SerializableInvokeResultV1,
};
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx, OplogEntryVersion};
use crate::error::GolemError;
use crate::get_oplog_entry;
use crate::services::component::ComponentService;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::services::rpc::{RpcDemand, RpcError};
use crate::workerctx::{InvocationContextManagement, InvocationManagement, WorkerCtx};
use anyhow::anyhow;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::exports::function_by_name;
use golem_common::model::invocation_context::{AttributeValue, InvocationContextSpan, SpanId};
use golem_common::model::oplog::{DurableFunctionType, OplogEntry, PersistenceLevel};
use golem_common::model::{
    AccountId, ComponentId, IdempotencyKey, OwnedWorkerId, ScheduledAction, TargetWorkerId,
    WorkerId,
};
use golem_common::serialization::try_deserialize;
use golem_wasm_rpc::golem_rpc_0_2_x::types::{
    CancellationToken, FutureInvokeResult, HostCancellationToken, HostFutureInvokeResult, Pollable,
    Uri,
};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{
    CancellationTokenEntry, FutureInvokeResultEntry, HostWasmRpc, SubscribeAny, Value,
    ValueAndType, WasmRpcEntry, WitType, WitValue,
};
use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use tracing::{error, warn, Instrument};
use uuid::Uuid;
use wasmtime::component::Resource;
use wasmtime_wasi::bindings::cli::environment::Host;
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;
use wasmtime_wasi::subscribe;

impl<Ctx: WorkerCtx> HostWasmRpc for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        worker_id: golem_wasm_rpc::golem_rpc_0_2_x::types::WorkerId,
    ) -> anyhow::Result<Resource<WasmRpcEntry>> {
        self.observe_function_call("golem::rpc::wasm-rpc", "new");

        let worker_id: WorkerId = worker_id.into();
        let remote_worker_id = worker_id.into_target_worker_id();

        construct_wasm_rpc_resource(self, remote_worker_id).await
    }

    async fn ephemeral(
        &mut self,
        component_id: golem_wasm_rpc::golem_rpc_0_2_x::types::ComponentId,
    ) -> anyhow::Result<Resource<WasmRpcEntry>> {
        self.observe_function_call("golem::rpc::wasm-rpc", "ephemeral");

        let component_id: ComponentId = component_id.into();
        let remote_worker_id = TargetWorkerId {
            component_id,
            worker_name: None,
        };

        construct_wasm_rpc_resource(self, remote_worker_id).await
    }

    async fn invoke_and_await(
        &mut self,
        self_: Resource<WasmRpcEntry>,
        function_name: String,
        mut function_params: Vec<WitValue>,
    ) -> anyhow::Result<Result<WitValue, golem_wasm_rpc::RpcError>> {
        let args = self.get_arguments().await?;
        let env = self.get_environment().await?;

        let entry = self.table().get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id().clone();
        let connection_span_id = payload.span_id().clone();

        Self::add_self_parameter_if_needed(&mut function_params, payload);

        let current_idempotency_key = self
            .get_current_idempotency_key()
            .await
            .unwrap_or(IdempotencyKey::fresh());
        let oplog_index = self.state.current_oplog_index().await;

        // NOTE: Now that IdempotencyKey::derived is used, we no longer need to persist this, but we do to avoid breaking existing oplogs
        let durability = Durability::<(u64, u64), SerializableError>::new(
            self,
            "golem::rpc::wasm-rpc",
            "invoke-and-await idempotency key",
            DurableFunctionType::ReadLocal,
        )
        .await?;
        let uuid = if durability.is_live() {
            let key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);
            let uuid = Uuid::parse_str(&key.value.to_string())?; // this is guaranteed to be a uuid
            durability
                .persist_serializable(self, (), Ok(uuid.as_u64_pair()))
                .await?;
            uuid
        } else {
            let (high_bits, low_bits) =
                durability.replay::<(u64, u64), anyhow::Error>(self).await?;
            Uuid::from_u64_pair(high_bits, low_bits)
        };
        let idempotency_key = IdempotencyKey::from_uuid(uuid);

        let span =
            create_invocation_span(self, &connection_span_id, &function_name, &idempotency_key)
                .await?;

        let durability = Durability::<TypeAnnotatedValue, SerializableError>::new(
            self,
            "golem::rpc::wasm-rpc",
            "invoke-and-await result",
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let result: Result<WitValue, RpcError> = if durability.is_live() {
            let input = SerializableInvokeRequest {
                remote_worker_id: remote_worker_id.worker_id(),
                idempotency_key: idempotency_key.clone(),
                function_name: function_name.clone(),
                function_params: try_get_typed_parameters::<Ctx>(
                    self.state.component_service.clone(),
                    &remote_worker_id.account_id,
                    &remote_worker_id.worker_id.component_id,
                    &function_name,
                    &function_params,
                )
                .await,
            };
            let stack = self
                .state
                .invocation_context
                .clone_as_inherited_stack(span.span_id());
            let result = self
                .rpc()
                .invoke_and_await(
                    &remote_worker_id,
                    Some(idempotency_key),
                    function_name,
                    function_params,
                    self.worker_id(),
                    &args,
                    &env,
                    stack,
                )
                .await;
            durability
                .persist_serializable(self, input, result.clone().map_err(|err| (&err).into()))
                .await?;
            result.and_then(|tav| {
                tav.try_into()
                    .map_err(|s: String| RpcError::ProtocolError { details: s })
            })
        } else {
            let (bytes, oplog_entry_version) = durability.replay_raw(self).await?;
            match oplog_entry_version {
                OplogEntryVersion::V1 => {
                    // Legacy oplog entry, used WitValue in its payload
                    let wit_value: Result<WitValue, SerializableError> = try_deserialize(&bytes)
                        .map_err(|err| {
                            GolemError::unexpected_oplog_entry(
                                "ImportedFunctionInvoked payload",
                                err,
                            )
                        })?
                        .expect("Empty payload");
                    wit_value.map_err(|err| err.into())
                }
                OplogEntryVersion::V2 => {
                    // New oplog entry, uses TypeAnnotatedValue in its payload
                    let typed_value: Result<
                        Result<TypeAnnotatedValue, SerializableError>,
                        GolemError,
                    > = try_deserialize(&bytes)
                        .map_err(|err| {
                            GolemError::unexpected_oplog_entry(
                                "ImportedFunctionInvoked payload",
                                err,
                            )
                        })
                        .map(|ok| ok.expect("Empty payload"));

                    match typed_value {
                        Ok(Ok(typed_value)) => typed_value
                            .try_into()
                            .map_err(|s: String| RpcError::ProtocolError { details: s }),
                        Ok(Err(err)) => Err(err.into()),
                        Err(err) => Err(err.into()),
                    }
                }
            }
        };

        self.finish_span(span.span_id()).await?;

        match result {
            Ok(wit_value) => Ok(Ok(wit_value)),
            Err(err) => {
                error!("RPC error: {err}");
                Ok(Err(err.into()))
            }
        }
    }

    async fn invoke(
        &mut self,
        self_: Resource<WasmRpcEntry>,
        function_name: String,
        mut function_params: Vec<WitValue>,
    ) -> anyhow::Result<Result<(), golem_wasm_rpc::RpcError>> {
        let args = self.get_arguments().await?;
        let env = self.get_environment().await?;

        let entry = self.table().get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id().clone();
        let connection_span_id = payload.span_id().clone();

        Self::add_self_parameter_if_needed(&mut function_params, payload);

        let current_idempotency_key = self
            .get_current_idempotency_key()
            .await
            .unwrap_or(IdempotencyKey::fresh());
        let oplog_index = self.state.current_oplog_index().await;

        // NOTE: Now that IdempotencyKey::derived is used, we no longer need to persist this, but we do to avoid breaking existing oplogs
        let durability = Durability::<(u64, u64), SerializableError>::new(
            self,
            "golem::rpc::wasm-rpc",
            "invoke-and-await idempotency key", // NOTE: must keep invoke-and-await in the name for compatibility with Golem 1.0
            DurableFunctionType::ReadLocal,
        )
        .await?;
        let uuid = if durability.is_live() {
            let key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);
            let uuid = Uuid::parse_str(&key.value.to_string())?; // this is guaranteed to be a uuid
            durability
                .persist_serializable(self, (), Ok(uuid.as_u64_pair()))
                .await?;
            uuid
        } else {
            let (high_bits, low_bits) =
                durability.replay::<(u64, u64), anyhow::Error>(self).await?;
            Uuid::from_u64_pair(high_bits, low_bits)
        };

        let idempotency_key = IdempotencyKey::from_uuid(uuid);

        let span =
            create_invocation_span(self, &connection_span_id, &function_name, &idempotency_key)
                .await?;

        let durability = Durability::<(), SerializableError>::new(
            self,
            "golem::rpc::wasm-rpc",
            "invoke",
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let result: Result<(), RpcError> = if durability.is_live() {
            let input = SerializableInvokeRequest {
                remote_worker_id: remote_worker_id.worker_id(),
                idempotency_key: idempotency_key.clone(),
                function_name: function_name.clone(),
                function_params: try_get_typed_parameters::<Ctx>(
                    self.state.component_service.clone(),
                    &remote_worker_id.account_id,
                    &remote_worker_id.worker_id.component_id,
                    &function_name,
                    &function_params,
                )
                .await,
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
                    self.worker_id(),
                    &args,
                    &env,
                    stack,
                )
                .await;
            durability.persist(self, input, result).await
        } else {
            durability.replay(self).await
        };

        self.finish_span(span.span_id()).await?;

        match result {
            Ok(result) => Ok(Ok(result)),
            Err(err) => {
                error!("RPC error for: {err}");
                Ok(Err(err.into()))
            }
        }
    }

    async fn async_invoke_and_await(
        &mut self,
        this: Resource<WasmRpcEntry>,
        function_name: String,
        mut function_params: Vec<WitValue>,
    ) -> anyhow::Result<Resource<FutureInvokeResult>> {
        let args = self.get_arguments().await?;
        let env = self.get_environment().await?;

        let begin_index = self
            .state
            .begin_function(&DurableFunctionType::WriteRemote)
            .await?;

        let entry = self.table().get(&this)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id().clone();
        let connection_span_id = payload.span_id().clone();

        Self::add_self_parameter_if_needed(&mut function_params, payload);

        let current_idempotency_key = self
            .get_current_idempotency_key()
            .await
            .unwrap_or(IdempotencyKey::fresh());
        let oplog_index = self.state.current_oplog_index().await;

        // NOTE: Now that IdempotencyKey::derived is used, we no longer need to persist this, but we do to avoid breaking existing oplogs
        let durability = Durability::<(u64, u64), SerializableError>::new(
            self,
            "golem::rpc::wasm-rpc",
            "invoke-and-await idempotency key", // NOTE: must keep invoke-and-await in the name for compatibility with Golem 1.0
            DurableFunctionType::ReadLocal,
        )
        .await?;
        let uuid = if durability.is_live() {
            let key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);
            let uuid = Uuid::parse_str(&key.value.to_string())?; // this is guaranteed to be a uuid
            durability
                .persist_serializable(self, (), Ok(uuid.as_u64_pair()))
                .await?;
            uuid
        } else {
            let (high_bits, low_bits) =
                durability.replay::<(u64, u64), anyhow::Error>(self).await?;
            Uuid::from_u64_pair(high_bits, low_bits)
        };
        let idempotency_key = IdempotencyKey::from_uuid(uuid);

        let span =
            create_invocation_span(self, &connection_span_id, &function_name, &idempotency_key)
                .await?;

        let worker_id = self.worker_id().clone();
        let request = SerializableInvokeRequest {
            remote_worker_id: remote_worker_id.worker_id(),
            idempotency_key: idempotency_key.clone(),
            function_name: function_name.clone(),
            function_params: try_get_typed_parameters::<Ctx>(
                self.state.component_service.clone(),
                &remote_worker_id.account_id,
                &remote_worker_id.worker_id.component_id,
                &function_name,
                &function_params,
            )
            .await,
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
                            &worker_id,
                            &args,
                            &env,
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
                }),
            })?;
            Ok(fut)
        } else {
            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Deferred {
                    remote_worker_id,
                    self_worker_id: worker_id,
                    args,
                    env,
                    function_name,
                    function_params,
                    idempotency_key,
                    span_id: span.span_id().clone(),
                }),
            })?;
            Ok(fut)
        };

        match &result {
            Ok(future_invoke_result) => {
                // We have to call state.end_function to mark the completion of the remote write operation when we get a response.
                // For that we need to store begin_index and associate it with the response handle.
                let handle = future_invoke_result.rep();
                self.state.open_function_table.insert(handle, begin_index);
            }
            Err(_) => {
                self.state
                    .end_function(&DurableFunctionType::WriteRemote, begin_index)
                    .await?;
            }
        }

        result
    }

    async fn schedule_invocation(
        &mut self,
        this: Resource<WasmRpcEntry>,
        datetime: golem_wasm_rpc::wasi::clocks::wall_clock::Datetime,
        full_function_name: String,
        function_input: Vec<golem_wasm_rpc::golem_rpc_0_2_x::types::WitValue>,
    ) -> anyhow::Result<()> {
        self.schedule_cancelable_invocation(this, datetime, full_function_name, function_input)
            .await?;

        Ok(())
    }

    async fn schedule_cancelable_invocation(
        &mut self,
        this: Resource<WasmRpcEntry>,
        datetime: golem_wasm_rpc::wasi::clocks::wall_clock::Datetime,
        function_name: String,
        mut function_params: Vec<golem_wasm_rpc::golem_rpc_0_2_x::types::WitValue>,
    ) -> anyhow::Result<Resource<CancellationToken>> {
        let durability = Durability::<SerializableScheduleId, GolemError>::new(
            self,
            "golem::rpc::wasm-rpc",
            "schedule_invocation",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let schedule_id = if durability.is_live() {
            let entry = self.table().get(&this)?;
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
            let remote_worker_id = payload.remote_worker_id().clone();

            Self::add_self_parameter_if_needed(&mut function_params, payload);

            let current_idempotency_key = self
                .state
                .get_current_idempotency_key()
                .expect("Expected to get an idempotency key as we are inside an invocation");

            let current_oplog_index = self.state.current_oplog_index().await;

            let idempotency_key =
                IdempotencyKey::derived(&current_idempotency_key, current_oplog_index);

            let serializable_input = SerializableScheduleInvocationRequest {
                remote_worker_id: remote_worker_id.worker_id(),
                idempotency_key: idempotency_key.clone(),
                function_name: function_name.clone(),
                function_params: try_get_typed_parameters::<Ctx>(
                    self.state.component_service.clone(),
                    &remote_worker_id.account_id,
                    &remote_worker_id.worker_id.component_id,
                    &function_name,
                    &function_params,
                )
                .await,
                datetime: <SerializableDateTime as From<DateTime<Utc>>>::from(datetime.into()),
            };

            let stack = self
                .state
                .invocation_context
                .clone_as_inherited_stack(&self.state.current_span_id);
            let action = ScheduledAction::Invoke {
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

            let serializable_schedule_id = SerializableScheduleId::from_domain(&result);

            durability
                .persist_serializable(
                    self,
                    serializable_input,
                    Ok(serializable_schedule_id.clone()),
                )
                .await?;

            serializable_schedule_id
        } else {
            durability
                .replay::<SerializableScheduleId, GolemError>(self)
                .await?
        };

        let cancellation_token = CancellationTokenEntry {
            schedule_id: schedule_id.data,
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

impl From<RpcError> for golem_wasm_rpc::RpcError {
    fn from(value: RpcError) -> Self {
        match value {
            RpcError::ProtocolError { details } => golem_wasm_rpc::RpcError::ProtocolError(details),
            RpcError::Denied { details } => golem_wasm_rpc::RpcError::Denied(details),
            RpcError::NotFound { details } => golem_wasm_rpc::RpcError::NotFound(details),
            RpcError::RemoteInternalError { details } => {
                golem_wasm_rpc::RpcError::RemoteInternalError(details)
            }
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum FutureInvokeResultState {
    Pending {
        request: SerializableInvokeRequest,
        handle: AbortOnDropJoinHandle<Result<Result<TypeAnnotatedValue, RpcError>, anyhow::Error>>,
        span_id: SpanId,
    },
    Completed {
        request: SerializableInvokeRequest,
        result: Result<Result<TypeAnnotatedValue, RpcError>, anyhow::Error>,
        span_id: SpanId,
    },
    Deferred {
        remote_worker_id: OwnedWorkerId,
        self_worker_id: WorkerId,
        args: Vec<String>,
        env: Vec<(String, String)>,
        function_name: String,
        function_params: Vec<WitValue>,
        idempotency_key: IdempotencyKey,
        span_id: SpanId,
    },
    Consumed {
        request: SerializableInvokeRequest,
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
}

#[async_trait]
impl SubscribeAny for FutureInvokeResultState {
    async fn ready(&mut self) {
        if let Self::Pending {
            handle,
            request,
            span_id,
        } = self
        {
            *self = Self::Completed {
                result: handle.await,
                request: request.clone(),
                span_id: span_id.clone(),
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
    ) -> anyhow::Result<Option<Result<WitValue, golem_wasm_rpc::RpcError>>> {
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

        let handle = this.rep();
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

            let (result, serializable_invoke_request, serializable_invoke_result) = match entry {
                FutureInvokeResultState::Consumed { request } => {
                    let message = "future-invoke-result already consumed";
                    (
                        Err(anyhow!(message)),
                        request.clone(),
                        SerializableInvokeResult::Failed(SerializableError::Generic {
                            message: message.to_string(),
                        }),
                    )
                }
                FutureInvokeResultState::Pending { request, .. } => {
                    (Ok(None), request.clone(), SerializableInvokeResult::Pending)
                }
                FutureInvokeResultState::Completed { request, .. } => {
                    let request = request.clone();
                    let result =
                        std::mem::replace(entry, FutureInvokeResultState::Consumed { request });
                    if let FutureInvokeResultState::Completed {
                        request, result, ..
                    } = result
                    {
                        match result {
                            Ok(Ok(result)) => (
                                Ok(Some(Ok(result.clone()))),
                                request,
                                SerializableInvokeResult::Completed(Ok(result)),
                            ),
                            Ok(Err(rpc_error)) => (
                                Ok(Some(Err(rpc_error.clone().into()))),
                                request,
                                SerializableInvokeResult::Completed(Err(rpc_error)),
                            ),
                            Err(err) => {
                                let serializable_err = (&err).into();
                                (
                                    Err(err),
                                    request,
                                    SerializableInvokeResult::Failed(serializable_err),
                                )
                            }
                        }
                    } else {
                        panic!("unexpected state: not FutureInvokeResultState::Completed")
                    }
                }
                FutureInvokeResultState::Deferred { .. } => {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let handle = wasmtime_wasi::runtime::spawn(
                        async move {
                            let request = rx.await.map_err(|err| anyhow!(err))?;
                            let FutureInvokeResultState::Deferred {
                                remote_worker_id,
                                self_worker_id,
                                args,
                                env,
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
                                    &self_worker_id,
                                    &args,
                                    &env,
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
                    let request = SerializableInvokeRequest {
                        remote_worker_id: remote_worker_id.worker_id(),
                        idempotency_key: idempotency_key.clone(),
                        function_name: function_name.clone(),
                        function_params: try_get_typed_parameters::<Ctx>(
                            component_service,
                            &remote_worker_id.account_id,
                            &remote_worker_id.worker_id.component_id,
                            function_name,
                            function_params,
                        )
                        .await,
                    };
                    tx.send(std::mem::replace(
                        entry,
                        FutureInvokeResultState::Pending {
                            handle,
                            request: request.clone(),
                            span_id: span_id.clone(),
                        },
                    ))
                    .map_err(|_| anyhow!("failed to send request to handler"))?;
                    (Ok(None), request, SerializableInvokeResult::Pending)
                }
            };

            if self.state.snapshotting_mode.is_none() {
                self.state
                    .oplog
                    .add_imported_function_invoked(
                        "golem::rpc::future-invoke-result::get".to_string(),
                        &serializable_invoke_request,
                        &serializable_invoke_result,
                        DurableFunctionType::WriteRemote,
                    )
                    .await
                    .unwrap_or_else(|err| panic!("failed to serialize RPC response: {err}"));

                if !matches!(
                    serializable_invoke_result,
                    SerializableInvokeResult::Pending
                ) {
                    match self.state.open_function_table.get(&handle) {
                        Some(begin_index) => {
                            self.state
                                .end_function(&DurableFunctionType::WriteRemote, *begin_index)
                                .await?;
                            self.state.open_function_table.remove(&handle);
                        }
                        None => {
                            warn!("No matching BeginRemoteWrite index was found when RPC response arrived. Handle: {}; open functions: {:?}", handle, self.state.open_function_table);
                        }
                    }

                    self.finish_span(&span_id).await?;
                }

                self.state.oplog.commit(CommitLevel::DurableOnly).await;
            }

            match result {
                Ok(Some(Ok(tav))) => {
                    let wit_value = tav.try_into().map_err(|s: String| anyhow!(s))?;
                    Ok(Some(Ok(wit_value)))
                }
                Ok(Some(Err(error))) => Ok(Some(Err(error))),
                Ok(None) => Ok(None),
                Err(err) => Err(err),
            }
        } else if self.state.persistence_level == PersistenceLevel::PersistNothing {
            Err(
                GolemError::runtime("Trying to replay an RPC call in a PersistNothing block")
                    .into(),
            )
        } else {
            let (_, oplog_entry) =
                get_oplog_entry!(self.state.replay_state, OplogEntry::ImportedFunctionInvoked)
                    .map_err(|golem_err| {
                        anyhow!(
                    "failed to get golem::rpc::future-invoke-result::get oplog entry: {golem_err}"
                )
                    })?;

            let serialized_invoke_result: Result<SerializableInvokeResult, String> = self
                .state
                .oplog
                .get_payload_of_entry::<SerializableInvokeResult>(&oplog_entry)
                .await
                .map(|v| v.unwrap());

            if let Ok(serialized_invoke_result) = serialized_invoke_result {
                if !matches!(serialized_invoke_result, SerializableInvokeResult::Pending) {
                    match self.state.open_function_table.get(&handle) {
                        Some(begin_index) => {
                            self.state
                                .end_function(&DurableFunctionType::WriteRemote, *begin_index)
                                .await?;
                            self.state.open_function_table.remove(&handle);
                        }
                        None => {
                            warn!("No matching BeginRemoteWrite index was found when invoke response arrived. Handle: {}; open functions: {:?}", handle, self.state.open_function_table);
                        }
                    }

                    self.finish_span(&span_id).await?;
                }

                match serialized_invoke_result {
                    SerializableInvokeResult::Pending => Ok(None),
                    SerializableInvokeResult::Completed(result) => match result {
                        Ok(tav) => {
                            let wit_value = tav.try_into().map_err(|s: String| anyhow!(s))?;
                            Ok(Some(Ok(wit_value)))
                        }
                        Err(error) => Ok(Some(Err(error.into()))),
                    },
                    SerializableInvokeResult::Failed(error) => Err(error.into()),
                }
            } else {
                let serialized_invoke_result = self
                    .state
                    .oplog
                    .get_payload_of_entry::<SerializableInvokeResultV1>(&oplog_entry)
                    .await
                    .unwrap_or_else(|err| {
                        panic!(
                            "failed to deserialize function response: {:?}: {err}",
                            oplog_entry
                        )
                    })
                    .unwrap();

                if !matches!(
                    serialized_invoke_result,
                    SerializableInvokeResultV1::Pending
                ) {
                    match self.state.open_function_table.get(&handle) {
                        Some(begin_index) => {
                            self.state
                                .end_function(&DurableFunctionType::WriteRemote, *begin_index)
                                .await?;
                            self.state.open_function_table.remove(&handle);
                        }
                        None => {
                            warn!("No matching BeginRemoteWrite index was found when invoke response arrived. Handle: {}; open functions: {:?}", handle, self.state.open_function_table);
                        }
                    }
                }

                match serialized_invoke_result {
                    SerializableInvokeResultV1::Pending => Ok(None),
                    SerializableInvokeResultV1::Completed(result) => match result {
                        Ok(wit_value) => Ok(Some(Ok(wit_value))),
                        Err(error) => Ok(Some(Err(error.into()))),
                    },
                    SerializableInvokeResultV1::Failed(error) => Err(error.into()),
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
        let schedule_id = SerializableScheduleId {
            data: entry.schedule_id.clone(),
        };

        let durability = Durability::<(), GolemError>::new(
            self,
            "golem::rpc::cancellation-token",
            "cancel",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        if durability.is_live() {
            self.scheduler_service()
                .cancel(schedule_id.as_domain().map_err(|e| anyhow!(e))?)
                .await;

            durability
                .persist_serializable(self, schedule_id, Ok(()))
                .await?;
        } else {
            durability.replay::<(), GolemError>(self).await?;
        };

        Ok(())
    }

    async fn drop(&mut self, this: Resource<CancellationToken>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::future-invoke-result", "drop");
        let _ = self.table().delete(this)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> golem_wasm_rpc::Host for DurableWorkerCtx<Ctx> {
    async fn parse_uuid(
        &mut self,
        uuid: String,
    ) -> anyhow::Result<Result<golem_wasm_rpc::Uuid, String>> {
        Ok(Uuid::parse_str(&uuid)
            .map(|uuid| uuid.into())
            .map_err(|e| e.to_string()))
    }

    async fn uuid_to_string(&mut self, uuid: golem_wasm_rpc::Uuid) -> anyhow::Result<String> {
        let uuid: uuid::Uuid = uuid.into();
        Ok(uuid.to_string())
    }

    // NOTE: these extract functions are only added as a workaround for the fact that the binding
    // generator does not include types that are not used in any exported _functions_
    async fn extract_value(
        &mut self,
        vnt: golem_wasm_rpc::golem_rpc_0_2_x::types::ValueAndType,
    ) -> anyhow::Result<WitValue> {
        Ok(vnt.value)
    }

    async fn extract_type(
        &mut self,
        vnt: golem_wasm_rpc::golem_rpc_0_2_x::types::ValueAndType,
    ) -> anyhow::Result<WitType> {
        Ok(vnt.typ)
    }
}

async fn construct_wasm_rpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_worker_id: TargetWorkerId,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let remote_worker_id = ctx
        .generate_unique_local_worker_id(remote_worker_id)
        .await?;

    let span = create_rpc_connection_span(ctx, &remote_worker_id).await?;

    let remote_worker_id = OwnedWorkerId::new(&ctx.owned_worker_id.account_id, &remote_worker_id);
    let demand = ctx.rpc().create_demand(&remote_worker_id).await;
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
async fn try_get_typed_parameters<Ctx: WorkerCtx>(
    components: Arc<dyn ComponentService<Ctx::Types>>,
    account_id: &AccountId,
    component_id: &ComponentId,
    function_name: &str,
    params: &[WitValue],
) -> Vec<ValueAndType> {
    if let Ok(metadata) = components
        .get_metadata(account_id, component_id, None)
        .await
    {
        if let Ok(Some(function)) = function_by_name(&metadata.exports, function_name) {
            if function.parameters.len() == params.len() {
                return params
                    .iter()
                    .zip(function.parameters)
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
        .start_span(&[
            (
                "name".to_string(),
                AttributeValue::String("rpc-connection".to_string()),
            ),
            (
                "target_worker_id".to_string(),
                AttributeValue::String(target_worker_id.to_string()),
            ),
        ])
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
