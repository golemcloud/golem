use crate::dependency_manager::{ComponentDependency, RibDependencyManager};
use crate::embedded_executor::{start, BootstrapDependencies, EmbeddedWorkerExecutor};
use crate::invoke::WorkerFunctionInvoke;
use crate::rib_repl::{ComponentDetails, RibRepl};
use async_trait::async_trait;
use golem_common::model::{ComponentId, TargetWorkerId};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::ValueAndType;
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, RibFunctionInvoke};
use std::env;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use golem_test_framework::config::TestDependencies;
use crate::result_printer::{DefaultResultPrinter, ReplPrinter};

mod compiler;
mod dependency_manager;
mod embedded_executor;
mod history;
mod invoke;
mod repl_state;
mod result_printer;
mod rib_edit;
mod rib_repl;

// This is only available for testing purposes
// and is not a public binary artefact
// and doesn't need a formalised command line arguments here
// simply do `cargo run -- <component_name> <source_path>`
// Local testing of REPL (example, if golem developers need to test a component quickly with golem)
// without a published REPL, they can do as follows
// cargo run
#[tokio::main]
async fn main() {
    let dependencies = BootstrapDependencies::new().await;

    let embedded_worker_executor = start(&dependencies)
        .await
        .expect("Failed to start embedded worker executor");

    let shared_executor = Arc::new(embedded_worker_executor);

    let worker_function_invoke = Arc::new(invoke::DefaultWorkerFunctionInvoke::new(
       shared_executor.clone()
    ));

    let default_dependency_manager =
        Arc::new(dependency_manager::DefaultRibDependencyManager::new(shared_executor.clone())
            .await
            .expect("Failed to create default dependency manager"));


    let printer = DefaultResultPrinter;

    let mut repl = RibRepl::bootstrap(
        None,
        default_dependency_manager,
        worker_function_invoke,
        Box::new(printer.clone()),
        Some(ComponentDetails {
            component_name: "shopping-cart".to_string(),
            source_path: shared_executor.component_directory().join("shopping-cart.wasm"),
        }),
    ).await;

    match &mut repl {
        Ok(repl) => {
           repl.run().await
        }
        Err(err) => {
            printer.print_bootstrap_error(&err);
            return;
        }
    }

}
