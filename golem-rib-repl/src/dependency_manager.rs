use crate::embedded_executor::{start, BootstrapDependencies, EmbeddedWorkerExecutor};
use async_trait::async_trait;
use golem_common::model::{ComponentId, ComponentType, TargetWorkerId};
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_ast::analysis::AnalysedExport;
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::Path;

#[async_trait]
pub trait RibDependencyManager {
    // Deploy all components within the context, if not already registered with the golem engine
    // and returning the list of components that were registered.
    // This is upto the client of REPL to decide on resolving the paths
    // Rib (in future will) work with multiple components
    async fn add_components(&self) -> Result<ReplDependencies, String>;

    // How to deploy a specific component, and this will come into action
    // if we started the REPL pointing to a specific component, or dynamically
    // load component during the usage of REPL using `:load` command.
    async fn add_component_dependency(
        &self,
        source_path: &Path,
        component_name: String,
    ) -> Result<ComponentDependency, String>;
}

pub struct ReplDependencies {
    pub component_dependencies: Vec<ComponentDependency>,
}

#[derive(Clone, Debug)]
pub struct ComponentDependency {
    pub component_id: ComponentId,
    pub metadata: Vec<AnalysedExport>,
}

// A default Rib dependency manager is mainly allowing rib to be used standalone
// without the nuances of app manifest. This is mainly used for testing the REPL itself
pub struct DefaultRibDependencyManager {
    embedded_worker_executor: EmbeddedWorkerExecutor,
}

impl DefaultRibDependencyManager {
    pub async fn init() -> Result<Self, String> {
        let dependencies = BootstrapDependencies::new().await;

        let embedded_worker_executor = start(&dependencies)
            .await
            .expect("Failed to start embedded worker executor");

        Ok(Self {
            embedded_worker_executor,
        })
    }
}

#[async_trait]
impl RibDependencyManager for DefaultRibDependencyManager {
    async fn add_components(&self) -> Result<Vec<ComponentDependency>, String> {
        Err("multiple components not supported in embedded mode".to_string())
    }

    async fn add_component_dependency(
        &self,
        source_path: &Path,
        component_name: String,
    ) -> Result<ComponentDependency, String> {
        let component_id = self
            .embedded_worker_executor
            .component(component_name.as_str())
            .store()
            .await;

        let source_path = source_path.join(format!("{component_name}.wasm"));

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
