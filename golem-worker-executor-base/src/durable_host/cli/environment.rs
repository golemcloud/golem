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
use wasmtime_wasi::bindings::cli::environment::Host;

// NOTE: No need to persist the results of these functions as the result values are persisted as part of the initial Create oplog entry
#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_environment(&mut self) -> anyhow::Result<Vec<(String, String)>> {
        self.observe_function_call("golem_environment", "get_environment");
        Host::get_environment(&mut self.as_wasi_view()).await
    }

    async fn get_arguments(&mut self) -> anyhow::Result<Vec<String>> {
        self.observe_function_call("golem_environment", "get_arguments");
        Host::get_arguments(&mut self.as_wasi_view()).await
    }

    async fn initial_cwd(&mut self) -> anyhow::Result<Option<String>> {
        self.observe_function_call("golem_environment", "initial_cwd");
        self.as_wasi_view().initial_cwd().await
    }
}
