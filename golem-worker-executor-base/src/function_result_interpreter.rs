use golem_common::model::CallingConvention;
use golem_wasm_rpc::protobuf::{Type, TypedTuple};

use golem_wasm_ast::analysis::{AnalysedFunctionResult, AnalysedType};
use golem_wasm_rpc::json;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::TypedOption;

pub fn interpret_function_results(
    function_results: Vec<golem_wasm_rpc::Value>,
    expected_types: Vec<AnalysedFunctionResult>,
    calling_convention: CallingConvention,
) -> Result<TypeAnnotatedValue, Vec<String>> {
    match calling_convention {
        CallingConvention::Component => {
            let result_json = json::function_result_typed(function_results, &expected_types)?;
            Ok(result_json)
        }

        CallingConvention::Stdio => match function_results.first() {
            Some(golem_wasm_rpc::Value::String(s)) => {
                let analysed_typ = AnalysedType::Str;

                if s.is_empty() {
                    let option = TypeAnnotatedValue::Option(Box::new(TypedOption {
                        value: None,
                        typ: Some(Type::from(&analysed_typ)),
                    }));

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
            _ => Err(vec![
                "Expecting a single string as the result value when using stdio calling convention"
                    .to_string(),
            ]),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::interpret_function_results;
    use golem_common::model::CallingConvention;
    use golem_wasm_ast::analysis::{AnalysedFunctionResult, AnalysedType};
    use golem_wasm_rpc::json;
    use golem_wasm_rpc::Value as WasmRpcValue;
    use serde_json::Value;

    #[test]
    fn test_function_result_interpreter_stdio() {
        let str_val = vec![WasmRpcValue::String("str".to_string())];

        let res = interpret_function_results(
            str_val,
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
