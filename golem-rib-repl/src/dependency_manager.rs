use crate::local::{start, EmbeddedWorkerExecutor, LocalRunnerDependencies};
use async_trait::async_trait;
use golem_common::model::{ComponentId, ComponentType, TargetWorkerId};
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::{TestDslUnsafe};
use golem_wasm_ast::analysis::AnalysedExport;
use std::collections::HashMap;
use std::fmt::Debug;

#[async_trait]
pub trait RibDependencyManager {
    // Deploy all components within the context, if not already registered with the golem engine
    // and returning the list of components that were registered.
    async fn register_global(&self) -> Result<Vec<ComponentDependency>, String>;

    // Deploy a specific component if not done already with the Golem engine
    async fn register_component(
        &self,
        component_name: String,
    ) -> Result<ComponentDependency, String>;
}

#[derive(Clone, Debug)]
pub struct ComponentDependency {
    pub component_id: ComponentId,
    pub metadata: Vec<AnalysedExport>,
}

// A default Rib dependency manager is mainly allowing rib to be used standalone
// without the nuances of app manifest. This is mainly used for testing the REPL itself
pub struct DefaultRibDependencyManager<'a> {
    embedded_worker_executor: &'a EmbeddedWorkerExecutor,
}

impl<'a> DefaultRibDependencyManager<'a> {
    pub async fn new(embedded_worker_executor: &'a EmbeddedWorkerExecutor) -> Result<Self, String> {
        Ok(Self {
            embedded_worker_executor,
        })
    }
}

#[async_trait]
impl<'a> RibDependencyManager for DefaultRibDependencyManager<'a> {
    async fn register_global(&self) -> Result<Vec<ComponentDependency>, String> {
        Err("multiple components not supported in local mode".to_string())
    }

    async fn register_component(
        &self,
        component_name: String,
    ) -> Result<ComponentDependency, String> {
        let component_id = self
            .embedded_worker_executor
            .component(component_name.as_str())
            .store()
            .await;

        let source_path = self
            .embedded_worker_executor
            .component_directory()
            .join(format!("{component_name}.wasm"));

        let result = self
            .embedded_worker_executor
            .component_service()
            .get_or_add_component(
                &source_path,
                &component_name,
                ComponentType::Durable,
                &[],
                &HashMap::new(),
                false,
            )
            .await;

        Ok(ComponentDependency {
            component_id,
            metadata: result
                .metadata
                .map(|metadata| {
                    metadata
                        .exports
                        .iter()
                        .map(|m| AnalysedExport::try_from(m.clone()).unwrap())
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}
