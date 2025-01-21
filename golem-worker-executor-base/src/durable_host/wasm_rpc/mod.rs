// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod serialized;

use crate::durable_host::serialized::SerializableError;
use crate::durable_host::wasm_rpc::serialized::{
    SerializableInvokeRequest, SerializableInvokeResult, SerializableInvokeResultV1,
};
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx, OplogEntryVersion};
use crate::error::GolemError;
use crate::get_oplog_entry;
use crate::model::PersistenceLevel;
use crate::services::component::ComponentService;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::services::rpc::{RpcDemand, RpcError};
use crate::workerctx::{InvocationManagement, WorkerCtx};
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::exports::function_by_name;
use golem_common::model::oplog::{DurableFunctionType, OplogEntry};
use golem_common::model::{
    AccountId, ComponentId, IdempotencyKey, OwnedWorkerId, TargetWorkerId, WorkerId,
};
use golem_common::serialization::try_deserialize;
use golem_common::uri::oss::urn::{WorkerFunctionUrn, WorkerOrFunctionUrn};
use golem_wasm_rpc::golem::rpc::types::{
    FutureInvokeResult, HostFutureInvokeResult, Pollable, Uri,
};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{
    FutureInvokeResultEntry, HostWasmRpc, SubscribeAny, Value, ValueAndType, WasmRpcEntry, WitType,
    WitValue,
};
use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;
use tracing::{error, warn};
use uuid::Uuid;
use wasmtime::component::Resource;
use wasmtime_wasi::bindings::cli::environment::Host;
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;
use wasmtime_wasi::subscribe;

#[async_trait]
impl<Ctx: WorkerCtx> HostWasmRpc for DurableWorkerCtx<Ctx> {
    async fn new(&mut self, location: Uri) -> anyhow::Result<Resource<WasmRpcEntry>> {
        self.observe_function_call("golem::rpc::wasm-rpc", "new");

        match location.parse_as_golem_urn() {
            Some((remote_worker_id, None)) => {
                let remote_worker_id = self
                    .generate_unique_local_worker_id(remote_worker_id)
                    .await?;

                let remote_worker_id =
                    OwnedWorkerId::new(&self.owned_worker_id.account_id, &remote_worker_id);
                let demand = self.rpc().create_demand(&remote_worker_id).await;
                let entry = self.table().push(WasmRpcEntry {
                    payload: Box::new(WasmRpcEntryPayload::Interface {
                        demand,
                        remote_worker_id,
                    }),
                })?;
                Ok(entry)
            }
            _ => Err(anyhow!(
                "Invalid URI: {}. Must be urn:worker:component-id/worker-name",
                location.value
            )),
        }
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
                function_params: try_get_typed_parameters(
                    self.state.component_service.clone(),
                    &remote_worker_id.account_id,
                    &remote_worker_id.worker_id.component_id,
                    &function_name,
                    &function_params,
                )
                .await,
            };
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
                function_params: try_get_typed_parameters(
                    self.state.component_service.clone(),
                    &remote_worker_id.account_id,
                    &remote_worker_id.worker_id.component_id,
                    &function_name,
                    &function_params,
                )
                .await,
            };
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
                )
                .await;
            durability.persist(self, input, result).await
        } else {
            durability.replay(self).await
        };

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
        let worker_id = self.worker_id().clone();
        let request = SerializableInvokeRequest {
            remote_worker_id: remote_worker_id.worker_id(),
            idempotency_key: idempotency_key.clone(),
            function_name: function_name.clone(),
            function_params: try_get_typed_parameters(
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

            let handle = wasmtime_wasi::runtime::spawn(async move {
                Ok(rpc
                    .invoke_and_await(
                        &remote_worker_id,
                        Some(idempotency_key),
                        function_name,
                        function_params,
                        &worker_id,
                        &args,
                        &env,
                    )
                    .await)
            });

            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Pending { handle, request }),
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

    async fn drop(&mut self, rep: Resource<WasmRpcEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::wasm-rpc", "drop");

        let _ = self.table().delete(rep)?;
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
    },
    Completed {
        request: SerializableInvokeRequest,
        result: Result<Result<TypeAnnotatedValue, RpcError>, anyhow::Error>,
    },
    Deferred {
        remote_worker_id: OwnedWorkerId,
        self_worker_id: WorkerId,
        args: Vec<String>,
        env: Vec<(String, String)>,
        function_name: String,
        function_params: Vec<WitValue>,
        idempotency_key: IdempotencyKey,
    },
    Consumed {
        request: SerializableInvokeRequest,
    },
}

