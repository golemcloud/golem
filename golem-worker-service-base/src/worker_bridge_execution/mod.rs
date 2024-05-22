use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::TypeAnnotatedValue;
use serde_json::Value;

use golem_common::model::{ComponentId, IdempotencyKey};

mod content_type_mapper;
mod refined_worker_response;
pub mod to_response;
mod worker_request_executor;

use crate::merge::Merge;
pub use refined_worker_response::*;
pub use worker_request_executor::*;

#[derive(PartialEq, Debug, Clone)]
pub struct WorkerRequest {
    pub component_id: ComponentId,
    pub worker_name: String,
    pub function_name: String,
    pub function_params: Vec<Value>,
    pub idempotency_key: Option<IdempotencyKey>,
}

