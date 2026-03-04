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

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::cli::WasiCliView as _;
use wasmtime_wasi::p2::bindings::cli::exit::Host;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn exit(&mut self, status: Result<(), ()>) -> wasmtime::Result<()> {
        self.observe_function_call("cli::exit", "exit");
        Host::exit(&mut self.as_wasi_view().cli(), status)
    }

    fn exit_with_code(&mut self, status_code: u8) -> wasmtime::Result<()> {
        self.observe_function_call("cli::exit", "exit_with_code");
        Host::exit_with_code(&mut self.as_wasi_view().cli(), status_code)
    }
}
