pub(crate) use missing_fields::*;
pub(crate) use path::*;
pub(crate) use type_check_error::*;
pub(crate) use type_mismatch::*;
pub(crate) use unresolved_types::*;

mod exhaustive_pattern_match;
mod missing_fields;
mod path;
mod type_check_error;
mod type_mismatch;
mod type_mismatch_call_args;
mod unresolved_types;

use crate::type_checker::exhaustive_pattern_match::check_exhaustive_pattern_match;
use crate::type_checker::type_mismatch_call_args::check_type_errors_in_function_call;
use crate::{Expr, FunctionTypeRegistry};

pub fn type_check(
    expr: &mut Expr,
    function_type_registry: &FunctionTypeRegistry,
) -> Result<(), String> {
    check_type_errors_in_function_call(expr, function_type_registry)
        .map_err(|function_call_type_check_error| function_call_type_check_error.to_string())?;
    check_unresolved_types(expr).map_err(|unresolved_error| unresolved_error.to_string())?;
    check_exhaustive_pattern_match(expr, function_type_registry)
        .map_err(|exhaustive_check_error| exhaustive_check_error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod type_check_tests {

    mod unresolved_types_error {
        use test_r::test;

        use crate::type_checker::type_check_tests::internal;
        use crate::{compile, Expr};

        #[test]
        fn test_invalid_key_in_record_in_function_call() {
            let expr = r#"
          let result = foo({x: 3, a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: "foo", c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_record();

            let result = compile(&expr, &metadata).unwrap_err();

            let expected = "Invalid argument in `foo`: `{x: 3, a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, \"foo\")}, b: \"foo\", c: [1, 2, 3], d: {da: 4}}`. Expected type: record<a: record<aa: s32, ab: s32, ac: list<s32>, ad: record<ada: s32>, ae: tuple<s32, string>>, b: u64, c: list<s32>, d: record<da: s32>>. Unable to determine the type of `3` in the record at path `x`. Number literals must have a type annotation. Example: `1u64`";
            assert_eq!(result, expected);
        }
    }

    mod type_mismatch_errors {
        use test_r::test;

        use crate::type_checker::type_check_tests::internal;
        use crate::{compile, Expr};

        #[test]
        fn test_type_mismatch_in_record_in_function_call1() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: "foo", c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_record();

            let result = compile(&expr, &metadata).unwrap_err();

            let expected = "Invalid argument in `foo`: `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, \"foo\")}, b: \"foo\", c: [1, 2, 3], d: {da: 4}}`. Type mismatch for `b`. Expected `u64`";
            assert_eq!(result, expected);
        }

        #[test]
        fn test_type_mismatch_in_record_in_function_call2() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: ["foo", "bar"], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_record();

            let result = compile(&expr, &metadata).unwrap_err();

            let expected = "Invalid argument in `foo`: `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, \"foo\")}, b: 2, c: [\"foo\", \"bar\"], d: {da: 4}}`. Type mismatch for `c`. Expected `list<s32>`";
            assert_eq!(result, expected);
        }

        #[test]
        fn test_type_mismatch_in_record_in_function_call3() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: [1, 2], d: {da: "foo"}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_record();

            let result = compile(&expr, &metadata).unwrap_err();

            let expected = "Invalid argument in `foo`: `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, \"foo\")}, b: 2, c: [1, 2], d: {da: \"foo\"}}`. Type mismatch for `d.da`. Expected `s32`";
            assert_eq!(result, expected);
        }

        #[test]
        fn test_type_mismatch_in_nested_record_in_function_call1() {
            let expr = r#"
          let result = foo({a: {aa: "foo", ab: 2, ac: [1, 2], ad: {ada: "1"}, ae: (1, "foo")}, b: 3, c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_record();

            let result = compile(&expr, &metadata).unwrap_err();

            let expected = "Invalid argument in `foo`: `{a: {aa: \"foo\", ab: 2, ac: [1, 2], ad: {ada: \"1\"}, ae: (1, \"foo\")}, b: 3, c: [1, 2, 3], d: {da: 4}}`. Type mismatch for `a.aa`. Expected `s32`";
            assert_eq!(result, expected);
        }

        #[test]
        fn test_type_mismatch_in_nested_record_in_function_call2() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: "1"}, ae: (1, "foo")}, b: 3, c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_record();

            let result = compile(&expr, &metadata).unwrap_err();

            let expected = "Invalid argument in `foo`: `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: \"1\"}, ae: (1, \"foo\")}, b: 3, c: [1, 2, 3], d: {da: 4}}`. Type mismatch for `a.ad.ada`. Expected `s32`";
            assert_eq!(result, expected);
        }

        #[test]
        fn test_type_mismatch_in_nested_record_in_function_call3() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: 1, ad: {ada: 1}, ae:(1, "foo")}, b: 3, c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_record();

            let result = compile(&expr, &metadata).unwrap_err();

            let expected = "Invalid argument in `foo`: `{a: {aa: 1, ab: 2, ac: 1, ad: {ada: 1}, ae: (1, \"foo\")}, b: 3, c: [1, 2, 3], d: {da: 4}}`. Type mismatch for `a.ac`. Expected `list<s32>`";
            assert_eq!(result, expected);
        }

        #[test]
        fn test_type_mismatch_in_nested_record_in_function_call4() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_record();

            let result = compile(&expr, &metadata).unwrap_err();

            let expected = "Invalid argument in `foo`: `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3], d: {da: 4}}`. Type mismatch for `a.ae[1]`. Expected `string`";
            assert_eq!(result, expected);
        }
    }

    mod internal {
        use golem_wasm_ast::analysis::analysed_type::{list, record, s32, str, tuple, u64};
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            NameTypePair,
        };

        pub(crate) fn get_metadata_record() -> Vec<AnalysedExport> {
            let analysed_export = AnalysedExport::Function(AnalysedFunction {
                name: "foo".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "arg1".to_string(),
                    typ: record(vec![
                        NameTypePair {
                            name: "a".to_string(),
                            typ: record(vec![
                                NameTypePair {
                                    name: "aa".to_string(),
                                    typ: s32(),
                                },
                                NameTypePair {
                                    name: "ab".to_string(),
                                    typ: s32(),
                                },
                                NameTypePair {
                                    name: "ac".to_string(),
                                    typ: list(s32()),
                                },
                                NameTypePair {
                                    name: "ad".to_string(),
                                    typ: record(vec![NameTypePair {
                                        name: "ada".to_string(),
                                        typ: s32(),
                                    }]),
                                },
                                NameTypePair {
                                    name: "ae".to_string(),
                                    typ: tuple(vec![s32(), str()]),
                                },
                            ]),
                        },
                        NameTypePair {
                            name: "b".to_string(),
                            typ: u64(),
                        },
                        NameTypePair {
                            name: "c".to_string(),
                            typ: list(s32()),
                        },
                        NameTypePair {
                            name: "d".to_string(),
                            typ: record(vec![NameTypePair {
                                name: "da".to_string(),
                                typ: s32(),
                            }]),
                        },
                    ]),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: str(),
                }],
            });

            vec![analysed_export]
        }
    }
}
