use golem_common::model::{ComponentId, IdempotencyKey};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

mod http_content_type_mapper;
pub mod to_response;
mod gateway_worker_request_executor;
pub mod router;
pub mod api_definition_lookup;
pub mod rib_input_value_resolver;
pub mod gateway_binding_resolver;

pub use gateway_worker_request_executor::*;

#[derive(PartialEq, Debug, Clone)]
pub struct GatewayResolvedWorkerRequest {
    pub component_id: ComponentId,
    pub worker_name: Option<String>,
    pub function_name: String,
    pub function_params: Vec<TypeAnnotatedValue>,
    pub idempotency_key: Option<IdempotencyKey>,
}
