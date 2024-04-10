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

use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::metrics::wasm::record_host_function_call;
use crate::services::rpc::{RpcDemand, RpcError};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::oplog::WrappedFunctionType;
use golem_common::model::{TemplateId, WorkerId};
use golem_wasm_rpc::golem::rpc::types::Uri;
use golem_wasm_rpc::{HostWasmRpc, WasmRpcEntry, WitValue};
use std::str::FromStr;
use tracing::{debug, error};
use wasmtime::component::Resource;

#[async_trait]
impl<Ctx: WorkerCtx> HostWasmRpc for DurableWorkerCtx<Ctx> {
    async fn new(&mut self, location: Uri) -> anyhow::Result<Resource<WasmRpcEntry>> {
        record_host_function_call("golem::rpc::wasm-rpc", "new");

        match location.parse_as_golem_uri() {
            Some((remote_worker_id, None)) => {
                let demand = self.rpc().create_demand(&remote_worker_id).await;
                let entry = self.table.push(WasmRpcEntry {
                    payload: Box::new(WasmRpcEntryPayload {
                        demand,
                        remote_worker_id,
                    }),
                })?;
                Ok(entry)
            }
            _ => Err(anyhow!(
                "Invalid URI: {}. Must be worker://template-id/worker-name",
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

        let entry = self.table.get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id.clone();

        let result = Durability::<Ctx, WitValue, SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem::rpc::wasm-rpc::invoke-and-await",
            |ctx| {
                Box::pin(async move {
                    ctx.rpc()
                        .invoke_and_await(
                            &remote_worker_id,
                            function_name,
                            function_params,
                            &ctx.state.account_id,
                        )
                        .await
                })
            },
        )
        .await;

        match result {
            Ok(result) => {
                debug!("RPC result for {}: {result:?}", self.worker_id);
                Ok(Ok(result))
            }
            Err(err) => {
                error!("RPC error for {}: {err}", self.worker_id);
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

        let entry = self.table.get(&self_)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let remote_worker_id = payload.remote_worker_id.clone();

        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem::rpc::wasm-rpc::invoke",
            |ctx| {
                Box::pin(async move {
                    ctx.rpc()
                        .invoke(
                            &remote_worker_id,
                            function_name,
                            function_params,
                            &ctx.state.account_id,
                        )
                        .await
                })
            },
        )
        .await;

        match result {
            Ok(result) => Ok(Ok(result)),
            Err(err) => {
                error!("RPC error for {}: {err}", self.worker_id);
                Ok(Err(err.into()))
            }
        }
    }

    fn drop(&mut self, rep: Resource<WasmRpcEntry>) -> anyhow::Result<()> {
        record_host_function_call("golem::rpc::wasm-rpc", "drop");

        let _ = self.table.delete(rep)?;
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

#[async_trait]
impl<Ctx: WorkerCtx> golem_wasm_rpc::Host for DurableWorkerCtx<Ctx> {}

pub struct WasmRpcEntryPayload {
    #[allow(dead_code)]
    demand: Box<dyn RpcDemand>,
    remote_worker_id: WorkerId,
}

pub trait UriExtensions {
    fn parse_as_golem_uri(&self) -> Option<(WorkerId, Option<String>)>;

    fn golem_uri(worker_id: &WorkerId, function_name: Option<&str>) -> Self;
}

impl UriExtensions for Uri {
    fn parse_as_golem_uri(&self) -> Option<(WorkerId, Option<String>)> {
        if self.value.starts_with("worker://") {
            let parts = self.value[9..].split('/').collect::<Vec<_>>();
            match parts.len() {
                2 => {
                    let template_id = TemplateId::from_str(parts[0]).ok()?;
                    let worker_name = parts[1].to_string();
                    Some((
                        WorkerId {
                            template_id,
                            worker_name,
                        },
                        None,
                    ))
                }
                3 => {
                    let template_id = TemplateId::from_str(parts[0]).ok()?;
                    let worker_name = parts[1].to_string();
                    let function_name = parts[2].to_string();
                    Some((
                        WorkerId {
                            template_id,
                            worker_name,
                        },
                        Some(function_name),
                    ))
                }
                _ => None,
            }
        } else {
            None
        }
    }

    fn golem_uri(worker_id: &WorkerId, function_name: Option<&str>) -> Self {
        Self {
            value: match function_name {
                Some(function_name) => format!("{}/{}", worker_id.uri(), function_name),
                None => worker_id.uri(),
            },
        }
    }
}
