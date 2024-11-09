use golem_common::model::{ComponentId, IdempotencyKey};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

pub mod api_definition_lookup;
pub mod file_server_binding_handler;
pub mod gateway_binding_executor;
pub mod gateway_binding_resolver;
mod gateway_worker_request_executor;
mod http_content_type_mapper;
pub mod rib_input_value_resolver;
pub mod router;
pub mod to_response;

pub use gateway_worker_request_executor::*;

#[derive(PartialEq, Debug, Clone)]
pub struct GatewayResolvedWorkerRequest {
    pub component_id: ComponentId,
    pub worker_name: Option<String>,
    pub function_name: String,
    pub function_params: Vec<TypeAnnotatedValue>,
    pub idempotency_key: Option<IdempotencyKey>,
}
