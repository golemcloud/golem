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
use crate::model::WorkerConfig;
use crate::services::HasWorker;
use crate::worker::merge_worker_env_with_component_env;
use crate::workerctx::WorkerCtx;
use golem_common::model::WorkerId;
use wasmtime_wasi::p2::bindings::cli::environment::Host;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_environment(&mut self) -> anyhow::Result<Vec<(String, String)>> {
        let component_env = self.state.component_metadata.env.clone();

        let worker_metadata = self.public_state.worker().get_initial_worker_metadata();
        let mut env = merge_worker_env_with_component_env(Some(worker_metadata.env), component_env);

        let current_worker_name = if let Some(agent_id) = self.agent_id() {
            let updated_agent_id = agent_id.with_phantom_id(self.state.current_phantom_id);
            updated_agent_id.to_string()
        } else {
            self.owned_worker_id.worker_name()
        };

        WorkerConfig::enrich_env(
            &mut env,
            &WorkerId {
                component_id: self.owned_worker_id.component_id(),
                worker_name: current_worker_name,
            },
            &self.state.agent_id.as_ref().map(|id| id.agent_type.clone()),
            self.state.component_metadata.revision,
        );

        Ok(env)
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
