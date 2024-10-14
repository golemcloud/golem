use crate::{Expr, FunctionTypeRegistry, RegistryKey};
use std::collections::VecDeque;

pub fn check_call_args(
    expr: &mut Expr,
    type_registry: &FunctionTypeRegistry,
) -> Result<(), String> {
    let mut queue = VecDeque::new();

    queue.push_back(expr);

    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::Call(call_type, args, ..) => {
                internal::check_call_args(call_type, args, type_registry)?;
            }
            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
}

mod internal {
    use super::*;
    use crate::call_type::CallType;
    use crate::type_checker::{check_type_mismatch, validate};

    pub(crate) fn check_call_args(
        call_type: &mut CallType,
        args: &mut Vec<Expr>,
        type_registry: &FunctionTypeRegistry,
    ) -> Result<(), String> {
        let registry_value = type_registry
            .types
            .get(&RegistryKey::from_call_type(call_type))
            .ok_or(format!(
                "Function {} is not defined in the registry",
                call_type
            ))?;

        let expected_arg_types = registry_value.argument_types();

        let mut filtered_expected_types = expected_arg_types.clone();

        if call_type.is_resource_method() {
            filtered_expected_types.remove(0);
        }

        for (actual_arg, expected_arg_type) in args.iter_mut().zip(filtered_expected_types) {
            let actual_arg_type = &actual_arg.inferred_type();

            validate(&expected_arg_type, actual_arg_type, actual_arg).map_err(|e| {
                format!(
                    "`{}` has invalid argument `{}`: {}",
                    call_type,
                    actual_arg.to_string(),
                    e
                )
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod type_check_tests {
    use super::*;
    use crate::compile;

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

        let expected = "`foo` has invalid argument `{a: \"foo\", b: 2}`: Invalid type for field `a`\nExpected type `s32` ";
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
