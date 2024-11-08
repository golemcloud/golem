use crate::gateway_binding::WorkerBindingCompiled;
use golem_service_base::model::VersionedComponentId;
use rib::Expr;
use crate::gateway_middleware::Middleware;

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerBinding {
    pub component_id: VersionedComponentId,
    pub worker_name: Option<Expr>,
    pub idempotency_key: Option<Expr>,
    pub response: ResponseMapping,
    pub middlewares: Vec<Middleware>
}

// ResponseMapping will consist of actual logic such as invoking worker functions
#[derive(Debug, Clone, PartialEq)]
pub struct ResponseMapping(pub Expr);

impl From<WorkerBindingCompiled> for WorkerBinding {
    fn from(value: WorkerBindingCompiled) -> Self {
        let worker_binding = value.clone();

        WorkerBinding {
            component_id: worker_binding.component_id,
            worker_name: worker_binding
                .worker_name_compiled
                .map(|compiled| compiled.worker_name),
            idempotency_key: worker_binding
                .idempotency_key_compiled
                .map(|compiled| compiled.idempotency_key),
            response: ResponseMapping(worker_binding.response_compiled.response_rib_expr),
            middlewares: value.middleware
        }
    }
}
