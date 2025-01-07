// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_common::model::{ComponentId, IdempotencyKey};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

pub mod api_definition_lookup;
pub mod auth_call_back_binding_handler;
pub mod file_server_binding_handler;
pub mod gateway_binding_resolver;
pub mod gateway_http_input_executor;
pub mod gateway_session;
mod gateway_worker_request_executor;
mod http_content_type_mapper;
pub mod rib_input_value_resolver;
pub mod router;
pub mod to_response;
pub mod to_response_failure;

pub use gateway_worker_request_executor::*;

#[derive(PartialEq, Debug, Clone)]
pub struct GatewayResolvedWorkerRequest<Namespace> {
    pub component_id: ComponentId,
    pub worker_name: Option<String>,
    pub function_name: String,
    pub function_params: Vec<TypeAnnotatedValue>,
    pub idempotency_key: Option<IdempotencyKey>,
    pub namespace: Namespace,
}
