use golem_wasm_ast::analysis::AnalysedFunctionResult;
use golem_wasm_rpc::json;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

pub fn interpret_function_results(
    function_results: Vec<golem_wasm_rpc::Value>,
    expected_types: Vec<AnalysedFunctionResult>,
) -> Result<TypeAnnotatedValue, Vec<String>> {
    let result = json::function_result_typed(function_results, &expected_types)?;
    Ok(result)
}
