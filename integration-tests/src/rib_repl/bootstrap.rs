use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::base_model::{ComponentId, TargetWorkerId};
use golem_rib_repl::WorkerFunctionInvoke;
use golem_rib_repl::{ReplComponentDependencies, RibDependencyManager};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::ValueAndType;
use rib::{ComponentDependency, ComponentDependencyKey};
use std::path::Path;
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
    async fn get_dependencies(&self) -> anyhow::Result<ReplComponentDependencies> {
        Err(anyhow!("test will need to run with a single component"))
    }

    async fn add_component(
        &self,
        _source_path: &Path,
        component_name: String,
    ) -> anyhow::Result<ComponentDependency> {
        let component_id = self
            .dependencies
            .admin()
            .await
            .component(component_name.as_str())
            .store()
            .await;

        let metadata = self
            .dependencies
            .admin()
            .await
            .get_latest_component_metadata(&component_id)
            .await;

        let component_dependency_key = ComponentDependencyKey {
            component_name,
            component_id: component_id.0,
            root_package_name: metadata.root_package_name().clone(),
            root_package_version: metadata.root_package_version().clone(),
        };

        Ok(ComponentDependency::new(
            component_dependency_key,
            metadata.exports().to_vec(),
        ))
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
        _component_name: &str,
        worker_name: Option<String>,
        function_name: &str,
        args: Vec<ValueAndType>,
        _return_type: Option<AnalysedType>,
    ) -> anyhow::Result<Option<ValueAndType>> {
        let target_worker_id = worker_name
            .map(|w| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: Some(w),
            })
            .unwrap_or_else(|| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: None,
            });

        self.embedded_worker_executor
            .admin()
            .await
            .invoke_and_await_typed(target_worker_id, function_name, args)
            .await
            .map_err(|e| anyhow!("Failed to invoke function: {:?}", e))
    }
}
