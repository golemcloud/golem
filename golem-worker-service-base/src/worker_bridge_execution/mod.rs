use golem_common::model::{ComponentId, IdempotencyKey};
use golem_common::precise_json::PreciseJson;

mod content_type_mapper;
mod refined_worker_response;
pub mod to_response;
mod worker_request_executor;
pub use refined_worker_response::*;
use rib::ParsedFunctionName;
pub use worker_request_executor::*;

#[derive(PartialEq, Debug, Clone)]
pub struct WorkerRequest {
    pub component_id: ComponentId,
    pub worker_name: String,
    pub function_name: ParsedFunctionName,
    pub function_params: Vec<PreciseJson>,
    pub idempotency_key: Option<IdempotencyKey>,
}
