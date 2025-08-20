use golem_rib_repl::{ComponentSource, RibRepl, RibReplConfig};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use integration_tests::rib_repl::bootstrap::*;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig::new()).await;

    let component_name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "shopping-cart".to_string());

    let mut rib_repl = RibRepl::bootstrap(RibReplConfig {
        history_file: None,
        dependency_manager: Arc::new(TestRibReplDependencyManager::new(deps.clone())),
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
}
