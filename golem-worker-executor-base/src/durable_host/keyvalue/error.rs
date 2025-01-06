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

use async_trait::async_trait;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::keyvalue::wasi_keyvalue_error::{Error, Host, HostError};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> HostError for DurableWorkerCtx<Ctx> {
    async fn trace(&mut self, self_: Resource<Error>) -> anyhow::Result<String> {
        record_host_function_call("keyvalue::wasi_cloud_error", "trace");
        let trace = self
            .as_wasi_view()
            .table()
            .get::<ErrorEntry>(&self_)?
            .trace
            .clone();
        Ok(trace)
    }

    async fn drop(&mut self, rep: Resource<Error>) -> anyhow::Result<()> {
        record_host_function_call("keyvalue::wasi_cloud_error", "drop_error");
        self.as_wasi_view().table().delete::<ErrorEntry>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

pub struct ErrorEntry {
    trace: String,
}

impl ErrorEntry {
    pub fn new(trace: String) -> Self {
        Self { trace }
    }
}
