pub use expr_visitor::*;
pub use function_type_inference::*;
pub use identifier_inference::*;
pub use name_binding::*;
pub use pattern_match_binding::*;
pub use refine::*;
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
mod pattern_match_binding;
mod refine;
mod rib_input_type;
mod type_check;
mod type_pull_up;
mod type_push_down;
mod type_reset;
mod type_unification;
mod variant_resolution;

#[cfg(test)]
mod type_inference_tests {

    mod let_binding_tests {
        use crate::type_inference::type_inference_tests::internal;
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

            let function_type_registry = internal::get_function_type_registry();

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

            let expected =
                Expr::Multiple(vec![let_binding, call_expr], InferredType::Sequence(vec![]));

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

            let function_type_registry = internal::get_function_type_registry();

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
                InferredType::Sequence(vec![]),
            );

            assert_eq!(expr, expected);
        }
    }
    mod literal_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_number_literal_type_inference() {
            let rib_expr = r#"
          let x: u64 = 1;
          x

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Number(Number { value: 1f64 }, InferredType::U64)),
                        InferredType::Unknown,
                    ),
                    Expr::Identifier(VariableId::local("x", 0), InferredType::U64),
                ],
                InferredType::U64,
            );

            assert_eq!(expr, expected);
        }

        #[test]
        fn test_string_literal_type_inference() {
            let rib_expr = r#"
          let x = "1";
          x

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::literal("1")),
                        InferredType::Unknown,
                    ),
                    Expr::Identifier(VariableId::local("x", 0), InferredType::Str),
                ],
                InferredType::Str,
            );

            assert_eq!(expr, expected);
        }
    }
    mod comparison_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_comparison_type_inference() {
            let rib_expr = r#"
          let x: u64 = 1;
          let y: u64 = 2;
          x > y;
          x >= y;
          x < y;
          x <= y;
          x == y
          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Number(Number { value: 1f64 }, InferredType::U64)),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        Box::new(Expr::Number(Number { value: 2f64 }, InferredType::U64)),
                        InferredType::Unknown,
                    ),
                    Expr::GreaterThan(
                        Box::new(Expr::Identifier(
                            VariableId::local("x", 0),
                            InferredType::U64,
                        )),
                        Box::new(Expr::Identifier(
                            VariableId::local("y", 0),
                            InferredType::U64,
                        )),
                        InferredType::Bool,
                    ),
                    Expr::GreaterThanOrEqualTo(
                        Box::new(Expr::Identifier(
                            VariableId::local("x", 0),
                            InferredType::U64,
                        )),
                        Box::new(Expr::Identifier(
                            VariableId::local("y", 0),
                            InferredType::U64,
                        )),
                        InferredType::Bool,
                    ),
                    Expr::LessThan(
                        Box::new(Expr::Identifier(
                            VariableId::local("x", 0),
                            InferredType::U64,
                        )),
                        Box::new(Expr::Identifier(
                            VariableId::local("y", 0),
                            InferredType::U64,
                        )),
                        InferredType::Bool,
                    ),
                    Expr::LessThanOrEqualTo(
                        Box::new(Expr::Identifier(
                            VariableId::local("x", 0),
                            InferredType::U64,
                        )),
                        Box::new(Expr::Identifier(
                            VariableId::local("y", 0),
                            InferredType::U64,
                        )),
                        InferredType::Bool,
                    ),
                    Expr::EqualTo(
                        Box::new(Expr::Identifier(
                            VariableId::local("x", 0),
                            InferredType::U64,
                        )),
                        Box::new(Expr::Identifier(
                            VariableId::local("y", 0),
                            InferredType::U64,
                        )),
                        InferredType::Bool,
                    ),
                ],
                InferredType::Bool,
            );

            assert_eq!(expr, expected);
        }
    }
    mod concat_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, VariableId};

        #[test]
        fn test_concat_type_inference() {
            let rib_expr = r#"
          let x = "1";
          let y = "2";
          "${x}${y}"
          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::literal("1")),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        Box::new(Expr::literal("2")),
                        InferredType::Unknown,
                    ),
                    Expr::Concat(
                        vec![
                            Expr::Identifier(VariableId::local("x", 0), InferredType::Str),
                            Expr::Identifier(VariableId::local("y", 0), InferredType::Str),
                        ],
                        InferredType::Str,
                    ),
                ],
                InferredType::Str,
            );

            assert_eq!(expr, expected);
        }
    }
    mod boolean_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, VariableId};

        #[test]
        fn test_boolean_literal_type_inference() {
            let rib_expr = r#"
          let x = true;
          x

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::boolean(true)),
                        InferredType::Unknown,
                    ),
                    Expr::Identifier(VariableId::local("x", 0), InferredType::Bool),
                ],
                InferredType::Bool,
            );

            assert_eq!(expr, expected);
        }
    }
    mod cond_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_cond_type_inference() {
            let rib_expr = r#"
          let x: u64 = 1;
          let y: u64 = 2;
          let res1 = "foo";
          let res2 = "bar";
          if x > y then res1 else res2
          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Number(Number { value: 1f64 }, InferredType::U64)),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        Box::new(Expr::Number(Number { value: 2f64 }, InferredType::U64)),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("res1", 0),
                        Box::new(Expr::literal("foo")),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("res2", 0),
                        Box::new(Expr::literal("bar")),
                        InferredType::Unknown,
                    ),
                    Expr::Cond(
                        Box::new(Expr::GreaterThan(
                            Box::new(Expr::Identifier(
                                VariableId::local("x", 0),
                                InferredType::U64,
                            )),
                            Box::new(Expr::Identifier(
                                VariableId::local("y", 0),
                                InferredType::U64,
                            )),
                            InferredType::Bool,
                        )),
                        Box::new(Expr::Identifier(
                            VariableId::local("res1", 0),
                            InferredType::Str,
                        )),
                        Box::new(Expr::Identifier(
                            VariableId::local("res2", 0),
                            InferredType::Str,
                        )),
                        InferredType::Str,
                    ),
                ],
                InferredType::Str,
            );

            assert_eq!(expr, expected);
        }
    }
    mod identifier_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_identifier_type_inference() {
            let rib_expr = r#"
          let x = "1";
          let y = x;
          y

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            dbg!(expr.clone());

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::literal("1")),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        Box::new(Expr::Identifier(
                            VariableId::local("x", 0),
                            InferredType::Str,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Identifier(VariableId::local("y", 0), InferredType::Str),
                ],
                InferredType::Str,
            );

            assert_eq!(expr, expected);
        }

        #[test]
        fn test_identifier_type_inference_multiple_re_assign() {
            let rib_expr = r#"
          let x: u64 = 1;
          let y = x;
          let z = y;
          z

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Number(Number { value: 1f64 }, InferredType::U64)),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        Box::new(Expr::Identifier(
                            VariableId::local("x", 0),
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("z", 0),
                        Box::new(Expr::Identifier(
                            VariableId::local("y", 0),
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Identifier(VariableId::local("z", 0), InferredType::U64),
                ],
                InferredType::U64,
            );

            assert_eq!(expr, expected);
        }
    }
    mod list_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_list_type_inference() {
            let rib_expr = r#"
          let x: list<u64> = [1, 2, 3];
          x

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Sequence(
                            vec![
                                Expr::Number(Number { value: 1f64 }, InferredType::U64),
                                Expr::Number(Number { value: 2f64 }, InferredType::U64),
                                Expr::Number(Number { value: 3f64 }, InferredType::U64),
                            ],
                            InferredType::List(Box::new(InferredType::U64)),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Identifier(
                        VariableId::local("x", 0),
                        InferredType::List(Box::new(InferredType::U64)),
                    ),
                ],
                InferredType::List(Box::new(InferredType::U64)),
            );

            assert_eq!(expr, expected);
        }
    }
    mod select_index_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_select_index_type_inference() {
            let rib_expr = r#"
          let x = [1, 2, 3];
          x[0]

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Sequence(
                            vec![
                                Expr::Number(Number { value: 1f64 }, InferredType::U64),
                                Expr::Number(Number { value: 2f64 }, InferredType::U64),
                                Expr::Number(Number { value: 3f64 }, InferredType::U64),
                            ],
                            InferredType::List(Box::new(InferredType::U64)),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::SelectIndex(
                        Box::new(Expr::Identifier(
                            VariableId::local("x", 0),
                            InferredType::List(Box::new(InferredType::U64)),
                        )),
                        0,
                        InferredType::U64,
                    ),
                ],
                InferredType::U64,
            );

            assert_eq!(expr, expected);
        }
    }
    mod select_field_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_select_field_type_inference() {
            let rib_expr = r#"
          let x = { foo : 1 };
          x.foo

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = crate::Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Record(
                            vec![(
                                "foo".to_string(),
                                Box::new(Expr::Number(Number { value: 1f64 }, InferredType::U64)),
                            )],
                            InferredType::Record(vec![("foo".to_string(), InferredType::U64)]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::SelectField(
                        Box::new(Expr::Identifier(
                            VariableId::local("x", 0),
                            InferredType::Record(vec![("foo".to_string(), InferredType::U64)]),
                        )),
                        "foo".to_string(),
                        InferredType::U64,
                    ),
                ],
                InferredType::U64,
            );

            assert_eq!(expr, expected);
        }
    }
    mod tuple_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_tuple_type_inference() {
            let rib_expr = r#"
          let x = (1, "2");
          x

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Tuple(
                            vec![
                                Expr::Number(Number { value: 1f64 }, InferredType::U64),
                                Expr::literal("2"),
                            ],
                            InferredType::Tuple(vec![InferredType::U64, InferredType::Str]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Identifier(
                        VariableId::local("x", 0),
                        InferredType::Tuple(vec![InferredType::U64, InferredType::Str]),
                    ),
                ],
                InferredType::Tuple(vec![InferredType::U64, InferredType::Str]),
            );

            assert_eq!(expr, expected);
        }
    }
    mod variable_conflict_tests {
        use crate::{
            ArmPattern, Expr, FunctionTypeRegistry, InferredType, MatchArm, Number, VariableId,
        };

        #[test]
        fn test_variable_conflict_case() {
            let expr_str = r#"
              let y = 1;
              let z = some(y);

              match z {
                 some(z) => y,
                 some(z) => z
              }
            "#;

            let mut expr = Expr::from_text(expr_str).unwrap();

            expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();

            dbg!(expr.clone());
            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("y", 0),
                        Box::new(Expr::Number(Number { value: 1f64 }, InferredType::U64)),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("z", 0),
                        Box::new(Expr::Option(
                            Some(Box::new(Expr::Identifier(
                                VariableId::local("y", 0),
                                InferredType::U64,
                            ))),
                            InferredType::Option(Box::new(InferredType::U64)),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::PatternMatch(
                        Box::new(Expr::Identifier(
                            VariableId::local("z", 0),
                            InferredType::Option(Box::new(InferredType::U64)),
                        )),
                        vec![
                            MatchArm::new(
                                ArmPattern::Literal(Box::new(Expr::Option(
                                    Some(Box::new(Expr::Identifier(
                                        VariableId::match_identifier("z".to_string(), 1),
                                        InferredType::U64,
                                    ))),
                                    InferredType::Option(Box::new(InferredType::U64)),
                                ))),
                                Expr::Identifier(VariableId::local("y", 0), InferredType::U64),
                            ),
                            MatchArm::new(
                                ArmPattern::Literal(Box::new(Expr::Option(
                                    Some(Box::new(Expr::Identifier(
                                        VariableId::match_identifier("z".to_string(), 2),
                                        InferredType::U64,
                                    ))),
                                    InferredType::Option(Box::new(InferredType::U64)),
                                ))),
                                Expr::Identifier(
                                    VariableId::match_identifier("z".to_string(), 2),
                                    InferredType::U64,
                                ),
                            ),
                        ],
                        InferredType::U64,
                    ),
                ],
                InferredType::U64,
            );

            assert_eq!(expr, expected)
        }
    }
    mod pattern_match_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{
            ArmPattern, Expr, FunctionTypeRegistry, InferredType, InvocationName, MatchArm, Number,
            ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite, VariableId,
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

            let function_type_registry = internal::get_function_type_registry();
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
                InferredType::Sequence(vec![]),
            );

            assert_eq!(expr, expected);
        }

        #[test]
        fn test_pattern_match_with_record() {
            let expr_str = r#"
              let x = { foo : "bar" };
              match some(x) {
                some(x) => x
              }
            "#;

            let mut expr = Expr::from_text(expr_str).unwrap();

            expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Record(
                            vec![("foo".to_string(), Box::new(Expr::literal("bar")))],
                            InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::PatternMatch(
                        Box::new(Expr::Option(
                            Some(Box::new(Expr::Identifier(
                                VariableId::local("x", 0),
                                InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                            ))),
                            InferredType::Option(Box::new(InferredType::Record(vec![(
                                "foo".to_string(),
                                InferredType::Str,
                            )]))),
                        )),
                        vec![MatchArm::new(
                            ArmPattern::Literal(Box::new(Expr::Option(
                                Some(Box::new(Expr::Identifier(
                                    VariableId::match_identifier("x".to_string(), 1),
                                    InferredType::Record(vec![(
                                        "foo".to_string(),
                                        InferredType::Str,
                                    )]),
                                ))),
                                InferredType::Option(Box::new(InferredType::Record(vec![(
                                    "foo".to_string(),
                                    InferredType::Str,
                                )]))),
                            ))),
                            Expr::Identifier(
                                VariableId::match_identifier("x".to_string(), 1),
                                InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                            ),
                        )],
                        InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                    ),
                ],
                InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
            );

            assert_eq!(expr, expected)
        }

        #[test]
        fn test_pattern_match_with_record_with_select_field() {
            let expr_str = r#"
              let x = { foo : "bar" };
              match some(x) {
                some(x) => x.foo
              }
            "#;

            let mut expr = Expr::from_text(expr_str).unwrap();

            expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Record(
                            vec![("foo".to_string(), Box::new(Expr::literal("bar")))],
                            InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::PatternMatch(
                        Box::new(Expr::Option(
                            Some(Box::new(Expr::Identifier(
                                VariableId::local("x", 0),
                                InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                            ))),
                            InferredType::Option(Box::new(InferredType::Record(vec![(
                                "foo".to_string(),
                                InferredType::Str,
                            )]))),
                        )),
                        vec![MatchArm::new(
                            ArmPattern::Literal(Box::new(Expr::Option(
                                Some(Box::new(Expr::Identifier(
                                    VariableId::match_identifier("x".to_string(), 1),
                                    InferredType::Record(vec![(
                                        "foo".to_string(),
                                        InferredType::Str,
                                    )]),
                                ))),
                                InferredType::Option(Box::new(InferredType::Record(vec![(
                                    "foo".to_string(),
                                    InferredType::Str,
                                )]))),
                            ))),
                            Expr::SelectField(
                                Box::new(Expr::Identifier(
                                    VariableId::match_identifier("x".to_string(), 1),
                                    InferredType::Record(vec![(
                                        "foo".to_string(),
                                        InferredType::Str,
                                    )]),
                                )),
                                "foo".to_string(),
                                InferredType::Str,
                            ),
                        )],
                        InferredType::Str,
                    ),
                ],
                InferredType::Str,
            );

            assert_eq!(expr, expected)
        }

        #[test]
        fn test_pattern_match_with_record_with_select_index() {
            let expr_str = r#"
              let x = { foo : "bar" };
              let y: list<u64> = [1, 2, 3];

              match some(x) {
                some(x) => x.foo
              };

              match some(y) {
                 some(y) => y[0]
              }
            "#;

            let mut expr = Expr::from_text(expr_str).unwrap();

            expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Record(
                            vec![("foo".to_string(), Box::new(Expr::literal("bar")))],
                            InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        Box::new(Expr::Sequence(
                            vec![
                                Expr::Number(Number { value: 1f64 }, InferredType::U64),
                                Expr::Number(Number { value: 2f64 }, InferredType::U64),
                                Expr::Number(Number { value: 3f64 }, InferredType::U64),
                            ],
                            InferredType::List(Box::new(InferredType::U64)),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::PatternMatch(
                        Box::new(Expr::Option(
                            Some(Box::new(Expr::Identifier(
                                VariableId::local("x", 0),
                                InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                            ))),
                            InferredType::Option(Box::new(InferredType::Record(vec![(
                                "foo".to_string(),
                                InferredType::Str,
                            )]))),
                        )),
                        vec![MatchArm::new(
                            ArmPattern::Literal(Box::new(Expr::Option(
                                Some(Box::new(Expr::Identifier(
                                    VariableId::match_identifier("x".to_string(), 1),
                                    InferredType::Record(vec![(
                                        "foo".to_string(),
                                        InferredType::Str,
                                    )]),
                                ))),
                                InferredType::Option(Box::new(InferredType::Record(vec![(
                                    "foo".to_string(),
                                    InferredType::Str,
                                )]))),
                            ))),
                            Expr::SelectField(
                                Box::new(Expr::Identifier(
                                    VariableId::match_identifier("x".to_string(), 1),
                                    InferredType::Record(vec![(
                                        "foo".to_string(),
                                        InferredType::Str,
                                    )]),
                                )),
                                "foo".to_string(),
                                InferredType::Str,
                            ),
                        )],
                        InferredType::Str,
                    ),
                    Expr::PatternMatch(
                        Box::new(Expr::Option(
                            Some(Box::new(Expr::Identifier(
                                VariableId::local("y", 0),
                                InferredType::List(Box::new(InferredType::U64)),
                            ))),
                            InferredType::Option(Box::new(InferredType::List(Box::new(
                                InferredType::U64,
                            )))),
                        )),
                        vec![MatchArm::new(
                            ArmPattern::Literal(Box::new(Expr::Option(
                                Some(Box::new(Expr::Identifier(
                                    VariableId::match_identifier("y".to_string(), 2),
                                    InferredType::List(Box::new(InferredType::U64)),
                                ))),
                                InferredType::Option(Box::new(InferredType::List(Box::new(
                                    InferredType::U64,
                                )))),
                            ))),
                            Expr::SelectIndex(
                                Box::new(Expr::Identifier(
                                    VariableId::match_identifier("y".to_string(), 2),
                                    InferredType::List(Box::new(InferredType::U64)),
                                )),
                                0,
                                InferredType::U64,
                            ),
                        )],
                        InferredType::U64,
                    ),
                ],
                InferredType::U64,
            );

            assert_eq!(expr, expected)
        }
    }
    mod option_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_option_type_inference() {
            let rib_expr = r#"
          let x: option<u64> = some(1);
          x

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Option(
                            Some(Box::new(Expr::Number(
                                Number { value: 1f64 },
                                InferredType::U64,
                            ))),
                            InferredType::Option(Box::new(InferredType::U64)),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Identifier(
                        VariableId::local("x", 0),
                        InferredType::Option(Box::new(InferredType::U64)),
                    ),
                ],
                InferredType::Option(Box::new(InferredType::U64)),
            );

            assert_eq!(expr, expected);
        }
    }
    mod record_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_record_type_inference() {
            let rib_expr = r#"
          let number: u64 = 1;
          let x = { foo : number };
          x

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("number", 0),
                        Box::new(Expr::Number(Number { value: 1f64 }, InferredType::U64)),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("x", 0),
                        Box::new(Expr::Record(
                            vec![(
                                "foo".to_string(),
                                Box::new(Expr::Identifier( VariableId::local("number", 0), InferredType::U64)),
                            )],
                            InferredType::Record(vec![("foo".to_string(), InferredType::U64)]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Identifier(
                        VariableId::local("x", 0),
                        InferredType::Record(vec![("foo".to_string(), InferredType::U64)]),
                    ),
                ],
                InferredType::Record(vec![("foo".to_string(), InferredType::U64)]),
            );

            assert_eq!(expr, expected);
        }
    }
    mod result_type_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, VariableId};

        #[test]
        fn test_result_type_inference() {
            let rib_expr = r#"
          let p = err("foo");
          let q = ok("bar");
          { a : p, b: q }
          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            dbg!(expr.clone());

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("p", 0),
                        Box::new(Expr::Result(
                            Err(Box::new(Expr::literal("foo"))),
                            InferredType::Result {
                                ok: Some(Box::new(InferredType::Unknown)),
                                error: Some(Box::new(InferredType::Str)),
                            },
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("q", 0),
                        Box::new(Expr::Result(
                            Ok(Box::new(Expr::literal("bar"))),
                            InferredType::Result {
                                ok: Some(Box::new(InferredType::Str)),
                                error: Some(Box::new(InferredType::Unknown)),
                            },
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Record(
                        vec![
                            (
                                "a".to_string(),
                                Box::new(Expr::Identifier(
                                    VariableId::local("p", 0),
                                    InferredType::Result {
                                        ok: Some(Box::new(InferredType::Unknown)),
                                        error: Some(Box::new(InferredType::Str)),
                                    },
                                )),
                            ),
                            (
                                "b".to_string(),
                                Box::new(Expr::Identifier(
                                    VariableId::local("q", 0),
                                    InferredType::Result {
                                        ok: Some(Box::new(InferredType::Str)),
                                        error: Some(Box::new(InferredType::Unknown)),
                                    },
                                )),
                            ),
                        ],
                        InferredType::Record(vec![
                            (
                                "a".to_string(),
                                InferredType::Result {
                                    ok: Some(Box::new(InferredType::Unknown)),
                                    error: Some(Box::new(InferredType::Str)),
                                },
                            ),
                            (
                                "b".to_string(),
                                InferredType::Result {
                                    ok: Some(Box::new(InferredType::Str)),
                                    error: Some(Box::new(InferredType::Unknown)),
                                },
                            ),
                        ]),
                    ),
                ],
                InferredType::Record(vec![
                    (
                        "a".to_string(),
                        InferredType::Result {
                            ok: Some(Box::new(InferredType::Unknown)),
                            error: Some(Box::new(InferredType::Str)),
                        },
                    ),
                    (
                        "b".to_string(),
                        InferredType::Result {
                            ok: Some(Box::new(InferredType::Str)),
                            error: Some(Box::new(InferredType::Unknown)),
                        },
                    ),
                ]),
            );

            assert_eq!(expr, expected);
        }
    }
    mod internal {
        use crate::FunctionTypeRegistry;
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedType, TypeU32,
            TypeU64,
        };

        pub(crate) fn get_function_type_registry() -> FunctionTypeRegistry {
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
    }
}
