use async_trait::async_trait;
use golem_wasm_ast::analysis::AnalysedExport;
use std::fmt::Debug;
use std::path::Path;
use uuid::Uuid;

/// Dependency manager for the Rib REPL environment.
#[async_trait]
pub trait RibDependencyManager {
    /// Deploys all components within the current context if they are not already registered
    /// with the Golem engine. Returns a list of successfully registered components.
    ///
    /// Note: It is the responsibility of the REPL client to resolve component paths.
    /// In the future, Rib may support multiple components.
    async fn get_dependencies(&self) -> Result<ReplDependencies, String>;

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
    async fn add_component(
        &self,
        source_path: &Path,
        component_name: String,
    ) -> Result<ComponentMetadata, String>;
}

pub struct ReplDependencies {
    pub component_dependencies: Vec<ComponentMetadata>,
}

#[derive(Clone, Debug)]
pub struct ComponentMetadata {
    pub component_id: Uuid,
    pub metadata: Vec<AnalysedExport>,
}
