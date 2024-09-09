use golem_common::model::{ComponentId, IdempotencyKey};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

mod content_type_mapper;
pub mod to_response;
mod worker_request_executor;
use rib::ParsedFunctionName;
pub use worker_request_executor::*;

#[derive(PartialEq, Debug, Clone)]
pub struct WorkerRequest {
    pub component_id: ComponentId,
    pub worker_name: String,
    pub function_name: ParsedFunctionName,
    pub function_params: Vec<TypeAnnotatedValue>,
    pub idempotency_key: Option<IdempotencyKey>,
}
