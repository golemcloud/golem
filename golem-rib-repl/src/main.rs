use golem_rib_repl::repl_printer::{DefaultResultPrinter, ReplPrinter};
use golem_rib_repl::rib_repl::{ComponentDetails, RibRepl};
use golem_test_framework::config::TestDependencies;
use std::process::exit;
use std::sync::Arc;

#[cfg(feature = "embedded")]
use golem_rib_repl::embedded::*;

// This is to experiment with the REPL through `cargo run --features embedded`
#[tokio::main]
async fn main() {
    #[cfg(feature = "embedded")]
    get_repl().await.run().await;
}

#[cfg(feature = "embedded")]
async fn get_repl() -> RibRepl {
    let dependencies = BootstrapDependencies::new().await;

    let embedded_worker_executor = start(&dependencies)
        .await
        .expect("Failed to start embedded worker executor");

    let shared_executor = Arc::new(embedded_worker_executor);

    let worker_function_invoke =
        Arc::new(EmbeddedWorkerFunctionInvoke::new(shared_executor.clone()));

    let default_dependency_manager = Arc::new(
        DefaultRibDependencyManager::new(shared_executor.clone())
            .await
            .expect("Failed to create default dependency manager"),
    );

    let printer = DefaultResultPrinter;

    let repl = RibRepl::bootstrap(
        None,
        default_dependency_manager,
        worker_function_invoke,
        Box::new(printer.clone()),
        Some(ComponentDetails {
            component_name: "shopping-cart".to_string(),
            source_path: shared_executor
                .component_directory()
                .join("shopping-cart.wasm"),
        }),
    )
    .await;

    if let Err(err) = &repl {
        printer.print_bootstrap_error(&err);
    }

    match repl {
        Ok(repl) => repl,
        Err(err) => {
            printer.print_bootstrap_error(&err);
            exit(1);
        }
    }
}
