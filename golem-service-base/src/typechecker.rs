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
use golem_wasm_rpc::protobuf::{Type, TypedTuple};

use golem_wasm_ast::analysis::{AnalysedFunctionResult, AnalysedType};
use golem_wasm_rpc::json;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::TypedOption;

pub trait TypeCheckOut {
    fn validate_function_result(
        self,
        expected_types: Vec<AnalysedFunctionResult>,
        calling_convention: CallingConvention,
    ) -> Result<TypeAnnotatedValue, Vec<String>>;
}

impl TypeCheckOut for Vec<golem_wasm_rpc::Value> {
    fn validate_function_result(
        self,
        expected_types: Vec<AnalysedFunctionResult>,
        calling_convention: CallingConvention,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        match calling_convention {
            CallingConvention::Component => {
                let result_json = json::function_result_typed(self, &expected_types)?;
                Ok(result_json)
            }

            CallingConvention::Stdio => {
                match self.first() {
                    Some(golem_wasm_rpc::Value::String(s)) => {
                        let analysed_typ = AnalysedType::Str;

                        if s.is_empty() {
                            let option = TypeAnnotatedValue::Option(
                                Box::new(TypedOption {
                                    value: None,
                                    typ: Some(Type::from(&analysed_typ)),
                                })
                            );

                            let optional = AnalysedType::Option(Box::new(analysed_typ.clone()));

                            Ok(TypeAnnotatedValue::Tuple(TypedTuple {
                                typ: vec![Type::from(&optional)],
                                value: vec![golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                                    type_annotated_value: Some(option),
                                }],
                            }))

                        } else {
                            Ok(TypeAnnotatedValue::Tuple(TypedTuple {
                                typ: vec![Type::from(&analysed_typ)],
                                value: vec![golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                                    type_annotated_value: Some(TypeAnnotatedValue::Str(s.to_string())),
                                }],
                            }))
                        }
                    }
                    _ => Err(vec!["Expecting a single string as the result value when using stdio calling convention".to_string()]),
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
    use golem_wasm_rpc::Value as WasmRpcValue;
    use serde_json::Value;

    #[test]
    fn test_validate_function_result_stdio() {
        let str_val = vec![WasmRpcValue::String("str".to_string())];

        let res = str_val
            .validate_function_result(
                vec![AnalysedFunctionResult {
                    name: Some("a".to_string()),
                    typ: AnalysedType::Str,
                }],
                CallingConvention::Stdio,
            )
            .map(|typed_value| json::get_json_from_typed_value(&typed_value));

        assert_eq!(
            res,
            Ok(Value::Array(vec![Value::String("str".to_string())]))
        );
    }
}
