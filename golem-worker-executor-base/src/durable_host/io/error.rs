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

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use wasmtime::component::Resource;
use wasmtime_wasi::bindings::io::error::{Error, Host, HostError};

#[async_trait]
impl<Ctx: WorkerCtx> HostError for DurableWorkerCtx<Ctx> {
    fn to_debug_string(&mut self, self_: Resource<Error>) -> anyhow::Result<String> {
        self.observe_function_call("io::error", "to_debug_string");
        HostError::to_debug_string(&mut self.as_wasi_view(), self_)
    }

    fn drop(&mut self, rep: Resource<Error>) -> anyhow::Result<()> {
        self.observe_function_call("io::error", "drop");
        HostError::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}
