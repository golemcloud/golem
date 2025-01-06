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

use crate::gateway_binding::{HttpRequestDetails, WorkerDetail};
use golem_common::SafeDisplay;
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use rib::{RibInput, RibInputTypeInfo};
use std::collections::HashMap;
use std::fmt::Display;
use tracing::warn;

// `RibInputValueResolver` is responsible
// for extracting `RibInputValue` from any input, given the requirements as `RibInputTypeInfo`.
// Example: HttpRequestDetails can be converted to RibInputValue
// Note that `RibInputTypeInfo` is obtained from compiling a rib script.
pub trait RibInputValueResolver {
    fn resolve_rib_input_value(
        &self,
        required_type: &RibInputTypeInfo,
    ) -> Result<RibInput, RibInputTypeMismatch>;
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

impl RibInputValueResolver for HttpRequestDetails {
    fn resolve_rib_input_value(
        &self,
        required_types: &RibInputTypeInfo,
    ) -> Result<RibInput, RibInputTypeMismatch> {
        let request_type_info = required_types.types.get("request");

        let rib_input_with_request_content = &self.as_json();

        match request_type_info {
            Some(request_type) => {
                warn!("received: {:?}", rib_input_with_request_content);
                let input = TypeAnnotatedValue::parse_with_type(rib_input_with_request_content, request_type)
                        .map_err(|err| RibInputTypeMismatch(format!("Input request details don't match the requirements for rib expression to execute: {}. Requirements. {:?}", err.join(", "), request_type)))?;
                let input = input.try_into().map_err(|err| {
                    RibInputTypeMismatch(format!(
                        "Internal error converting between value representations: {err}"
                    ))
                })?;

                let mut rib_input_map = HashMap::new();
                rib_input_map.insert("request".to_string(), input);
                Ok(RibInput {
                    input: rib_input_map,
                })
            }
            None => Ok(RibInput::default()),
        }
    }
}

impl RibInputValueResolver for WorkerDetail {
    fn resolve_rib_input_value(
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
