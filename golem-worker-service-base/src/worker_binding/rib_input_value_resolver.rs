use crate::worker_binding::{RequestDetails, WorkerDetail};
use golem_service_base::type_inference::infer_analysed_type;
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use rib::RibInputTypeInfo;
use std::collections::HashMap;
use std::fmt::Display;

pub trait RibInputValueResolver {
    fn resolve_rib_input_value(
        &self,
        required_type: &RibInputTypeInfo,
    ) -> Result<RibInputValue, RibInputTypeMismatch>;
}

#[derive(Debug)]
pub struct RibInputTypeMismatch(pub String);

impl Display for RibInputTypeMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rib input type mismatch: {}", self.0)
    }
}

#[derive(Clone, Debug)]
pub struct RibInputValue {
    pub value: HashMap<String, TypeAnnotatedValue>,
}

impl RibInputValue {
    pub fn empty() -> Self {
        RibInputValue {
            value: HashMap::new(),
        }
    }

    pub fn merge(&self, other: RibInputValue) -> RibInputValue {
        let mut cloned = self.clone();
        cloned.value.extend(other.value);
        cloned
    }
}

impl RibInputValueResolver for RequestDetails {
    fn resolve_rib_input_value(
        &self,
        required_types: &RibInputTypeInfo,
    ) -> Result<RibInputValue, RibInputTypeMismatch> {
        let request_type_info = required_types.types.get("request");

        let rib_input_with_request_content = &self.as_json();

        let request_type_annotated_value = match request_type_info {
            Some(request_type) => {
                TypeAnnotatedValue::parse_with_type(rib_input_with_request_content, request_type)
                        .map_err(|err| RibInputTypeMismatch(format!("Input request details don't match the requirements for rib expression to execute: {}. Requirements. {:?}", err.join(", "), request_type)))?
            }
            None => {
                let analysed_type = infer_analysed_type(rib_input_with_request_content);

                TypeAnnotatedValue::parse_with_type(rib_input_with_request_content, &analysed_type)
                    .map_err(|err| RibInputTypeMismatch(format!("Internal Error: Input request has been inferred  to {:?} but failed to get converted to a valid input. {}", analysed_type, err.join(", "))))?
            }
        };

        let mut rib_input_map = HashMap::new();
        rib_input_map.insert("request".to_string(), request_type_annotated_value);

        Ok(RibInputValue {
            value: rib_input_map,
        })
    }
}

impl RibInputValueResolver for WorkerDetail {
    fn resolve_rib_input_value(
        &self,
        required_types: &RibInputTypeInfo,
    ) -> Result<RibInputValue, RibInputTypeMismatch> {
        let request_type_info = required_types.types.get("worker");

        match request_type_info {
            Some(worker_details_type) => {
                let rib_input_with_request_content = &self.as_json();
                let request_value =
                    TypeAnnotatedValue::parse_with_type(rib_input_with_request_content, worker_details_type)
                        .map_err(|err| RibInputTypeMismatch(format!("Worker details don't match the requirements for rib expression to execute: {}. Requirements. {:?}", err.join(", "), worker_details_type)))?;

                let mut rib_input_map = HashMap::new();
                rib_input_map.insert("worker".to_string(), request_value);
                Ok(RibInputValue {
                    value: rib_input_map,
                })
            }
            None => Ok(RibInputValue::empty()),
        }
    }
}
