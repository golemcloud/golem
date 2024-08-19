pub use expr_visitor::*;
pub use function_type_inference::*;
pub use identifier_inference::*;
pub use name_binding::*;
pub use rib_input_type::*;
pub use type_check::*;
pub use type_pull_up::*;
pub use type_push_down::*;
pub use type_reset::*;
pub use type_unification::*;
pub use variant_resolution::*;

mod expr_visitor;
mod function_type_inference;
mod identifier_inference;
mod name_binding;
mod rib_input_type;
mod type_check;
mod type_pull_up;
mod type_push_down;
mod type_reset;
mod type_unification;
mod variant_resolution;

#[cfg(test)]
mod type_inference_tests {
    use crate::type_registry::FunctionTypeRegistry;
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
    mod let_binding_tests {
        use crate::type_inference::type_inference_tests::get_function_type_registry;
        use crate::{
            Expr, InferredType, InvocationName, Number, ParsedFunctionName,
            ParsedFunctionReference, ParsedFunctionSite, VariableId,
        };

        #[test]
        fn test_simple_let_binding_type_inference() {
            let rib_expr = r#"
          let x = 1;
          foo(x)
        "#;

            let function_type_registry = get_function_type_registry();

            let mut expr = Expr::from_text(rib_expr).unwrap();

            expr.infer_types(&function_type_registry).unwrap();

            let let_binding = Expr::Let(
                VariableId::local("x", 0),
                Box::new(Expr::Number(Number { value: 1f64 }, InferredType::U64)), // The number in let expression is identified to be a U64
                InferredType::Unknown, // Type of a let expression can be unit, we are not updating this part
            );

            let call_expr = Expr::Call(
                InvocationName::Function(ParsedFunctionName {
                    site: ParsedFunctionSite::Global,
                    function: ParsedFunctionReference::Function {
                        function: "foo".to_string(),
                    },
                }),
                vec![Expr::Identifier(
                    VariableId::local("x", 0),
                    InferredType::U64, // Variable identified to be a U64
                )],
                InferredType::Sequence(vec![]),
            );

            let expected = Expr::Multiple(
                vec![let_binding, call_expr],
                InferredType::Unknown, // TODO; we could update the type of the total expression easily - simply the type of the last expression
            );

            assert_eq!(expr, expected);
        }

        #[test]
        fn test_multiple_let_binding_expressions() {
            let rib_expr = r#"
          let x = 1;
          let y = 2;
          foo(x);
          baz(y)
        "#;

            let function_type_registry = get_function_type_registry();

            let mut expr = Expr::from_text(rib_expr).unwrap();

            expr.infer_types(&function_type_registry).unwrap();

            let let_binding1 = Expr::Let(
                VariableId::local("x", 0),
                Box::new(Expr::Number(Number { value: 1f64 }, InferredType::U64)), // The number in let expression is identified to be a U64
                InferredType::Unknown, // Type of a let expression can be unit, we are not updating this part
            );

            let let_binding2 = Expr::Let(
                VariableId::local("y", 0),
                Box::new(Expr::Number(Number { value: 2f64 }, InferredType::U32)), // The number in let expression is identified to be a U64
                InferredType::Unknown, // Type of a let expression can be unit, we are not updating this part
            );

            let call_expr1 = Expr::Call(
                InvocationName::Function(ParsedFunctionName {
                    site: ParsedFunctionSite::Global,
                    function: ParsedFunctionReference::Function {
                        function: "foo".to_string(),
                    },
                }),
                vec![Expr::Identifier(
                    VariableId::local("x", 0),
                    InferredType::U64, // Variable identified to be a U64
                )],
                InferredType::Sequence(vec![]),
            );

            let call_expr2 = Expr::Call(
                InvocationName::Function(ParsedFunctionName {
                    site: ParsedFunctionSite::Global,
                    function: ParsedFunctionReference::Function {
                        function: "baz".to_string(),
                    },
                }),
                vec![Expr::Identifier(
                    VariableId::local("y", 0),
                    InferredType::U32, // Variable identified to be a U64
                )],
                InferredType::Sequence(vec![]),
            );

            let expected = Expr::Multiple(
                vec![let_binding1, let_binding2, call_expr1, call_expr2],
                InferredType::Unknown,
            );

            assert_eq!(expr, expected);
        }
    }

    mod pattern_match_tests {
        use crate::type_inference::type_inference_tests::get_function_type_registry;
        use crate::{
            ArmPattern, Expr, InferredType, InvocationName, MatchArm, Number, ParsedFunctionName,
            ParsedFunctionReference, ParsedFunctionSite, VariableId,
        };

        #[test]
        fn test_simple_pattern_match_type_inference() {
            let rib_expr = r#"
                let x = 1;
                let y = 2;

                match x {
                  1 => foo(x),
                  2 => baz(y)
                }"#;

            let function_type_registry = get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let let_binding1 = Expr::Let(
                VariableId::local("x", 0),
                Box::new(Expr::Number(Number { value: 1f64 }, InferredType::U64)),
                InferredType::Unknown,
            );

            let let_binding2 = Expr::Let(
                VariableId::local("y", 0),
                Box::new(Expr::Number(Number { value: 2f64 }, InferredType::U32)),
                InferredType::Unknown,
            );

            let match_expr_expected = Expr::PatternMatch(
                Box::new(Expr::Identifier(
                    VariableId::local("x", 0),
                    InferredType::U64,
                )),
                vec![
                    MatchArm::new(
                        ArmPattern::Literal(Box::new(Expr::Number(
                            Number { value: 1f64 },
                            InferredType::U64,
                        ))),
                        Expr::Call(
                            InvocationName::Function(ParsedFunctionName {
                                site: ParsedFunctionSite::Global,
                                function: ParsedFunctionReference::Function {
                                    function: "foo".to_string(),
                                },
                            }),
                            vec![Expr::Identifier(
                                VariableId::local("x", 0),
                                InferredType::U64,
                            )],
                            InferredType::Sequence(vec![]),
                        ),
                    ),
                    MatchArm::new(
                        ArmPattern::Literal(Box::new(Expr::Number(
                            Number { value: 2f64 },
                            InferredType::U64, // because predicate is u64
                        ))),
                        Expr::Call(
                            InvocationName::Function(ParsedFunctionName {
                                site: ParsedFunctionSite::Global,
                                function: ParsedFunctionReference::Function {
                                    function: "baz".to_string(),
                                },
                            }),
                            vec![Expr::Identifier(
                                VariableId::local("y", 0),
                                InferredType::U32,
                            )],
                            InferredType::Sequence(vec![]),
                        ),
                    ),
                ],
                InferredType::Sequence(vec![]),
            );

            let expected = Expr::Multiple(
                vec![let_binding1, let_binding2, match_expr_expected],
                InferredType::Unknown,
            );

            assert_eq!(expr, expected);
        }
    }
}
