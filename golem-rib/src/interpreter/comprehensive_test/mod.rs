mod component_metadata;
mod data_types;
mod function_metadata;
mod mock_data;
#[cfg(test)]
mod mock_interpreter;
mod test_utils;

use crate::{compiler, Expr};
use golem_wasm_ast::analysis::{AnalysedType, TypeStr, TypeU64, TypeU8};

#[tokio::test]
async fn test_interpreter_complex_rib() {
    let expr = r#"

              let str1: str = request.body.name;
              let str2: str = request.headers.name;
              let str3: str = request.path.name;

              let unused = function-unit-response(str1);

              let str_output = function-no-arg();

              let unused = function-no-arg-unit();

              let str_response = function-str-response(str_output);

              let number_response = function-number-response(str1);

              let option_str_response = function-option-str-response(str2);

              let option_number_response = function-option-number-response(str1);

              let option_option_response = function-option-option-response(str1);

              let option_variant_response = function-option-variant-response(str1);

              let option_enum_response = function-option-enum-response(str1);

              let option_tuple_response = function-option-tuple-response(str1);

              let option_record_response = function-option-record-response(str1);

              let option_list_response = function-option-list-response(str1);

              let list_number_response = function-list-number-response(str1);

              let list_str_response = function-list-str-response(str1);

              let list_option_response = function-list-option-response(str1);

              let list_list_response = function-list-list-response(str1);

              let list_variant_response = function-list-variant-response(str1);

              let list_enum_response = function-list-enum-response(str1);

              let list_tuple_response = function-list-tuple-response(str1);

              let list_record_response = function-list-record-response(str1);

              let result_of_str_response = function-result-str-response(str1);

              let result_of_number_response = function-result-number-response(str1);

              let result_of_variant_response = function-result-variant-response(str1);

              let result_of_enum_response = function-result-enum-response(str1);

              let result_of_tuple_response = function-result-tuple-response(str1);

              let result_of_flag_response = function-result-flag-response(str1);

              let result_of_record_response = function-result-record-response(str1);

              let result_of_list_response = function-result-list-response(str1);

              let tuple_response = function-tuple-response(str1);

              let enum_response = function-enum-response(str1);

              let flag_response = function-flag-response(str1);

              let variant_response = function-variant-response(str1);

              let str_response_processed = str_response == "foo";

              let number_response_processed = if number_response == 42u64 then "foo" else "bar";

              let option_str_response_processed = match option_str_response {
                some(text) => text,
                none => "not found"
              };

              let option_number_response_processed = match option_number_response {
                some(number) => number,
                none => 0
              };

              let option_option_response_processed = match option_option_response {
                 some(some(x)) => x,
                 none => "not found"
              };



              str_response
        "#;

    let expr = Expr::from_text(expr).unwrap();

    let compiled_expr = compiler::compile(&expr, &component_metadata::component_metadata())
        .unwrap()
        .byte_code;

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
