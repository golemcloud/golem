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

use async_trait::async_trait;
use wasmtime::component::Resource;
use wasmtime_wasi_http::bindings::http::types::ErrorCode;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi_http::bindings::wasi::http::outgoing_handler::{
    FutureIncomingResponse, Host, OutgoingRequest, RequestOptions,
};

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn handle(
        &mut self,
        request: Resource<OutgoingRequest>,
        options: Option<Resource<RequestOptions>>,
    ) -> anyhow::Result<Result<Resource<FutureIncomingResponse>, ErrorCode>> {
        record_host_function_call("http::outgoing_handler", "handle");
        // Durability is handled by the WasiHttpView send_request method and the follow-up calls to await/poll the response future
        Host::handle(&mut self.as_wasi_http_view(), request, options)
    }
}
