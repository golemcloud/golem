#[cfg(test)]
mod mock_interpreter;
mod function_metadata;
mod component_metadata;
mod test_utils;
mod data_types;
mod mock_data;

use golem_wasm_ast::analysis::{AnalysedType, TypeStr, TypeU64, TypeU8};
use crate::{compiler, Expr};

#[tokio::test]
async fn test_interpreter_complex_rib() {
    let expr = r#"

              let str1: str = request.body.name;
              let str2: str = request.headers.name;
              let str3: str = requuest.path.name;

              let unused = function-unit-response(str1);

              let str_output = function-no-arg();

              let unused = function-no-arg-unit();

              let str_response = function-str-response(str_output);

              str_response
        "#;

    let expr = Expr::from_text(expr).unwrap();

    let compiled_expr = compiler::compile(&expr, &component_metadata::component_metadata()).unwrap().byte_code;

    let mut rib_executor = mock_interpreter::interpreter();
    let result = rib_executor.run(compiled_expr).await.unwrap();

    let expected_result = test_utils::get_type_annotated_value(
        &test_utils::analysed_type_record(vec![
            ("a", AnalysedType::Str(TypeStr)),
            ("b", AnalysedType::Str(TypeStr)),
            ("c", AnalysedType::Str(TypeStr)),
            ("d", AnalysedType::U64(TypeU64)),
            ("e", AnalysedType::Str(TypeStr)),
            ("f", AnalysedType::Str(TypeStr)),
            ("g", AnalysedType::Str(TypeStr)),
            ("h", AnalysedType::Str(TypeStr)),
            ("i", AnalysedType::U8(TypeU8)),
            ("j", AnalysedType::U8(TypeU8)),
            ("k", AnalysedType::Str(TypeStr)),
            ("m", AnalysedType::Str(TypeStr)),
            ("n", AnalysedType::Str(TypeStr)),
        ]),
        r#" { a : "bId", b : "bTitle2", c : "bStreet", d: 200, e: "success", f: "failure", g: "bar", h : "fuuz", i: 0, j: 1, k: "validated", m:"jon", n: "1" }"#,
    );
    assert_eq!(result.get_val().unwrap(), expected_result);
}

