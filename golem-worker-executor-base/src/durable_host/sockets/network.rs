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

use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::bindings::sockets::network::{Error, ErrorCode, Host, HostNetwork, Network};
use wasmtime_wasi::SocketError;

impl<Ctx: WorkerCtx> HostNetwork for DurableWorkerCtx<Ctx> {
    fn drop(&mut self, rep: Resource<Network>) -> anyhow::Result<()> {
        self.observe_function_call("sockets::network", "drop_network");
        HostNetwork::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn network_error_code(&mut self, err: Resource<Error>) -> anyhow::Result<Option<ErrorCode>> {
        self.observe_function_call("sockets::network", "network_error_code");
        Host::network_error_code(&mut self.as_wasi_view(), err)
    }

    fn convert_error_code(&mut self, err: SocketError) -> anyhow::Result<ErrorCode> {
        self.observe_function_call("sockets::network", "convert_error_code");
        Host::convert_error_code(&mut self.as_wasi_view(), err)
    }
}
