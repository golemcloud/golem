use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use crate::worker_binding::CompiledGolemWorkerBinding;
use golem_service_base::model::VersionedComponentId;
use rib::Expr;
use golem_common::model::WorkerBindingType;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub struct GolemWorkerBinding {
    pub component_id: VersionedComponentId,
    pub worker_name: Option<Expr>,
    pub idempotency_key: Option<Expr>,
    pub response: ResponseMapping,
    pub worker_binding_type: WorkerBindingType,
}

// ResponseMapping will consist of actual logic such as invoking worker functions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ResponseMapping(pub Expr);

impl From<CompiledGolemWorkerBinding> for GolemWorkerBinding {
    fn from(value: CompiledGolemWorkerBinding) -> Self {
        let worker_binding = value.clone();

        GolemWorkerBinding {
            component_id: worker_binding.component_id,
            worker_name: worker_binding
                .worker_name_compiled
                .map(|compiled| compiled.worker_name),
            idempotency_key: worker_binding
                .idempotency_key_compiled
                .map(|compiled| compiled.idempotency_key),
            response: ResponseMapping(worker_binding.response_compiled.response_rib_expr),
            worker_binding_type: worker_binding.worker_binding_type,
        }
    }
}
