use std::str::FromStr;
use crate::dependency_manager::{ComponentDependency, RibDependencyManager};
use crate::local::{start, EmbeddedWorkerExecutor, WorkerExecutorLocalDependencies};
use crate::rib_repl::RibRepl;
use async_trait::async_trait;
use golem_common::model::{ComponentId, TargetWorkerId};
use golem_test_framework::dsl::TestDsl;
use golem_wasm_rpc::ValueAndType;
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, RibFunctionInvoke};
use std::sync::Arc;
use uuid::Uuid;

mod dependency_manager;
mod history;
mod local;
mod result_printer;
mod rib_repl;
mod syntax_highlighter;

#[tokio::main]
async fn main() {
    let dependencies = WorkerExecutorLocalDependencies::new().await;

    let embedded_worker_executor = start(&dependencies)
        .await
        .expect("Failed to start embedded worker executor");

    let default_dependency_manager =
        dependency_manager::DefaultRibDependencyManager::new(&embedded_worker_executor)
            .await
            .expect("Failed to create default dependency manager");

    let component_dependency = default_dependency_manager
        .register_component("shopping-cart".to_string())
        .await
        .expect("Failed to register component");

    let rib_function_invoke =
        EmbeddedRibFunctionInvoke::new(&component_dependency, embedded_worker_executor);

    let mut repl = RibRepl::new(
        None,
        component_dependency,
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
            .map_err(|e| e.to_string())
            .expect("Failed to invoke function")
            .map_err(|e| format!("Failed to invoke function: {:?}", e))
    }
}
