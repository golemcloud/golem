pub(crate) use type_mismatch_call_args::*;
mod type_mismatch_call_args;

#[cfg(test)]
mod type_check_tests {
    use crate::{compile, Expr};

    mod unresolved_types_error {
        use crate::{compile, Expr};
        use crate::type_checker::phases::type_check_tests::internal;

        #[test]
        fn test_invalid_key_in_record_in_function_call() {
            let expr = r#"
          let result = foo({c: 3, b: 2});
          result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let metadata = internal::get_metadata_record_arg();

            let result = compile(&expr, &metadata).unwrap_err();

            let expected = "Invalid argument in `foo`: `{c: 3, b: 2}`. Cannot infer the type of `3` in `c`. Expected type: record<a: s32, b: u64>";
            assert_eq!(result, expected);
        }

    }

    mod type_mismatch_errors {
        use crate::{compile, Expr};
        use crate::type_checker::phases::type_check_tests::internal;

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

            let expected = "Invalid argument in `foo`: `{a: {aa: 1, ab: 2, ac: [1, 2], ad: {ada: 1}, ae: (1, 2)}, b: 3, c: [1, 2, 3], d: {da: 4}}`. Type mismatch for `a.ae[1]`. Expected `str`";
            assert_eq!(result, expected);
        }
    }

    mod internal {
        use golem_wasm_ast::analysis::analysed_type::{list, record, s32, str, tuple, u64};
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            NameTypePair,
        };
        use golem_wasm_ast::component::ComponentExternName::Name;

        pub(crate) fn get_metadata_record_arg() -> Vec<AnalysedExport> {
            let analysed_export = AnalysedExport::Function(AnalysedFunction {
                name: "foo".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "arg1".to_string(),
                    typ: record(vec![
                        NameTypePair {
                            name: "a".to_string(),
                            typ: s32(),
                        },
                        NameTypePair {
                            name: "b".to_string(),
                            typ: u64(),
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
                                    typ: record(vec![
                                        NameTypePair {
                                            name: "ada".to_string(),
                                            typ: s32(),
                                        },
                                    ]),
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
                            typ: record(vec![
                                NameTypePair {
                                    name: "da".to_string(),
                                    typ: s32(),
                                },
                            ]),
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
