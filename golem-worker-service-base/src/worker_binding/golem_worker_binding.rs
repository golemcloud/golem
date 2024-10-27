use std::fmt::Formatter;
use bincode::{Decode, Encode};
use poem_openapi::Enum;
use serde::{Deserialize, Serialize};

use crate::worker_binding::CompiledGolemWorkerBinding;
use golem_service_base::model::VersionedComponentId;
use rib::Expr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct GolemWorkerBinding {
    #[serde(rename = "bindingType")]  // Explicitly specify this as camelCase
    pub binding_type: String,
    #[serde(rename = "componentId")]
    pub component_id: VersionedComponentId,
    #[serde(rename = "workerName")]
    pub worker_name: Expr,
    #[serde(rename = "idempotencyKey")]
    pub idempotency_key: Option<Expr>,
    pub response: ResponseMapping,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Enum, Encode, Decode)]
pub enum BindingType {
    #[serde(rename = "wit-worker")]
    WitWorker,
    #[serde(rename = "file-server")]
    FileServer,
}

impl BindingType {
    pub(crate) fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(BindingType::WitWorker),
            1 => Some(BindingType::FileServer),
            _ => None, // Return None for any invalid values
        }
    }
    pub(crate) fn to_i32(&self) -> i32 {
        match self {
            BindingType::WitWorker => 0,
            BindingType::FileServer => 1
        }
    }

    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BindingType::WitWorker => write!(f, "wit-worker"),
            BindingType::FileServer => write!(f, "file-server"),
        }
    }
}
// ResponseMapping will consist of actual logic such as invoking worker functions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ResponseMapping(pub Expr);

impl From<CompiledGolemWorkerBinding> for GolemWorkerBinding {
    fn from(value: CompiledGolemWorkerBinding) -> Self {
        let worker_binding = value.clone();

        GolemWorkerBinding {
            binding_type: worker_binding.binding_type,
            component_id: worker_binding.component_id,
            worker_name: worker_binding.worker_name_compiled.worker_name,
            idempotency_key: worker_binding
                .idempotency_key_compiled
                .map(|idempotency_key_compiled| idempotency_key_compiled.idempotency_key),
            response: ResponseMapping(worker_binding.response_compiled.response_rib_expr),
        }
    }
}
