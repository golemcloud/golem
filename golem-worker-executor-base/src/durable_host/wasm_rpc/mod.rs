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

use crate::durable_host::DurableWorkerCtx;
use crate::preview2::golem;
use crate::preview2::golem::rpc::types::{Uri, WasmRpc, WitValue};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use wasmtime::component::Resource;

#[async_trait]
impl<Ctx: WorkerCtx> golem::rpc::types::HostWasmRpc for DurableWorkerCtx<Ctx> {
    async fn new(&mut self, location: Uri) -> anyhow::Result<Resource<WasmRpc>> {
        todo!()
    }

    async fn invoke_and_await_json(
        &mut self,
        self_: Resource<WasmRpc>,
        function_name: String,
        function_params: Vec<String>,
    ) -> anyhow::Result<Result<String, ()>> {
        todo!()
    }

    async fn invoke_and_await(
        &mut self,
        self_: Resource<WasmRpc>,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<Result<WitValue, ()>> {
        todo!()
    }

    fn drop(&mut self, rep: Resource<WasmRpc>) -> anyhow::Result<()> {
        todo!()
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> golem::rpc::types::Host for DurableWorkerCtx<Ctx> {}
