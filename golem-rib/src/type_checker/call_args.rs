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
    use crate::type_checker::validate;

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

        for (arg, expected_arg_type) in args.iter_mut().zip(filtered_expected_types) {
            dbg!(arg.clone());
            dbg!(expected_arg_type.clone());
            validate(&expected_arg_type, &arg.inferred_type()).map_err(|e| {
                format!(
                    "`{}` has invalid argument `{}`: {}",
                    call_type,
                    arg.to_string(),
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
    fn test_check_call_args() {
        let expr = r#"
          let result = foo({a: "foo", b: 2});
          result
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let metadata = internal::get_metadata();

        let result = compile(&expr, &metadata);

        dbg!(result.clone());

        assert!(false);
    }

    mod internal {
        use golem_wasm_ast::analysis::analysed_type::{record, s32, str, u64};
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            AnalysedType, NameTypePair, TypeRecord,
        };

        pub(crate) fn get_metadata() -> Vec<AnalysedExport> {
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
    }
}
