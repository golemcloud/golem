use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use golem_service_base::model::VersionedComponentId;
use rib::Expr;
pub(crate) use worker_binding_compiled::*;
pub(crate) use crate::gateway_execution::rib_input_value_resolver::*;
pub(crate) use crate::gateway_execution::worker_binding_resolver::*;
use crate::gateway_plugins::Plugin;

mod worker_binding_compiled;
mod worker_binding;

// A gateway binding is more or less the binding to the backend
// This is similar to gateway-integration in other API gateways.
// A binding can talk to a worker-backend or it can interact with the plugins.
// A plugin depends on the type of requests gateway is supporting.
// Example: A binding can be specific to http middlewares, or plugins.
// A plugin hardly need to interact with the workers. If a particular
// plugin can be implemented through workers, they can rather reuse `WorkerBinding`
// internally.
pub enum GatewayBinding {
    Default(WorkerBinding),
    Plugin(Plugin)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub struct WorkerBinding {
    pub component_id: VersionedComponentId,
    pub worker_name: Option<Expr>,
    pub idempotency_key: Option<Expr>,
    pub response: ResponseMapping,
}

// ResponseMapping will consist of actual logic such as invoking worker functions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
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
        }
    }
}
