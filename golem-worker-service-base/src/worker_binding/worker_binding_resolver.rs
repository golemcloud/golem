use golem_wasm_rpc::TypeAnnotatedValue;

use crate::worker_binding::GolemWorkerBinding;

// For any input request type, there should be a way to resolve the
// worker binding template, which is then used to form the worker request
pub trait WorkerBindingResolver<ApiDefinition> {
    fn resolve(&self, api_specification: &ApiDefinition) -> Option<ResolvedWorkerBinding>;
}

#[derive(Debug, Clone)]
pub struct ResolvedWorkerBinding {
    pub resolved_worker_binding_template: GolemWorkerBinding,
    pub typed_value_from_input: TypeAnnotatedValue,
}
