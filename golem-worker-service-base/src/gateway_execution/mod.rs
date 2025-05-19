// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_common::model::{ComponentId, IdempotencyKey};
use golem_common::SafeDisplay;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use std::collections::HashMap;
use std::fmt::Display;
pub mod api_definition_lookup;
pub mod auth_call_back_binding_handler;
pub mod file_server_binding_handler;
pub mod gateway_binding_resolver;
pub mod gateway_http_input_executor;
pub mod gateway_session;
mod gateway_worker_request_executor;
mod http_content_type_mapper;
pub mod http_handler_binding_handler;
pub mod request;
pub mod router;
pub mod to_response;
pub mod to_response_failure;
pub use gateway_worker_request_executor::*;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use rib::{RibInput, RibInputTypeInfo};
use serde_json::Value;

#[derive(PartialEq, Debug, Clone)]
pub struct GatewayResolvedWorkerRequest<Namespace> {
    pub component_id: ComponentId,
    pub worker_name: Option<String>,
    pub function_name: String,
    pub function_params: Vec<TypeAnnotatedValue>,
    pub idempotency_key: Option<IdempotencyKey>,
    pub invocation_context: InvocationContextStack,
    pub namespace: Namespace,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerDetails {
    pub component_id: ComponentId,
    pub worker_name: Option<String>,
    pub idempotency_key: Option<IdempotencyKey>,
    pub invocation_context: InvocationContextStack,
}

impl WorkerDetails {
    fn as_json(&self) -> Value {
        let mut worker_detail_content = HashMap::new();
        worker_detail_content.insert(
            "component_id".to_string(),
            Value::String(self.component_id.0.to_string()),
        );

        if let Some(worker_name) = &self.worker_name {
            worker_detail_content
                .insert("name".to_string(), Value::String(worker_name.to_string()));
        }

        if let Some(idempotency_key) = &self.idempotency_key {
            worker_detail_content.insert(
                "idempotency_key".to_string(),
                Value::String(idempotency_key.value.clone()),
            );
        }

        let map = serde_json::Map::from_iter(worker_detail_content);

        Value::Object(map)
    }

    pub fn resolve_rib_input_value(
        &self,
        required_types: &RibInputTypeInfo,
    ) -> Result<RibInput, RibInputTypeMismatch> {
        let request_type_info = required_types.types.get("worker");

        match request_type_info {
            Some(worker_details_type) => {
                let rib_input_with_request_content = &self.as_json();
                let request_value =
                    TypeAnnotatedValue::parse_with_type(rib_input_with_request_content, worker_details_type)
                        .map_err(|err| RibInputTypeMismatch(format!("Worker details don't match the requirements for rib expression to execute: {}. Requirements. {:?}", err.join(", "), worker_details_type)))?;
                let request_value = request_value.try_into().map_err(|err| {
                    RibInputTypeMismatch(format!(
                        "Internal error converting between value representations: {err}"
                    ))
                })?;

                let mut rib_input_map = HashMap::new();
                rib_input_map.insert("worker".to_string(), request_value);
                Ok(RibInput {
                    input: rib_input_map,
                })
            }
            None => Ok(RibInput::default()),
        }
    }
}

#[derive(Debug)]
pub struct RibInputTypeMismatch(pub String);

impl Display for RibInputTypeMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rib input type mismatch: {}", self.0)
    }
}

impl SafeDisplay for RibInputTypeMismatch {
    fn to_safe_string(&self) -> String {
        self.0.clone()
    }
}
