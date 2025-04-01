use std::env;
use crate::dependency_manager::{ComponentDependency, RibDependencyManager};
use crate::embedded_executor::{start, EmbeddedWorkerExecutor, BootstrapDependencies};
use crate::rib_repl::RibRepl;
use async_trait::async_trait;
use golem_common::model::{ComponentId, TargetWorkerId};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::ValueAndType;
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, RibFunctionInvoke};
use std::str::FromStr;
use std::sync::Arc;
use crate::invoke::WorkerFunctionInvoke;

mod compiler;
mod dependency_manager;
mod history;
mod embedded_executor;
mod repl_state;
mod result_printer;
mod rib_edit;
mod rib_repl;
mod invoke;

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
        Arc::new(dependency_manager::DefaultRibDependencyManager::init()
            .await
            .expect("Failed to create default dependency manager"));



    let mut repl = RibRepl::new(
        None,
        default_dependency_manager: Arc::new(default_dependency_manager),
        Arc::new(rib_function_invoke),
        None,
        Some("shopping-cart".to_string()),
    );
    repl.run().await;
}