#[async_trait]
impl SubscribeAny for FutureInvokeResultState {
    async fn ready(&mut self) {
        if let Self::Pending { handle, request } = self {
            *self = Self::Completed {
                result: handle.await,
                request: request.clone(),
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

#[async_trait]
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

        let handle = this.rep();
        if self.state.is_live() || self.state.persistence_level == PersistenceLevel::PersistNothing
        {
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
                    if let FutureInvokeResultState::Completed { request, result } = result {
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
                    let handle = wasmtime_wasi::runtime::spawn(async move {
                        let request = rx.await.map_err(|err| anyhow!(err))?;
                        let FutureInvokeResultState::Deferred {
                            remote_worker_id,
                            self_worker_id,
                            args,
                            env,
                            function_name,
                            function_params,
                            idempotency_key,
                        } = request
                        else {
                            return Err(anyhow!("unexpected incoming response state".to_string()));
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
                            )
                            .await)
                    });
                    let FutureInvokeResultState::Deferred {
                        remote_worker_id,
                        function_name,
                        function_params,
                        idempotency_key,
                        ..
                    } = &entry
                    else {
                        return Err(anyhow!("unexpected state entry".to_string()));
                    };
                    let request = SerializableInvokeRequest {
                        remote_worker_id: remote_worker_id.worker_id(),
                        idempotency_key: idempotency_key.clone(),
                        function_name: function_name.clone(),
                        function_params: try_get_typed_parameters(
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
                        },
                    ))
                    .map_err(|_| anyhow!("failed to send request to handler"))?;
                    (Ok(None), request, SerializableInvokeResult::Pending)
                }
            };

            if self.state.persistence_level != PersistenceLevel::PersistNothing {
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

                if matches!(
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

#[async_trait]
impl<Ctx: WorkerCtx> golem_wasm_rpc::Host for DurableWorkerCtx<Ctx> {
    // NOTE: these extract functions are only added as a workaround for the fact that the binding
    // generator does not include types that are not used in any exported _functions_
    async fn extract_value(
        &mut self,
        vnt: golem_wasm_rpc::golem::rpc::types::ValueAndType,
    ) -> anyhow::Result<WitValue> {
        Ok(vnt.value)
    }

    async fn extract_type(
        &mut self,
        vnt: golem_wasm_rpc::golem::rpc::types::ValueAndType,
    ) -> anyhow::Result<WitType> {
        Ok(vnt.typ)
    }
}

/// Tries to get a `ValueAndType` representation for the given `WitValue` parameters by querying the latest component metadata for the
/// target component.
/// If the query fails, or the expected function name is not in its metadata or the number of parameters does not match, then it returns an
/// empty vector.
///
/// This should only be used for generating "debug information" for the stored oplog entries.
async fn try_get_typed_parameters(
    components: Arc<dyn ComponentService + Send + Sync>,
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
    },
    Resource {
        #[allow(dead_code)]
        demand: Box<dyn RpcDemand>,
        remote_worker_id: OwnedWorkerId,
        resource_uri: Uri,
        resource_id: u64,
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

    #[allow(clippy::borrowed_box)]
    pub fn demand(&self) -> &Box<dyn RpcDemand> {
        match self {
            Self::Interface { demand, .. } => demand,
            Self::Resource { demand, .. } => demand,
        }
    }
}

pub trait UrnExtensions {
    fn parse_as_golem_urn(&self) -> Option<(TargetWorkerId, Option<String>)>;

    fn golem_urn(worker_id: &WorkerId, function_name: Option<&str>) -> Self;
}

impl UrnExtensions for Uri {
    fn parse_as_golem_urn(&self) -> Option<(TargetWorkerId, Option<String>)> {
        let urn = WorkerOrFunctionUrn::from_str(&self.value).ok()?;

        match urn {
            WorkerOrFunctionUrn::Worker(w) => Some((w.id, None)),
            WorkerOrFunctionUrn::Function(f) => {
                Some((f.id.into_target_worker_id(), Some(f.function)))
            }
        }
    }

    fn golem_urn(worker_id: &WorkerId, function_name: Option<&str>) -> Self {
        Self {
            value: match function_name {
                Some(function_name) => WorkerFunctionUrn {
                    id: worker_id.clone(),
                    function: function_name.to_string(),
                }
                .to_string(),
                None => worker_id.uri(),
            },
        }
    }
}
