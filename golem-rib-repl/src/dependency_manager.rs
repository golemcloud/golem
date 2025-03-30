use std::path::PathBuf;
use async_trait::async_trait;
use golem_wasm_ast::analysis::AnalysedExport;

#[async_trait]
pub trait RibDependencyManager {
    // Deploy all components within the context, if not already registered with the golem engine
    // and returning the list of components that were registered.
    async fn register_global(&mut self) -> Result<Vec<ComponentDependency>, String>;

    // Deploy a specific component if not done already with the Golem engine
    async fn register_component(
        &mut self,
        component_name: String,
    ) -> Result<ComponentDependency, String>;
}

pub struct ComponentDependency {
    pub component_name: String,
    pub metadata: Vec<AnalysedExport>,
}


// A default Rib dependency manager
pub struct DefaultRibDependencyManager {
    pub component_name: Vec<PathBuf>,
    pub executor: Vec<AnalysedExport>,
}