use crate::embedded_executor::{EmbeddedWorkerExecutor};
use async_trait::async_trait;
use golem_common::model::{ComponentId, ComponentType};
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_ast::analysis::AnalysedExport;
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::Path;
use std::sync::Arc;

/// Dependency manager for the Rib REPL environment.
#[async_trait]
pub trait RibDependencyManager {
    /// Deploys all components within the current context if they are not already registered
    /// with the Golem engine. Returns a list of successfully registered components.
    ///
    /// Note: It is the responsibility of the REPL client to resolve component paths.
    /// In the future, Rib may support multiple components.
    async fn add_components(&self) -> Result<ReplDependencies, String>;

    /// Deploys a specific component if the REPL was started with a reference to it.
    ///
    /// Currently, dynamic component loading is not supported.
    ///
    /// # Arguments
    ///
    /// * `source_path` - The file path to the component.
    /// * `component_name` - The name of the component.
    ///
    /// # Returns
    ///
    /// Returns a `ComponentDependency` on success or an error message as a `String` if the operation fails.
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
    pub embedded_worker_executor: Arc<EmbeddedWorkerExecutor>,
}

impl DefaultRibDependencyManager {
    pub async fn new(embedded_worker_executor: Arc<EmbeddedWorkerExecutor>) -> Result<Self, String> {

        Ok(Self {
            embedded_worker_executor
        })
    }
}

#[async_trait]
impl RibDependencyManager for DefaultRibDependencyManager {
    async fn add_components(&self) -> Result<ReplDependencies, String> {
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
