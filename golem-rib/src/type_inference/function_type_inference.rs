use crate::type_registry::{FunctionTypeRegistry, RegistryKey, RegistryValue};
use crate::{Expr, InferredType};
use std::collections::VecDeque;

// At this point we simply update the types to the parameter type expressions and the call expression itself.
pub fn infer_function_types(expr: &mut Expr, function_type_registry: &FunctionTypeRegistry) {
    let mut queue = VecDeque::new();
    queue.push_back(expr);
    // From the end to top
    while let Some(expr) = queue.pop_back() {
        // call(x), let x = 1;
        match expr {
            Expr::Call(parsed_fn_name, args, inferred_type) => {
                let key = RegistryKey::from_invocation_name(parsed_fn_name);
                if let Some(value) = function_type_registry.types.get(&key) {
                    match value {
                        RegistryValue::Value(_) => {}
                        RegistryValue::Function {
                            parameter_types,
                            return_types,
                        } => {
                            if parameter_types.len() == args.len() {
                                for (arg, param_type) in args.iter_mut().zip(parameter_types) {
                                    arg.add_infer_type_mut(param_type.clone().into());
                                    // TODO; Probably not necessary as we push down in a separate phase.
                                    // Tests failing, so keeping it for now.
                                    arg.push_types_down()
                                }
                                *inferred_type = {
                                    if return_types.len() == 1 {
                                        return_types[0].clone().into()
                                    } else {
                                        InferredType::Sequence(
                                            return_types.iter().map(|t| t.clone().into()).collect(),
                                        )
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Continue for nested expressions
            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }


}

#[cfg(test)]
mod function_parameters_inference_tests {
    use crate::type_registry::FunctionTypeRegistry;
    use crate::{Expr, InferredType, InvocationName, ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite, VariableId};
    use golem_wasm_ast::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedType, TypeU32, TypeU64,
    };

    fn get_function_type_registry() -> FunctionTypeRegistry {
        let metadata = vec![
            AnalysedExport::Function(AnalysedFunction {
                name: "foo".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "my_parameter".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                }],
                results: vec![],
            }),
            AnalysedExport::Function(AnalysedFunction {
                name: "baz".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "my_parameter".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                }],
                results: vec![],
            }),
        ];
        FunctionTypeRegistry::from_export_metadata(&metadata)
    }

    #[test]
    fn test_infer_function_types() {
        let rib_expr = r#"
          let x = 1;
          foo(x)
        "#;

        let function_type_registry = get_function_type_registry();

        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_function_types(&function_type_registry);

        let let_binding = Expr::let_binding("x", Expr::number(1f64));

        let call_expr = Expr::Call(
            InvocationName::Function(ParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: ParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            vec![Expr::Identifier(
                VariableId::global("x".to_string()),
                InferredType::U64, // Call argument's types are updated
            )],
            InferredType::Sequence(vec![]), // Call Expressions return type is updated
        );

        let expected = Expr::Multiple(vec![let_binding, call_expr], InferredType::Unknown);

        assert_eq!(expr, expected);
    }
}
