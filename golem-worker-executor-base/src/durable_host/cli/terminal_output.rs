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

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::bindings::cli::terminal_output::{Host, HostTerminalOutput, TerminalOutput};

#[async_trait]
impl<Ctx: WorkerCtx> HostTerminalOutput for DurableWorkerCtx<Ctx> {
    fn drop(&mut self, rep: Resource<TerminalOutput>) -> anyhow::Result<()> {
        record_host_function_call("cli::terminal_output::terminal_output", "drop");
        HostTerminalOutput::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

#[async_trait]
impl<Ctx: WorkerCtx> HostTerminalOutput for &mut DurableWorkerCtx<Ctx> {
    fn drop(&mut self, rep: Resource<TerminalOutput>) -> anyhow::Result<()> {
        (*self).drop(rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {}
