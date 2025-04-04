use async_trait::async_trait;
use golem_common::base_model::{ComponentId, TargetWorkerId};
use golem_rib_repl::dependency_manager::{
    ReplDependencies, RibComponentMetadata, RibDependencyManager,
};
use golem_rib_repl::invoke::WorkerFunctionInvoke;
use golem_rib_repl::repl_printer::DefaultReplResultPrinter;
use golem_rib_repl::rib_repl::{ComponentSource, RibRepl};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::ValueAndType;
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName};
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;


pub struct TestRibReplDependencyManager {
    dependencies: EnvBasedTestDependencies,
}

impl TestRibReplDependencyManager {
    pub fn new(dependencies: EnvBasedTestDependencies) -> Self {
        Self { dependencies }
    }
}

#[async_trait]
impl RibDependencyManager for TestRibReplDependencyManager {
    async fn get_dependencies(&self) -> Result<ReplDependencies, String> {
        Err("test will need to run with a single component".to_string())
    }

    async fn add_component(
        &self,
        _source_path: &Path,
        component_name: String,
    ) -> Result<RibComponentMetadata, String> {
        let component_id = self
            .dependencies
            .component(component_name.as_str())
            .store()
            .await;
        let metadata = self
            .dependencies
            .get_latest_component_metadata(&component_id)
            .await;
        Ok(RibComponentMetadata {
            component_id: component_id.0,
            metadata: metadata.exports,
        })
    }
}

// Embedded RibFunctionInvoke implementation
pub struct TestRibReplWorkerFunctionInvoke {
    embedded_worker_executor: EnvBasedTestDependencies,
}

impl TestRibReplWorkerFunctionInvoke {
    pub fn new(embedded_worker_executor: EnvBasedTestDependencies) -> Self {
        Self {
            embedded_worker_executor,
        }
    }
}

#[async_trait]
impl WorkerFunctionInvoke for TestRibReplWorkerFunctionInvoke {
    async fn invoke(
        &self,
        component_id: Uuid,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
    ) -> Result<ValueAndType, String> {
        let target_worker_id = worker_name
            .map(|w| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: Some(w.0),
            })
            .unwrap_or_else(|| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: None,
            });

        let function_name = function_name.0;

        self.embedded_worker_executor
            .invoke_and_await_typed(target_worker_id, function_name.as_str(), args.0)
            .await
            .map_err(|e| format!("Failed to invoke function: {:?}", e))
    }
}