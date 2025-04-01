use std::env;
use crate::dependency_manager::{ComponentDependency, RibDependencyManager};
use crate::embedded_executor::{start, EmbeddedWorkerExecutor, BootstrapDependencies};
use crate::rib_repl::RibRepl;
use async_trait::async_trait;
use golem_common::model::TargetWorkerId;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::ValueAndType;
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, RibFunctionInvoke};
use std::str::FromStr;
use std::sync::Arc;

mod compiler;
mod dependency_manager;
mod history;
mod embedded_executor;
mod repl_state;
mod result_printer;
mod rib_edit;
mod rib_repl;
mod bootstrap;

// This is only available for testing purposes
// and is not a public binary artefact
// and doesn't need a formalised command line arguments here
// simply do `cargo run -- <component_name> <source_path>`
// Local testing of REPL (example, if golem developers need to test a component quickly with golem)
// without a published REPL, they can do as follows
// cargo run
#[tokio::main]
async fn main() {

    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: cargo run -- <source_path> <component_name>");
        std::process::exit(1);
    }

    let dependencies = BootstrapDependencies::new().await;

    let embedded_worker_executor = start(&dependencies)
        .await
        .expect("Failed to start embedded worker executor");

    let default_dependency_manager =
        dependency_manager::DefaultRibDependencyManager::init()
            .await
            .expect("Failed to create default dependency manager");

    let component_dependency = default_dependency_manager
        .add_component_dependency(, "shopping-cart".to_string())
        .await
        .expect("Failed to register component");

    let rib_function_invoke =
        EmbeddedRibFunctionInvoke::new(&component_dependency, embedded_worker_executor);

    let mut repl = RibRepl::new(
        None,
        default_dependency_manager,
        Arc::new(rib_function_invoke),
        None,
        Some("shopping-cart".to_string()),
    );
    repl.run().await;
}

struct EmbeddedRibFunctionInvoke {
    dependency: ComponentDependency,
    embedded_worker_executor: EmbeddedWorkerExecutor,
}
impl EmbeddedRibFunctionInvoke {
    pub fn new(
        dependency: &ComponentDependency,
        embedded_worker_executor: EmbeddedWorkerExecutor,
    ) -> Self {
        Self {
            dependency: dependency.clone(),
            embedded_worker_executor,
        }
    }
}

#[async_trait]
impl RibFunctionInvoke for EmbeddedRibFunctionInvoke {
    async fn invoke(
        &self,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
    ) -> Result<ValueAndType, String> {
        let target_worker_id = worker_name
            .map(|w| TargetWorkerId {
                component_id: self.dependency.component_id.clone(),
                worker_name: Some(w.0),
            })
            .unwrap_or(TargetWorkerId {
                component_id: self.dependency.component_id.clone(),
                worker_name: None,
            });

        let function_name = function_name.0;

        self.embedded_worker_executor
            .invoke_and_await_typed(target_worker_id, function_name.as_str(), args.0)
            .await
            .map_err(|e| format!("Failed to invoke function: {:?}", e))
    }
}
