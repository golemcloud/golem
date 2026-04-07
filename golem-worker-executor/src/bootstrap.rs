// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::services::NoAdditionalDeps;
use crate::services::golem_config::GolemConfig;
use crate::workerctx::default::Context;
use crate::{Bootstrap, RunDetails, bootstrap_and_run_worker_executor};
use async_trait::async_trait;
use golem_service_base::clients::registry::RegistryService;
use prometheus::Registry;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::task::JoinSet;

#[cfg(test)]
test_r::enable!();

pub struct ServerBootstrap;

#[async_trait]
impl Bootstrap<Context> for ServerBootstrap {
    fn create_additional_deps(
        &self,
        _registry_service: Arc<dyn RegistryService>,
    ) -> NoAdditionalDeps {
        NoAdditionalDeps {}
    }
}

pub async fn run(
    golem_config: GolemConfig,
    prometheus_registry: Registry,
    runtime: Handle,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<RunDetails, anyhow::Error> {
    bootstrap_and_run_worker_executor(
        &ServerBootstrap,
        golem_config,
        prometheus_registry,
        runtime,
        join_set,
        true,
    )
    .await
}
