use golem_rib_repl::rib_repl::{ComponentSource, RibRepl};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use integration_tests::rib_repl::bootstrap::*;
use std::sync::Arc;
#[tokio::main]
async fn main() {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig::new()).await;

    // component name from args
    let component_name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "shopping-cart".to_string());

    let mut rib_repl = RibRepl::bootstrap(
        None,
        Arc::new(TestRibReplDependencyManager::new(deps.clone())),
        Arc::new(TestRibReplWorkerFunctionInvoke::new(deps.clone())),
        None,
        Some(ComponentSource {
            component_name: component_name.to_string(),
            source_path: deps
                .component_directory()
                .join(format!("{}.wasm", component_name)),
        }),
    )
    .await
    .expect("Failed to bootstrap REPL");

    rib_repl.run().await
}
