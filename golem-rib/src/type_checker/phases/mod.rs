pub(crate) use type_mismatch_call_args::*;
mod type_mismatch_call_args;

#[cfg(test)]
mod type_check_tests {
    use super::*;
    use crate::{compile, Expr};

    #[test]
    fn test_invalid_value_in_record_field() {
        let expr = r#"
          let result = foo({a: "foo", b: 2});
          result
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let metadata = internal::get_metadata_record_arg();

        let result = compile(&expr, &metadata).unwrap_err();

        let expected = "`foo` has invalid argument `{a: \"foo\", b: 2}`: Invalid type for field `a`\nExpected type `s32` ";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_invalid_key_in_record_field() {
        let expr = r#"
          let result = foo({c: 3, b: 2});
          result
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let metadata = internal::get_metadata_record_arg();

        let result = compile(&expr, &metadata).unwrap_err();

        let expected = "`foo` has invalid argument `{c: 3, b: 2}`: Un-inferred type for field `c` in record\nExpected type `record<a: s32, b: u64>` ";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_invalid_keys_in_record_field() {
        let expr = r#"
          let result = foo({c: 3, d: 2});
          result
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let metadata = internal::get_metadata_record_arg();

        let result = compile(&expr, &metadata).unwrap_err();

        let expected = "`foo` has invalid argument `{c: 3, d: 2}`: Un-inferred type for field `c` in record\nExpected type `record<a: s32, b: u64>` ";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_invalid_value_in_record_nested_field() {
        let expr = r#"
          let result = foo({a: {b: "foo", c: 2}});
          result
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let metadata = internal::get_metadata_nested_record_arg();

        let result = compile(&expr, &metadata).unwrap_err();

        let expected = "Invalid argument in `foo`: `{a: {b: \"foo\", c: 2}}`. Type mismatch for `a.b`. Expected `s32`";
        assert_eq!(result, expected);
    }

    mod internal {
        use golem_wasm_ast::analysis::analysed_type::{record, s32, str, u64};
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            NameTypePair,
        };

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

        pub(crate) fn get_metadata_nested_record_arg() -> Vec<AnalysedExport> {
            let analysed_export = AnalysedExport::Function(AnalysedFunction {
                name: "foo".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "arg1".to_string(),
                    typ: record(vec![
                        NameTypePair {
                            name: "a".to_string(),
                            typ: record(vec![
                                NameTypePair {
                                    name: "b".to_string(),
                                    typ: s32(),
                                },
                                NameTypePair {
                                    name: "c".to_string(),
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
