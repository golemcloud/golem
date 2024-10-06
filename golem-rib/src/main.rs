use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
    AnalysedType, TypeStr,
};
use rib::{type_pull_up, Expr, FunctionTypeRegistry, InferredType, RibInputTypeInfo};
use std::collections::HashMap;

fn main() {
    let expr_str = r#"
              ["thaj"]
            "#;

    let mut expr = Expr::from_text(expr_str).unwrap();

    expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();

    dbg!(expr);
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
