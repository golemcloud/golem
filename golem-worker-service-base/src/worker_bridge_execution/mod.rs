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

// Every input request can be resolved to a worker request,
// along with the value of any variables that's associated with it.
#[derive(PartialEq, Debug, Clone)]
pub struct WorkerRequest {
    pub component_id: ComponentId,
    pub worker_name: String,
    pub function_name: String,
    pub function_params: Vec<Value>,
    pub idempotency_key: Option<IdempotencyKey>,
}

impl WorkerRequest {
    pub fn to_type_annotated_value(self) -> TypeAnnotatedValue {
        let mut required = TypeAnnotatedValue::Record {
            typ: vec![
                ("component_id".to_string(), AnalysedType::Str),
                ("name".to_string(), AnalysedType::Str),
                ("function_name".to_string(), AnalysedType::Str),
            ],
            value: vec![
                (
                    "component_id".to_string(),
                    TypeAnnotatedValue::Str(self.component_id.0.to_string()),
                ),
                (
                    "name".to_string(),
                    TypeAnnotatedValue::Str(self.worker_name),
                ),
                (
                    "function_name".to_string(),
                    TypeAnnotatedValue::Str(self.function_name),
                ),
            ],
        };

        let optional_idempotency_key = self.idempotency_key.map(|x| TypeAnnotatedValue::Record {
            // Idempotency key can exist in header of the request in which case users can refer to it as
            // request.headers.idempotency-key. In order to keep some consistency, we are keeping the same key name here,
            // if it exists as part of the API definition
            typ: vec![("idempotency-key".to_string(), AnalysedType::Str)],
            value: vec![(
                "idempotency-key".to_string(),
                TypeAnnotatedValue::Str(x.to_string()),
            )],
        });

        if let Some(idempotency_key) = optional_idempotency_key {
            required = required.merge(&idempotency_key).clone();
        }

        required
    }
}
