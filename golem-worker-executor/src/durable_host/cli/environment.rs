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

use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::CliGetEnvironment;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostResponseGetEnvironment,
};
use wasmtime_wasi::p2::bindings::cli::environment::Host;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_environment(&mut self) -> anyhow::Result<Vec<(String, String)>> {
        // NOTE: We need this to be persisted because the built-in environment variables may change by forking
        let durability =
            Durability::<CliGetEnvironment>::new(self, DurableFunctionType::ReadLocal).await?;
        let result = if durability.is_live() {
            let env_vars = Host::get_environment(&mut self.as_wasi_view()).await?; // This never fails
            durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponseGetEnvironment { env_vars },
                )
                .await?
        } else {
            durability.replay(self).await?
        };

        Ok(result.env_vars)
    }

    async fn get_arguments(&mut self) -> anyhow::Result<Vec<String>> {
        // NOTE: No need to persist the results of this function as the result values are persisted as part of the initial Create oplog entry
        self.observe_function_call("cli::environment", "get_arguments");
        Host::get_arguments(&mut self.as_wasi_view()).await
    }

    async fn initial_cwd(&mut self) -> anyhow::Result<Option<String>> {
        // NOTE: No need to persist the results of this function as the result values are persisted as part of the initial Create oplog entry
        self.observe_function_call("cli::environment", "initial_cwd");
        Host::initial_cwd(&mut self.as_wasi_view()).await
    }
}
