use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use crate::worker_binding::CompiledGolemWorkerBinding;
use golem_service_base::model::VersionedComponentId;
use rib::Expr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub struct GolemWorkerBinding {
    pub component_id: VersionedComponentId,
    pub worker_name: Expr,
    pub idempotency_key: Option<Expr>,
    pub response: ResponseMapping,
}

// ResponseMapping will consist of actual logic such as invoking worker functions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ResponseMapping(pub Expr);

impl From<CompiledGolemWorkerBinding> for GolemWorkerBinding {
    fn from(value: CompiledGolemWorkerBinding) -> Self {
        GolemWorkerBinding {
            component_id: value.component_id,
            worker_name: value.worker_name_compiled.worker_name,
            idempotency_key: value
                .idempotency_key_compiled
                .map(|idempotency_key_compiled| idempotency_key_compiled.idempotency_key),
            response: ResponseMapping(value.response_compiled.response_rib_expr),
        }
    }
}
