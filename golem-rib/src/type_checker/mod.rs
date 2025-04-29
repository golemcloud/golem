// Copyright 2024-2025 Golem Cloud
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

pub(crate) use check_instance_returns::*;
pub(crate) use exhaustive_pattern_match::*;
pub(crate) use invalid_math_expr::*;
pub(crate) use missing_fields::*;
pub use path::*;
pub(crate) use type_mismatch::*;
pub(crate) use unresolved_types::*;

mod check_instance_returns;
mod exhaustive_pattern_match;
mod invalid_math_expr;
mod invalid_worker_name;
mod missing_fields;
mod path;
mod type_check_in_function_calls;
mod type_mismatch;
mod unresolved_types;

use crate::rib_type_error::RibTypeError;
use crate::type_checker::exhaustive_pattern_match::check_exhaustive_pattern_match;
use crate::type_checker::invalid_math_expr::check_invalid_math_expr;
use crate::type_checker::invalid_worker_name::check_invalid_worker_name;
use crate::type_checker::type_check_in_function_calls::check_type_error_in_function_calls;
use crate::{Expr, FunctionTypeRegistry};

pub fn type_check(
    expr: &mut Expr,
    function_type_registry: &FunctionTypeRegistry,
) -> Result<(), RibTypeError> {
    check_type_error_in_function_calls(expr, function_type_registry)?;
    check_unresolved_types(expr)?;
    check_invalid_worker_name(expr)?;
    check_invalid_program_return(expr)?;
    check_invalid_math_expr(expr)?;
    check_exhaustive_pattern_match(expr, function_type_registry)?;
    Ok(())
}

#[cfg(test)]
mod type_check_tests {

    mod type_mismatch_errors {
        use test_r::test;

        use crate::type_checker::type_check_tests::internal;
        use crate::type_checker::type_check_tests::internal::strip_spaces;
        use crate::{compile, Expr};

        #[test]
        async fn test_inference_pattern_match_invalid_0() {
            let expr = r#"
          let x: option<u64> = some(1);
          match x {
            some(x) => x,
            none => "none"
          }
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 4, column 24
            `x`
            cause: type mismatch. expected string, found u64
            expected string based on pattern match branch at line 5 column 21
            "#;

            //assert!(false);
            assert_eq!(error_msg, strip_spaces(expected));
        }

        #[test]
        async fn test_inference_pattern_match_invalid_1() {
            let expr = r#"
          let x: option<u64> = some(1);
          match x {
            some(x) => {foo: x},
            none => {foo: "bar"}
          }
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 4, column 24
            `{foo: x}`
            cause: type mismatch. expected string, found u64
            expected string based on pattern match branch at line 5 column 21
            "#;

            //assert!(false);
            assert_eq!(error_msg, strip_spaces(expected));
        }

        #[test]
        async fn test_inference_pattern_match_invalid_2() {
            let expr = r#"
          let x: option<u64> = some(1);
          match x {
            some(x) => ok(1),
            none    => ok("none")
          }
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 4, column 24
            `ok(1)`
            cause: type mismatch. expected string, found s32
            expected string based on pattern match branch at line 5 column 24
            "#;

            //assert!(false);
            assert_eq!(error_msg, strip_spaces(expected));
        }

        #[test]
        async fn test_inference_pattern_match_invalid_3() {
            let expr = r#"
          let x: option<u64> = some(1);
          match x {
            some(x) => ok("none"),
            none    => ok(1)
          }
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 5, column 24
            `ok(1)`
            cause: type mismatch. expected string, found s32
            expected string based on pattern match branch at line 4 column 24
            "#;

            //assert!(false);
            assert_eq!(error_msg, strip_spaces(expected));
        }

        #[test]
        fn test_type_mismatch_in_record_in_function_call1() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: "foo", c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 28
            `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: "foo", c: [1, 2, 3], d: {da: 4}}`
            found within:
            `foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: "foo", c: [1, 2, 3], d: {da: 4}})`
            cause: type mismatch at path: `b`. expected u64
            invalid argument to the function `foo`
            "#;

            assert_eq!(error_msg, strip_spaces(expected));
        }

        #[test]
        fn test_type_mismatch_in_record_in_function_call2() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: ["foo", "bar"], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 28
            `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: ["foo", "bar"], d: {da: 4}}`
            found within:
            `foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: ["foo", "bar"], d: {da: 4}})`
            cause: type mismatch at path: `c`. expected list<s32>
            invalid argument to the function `foo`
            "#;

