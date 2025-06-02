use golem_wasm_ast::analysis::AnalysedFunctionResult;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{protobuf, TypeAnnotatedValueConstructors};

pub fn interpret_function_result(
    function_results: Option<golem_wasm_rpc::Value>,
    expected_types: Option<AnalysedFunctionResult>,
) -> Result<Option<TypeAnnotatedValue>, Vec<String>> {
    match (function_results, expected_types) {
        (None, None) => Ok(None),
        (Some(_), None) => Err(vec![
            "Unexpected result value (got some, expected: none)".to_string()
        ]),
        (None, Some(_)) => Err(vec![
            "Unexpected result value (got none, expected: some)".to_string()
        ]),
        (Some(value), Some(expected)) => {
            TypeAnnotatedValue::create(&value, &expected.typ).map(Some)
        }
    }
}
