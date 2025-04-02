use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_wasm_ast::analysis::AnalysedExport;
use std::fmt::Debug;
use std::path::Path;
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
