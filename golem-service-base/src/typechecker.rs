// Copyright 2024 Golem Cloud
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

use golem_common::model::CallingConvention;
use golem_wasm_rpc::protobuf::{val, Val};

use crate::type_inference::infer_analysed_type;
use golem_wasm_ast::analysis::{AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedType};
use golem_wasm_rpc::{json, protobuf, TypeExt};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::TypedOption;
use serde_json::Value;

pub trait TypeCheckIn {
    fn validate_function_parameters(
        self,
        expected_parameters: Vec<AnalysedFunctionParameter>,
        calling_convention: CallingConvention,
    ) -> Result<Vec<Val>, Vec<String>>;
}

impl TypeCheckIn for Vec<Value> {
    fn validate_function_parameters(
        self,
        expected_parameters: Vec<AnalysedFunctionParameter>,
        calling_convention: CallingConvention,
    ) -> Result<Vec<Val>, Vec<String>> {

        match calling_convention {
            CallingConvention::Component => {
                let parameter_values = json::function_parameters(&Value::Array(self), &expected_parameters)?;
                Ok(parameter_values
                    .into_iter()
                    .map(|value| value.into())
                    .collect())
            }
            CallingConvention::Stdio => {
                if expected_parameters.is_empty() {
                    let vval: Val = Val {
                        val: Some(val::Val::String(Value::Array(self).to_string())),
                    };

                    Ok(vec![vval])
                } else {
                    Err(vec!["The exported function should not have any parameters when using the stdio calling convention".to_string()])
                }
            }
        }
    }
}

impl TypeCheckIn for Vec<Val> {
    fn validate_function_parameters(
        self,
        expected_parameters: Vec<AnalysedFunctionParameter>,
        calling_convention: CallingConvention,
    ) -> Result<Vec<Val>, Vec<String>> {
        match calling_convention {
            CallingConvention::Component => {
                protobuf::function_parameters(&self, expected_parameters)?;
                Ok(self)
            }
            CallingConvention::Stdio => {
                if expected_parameters.is_empty() {
                    if self.len() == 1 {
                        match &self[0].val {
                            Some(val::Val::String(_)) => Ok(self.clone()),
                            _ => Err(vec!["The exported function should be called with a single string parameter".to_string()])
                        }
                    } else {
                        Err(vec![
                            "The exported function should be called with a single string parameter"
                                .to_string(),
                        ])
                    }
                } else {
                    Err(vec!["The exported function should not have any parameters when using the stdio calling convention".to_string()])
                }
            }
        }
    }
}

pub trait TypeCheckOut {
    fn validate_function_result(
        self,
        expected_types: Vec<AnalysedFunctionResult>,
        calling_convention: CallingConvention,
    ) -> Result<TypeAnnotatedValue, Vec<String>>;
}

impl TypeCheckOut for Vec<Val> {
    fn validate_function_result(
        self,
        expected_types: Vec<AnalysedFunctionResult>,
        calling_convention: CallingConvention,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        match calling_convention {
            CallingConvention::Component => {
                let mut errors = Vec::new();
                let mut results = Vec::new();
                for proto_value in self {
                    match proto_value.try_into() {
                        Ok(value) => results.push(value),
                        Err(err) => errors.push(err),
                    }
                }

                if errors.is_empty() {
                    let result_json = json::function_result_typed(results, &expected_types)?;
                    Ok(result_json)
                } else {
                    Err(errors)
                }
            }

            CallingConvention::Stdio => {
                if self.len() == 1 {
                    let value_opt = &self[0].val;

                    match value_opt {
                        Some(val::Val::String(s)) => {
                            let analysed_typ = AnalysedType::Str;
                            if s.is_empty() {
                                Ok(TypeAnnotatedValue::Option(
                                    Box::new(TypedOption {
                                        value: None,
                                        typ: Some(analysed_typ.to_type()),
                                    })
                                ))
                            } else {
                                let result: Value = serde_json::from_str(s).unwrap_or(Value::String(s.to_string()));
                                let typ = infer_analysed_type(&result);
                                json::get_typed_value_from_json(&result, &typ)
                            }
                        }
                        _ => Err(vec!["Expecting a single string as the result value when using stdio calling convention".to_string()]),
                    }
                } else {
                    Err(vec!["Expecting a single string as the result value when using stdio calling convention".to_string()])
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::typechecker::TypeCheckOut;
    use golem_common::model::CallingConvention;
    use golem_wasm_ast::analysis::{AnalysedFunctionResult, AnalysedType};
    use golem_wasm_rpc::json;
    use golem_wasm_rpc::protobuf::{val, Val};
    use serde_json::Value;

    #[test]
    fn test_validate_function_result_stdio() {
        let str_val = vec![Val {
            val: Some(val::Val::String("str".to_string())),
        }];

        let res = str_val
            .validate_function_result(
                vec![AnalysedFunctionResult {
                    name: Some("a".to_string()),
                    typ: AnalysedType::Str,
                }],
                CallingConvention::Stdio,
            )
            .map(|typed_value| json::get_json_from_typed_value(&typed_value));

        assert_eq!(res, Ok(Value::String("str".to_string())));

        let num_val = vec![Val {
            val: Some(val::Val::String("12.3".to_string())),
        }];

        let res = num_val
            .validate_function_result(
                vec![AnalysedFunctionResult {
                    name: Some("a".to_string()),
                    typ: AnalysedType::F64,
                }],
                CallingConvention::Stdio,
            )
            .map(|typed_value| json::get_json_from_typed_value(&typed_value));

        assert_eq!(
            res,
            Ok(Value::Number(serde_json::Number::from_f64(12.3).unwrap()))
        );

        let bool_val = vec![Val {
            val: Some(val::Val::String("true".to_string())),
        }];

        let res = bool_val
            .validate_function_result(
                vec![AnalysedFunctionResult {
                    name: Some("a".to_string()),
                    typ: AnalysedType::Bool,
                }],
                CallingConvention::Stdio,
            )
            .map(|typed_value| json::get_json_from_typed_value(&typed_value));

        assert_eq!(res, Ok(Value::Bool(true)));
    }
}
