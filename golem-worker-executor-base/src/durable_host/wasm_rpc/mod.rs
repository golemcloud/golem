// Copyright 2024 Golem Cloud
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
use crate::durable_host::wasm_rpc::serialized::SerializableInvokeResult;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::error::GolemError;
use crate::get_oplog_entry;
use crate::metrics::wasm::record_host_function_call;
use crate::model::PersistenceLevel;
use crate::services::oplog::OplogOps;
use crate::services::rpc::{RpcDemand, RpcError};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::oplog::{OplogEntry, WrappedFunctionType};
use golem_common::model::{IdempotencyKey, OwnedWorkerId, WorkerId};
use golem_common::uri::oss::urn::{WorkerFunctionUrn, WorkerOrFunctionUrn};
use golem_wasm_rpc::golem::rpc::types::{
    FutureInvokeResult, HostFutureInvokeResult, Pollable, Uri,
};
use golem_wasm_rpc::{FutureInvokeResultEntry, HostWasmRpc, SubscribeAny, WasmRpcEntry, WitValue};
use std::any::Any;
use std::str::FromStr;
use tracing::{error, warn};
use uuid::Uuid;
use wasmtime::component::Resource;
use wasmtime_wasi::bindings::cli::environment::Host;
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;
use wasmtime_wasi::subscribe;

#[async_trait]
impl<Ctx: WorkerCtx> HostWasmRpc for DurableWorkerCtx<Ctx> {
    async fn new(&mut self, location: Uri) -> anyhow::Result<Resource<WasmRpcEntry>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("golem::rpc::wasm-rpc", "new");

