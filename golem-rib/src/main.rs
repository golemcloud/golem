use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
    AnalysedType, TypeStr,
};
use rib::{type_pull_up, Expr, InferredType, RibInputTypeInfo};
use std::collections::HashMap;

fn main() {
    let request_value_type = AnalysedType::Str(TypeStr);
    let output_analysed_type = AnalysedType::Str(TypeStr);

    let expr = r#"
               let x: str = "afsal";
               my-worker-function(x)
            "#;

    let expr = Expr::from_text(expr).unwrap();

    let analysed_exports = get_component_metadata(
        "my-worker-function",
        vec![request_value_type.clone()],
        output_analysed_type,
    );

    let compiled = rib::compile(&expr, &analysed_exports).unwrap();

    dbg!(compiled);
}

fn get_component_metadata(
    function_name: &str,
    input_types: Vec<AnalysedType>,
    output: AnalysedType,
) -> Vec<AnalysedExport> {
    let analysed_function_parameters = input_types
        .into_iter()
        .enumerate()
        .map(|(index, typ)| AnalysedFunctionParameter {
            name: format!("param{}", index),
            typ,
        })
        .collect();

    vec![AnalysedExport::Function(AnalysedFunction {
        name: function_name.to_string(),
        parameters: analysed_function_parameters,
        results: vec![AnalysedFunctionResult {
            name: None,
            typ: output,
        }],
    })]
}