            assert_eq!(error_msg, strip_spaces(expected));
        }

        #[test]
        fn test_type_mismatch_in_record_in_function_call3() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: [1, 2], d: {da: "foo"}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 28
            `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: [1, 2], d: {da: "foo"}}`
            found within:
            `foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: [1, 2], d: {da: "foo"}})`
            cause: type mismatch at path: `d.da`. expected s32
            invalid argument to the function `foo`
            "#;

            assert_eq!(error_msg, strip_spaces(expected));
        }

        // Here the difference is, the shape itself is different losing the preciseness of the error.
        // The best precise error
        // is type-mismatch, however, here we get an ambiguity error. This can be improved,
        // by not allowing accumulation of conflicting types into Exprs that are part of a function call
        #[test]
        fn test_type_mismatch_in_record_in_function_call4() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: (1, 2), ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: [1, 2], d: {da: 1}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 51
            `(1, 2)`
            cause: ambiguous types: `list<number>`, `tuple<number, number>`
            "#;

            assert_eq!(error_msg, strip_spaces(expected));
        }

        #[test]
        fn test_type_mismatch_in_record_in_function_call5() {
            let expr = r#"
            let x = {a: "foo"};
          let result = foo({a: {aa: 1, ab: 2, ac: x, ad: {ada: 1}, ae: (1, "foo")}, b: 2, c: [1, 2], d: {da: 1}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 21
            `{a: "foo"}`
            cause: ambiguous types: `list<number>`, `record{a: str}`
            "#;

            assert_eq!(error_msg, strip_spaces(expected));
        }

        #[test]
        fn test_type_mismatch_in_nested_record_in_function_call1() {
            let expr = r#"
          let result = foo({a: {aa: "foo", ab: 2, ac: [1, 2], ad: {ada: "1"}, ae: (1, "foo")}, b: 3, c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 28
            `{a: {aa: "foo", ab: 2, ac: [1, 2], ad: {ada: "1"}, ae: (1, "foo")}, b: 3, c: [1, 2, 3], d: {da: 4}}`
            found within:
            `foo({a: {aa: "foo", ab: 2, ac: [1, 2], ad: {ada: "1"}, ae: (1, "foo")}, b: 3, c: [1, 2, 3], d: {da: 4}})`
            cause: type mismatch at path: `a.aa`. expected s32
            invalid argument to the function `foo`
            "#;

            assert_eq!(error_msg, strip_spaces(expected));
        }

        #[test]
        fn test_type_mismatch_in_nested_record_in_function_call2() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: "1"}, ae: (1, "foo")}, b: 3, c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 28
            `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: "1"}, ae: (1, "foo")}, b: 3, c: [1, 2, 3], d: {da: 4}}`
            found within:
            `foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: "1"}, ae: (1, "foo")}, b: 3, c: [1, 2, 3], d: {da: 4}})`
            cause: type mismatch at path: `a.ad.ada`. expected s32
            invalid argument to the function `foo`
            "#;

            assert_eq!(error_msg, strip_spaces(expected));
        }

        #[test]
        fn test_type_mismatch_in_nested_record_in_function_call3() {
            let expr = r#"
            let bar = {a: {aa: 1, ab: 2, ac: 1, ad: {ada: 1}, ae:(1, "foo")}, b: 3, c: [1, 2, 3], d: {da: 4}};
            let result = foo(bar);
            result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 3, column 30
            `bar`
            found within:
            `foo(bar)`
            cause: type mismatch at path: `a.ac`. expected list<s32>
            invalid argument to the function `foo`
            "#;

            assert_eq!(error_msg, strip_spaces(expected));
        }

        #[test]
        fn test_type_mismatch_in_nested_record_in_function_call4() {
            let expr = r#"
          let result = foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3], d: {da: 4}});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_with_record_input_params();

            let error_msg = compile(expr, &metadata).unwrap_err().to_string();

            let expected = r#"
            error in the following rib found at line 2, column 28
            `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3], d: {da: 4}}`
            found within:
            `foo({a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3], d: {da: 4}})`
            cause: type mismatch at path: `a.ae`. expected string
            invalid argument to the function `foo`
            "#;

            assert_eq!(error_msg, strip_spaces(expected));
        }
    }

    mod internal {
        use golem_wasm_ast::analysis::analysed_type::{list, record, s32, str, tuple, u64};
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            NameTypePair,
        };

        pub(crate) fn strip_spaces(input: &str) -> String {
            let lines = input.lines();

            let first_line = lines
                .clone()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("");
            let margin_width = first_line.chars().take_while(|c| c.is_whitespace()).count();

            let result = lines
                .map(|line| {
                    if line.trim().is_empty() {
                        String::new()
                    } else {
                        line[margin_width..].to_string()
                    }
                })
                .collect::<Vec<String>>()
                .join("\n");

            result.strip_prefix("\n").unwrap_or(&result).to_string()
        }

        pub(crate) fn get_metadata_with_record_input_params() -> Vec<AnalysedExport> {
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