        match location.parse_as_golem_urn() {
            Some((remote_worker_id, None)) => {
                let remote_worker_id =
                    OwnedWorkerId::new(&self.owned_worker_id.account_id, &remote_worker_id);
                let demand = self.rpc().create_demand(&remote_worker_id).await;
                let entry = self.table().push(WasmRpcEntry {
                    payload: Box::new(WasmRpcEntryPayload {
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
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<Result<WitValue, golem_wasm_rpc::RpcError>> {
        record_host_function_call("golem::rpc::wasm-rpc", "invoke-and-await");
        let args = self.get_arguments().await?;
        let env = self.get_environment().await?;

        let _permit = self.begin_async_host_function().await?;

        let entry = self.table().get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id.clone();

        let uuid = Durability::<Ctx, (u64, u64), SerializableError>::custom_wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem::rpc::wasm-rpc::invoke-and-await idempotency key",
            |_ctx| {
                Box::pin(async move {
                    let uuid = Uuid::new_v4();
                    Ok::<Uuid, GolemError>(uuid)
                })
            },
            |_ctx, uuid: &Uuid| Ok(uuid.as_u64_pair()),
            |_ctx, (high_bits, low_bits)| {
                Box::pin(async move { Ok(Uuid::from_u64_pair(high_bits, low_bits)) })
            },
        )
        .await?;
        let idempotency_key = IdempotencyKey::from_uuid(uuid);

        let result = Durability::<Ctx, WitValue, SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem::rpc::wasm-rpc::invoke-and-await",
            |ctx| {
                Box::pin(async move {
                    ctx.rpc()
                        .invoke_and_await(
                            &remote_worker_id,
                            Some(idempotency_key),
                            function_name,
                            function_params,
                            ctx.worker_id(),
                            &args,
                            &env,
                        )
                        .await
                })
            },
        )
        .await;

        match result {
            Ok(result) => Ok(Ok(result)),
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
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<Result<(), golem_wasm_rpc::RpcError>> {
        record_host_function_call("golem::rpc::wasm-rpc", "invoke");
        let args = self.get_arguments().await?;
        let env = self.get_environment().await?;

        let _permit = self.begin_async_host_function().await?;

        let entry = self.table().get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id.clone();

        let uuid = Durability::<Ctx, (u64, u64), SerializableError>::custom_wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem::rpc::wasm-rpc::invoke-and-await idempotency key",
            |_ctx| {
                Box::pin(async move {
                    let uuid = Uuid::new_v4();
                    Ok::<Uuid, GolemError>(uuid)
                })
            },
            |_ctx, uuid: &Uuid| Ok(uuid.as_u64_pair()),
            |_ctx, (high_bits, low_bits)| {
                Box::pin(async move { Ok(Uuid::from_u64_pair(high_bits, low_bits)) })
            },
        )
        .await?;
        let idempotency_key = IdempotencyKey::from_uuid(uuid);

        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem::rpc::wasm-rpc::invoke",
            |ctx| {
                Box::pin(async move {
                    ctx.rpc()
                        .invoke(
                            &remote_worker_id,
                            Some(idempotency_key),
                            function_name,
                            function_params,
                            ctx.worker_id(),
                            &args,
                            &env,
                        )
                        .await
                })
            },
        )
        .await;

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
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<Resource<FutureInvokeResult>> {
        record_host_function_call("golem::rpc::wasm-rpc", "async-invoke-and-await");
        let args = self.get_arguments().await?;
        let env = self.get_environment().await?;

        let _permit = self.begin_async_host_function().await?;
        let begin_index = self
            .state
            .begin_function(&WrappedFunctionType::WriteRemote)
            .await?;

        let entry = self.table().get(&this)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id.clone();

        let uuid = Durability::<Ctx, (u64, u64), SerializableError>::custom_wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem::rpc::wasm-rpc::invoke-and-await idempotency key",
            |_ctx| {
                Box::pin(async move {
                    let uuid = Uuid::new_v4();
                    Ok::<Uuid, GolemError>(uuid)
                })
            },
            |_ctx, uuid: &Uuid| Ok(uuid.as_u64_pair()),
            |_ctx, (high_bits, low_bits)| {
                Box::pin(async move { Ok(Uuid::from_u64_pair(high_bits, low_bits)) })
            },
        )
        .await?;
        let idempotency_key = IdempotencyKey::from_uuid(uuid);
        let worker_id = self.worker_id().clone();
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
                payload: Box::new(FutureInvokeResultState::Pending { handle }),
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
                    .end_function(&WrappedFunctionType::WriteRemote, begin_index)
                    .await?;
            }
        }

        result
    }

    fn drop(&mut self, rep: Resource<WasmRpcEntry>) -> anyhow::Result<()> {
        record_host_function_call("golem::rpc::wasm-rpc", "drop");

        let _ = self.table().delete(rep)?;
        Ok(())
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
        handle: AbortOnDropJoinHandle<Result<Result<WitValue, RpcError>, anyhow::Error>>,
    },
    Completed {
        result: Result<Result<WitValue, RpcError>, anyhow::Error>,
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
    Consumed,
}

#[async_trait]
impl SubscribeAny for FutureInvokeResultState {
    async fn ready(&mut self) {
        if let Self::Pending { handle } = self {
            *self = Self::Completed {
                result: handle.await,
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
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("golem::rpc::future-invoke-result", "subscribe");
        subscribe(self.table(), this, None)
    }

    async fn get(
        &mut self,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Option<Result<WitValue, golem_wasm_rpc::RpcError>>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("golem::rpc::future-invoke-result", "get");
        let rpc = self.rpc();

        let handle = this.rep();
        if self.state.is_live() || self.state.persistence_level == PersistenceLevel::PersistNothing
        {
            let entry = self.table().get_mut(&this)?;
            let entry = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();

            let (result, serializable_invoke_result) = match entry {
                FutureInvokeResultState::Consumed => {
                    let message = "future-invoke-result already consumed";
                    (
                        Err(anyhow!(message)),
                        SerializableInvokeResult::Failed(SerializableError::Generic {
                            message: message.to_string(),
                        }),
                    )
                }
                FutureInvokeResultState::Pending { .. } => {
                    (Ok(None), SerializableInvokeResult::Pending)
                }
                FutureInvokeResultState::Completed { .. } => {
                    let result = std::mem::replace(entry, FutureInvokeResultState::Consumed);
                    if let FutureInvokeResultState::Completed { result } = result {
                        match result {
                            Ok(Ok(result)) => (
                                Ok(Some(Ok(result.clone()))),
                                SerializableInvokeResult::Completed(Ok(result)),
                            ),
                            Ok(Err(rpc_error)) => (
                                Ok(Some(Err(rpc_error.clone().into()))),
                                SerializableInvokeResult::Completed(Err(rpc_error)),
                            ),
                            Err(err) => {
                                let serializable_err = (&err).into();
                                (Err(err), SerializableInvokeResult::Failed(serializable_err))
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
                    tx.send(std::mem::replace(
                        entry,
                        FutureInvokeResultState::Pending { handle },
                    ))
                    .map_err(|_| anyhow!("failed to send request to handler"))?;
                    (Ok(None), SerializableInvokeResult::Pending)
                }
            };

            if self.state.persistence_level != PersistenceLevel::PersistNothing {
                self.state
                    .oplog
                    .add_imported_function_invoked(
                        "golem::rpc::future-invoke-result::get".to_string(),
                        &serializable_invoke_result,
                        WrappedFunctionType::WriteRemote,
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
                                .end_function(&WrappedFunctionType::WriteRemote, *begin_index)
                                .await?;
                            self.state.open_function_table.remove(&handle);
                        }
                        None => {
                            warn!("No matching BeginRemoteWrite index was found when RPC response arrived. Handle: {}; open functions: {:?}", handle, self.state.open_function_table);
                        }
                    }
                }
                self.state.oplog.commit().await;
            }

            result
        } else {
            let (_, oplog_entry) =
                get_oplog_entry!(self.state.replay_state, OplogEntry::ImportedFunctionInvoked)
                    .map_err(|golem_err| {
                        anyhow!(
                    "failed to get golem::rpc::future-invoke-result::get oplog entry: {golem_err}"
                )
                    })?;

            let serialized_invoke_result = self
                .state
                .oplog
                .get_payload_of_entry::<SerializableInvokeResult>(&oplog_entry)
                .await
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to deserialize function response: {:?}: {err}",
                        oplog_entry
                    )
                })
                .unwrap();

            if !matches!(serialized_invoke_result, SerializableInvokeResult::Pending) {
                match self.state.open_function_table.get(&handle) {
                    Some(begin_index) => {
                        self.state
                            .end_function(&WrappedFunctionType::WriteRemote, *begin_index)
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
                SerializableInvokeResult::Completed(result) => {
                    Ok(Some(result.map_err(|err| err.into())))
                }
                SerializableInvokeResult::Failed(error) => Err(error.into()),
            }
        }
    }

    fn drop(&mut self, this: Resource<FutureInvokeResult>) -> anyhow::Result<()> {
        record_host_function_call("golem::rpc::future-invoke-result", "drop");
        let _ = self.table().delete(this)?;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> golem_wasm_rpc::Host for DurableWorkerCtx<Ctx> {}

pub struct WasmRpcEntryPayload {
    #[allow(dead_code)]
    demand: Box<dyn RpcDemand>,
    remote_worker_id: OwnedWorkerId,
}

pub trait UrnExtensions {
    fn parse_as_golem_urn(&self) -> Option<(WorkerId, Option<String>)>;

    fn golem_urn(worker_id: &WorkerId, function_name: Option<&str>) -> Self;
}

impl UrnExtensions for Uri {
    fn parse_as_golem_urn(&self) -> Option<(WorkerId, Option<String>)> {
        let urn = WorkerOrFunctionUrn::from_str(&self.value).ok()?;

        match urn {
            WorkerOrFunctionUrn::Worker(w) => Some((w.id, None)),
            WorkerOrFunctionUrn::Function(f) => Some((f.id, Some(f.function))),
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
