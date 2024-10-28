use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use crate::worker_binding::{CompiledGolemWorkerBinding, CompiledGolemWorkerBindingType};
use golem_service_base::model::VersionedComponentId;
use rib::Expr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub enum GolemWorkerBindingType {
    WitWorker,
    FileServer,
}

impl From<CompiledGolemWorkerBindingType> for GolemWorkerBindingType {
    fn from(binding: CompiledGolemWorkerBindingType) -> Self {
        match binding {
            CompiledGolemWorkerBindingType::WitWorker => GolemWorkerBindingType::WitWorker,
            CompiledGolemWorkerBindingType::FileServer => GolemWorkerBindingType::FileServer,
        }
    }
}

impl From<GolemWorkerBindingType> for i32 {
    fn from(value: GolemWorkerBindingType) -> Self {
        match value {
            GolemWorkerBindingType::WitWorker => 0,
            GolemWorkerBindingType::FileServer => 1,
        }
    }
}

impl TryFrom<&str> for GolemWorkerBindingType {
    type Error = String;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "wit-worker" => Ok(GolemWorkerBindingType::WitWorker),
            "file-server" => Ok(GolemWorkerBindingType::FileServer),
            _ => Err(format!("Unknown golem worker binding: {}", value)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub struct GolemWorkerBinding {
    pub r#type: GolemWorkerBindingType,
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
        let worker_binding = value.clone();

        GolemWorkerBinding {
            r#type: worker_binding.r#type.into(),
            component_id: worker_binding.component_id,
            worker_name: worker_binding.worker_name_compiled.worker_name,
            idempotency_key: worker_binding
                .idempotency_key_compiled
                .map(|idempotency_key_compiled| idempotency_key_compiled.idempotency_key),
            response: ResponseMapping(worker_binding.response_compiled.response_rib_expr),
        }
    }
}
