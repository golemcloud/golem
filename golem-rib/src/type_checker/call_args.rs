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
            _ => expr.visit_children_mut_bottom_up(&mut queue)
        }
    }

    Ok(())
}

mod internal {
    use super::*;
    use crate::call_type::CallType;
    use golem_wasm_ast::analysis::AnalysedType;
    use crate::InferredType;
    use crate::type_refinement::precise_types::{CharType, NumberType, RecordType};
    use crate::type_refinement::TypeRefinement;

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

        for (arg, expected_arg_type) in args.iter_mut().zip(expected_arg_types) {
           validate(expected_arg_type.clone(), &arg.inferred_type()).map_err(|e| format!("Invalid argument in function {}: {}", call_type, e))?;
        }

        Ok(())
    }


    fn validate(expected_analysed_type: AnalysedType, actual_type: &InferredType) -> Result<(), String> {
        match expected_analysed_type {
            AnalysedType::Record(fields) => {
                let resolved = RecordType::refine(&actual_type);

                match resolved {
                    Some(record_type) =>  {
                        for field in fields.fields {
                            let field_name = field.name.clone();
                            let expected_field_type = field.typ.clone();
                            let actual_field_type = record_type.inner_type_by_name(&field_name);
                            validate(expected_field_type, &actual_field_type)?;
                        }

                        Ok(())
                    }

                    None => Err(format!("Expected record type, but got {:?}", actual_type))
                }

            }

            AnalysedType::S32(_) | AnalysedType::U64(_) => {
                dbg!(actual_type.clone());
                let resolved =  NumberType::refine(&actual_type);
                dbg!(resolved.clone());



                if let Some(_) = resolved {
                    Ok(())
                } else {
                    Err(format!("Expected s32 type, but got {:?}", actual_type))
                }
            }


            AnalysedType::Chr(_) => {
                let resolved =  CharType::refine(&actual_type);

                if let Some(_) = resolved {
                    Ok(())
                } else {
                    Err(format!("Expected char type, but got {:?}", actual_type))
                }
            }

            _ => {
                Err(format!("The {:?} not yet supported", actual_type))
            }
        }

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
        use golem_wasm_ast::analysis::{AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedType, NameTypePair, TypeRecord};
        use golem_wasm_ast::analysis::analysed_type::{record, s32, str, u64};

        pub(crate) fn get_metadata() -> Vec<AnalysedExport> {
            let analysed_export = AnalysedExport::Function(
                AnalysedFunction {
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
                }
            );

            vec![analysed_export]
        }
    }
}
