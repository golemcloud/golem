use golem_wasm_rpc::TypeAnnotatedValue;

use golem_common::model::{ComponentId, IdempotencyKey};
use golem_service_base::model::ComponentMetadata;

mod content_type_mapper;
mod refined_worker_response;
pub mod to_response;
mod worker_request_executor;

use crate::evaluator::{ComponentElements, FQN, Function};
pub use refined_worker_response::*;
pub use worker_request_executor::*;

#[derive(PartialEq, Debug, Clone)]
pub struct WorkerRequest {
    pub component_id: ComponentId,
    pub worker_name: String,
    pub function: Function,
    pub function_params: Vec<TypeAnnotatedValue>,
    pub idempotency_key: Option<IdempotencyKey>,
}
