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

use anyhow::anyhow;
use async_trait::async_trait;
use wasmtime::component::Resource;
use wasmtime_wasi_http::bindings::http::types;
use wasmtime_wasi_http::bindings::wasi::http::outgoing_handler::Host;
use wasmtime_wasi_http::types::{HostFutureIncomingResponse, HostOutgoingRequest};
use wasmtime_wasi_http::{HttpError, HttpResult};

use golem_common::model::oplog::WrappedFunctionType;

use crate::durable_host::{DurableWorkerCtx, HttpRequestCloseOwner, HttpRequestState};
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn handle(
        &mut self,
        request: Resource<HostOutgoingRequest>,
        options: Option<Resource<types::RequestOptions>>,
    ) -> HttpResult<Resource<HostFutureIncomingResponse>> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(HttpError::trap)?;
        record_host_function_call("http::outgoing_handler", "handle");
        // Durability is handled by the WasiHttpView send_request method and the follow-up calls to await/poll the response future
        let begin_index = self
            .state
            .begin_function(&WrappedFunctionType::WriteRemoteBatched(None))
            .await
            .map_err(|err| HttpError::trap(anyhow!(err)))?;
        let result = Host::handle(&mut self.as_wasi_http_view(), request, options).await;

        match &result {
            Ok(future_incoming_response) => {
                // We have to call state.end_function to mark the completion of the remote write operation when we get a response.
                // For that we need to store begin_index and associate it with the response handle.
                let handle = future_incoming_response.rep();
                self.state.open_function_table.insert(handle, begin_index);
                self.state.open_http_requests.insert(
                    handle,
                    HttpRequestState {
                        close_owner: HttpRequestCloseOwner::FutureIncomingResponseDrop,
                        root_handle: handle,
                    },
                );
            }
            Err(_) => {
                self.state
                    .end_function(&WrappedFunctionType::WriteRemoteBatched(None), begin_index)
                    .await
                    .map_err(|err| HttpError::trap(anyhow!(err)))?;
            }
        }

        result
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {
    async fn handle(
        &mut self,
        request: Resource<HostOutgoingRequest>,
        options: Option<Resource<types::RequestOptions>>,
    ) -> HttpResult<Resource<HostFutureIncomingResponse>> {
        (*self).handle(request, options).await
    }
}
