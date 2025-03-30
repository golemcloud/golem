use crate::local::WorkerExecutorLocalDependencies;
use async_trait::async_trait;
use golem_wasm_ast::analysis::AnalysedExport;
use std::fmt::Debug;

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

// A default Rib dependency manager is mainly allowing rib to be used standalone
// without the nuances of app manifest. This is mainly used for testing the REPL itself
pub struct DefaultRibDependencyManager;

#[async_trait]
impl RibDependencyManager for DefaultRibDependencyManager {
    async fn register_global(&mut self) -> Result<Vec<ComponentDependency>, String> {
        let dependencies = WorkerExecutorLocalDependencies::new().await;
        Err("multiple components not supported in local mode".to_string())
    }

    async fn register_component(
        &mut self,
        component_name: String,
    ) -> Result<ComponentDependency, String> {
        // Implement the logic to register a specific component
        Ok(ComponentDependency {
            component_name,
            metadata: vec![],
        })
    }
}
