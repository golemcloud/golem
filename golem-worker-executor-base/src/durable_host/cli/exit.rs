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

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::bindings::cli::exit::Host;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn exit(&mut self, status: Result<(), ()>) -> anyhow::Result<()> {
        self.observe_function_call("cli::exit", "exit");
        Host::exit(&mut self.as_wasi_view(), status)
    }

    fn exit_with_code(&mut self, status_code: u8) -> anyhow::Result<()> {
        self.observe_function_call("cli::exit", "exit_with_code");
        Host::exit_with_code(&mut self.as_wasi_view(), status_code)
    }
}
