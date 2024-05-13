use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::TypeAnnotatedValue;
use serde_json::Value;

use golem_common::model::ComponentId;

use crate::evaluator::{Evaluator};

mod worker_bridge_response;
mod worker_request_executor;
pub mod to_response;

pub use worker_bridge_response::*;
pub use worker_request_executor::*;

// Every input request can be resolved to a worker request,
// along with the value of any variables that's associated with it.
#[derive(PartialEq, Debug, Clone)]
pub struct WorkerRequest {
    pub component_id: ComponentId,
    pub worker_name: String,
    pub function_name: String,
    pub function_params: Vec<Value>,
}

impl WorkerRequest {
    pub fn to_type_annotated_value(self) -> TypeAnnotatedValue {
        TypeAnnotatedValue::Record {
            typ: vec![
                ("component_id".to_string(), AnalysedType::Str),
                ("name".to_string(), AnalysedType::Str),
                ("function_name".to_string(), AnalysedType::Str),
            ],
            value: vec![
                ("component_id".to_string(), TypeAnnotatedValue::Str(self.component_id.0.to_string())),
                ("name".to_string(), TypeAnnotatedValue::Str(self.worker_name)),
                ("function_name".to_string(), TypeAnnotatedValue::Str(self.function_name)),
            ],
        }
    }
}