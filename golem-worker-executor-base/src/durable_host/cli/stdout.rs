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

use wasmtime::component::Resource;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::bindings::cli::stdout::{Host, OutputStream};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn get_stdout(&mut self) -> anyhow::Result<Resource<OutputStream>> {
        record_host_function_call("cli::stdout", "get_stdout");
        self.as_wasi_view().get_stdout()
    }
}
