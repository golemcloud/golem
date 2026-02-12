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

use golem_rib_repl::{ComponentSource, RibRepl, RibReplConfig};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use integration_tests::rib_repl::bootstrap::*;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = EnvBasedTestDependenciesConfig {
        golem_repo_root: PathBuf::from("."),
        ..Default::default()
    }
    .with_env_overrides();
    let deps = EnvBasedTestDependencies::new(config).await?;

    let component_name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "it_agent_counters_release".to_string());

    let mut rib_repl = RibRepl::bootstrap(RibReplConfig {
        history_file: None,
        dependency_manager: Arc::new(TestRibReplDependencyManager::new(deps.clone()).await?),
        worker_function_invoke: Arc::new(TestRibReplWorkerFunctionInvoke::new(deps.clone())),
        printer: None,
        component_source: Some(ComponentSource {
            component_name: component_name.to_string(),
            source_path: deps
                .component_directory()
                .join(format!("{component_name}.wasm")),
        }),
        prompt: None,
        command_registry: None,
    })
    .await
    .expect("Failed to bootstrap REPL");

    rib_repl.run().await;
    Ok(())
}
