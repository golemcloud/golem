// Copyright 2024 Golem Cloud
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

pub use enum_resolution::*;
pub use expr_visitor::*;
pub use function_type_inference::*;
pub use global_input_inference::*;
pub use identifier_inference::*;
pub use inference_fix_point::*;
pub use name_binding::*;
pub use pattern_match_binding::*;
pub use rib_input_type::*;
pub(crate) use type_binding::*;
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
mod rib_input_type;
mod type_check;
mod type_pull_up;
mod type_push_down;
mod type_reset;
mod type_unification;
mod variant_resolution;

mod enum_resolution;
mod global_input_inference;
mod inference_fix_point;
mod type_binding;

#[cfg(test)]
mod type_inference_tests {

    mod let_binding_tests {
        use crate::call_type::CallType;
        use crate::type_inference::type_inference_tests::internal;
        use crate::{
            Expr, InferredType, Number, ParsedFunctionName, ParsedFunctionReference,
            ParsedFunctionSite, VariableId,
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
                None,
                Box::new(Expr::Number(
                    Number { value: 1f64 },
                    None,
                    InferredType::U64,
                )), // The number in let expression is identified to be a U64
                InferredType::Unknown, // Type of a let expression can be unit, we are not updating this part
            );

            let call_expr = Expr::Call(
                CallType::Function(ParsedFunctionName {
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
                None,
                Box::new(Expr::Number(
                    Number { value: 1f64 },
                    None,
                    InferredType::U64,
                )),
                InferredType::Unknown,
            );

            let let_binding2 = Expr::Let(
                VariableId::local("y", 0),
                None,
                Box::new(Expr::Number(
                    Number { value: 2f64 },
                    None,
                    InferredType::U32,
                )),
                InferredType::Unknown,
            );

            let call_expr1 = Expr::Call(
                CallType::Function(ParsedFunctionName {
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
            );

            let call_expr2 = Expr::Call(
                CallType::Function(ParsedFunctionName {
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
            );

            let expected = Expr::Multiple(
                vec![let_binding1, let_binding2, call_expr1, call_expr2],
                InferredType::Sequence(vec![]),
            );

            assert_eq!(expr, expected);
        }
    }
    mod literal_tests {
        use crate::parser::type_name::TypeName;
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
                        Some(TypeName::U64),
                        Box::new(Expr::Number(
                            Number { value: 1f64 },
                            None,
                            InferredType::U64,
                        )),
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
                        None,
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
        use crate::parser::type_name::TypeName;
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
                        Some(TypeName::U64),
                        Box::new(Expr::Number(
                            Number { value: 1f64 },
                            None,
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        Some(TypeName::U64),
                        Box::new(Expr::Number(
                            Number { value: 2f64 },
                            None,
                            InferredType::U64,
                        )),
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
    mod enum_tests {
        use crate::type_inference::type_inference_tests::internal;
        use crate::type_inference::type_inference_tests::internal::{
            get_analysed_exports, get_analysed_type_enum,
        };
        use crate::{Expr, FunctionTypeRegistry};

        use golem_wasm_ast::analysis::{AnalysedType, TypeStr};

        #[tokio::test]
        async fn test_enum_construction_and_pattern_match() {
            let input_enum_type = get_analysed_type_enum(vec!["foo", "bar", "foo-bar"]);

            let output_enum_type =
                get_analysed_type_enum(vec!["success", "failure", "in-progress"]);

            let component_metadata = get_analysed_exports(
                "process",
                vec![
                    input_enum_type.clone(),
                    input_enum_type.clone(),
                    input_enum_type.clone(),
                    AnalysedType::Str(TypeStr),
                ],
                output_enum_type.clone(),
            );

            let expr = r#"
              let user: str = request.body.user-id;
              let query1 = foo;
              let query2 = bar;
              let query3 = foo-bar;
              let result = process(query1, query2, query3, user);

              let x = match result {
                success => "success ${user}",
                failure => "failed ${user}",
                in-progress => "in-progress"
              };

               let y = match query2 {
                foo => "y foo ${user}",
                bar => "y bar ${user}",
                foo-bar => "y foo-bar"
              };

              let z = match query3 {
                foo => "z foo ${user}",
                bar => "z bar ${user}",
                foo-bar => "z foo-bar"
              };

              { x: x, y: y, z: z }

            "#;

            let function_type_registry =
                FunctionTypeRegistry::from_export_metadata(&component_metadata);

            let mut expr = Expr::from_text(expr).unwrap();

            expr.infer_types(&function_type_registry).unwrap();

            let expected = internal::expected_expr_for_enum_test();

            assert_eq!(expr, expected);
        }
    }

    mod variant_tests {

        use crate::type_inference::type_inference_tests::internal::{
            get_analysed_exports, get_analysed_type_variant,
        };
        use crate::{Expr, FunctionTypeRegistry};

        use golem_wasm_ast::analysis::{AnalysedType, TypeStr, TypeU64};

        #[tokio::test]
        async fn test_variant_construction_and_pattern_match() {
            let input_variant_type = get_analysed_type_variant(vec![
                ("foo", Some(AnalysedType::U64(TypeU64))),
                ("bar-baz", Some(AnalysedType::Str(TypeStr))),
                ("foo-bar", None),
            ]);

            let output_variant_type = get_analysed_type_variant(vec![
                ("success", Some(AnalysedType::U64(TypeU64))),
                ("in-progress", Some(AnalysedType::Str(TypeStr))),
                ("failure", None),
            ]);

            let component_metadata = get_analysed_exports(
                "process",
                vec![
                    input_variant_type.clone(),
                    input_variant_type.clone(),
                    input_variant_type.clone(),
                    AnalysedType::Str(TypeStr),
                ],
                output_variant_type.clone(),
            );

            let expr = r#"
              let user = request.body.user-id;
              let query1 = foo(user);
              let query2 = bar-baz("jon");
              let query3 = foo-bar;
              let result = process(query1, query2, query3, user);

              let x = match result {
                success(number) => "success ${number}",
                failure => "failed ${user}",
                in-progress(txt) => "in-progress ${txt}"
              };

               let y = match query2 {
                foo(n) => "y foo ${n}",
                bar-baz(n) => "y bar ${n}",
                foo-bar => "y foo-bar"
              };

              let z = match query3 {
                foo(n) => "z foo ${n}",
                bar-baz(n) => "z bar ${n}",
                foo-bar => "z foo-bar"
              };

              { x: x, y: y, z: z }

            "#;

            let function_type_registry =
                FunctionTypeRegistry::from_export_metadata(&component_metadata);

            let mut expr = Expr::from_text(expr).unwrap();

            let result = expr.infer_types(&function_type_registry);

            dbg!(expr.clone());

            assert!(result.is_ok());
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
                        None,
                        Box::new(Expr::literal("1")),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        None,
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
                        None,
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
        use crate::parser::type_name::TypeName;
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
                        Some(TypeName::U64),
                        Box::new(Expr::Number(
                            Number { value: 1f64 },
                            None,
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        Some(TypeName::U64),
                        Box::new(Expr::Number(
                            Number { value: 2f64 },
                            None,
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("res1", 0),
                        None,
                        Box::new(Expr::literal("foo")),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("res2", 0),
                        None,
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
        use crate::parser::type_name::TypeName;
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

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        None,
                        Box::new(Expr::literal("1")),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        None,
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
                        Some(TypeName::U64),
                        Box::new(Expr::Number(
                            Number { value: 1f64 },
                            None,
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        None,
                        Box::new(Expr::Identifier(
                            VariableId::local("x", 0),
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("z", 0),
                        None,
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
        use crate::parser::type_name::TypeName;
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
                        Some(TypeName::List(Box::new(TypeName::U64))),
                        Box::new(Expr::Sequence(
                            vec![
                                Expr::Number(Number { value: 1f64 }, None, InferredType::U64),
                                Expr::Number(Number { value: 2f64 }, None, InferredType::U64),
                                Expr::Number(Number { value: 3f64 }, None, InferredType::U64),
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
        use crate::parser::type_name::TypeName;
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_select_index_type_inference() {
            let rib_expr = r#"
          let x: list<u64> = [1, 2, 3];
          x[0]

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Some(TypeName::List(Box::new(TypeName::U64))),
                        Box::new(Expr::Sequence(
                            vec![
                                Expr::Number(Number { value: 1f64 }, None, InferredType::U64),
                                Expr::Number(Number { value: 2f64 }, None, InferredType::U64),
                                Expr::Number(Number { value: 3f64 }, None, InferredType::U64),
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
        use crate::parser::type_name::TypeName;
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_select_field_type_inference() {
            let rib_expr = r#"
          let n: u64 = 1;
          let x = { foo : n };
          x.foo

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = crate::Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("n", 0),
                        Some(TypeName::U64),
                        Box::new(Expr::Number(
                            Number { value: 1f64 },
                            None,
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("x", 0),
                        None,
                        Box::new(Expr::Record(
                            vec![(
                                "foo".to_string(),
                                Box::new(Expr::Identifier(
                                    VariableId::local("n", 0),
                                    InferredType::U64,
                                )),
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
        use crate::parser::type_name::TypeName;
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, InferredType, Number, VariableId};

        #[test]
        fn test_tuple_type_inference() {
            let rib_expr = r#"
          let x: tuple<u64, str> = (1, "2");
          x

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Some(TypeName::Tuple(vec![TypeName::U64, TypeName::Str])),
                        Box::new(Expr::Tuple(
                            vec![
                                Expr::Number(Number { value: 1f64 }, None, InferredType::U64),
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
        use crate::parser::type_name::TypeName;
        use crate::{
            ArmPattern, Expr, FunctionTypeRegistry, InferredType, MatchArm, Number, VariableId,
        };

        #[test]
        fn test_variable_conflict_case() {
            let expr_str = r#"
              let y: u64 = 1;
              let z = some(y);

              match z {
                 some(z) => y,
                 some(z) => z
              }
            "#;

            let mut expr = Expr::from_text(expr_str).unwrap();

            expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("y", 0),
                        Some(TypeName::U64),
                        Box::new(Expr::Number(
                            Number { value: 1f64 },
                            None,
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("z", 0),
                        None,
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
        use crate::call_type::CallType;
        use crate::parser::type_name::TypeName;
        use crate::type_inference::type_inference_tests::internal;
        use crate::{
            ArmPattern, Expr, FunctionTypeRegistry, InferredType, MatchArm, Number,
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
                None,
                Box::new(Expr::Number(
                    Number { value: 1f64 },
                    None,
                    InferredType::U64,
                )),
                InferredType::Unknown,
            );

            let let_binding2 = Expr::Let(
                VariableId::local("y", 0),
                None,
                Box::new(Expr::Number(
                    Number { value: 2f64 },
                    None,
                    InferredType::U32,
                )),
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
                            None,
                            InferredType::U64,
                        ))),
                        Expr::Call(
                            CallType::Function(ParsedFunctionName {
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
                            None,
                            InferredType::U64, // because predicate is u64
                        ))),
                        Expr::Call(
                            CallType::Function(ParsedFunctionName {
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
        fn test_pattern_match_with_result() {
            let rib_expr = r#"
              let x: u64 = 1;

              match err(x) {
                err(_) => none,
                ok(_) => some(some(x))
              }
            "#;

            let function_type_registry = internal::get_function_type_registry();

            let mut expr = Expr::from_text(rib_expr).unwrap();

            let result = expr.infer_types(&function_type_registry);
            assert!(result.is_ok());
        }

        #[test]
        fn test_pattern_match_with_option() {
            let expr_str = r#"
              let x: u64 = 1;
              let y: u64 = 2;
              match some(x) {
                some(x) => some(some(x)),
                none => some(some(y))
              }
            "#;

            let mut expr = Expr::from_text(expr_str).unwrap();

            expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Some(TypeName::U64),
                        Box::new(Expr::Number(
                            Number { value: 1f64 },
                            None,
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        Some(TypeName::U64),
                        Box::new(Expr::Number(
                            Number { value: 2f64 },
                            None,
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::PatternMatch(
                        Box::new(Expr::Option(
                            Some(Box::new(Expr::Identifier(
                                VariableId::local("x", 0),
                                InferredType::U64,
                            ))),
                            InferredType::Option(Box::new(InferredType::U64)),
                        )),
                        vec![
                            MatchArm::new(
                                ArmPattern::Literal(Box::new(Expr::Option(
                                    Some(Box::new(Expr::Identifier(
                                        VariableId::match_identifier("x".to_string(), 1),
                                        InferredType::U64,
                                    ))),
                                    InferredType::Option(Box::new(InferredType::U64)),
                                ))),
                                Expr::Option(
                                    Some(Box::new(Expr::Option(
                                        Some(Box::new(Expr::Identifier(
                                            VariableId::match_identifier("x".to_string(), 1),
                                            InferredType::U64,
                                        ))),
                                        InferredType::Option(Box::new(InferredType::U64)),
                                    ))),
                                    InferredType::Option(Box::new(InferredType::Option(Box::new(
                                        InferredType::U64,
                                    )))),
                                ),
                            ),
                            MatchArm::new(
                                ArmPattern::Literal(Box::new(Expr::Option(
                                    None,
                                    InferredType::Option(Box::new(InferredType::U64)),
                                ))),
                                Expr::Option(
                                    Some(Box::new(Expr::Option(
                                        Some(Box::new(Expr::Identifier(
                                            VariableId::local("y", 0),
                                            InferredType::U64,
                                        ))),
                                        InferredType::Option(Box::new(InferredType::U64)),
                                    ))),
                                    InferredType::Option(Box::new(InferredType::Option(Box::new(
                                        InferredType::U64,
                                    )))),
                                ),
                            ),
                        ],
                        InferredType::Option(Box::new(InferredType::Option(Box::new(
                            InferredType::U64,
                        )))),
                    ),
                ],
                InferredType::Option(Box::new(InferredType::Option(Box::new(InferredType::U64)))),
            );

            assert_eq!(expr, expected)
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
                        None,
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
                        None,
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
                        None,
                        Box::new(Expr::Record(
                            vec![("foo".to_string(), Box::new(Expr::literal("bar")))],
                            InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        Some(TypeName::List(Box::new(TypeName::U64))),
                        Box::new(Expr::Sequence(
                            vec![
                                Expr::Number(Number { value: 1f64 }, None, InferredType::U64),
                                Expr::Number(Number { value: 2f64 }, None, InferredType::U64),
                                Expr::Number(Number { value: 3f64 }, None, InferredType::U64),
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
        use crate::parser::type_name::TypeName;
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
                        Some(TypeName::Option(Box::new(TypeName::U64))),
                        Box::new(Expr::Option(
                            Some(Box::new(Expr::Number(
                                Number { value: 1f64 },
                                None,
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

        #[test]
        fn test_optional_nested_type_inference() {
            let rib_expr = r#"
          let x: option<u64> = some(1);
          let y = some(x);
          y

          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Some(TypeName::Option(Box::new(TypeName::U64))),
                        Box::new(Expr::Option(
                            Some(Box::new(Expr::Number(
                                Number { value: 1f64 },
                                None,
                                InferredType::U64,
                            ))),
                            InferredType::Option(Box::new(InferredType::U64)),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        None,
                        Box::new(Expr::Option(
                            Some(Box::new(Expr::Identifier(
                                VariableId::local("x", 0),
                                InferredType::Option(Box::new(InferredType::U64)),
                            ))),
                            InferredType::Option(Box::new(InferredType::Option(Box::new(
                                InferredType::U64,
                            )))),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Identifier(
                        VariableId::local("y", 0),
                        InferredType::Option(Box::new(InferredType::Option(Box::new(
                            InferredType::U64,
                        )))),
                    ),
                ],
                InferredType::Option(Box::new(InferredType::Option(Box::new(InferredType::U64)))),
            );

            assert_eq!(expr, expected);
        }
    }
    mod record_tests {

        use crate::parser::type_name::TypeName;
        use crate::type_inference::type_inference_tests::internal;
        use crate::{Expr, FunctionTypeRegistry, InferredType, Number, VariableId};
        use golem_wasm_ast::analysis::{AnalysedType, TypeList, TypeOption, TypeStr};

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
                        Some(TypeName::U64),
                        Box::new(Expr::Number(
                            Number { value: 1f64 },
                            None,
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("x", 0),
                        None,
                        Box::new(Expr::Record(
                            vec![(
                                "foo".to_string(),
                                Box::new(Expr::Identifier(
                                    VariableId::local("number", 0),
                                    InferredType::U64,
                                )),
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

        #[test]
        fn test_record_type_inference_identifier() {
            let rib_expr = r#"
          let x: u64 = if true then 1u64 else 20u64;
          let y = {
             let z = {x: x};
             z
          };
          y
          "#;

            let function_type_registry = internal::get_function_type_registry();
            let mut expr = Expr::from_text(rib_expr).unwrap();
            expr.infer_types(&function_type_registry).unwrap();

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        Some(TypeName::U64),
                        Box::new(Expr::Cond(
                            Box::new(Expr::boolean(true)),
                            Box::new(Expr::Number(
                                Number { value: 1f64 },
                                Some(TypeName::U64),
                                InferredType::U64,
                            )),
                            Box::new(Expr::Number(
                                Number { value: 20f64 },
                                Some(TypeName::U64),
                                InferredType::U64,
                            )),
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        None,
                        Box::new(Expr::Multiple(
                            vec![
                                Expr::Let(
                                    VariableId::local("z", 0),
                                    None,
                                    Box::new(Expr::Record(
                                        vec![(
                                            "x".to_string(),
                                            Box::new(Expr::Identifier(
                                                VariableId::local("x", 0),
                                                InferredType::U64,
                                            )),
                                        )],
                                        InferredType::Record(vec![(
                                            "x".to_string(),
                                            InferredType::U64,
                                        )]),
                                    )),
                                    InferredType::Unknown,
                                ),
                                Expr::Identifier(
                                    VariableId::local("z", 0),
                                    InferredType::Record(vec![(
                                        "x".to_string(),
                                        InferredType::U64,
                                    )]),
                                ),
                            ],
                            InferredType::Record(vec![("x".to_string(), InferredType::U64)]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Identifier(
                        VariableId::local("y", 0),
                        InferredType::Record(vec![("x".to_string(), InferredType::U64)]),
                    ),
                ],
                InferredType::Record(vec![("x".to_string(), InferredType::U64)]),
            );

            assert_eq!(expr, expected);
        }

        #[test]
        fn test_record_type_inference_select_with_function_call() {
            let request_body_type = internal::get_analysed_type_record(vec![
                ("id".to_string(), AnalysedType::Str(TypeStr)),
                ("name".to_string(), AnalysedType::Str(TypeStr)),
                (
                    "titles".to_string(),
                    AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Str(TypeStr)),
                    }),
                ),
                (
                    "address".to_string(),
                    internal::get_analysed_type_record(vec![
                        ("street".to_string(), AnalysedType::Str(TypeStr)),
                        ("city".to_string(), AnalysedType::Str(TypeStr)),
                    ]),
                ),
            ]);

            let worker_response = internal::create_none(Some(&AnalysedType::Str(TypeStr)));

            let request_type = internal::get_analysed_type_record(vec![(
                "body".to_string(),
                request_body_type.clone(),
            )]);

            let return_type = AnalysedType::Option(TypeOption {
                inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
            });

            let component_metadata =
                internal::get_analysed_exports("foo", vec![request_type.clone()], return_type);

            let expr_str = r#"${
              let x = { body : { id: "bId", name: "bName", titles: request.body.titles, address: request.body.address } };
              let result = foo(x);
              match result {  some(value) => "personal-id", none =>  x.body.titles[1] }
            }"#;

            let mut expr = Expr::from_interpolated_str(expr_str).unwrap();

            let function_type_registry =
                FunctionTypeRegistry::from_export_metadata(&component_metadata);

            expr.infer_types(&function_type_registry).unwrap();

            let expected = internal::expected_expr_for_select_index();

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

            let expected = Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("p", 0),
                        None,
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
                        None,
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
        use crate::call_type::CallType;
        use crate::parser::type_name::TypeName;
        use crate::{
            ArmPattern, Expr, FunctionTypeRegistry, InferredType, MatchArm, MatchIdentifier,
            ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite, VariableId,
        };
        use golem_wasm_ast::analysis::TypeVariant;
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            AnalysedType, NameOptionTypePair, NameTypePair, TypeEnum, TypeRecord, TypeU32, TypeU64,
        };
        use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
        use golem_wasm_rpc::protobuf::TypedOption;

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

        pub(crate) fn get_analysed_type_enum(cases: Vec<&str>) -> AnalysedType {
            let type_enum = TypeEnum {
                cases: cases.into_iter().map(|s| s.to_string()).collect(),
            };

            AnalysedType::Enum(type_enum)
        }

        pub(crate) fn get_analysed_type_variant(
            variants: Vec<(&str, Option<AnalysedType>)>,
        ) -> AnalysedType {
            let name_option_pairs = variants
                .into_iter()
                .map(|(name, typ)| NameOptionTypePair {
                    name: name.to_string(),
                    typ,
                })
                .collect::<Vec<_>>();

            AnalysedType::Variant(TypeVariant {
                cases: name_option_pairs,
            })
        }

        pub(crate) fn get_analysed_type_record(
            record_type: Vec<(String, AnalysedType)>,
        ) -> AnalysedType {
            let record = TypeRecord {
                fields: record_type
                    .into_iter()
                    .map(|(name, typ)| NameTypePair { name, typ })
                    .collect(),
            };
            AnalysedType::Record(record)
        }

        pub(crate) fn create_none(typ: Option<&AnalysedType>) -> TypeAnnotatedValue {
            TypeAnnotatedValue::Option(Box::new(TypedOption {
                value: None,
                typ: typ.map(|t| t.into()),
            }))
        }

        pub(crate) fn get_analysed_exports(
            function_name: &str,
            input_types: Vec<AnalysedType>,
            output: AnalysedType,
        ) -> Vec<AnalysedExport> {
            let analysed_function_parameters = input_types
                .into_iter()
                .enumerate()
                .map(|(index, typ)| AnalysedFunctionParameter {
                    name: format!("param{}", index),
                    typ,
                })
                .collect();

            vec![AnalysedExport::Function(AnalysedFunction {
                name: function_name.to_string(),
                parameters: analysed_function_parameters,
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: output,
                }],
            })]
        }

        pub(crate) fn expected_expr_for_enum_test() -> Expr {
            Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("user", 0),
                        Some(TypeName::Str),
                        Box::new(Expr::SelectField(
                            Box::new(Expr::SelectField(
                                Box::new(Expr::Identifier(
                                    VariableId::global("request".to_string()),
                                    InferredType::Record(vec![(
                                        "body".to_string(),
                                        InferredType::Record(vec![(
                                            "user-id".to_string(),
                                            InferredType::Str,
                                        )]),
                                    )]),
                                )),
                                "body".to_string(),
                                InferredType::Record(vec![(
                                    "user-id".to_string(),
                                    InferredType::Str,
                                )]),
                            )),
                            "user-id".to_string(),
                            InferredType::Str,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("query1", 0),
                        None,
                        Box::new(Expr::Call(
                            CallType::EnumConstructor("foo".to_string()),
                            vec![],
                            InferredType::Enum(vec![
                                "foo".to_string(),
                                "bar".to_string(),
                                "foo-bar".to_string(),
                            ]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("query2", 0),
                        None,
                        Box::new(Expr::Call(
                            CallType::EnumConstructor("bar".to_string()),
                            vec![],
                            InferredType::Enum(vec![
                                "foo".to_string(),
                                "bar".to_string(),
                                "foo-bar".to_string(),
                            ]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("query3", 0),
                        None,
                        Box::new(Expr::Call(
                            CallType::EnumConstructor("foo-bar".to_string()),
                            vec![],
                            InferredType::Enum(vec![
                                "foo".to_string(),
                                "bar".to_string(),
                                "foo-bar".to_string(),
                            ]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("result", 0),
                        None,
                        Box::new(Expr::Call(
                            CallType::Function(ParsedFunctionName {
                                site: ParsedFunctionSite::Global,
                                function: ParsedFunctionReference::Function {
                                    function: "process".to_string(),
                                },
                            }),
                            vec![
                                Expr::Identifier(
                                    VariableId::local("query1", 0),
                                    InferredType::Enum(vec![
                                        "foo".to_string(),
                                        "bar".to_string(),
                                        "foo-bar".to_string(),
                                    ]),
                                ),
                                Expr::Identifier(
                                    VariableId::local("query2", 0),
                                    InferredType::Enum(vec![
                                        "foo".to_string(),
                                        "bar".to_string(),
                                        "foo-bar".to_string(),
                                    ]),
                                ),
                                Expr::Identifier(
                                    VariableId::local("query3", 0),
                                    InferredType::Enum(vec![
                                        "foo".to_string(),
                                        "bar".to_string(),
                                        "foo-bar".to_string(),
                                    ]),
                                ),
                                Expr::Identifier(VariableId::local("user", 0), InferredType::Str),
                            ],
                            InferredType::Enum(vec![
                                "success".to_string(),
                                "failure".to_string(),
                                "in-progress".to_string(),
                            ]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("x", 0),
                        None,
                        Box::new(Expr::PatternMatch(
                            Box::new(Expr::Identifier(
                                VariableId::local("result", 0),
                                InferredType::Enum(vec![
                                    "success".to_string(),
                                    "failure".to_string(),
                                    "in-progress".to_string(),
                                ]),
                            )),
                            vec![
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(Expr::Call(
                                        CallType::EnumConstructor("success".to_string()),
                                        vec![],
                                        InferredType::Enum(vec![
                                            "success".to_string(),
                                            "failure".to_string(),
                                            "in-progress".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(Expr::Concat(
                                        vec![
                                            Expr::Literal(
                                                "success ".to_string(),
                                                InferredType::Str,
                                            ),
                                            Expr::Identifier(
                                                VariableId::local("user", 0),
                                                InferredType::Str,
                                            ),
                                        ],
                                        InferredType::Str,
                                    )),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(Expr::Call(
                                        CallType::EnumConstructor("failure".to_string()),
                                        vec![],
                                        InferredType::Enum(vec![
                                            "success".to_string(),
                                            "failure".to_string(),
                                            "in-progress".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(Expr::Concat(
                                        vec![
                                            Expr::Literal("failed ".to_string(), InferredType::Str),
                                            Expr::Identifier(
                                                VariableId::local("user", 0),
                                                InferredType::Str,
                                            ),
                                        ],
                                        InferredType::Str,
                                    )),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(Expr::Call(
                                        CallType::EnumConstructor("in-progress".to_string()),
                                        vec![],
                                        InferredType::Enum(vec![
                                            "success".to_string(),
                                            "failure".to_string(),
                                            "in-progress".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(Expr::Literal(
                                        "in-progress".to_string(),
                                        InferredType::Str,
                                    )),
                                },
                            ],
                            InferredType::Str,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("y", 0),
                        None,
                        Box::new(Expr::PatternMatch(
                            Box::new(Expr::Identifier(
                                VariableId::local("query2", 0),
                                InferredType::Enum(vec![
                                    "foo".to_string(),
                                    "bar".to_string(),
                                    "foo-bar".to_string(),
                                ]),
                            )),
                            vec![
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(Expr::Call(
                                        CallType::EnumConstructor("foo".to_string()),
                                        vec![],
                                        InferredType::Enum(vec![
                                            "foo".to_string(),
                                            "bar".to_string(),
                                            "foo-bar".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(Expr::Concat(
                                        vec![
                                            Expr::Literal("y foo ".to_string(), InferredType::Str),
                                            Expr::Identifier(
                                                VariableId::local("user", 0),
                                                InferredType::Str,
                                            ),
                                        ],
                                        InferredType::Str,
                                    )),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(Expr::Call(
                                        CallType::EnumConstructor("bar".to_string()),
                                        vec![],
                                        InferredType::Enum(vec![
                                            "foo".to_string(),
                                            "bar".to_string(),
                                            "foo-bar".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(Expr::Concat(
                                        vec![
                                            Expr::Literal("y bar ".to_string(), InferredType::Str),
                                            Expr::Identifier(
                                                VariableId::local("user", 0),
                                                InferredType::Str,
                                            ),
                                        ],
                                        InferredType::Str,
                                    )),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(Expr::Call(
                                        CallType::EnumConstructor("foo-bar".to_string()),
                                        vec![],
                                        InferredType::Enum(vec![
                                            "foo".to_string(),
                                            "bar".to_string(),
                                            "foo-bar".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(Expr::Literal(
                                        "y foo-bar".to_string(),
                                        InferredType::Str,
                                    )),
                                },
                            ],
                            InferredType::Str,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("z", 0),
                        None,
                        Box::new(Expr::PatternMatch(
                            Box::new(Expr::Identifier(
                                VariableId::local("query3", 0),
                                InferredType::Enum(vec![
                                    "foo".to_string(),
                                    "bar".to_string(),
                                    "foo-bar".to_string(),
                                ]),
                            )),
                            vec![
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(Expr::Call(
                                        CallType::EnumConstructor("foo".to_string()),
                                        vec![],
                                        InferredType::Enum(vec![
                                            "foo".to_string(),
                                            "bar".to_string(),
                                            "foo-bar".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(Expr::Concat(
                                        vec![
                                            Expr::Literal("z foo ".to_string(), InferredType::Str),
                                            Expr::Identifier(
                                                VariableId::local("user", 0),
                                                InferredType::Str,
                                            ),
                                        ],
                                        InferredType::Str,
                                    )),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(Expr::Call(
                                        CallType::EnumConstructor("bar".to_string()),
                                        vec![],
                                        InferredType::Enum(vec![
                                            "foo".to_string(),
                                            "bar".to_string(),
                                            "foo-bar".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(Expr::Concat(
                                        vec![
                                            Expr::Literal("z bar ".to_string(), InferredType::Str),
                                            Expr::Identifier(
                                                VariableId::local("user", 0),
                                                InferredType::Str,
                                            ),
                                        ],
                                        InferredType::Str,
                                    )),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(Expr::Call(
                                        CallType::EnumConstructor("foo-bar".to_string()),
                                        vec![],
                                        InferredType::Enum(vec![
                                            "foo".to_string(),
                                            "bar".to_string(),
                                            "foo-bar".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(Expr::Literal(
                                        "z foo-bar".to_string(),
                                        InferredType::Str,
                                    )),
                                },
                            ],
                            InferredType::Str,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Record(
                        vec![
                            (
                                "x".to_string(),
                                Box::new(Expr::Identifier(
                                    VariableId::local("x", 0),
                                    InferredType::Str,
                                )),
                            ),
                            (
                                "y".to_string(),
                                Box::new(Expr::Identifier(
                                    VariableId::local("y", 0),
                                    InferredType::Str,
                                )),
                            ),
                            (
                                "z".to_string(),
                                Box::new(Expr::Identifier(
                                    VariableId::local("z", 0),
                                    InferredType::Str,
                                )),
                            ),
                        ],
                        InferredType::Record(vec![
                            ("x".to_string(), InferredType::Str),
                            ("y".to_string(), InferredType::Str),
                            ("z".to_string(), InferredType::Str),
                        ]),
                    ),
                ],
                InferredType::Record(vec![
                    ("x".to_string(), InferredType::Str),
                    ("y".to_string(), InferredType::Str),
                    ("z".to_string(), InferredType::Str),
                ]),
            )
        }

        pub(crate) fn expected_expr_for_select_index() -> Expr {
            Expr::Multiple(
                vec![
                    Expr::Let(
                        VariableId::local("x", 0),
                        None,
                        Box::new(Expr::Record(
                            vec![(
                                "body".to_string(),
                                Box::new(Expr::Record(
                                    vec![
                                        ("id".to_string(), Box::new(Expr::literal("bId"))),
                                        ("name".to_string(), Box::new(Expr::literal("bName"))),
                                        (
                                            "titles".to_string(),
                                            Box::new(Expr::SelectField(
                                                Box::new(Expr::SelectField(
                                                    Box::new(Expr::Identifier(
                                                        VariableId::global("request".to_string()),
                                                        InferredType::Record(vec![(
                                                            "body".to_string(),
                                                            InferredType::Record(vec![
                                                                (
                                                                    "address".to_string(),
                                                                    InferredType::Record(vec![
                                                                        (
                                                                            "street".to_string(),
                                                                            InferredType::Str,
                                                                        ),
                                                                        (
                                                                            "city".to_string(),
                                                                            InferredType::Str,
                                                                        ),
                                                                    ]),
                                                                ),
                                                                (
                                                                    "titles".to_string(),
                                                                    InferredType::List(Box::new(
                                                                        InferredType::Str,
                                                                    )),
                                                                ),
                                                            ]),
                                                        )]),
                                                    )),
                                                    "body".to_string(),
                                                    InferredType::Record(vec![
                                                        (
                                                            "address".to_string(),
                                                            InferredType::Record(vec![
                                                                (
                                                                    "street".to_string(),
                                                                    InferredType::Str,
                                                                ),
                                                                (
                                                                    "city".to_string(),
                                                                    InferredType::Str,
                                                                ),
                                                            ]),
                                                        ),
                                                        (
                                                            "titles".to_string(),
                                                            InferredType::List(Box::new(
                                                                InferredType::Str,
                                                            )),
                                                        ),
                                                    ]),
                                                )),
                                                "titles".to_string(),
                                                InferredType::List(Box::new(InferredType::Str)),
                                            )),
                                        ),
                                        (
                                            "address".to_string(),
                                            Box::new(Expr::SelectField(
                                                Box::new(Expr::SelectField(
                                                    Box::new(Expr::Identifier(
                                                        VariableId::global("request".to_string()),
                                                        InferredType::Record(vec![(
                                                            "body".to_string(),
                                                            InferredType::Record(vec![
                                                                (
                                                                    "address".to_string(),
                                                                    InferredType::Record(vec![
                                                                        (
                                                                            "street".to_string(),
                                                                            InferredType::Str,
                                                                        ),
                                                                        (
                                                                            "city".to_string(),
                                                                            InferredType::Str,
                                                                        ),
                                                                    ]),
                                                                ),
                                                                (
                                                                    "titles".to_string(),
                                                                    InferredType::List(Box::new(
                                                                        InferredType::Str,
                                                                    )),
                                                                ),
                                                            ]),
                                                        )]),
                                                    )),
                                                    "body".to_string(),
                                                    InferredType::Record(vec![
                                                        (
                                                            "address".to_string(),
                                                            InferredType::Record(vec![
                                                                (
                                                                    "street".to_string(),
                                                                    InferredType::Str,
                                                                ),
                                                                (
                                                                    "city".to_string(),
                                                                    InferredType::Str,
                                                                ),
                                                            ]),
                                                        ),
                                                        (
                                                            "titles".to_string(),
                                                            InferredType::List(Box::new(
                                                                InferredType::Str,
                                                            )),
                                                        ),
                                                    ]),
                                                )),
                                                "address".to_string(),
                                                InferredType::Record(vec![
                                                    ("street".to_string(), InferredType::Str),
                                                    ("city".to_string(), InferredType::Str),
                                                ]),
                                            )),
                                        ),
                                    ],
                                    InferredType::Record(vec![
                                        (
                                            "address".to_string(),
                                            InferredType::Record(vec![
                                                ("street".to_string(), InferredType::Str),
                                                ("city".to_string(), InferredType::Str),
                                            ]),
                                        ),
                                        ("id".to_string(), InferredType::Str),
                                        ("name".to_string(), InferredType::Str),
                                        (
                                            "titles".to_string(),
                                            InferredType::List(Box::new(InferredType::Str)),
                                        ),
                                    ]),
                                )),
                            )],
                            InferredType::Record(vec![(
                                "body".to_string(),
                                InferredType::Record(vec![
                                    (
                                        "address".to_string(),
                                        InferredType::Record(vec![
                                            ("street".to_string(), InferredType::Str),
                                            ("city".to_string(), InferredType::Str),
                                        ]),
                                    ),
                                    ("id".to_string(), InferredType::Str),
                                    ("name".to_string(), InferredType::Str),
                                    (
                                        "titles".to_string(),
                                        InferredType::List(Box::new(InferredType::Str)),
                                    ),
                                ]),
                            )]),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Let(
                        VariableId::local("result", 0),
                        None,
                        Box::new(Expr::Call(
                            CallType::Function(ParsedFunctionName {
                                site: ParsedFunctionSite::Global,
                                function: ParsedFunctionReference::Function {
                                    function: "foo".to_string(),
                                },
                            }),
                            vec![Expr::Identifier(
                                VariableId::local("x", 0),
                                InferredType::Record(vec![(
                                    "body".to_string(),
                                    InferredType::Record(vec![
                                        (
                                            "address".to_string(),
                                            InferredType::Record(vec![
                                                ("street".to_string(), InferredType::Str),
                                                ("city".to_string(), InferredType::Str),
                                            ]),
                                        ),
                                        ("id".to_string(), InferredType::Str),
                                        ("name".to_string(), InferredType::Str),
                                        (
                                            "titles".to_string(),
                                            InferredType::List(Box::new(InferredType::Str)),
                                        ),
                                    ]),
                                )]),
                            )],
                            InferredType::Option(Box::new(InferredType::Option(Box::new(
                                InferredType::Str,
                            )))),
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::PatternMatch(
                        Box::new(Expr::Identifier(
                            VariableId::local("result", 0),
                            InferredType::Option(Box::new(InferredType::Option(Box::new(
                                InferredType::Str,
                            )))),
                        )),
                        vec![
                            MatchArm {
                                arm_pattern: ArmPattern::Literal(Box::new(Expr::Option(
                                    Some(Box::new(Expr::Identifier(
                                        VariableId::MatchIdentifier(MatchIdentifier::new(
                                            "value".to_string(),
                                            1,
                                        )),
                                        InferredType::Option(Box::new(InferredType::Str)),
                                    ))),
                                    InferredType::Option(Box::new(InferredType::Option(Box::new(
                                        InferredType::Str,
                                    )))),
                                ))),
                                arm_resolution_expr: Box::new(Expr::literal("personal-id")),
                            },
                            MatchArm {
                                arm_pattern: ArmPattern::Literal(Box::new(Expr::Option(
                                    None,
                                    InferredType::Option(Box::new(InferredType::Option(Box::new(
                                        InferredType::Str,
                                    )))),
                                ))),
                                arm_resolution_expr: Box::new(Expr::SelectIndex(
                                    Box::new(Expr::SelectField(
                                        Box::new(Expr::SelectField(
                                            Box::new(Expr::Identifier(
                                                VariableId::local("x", 0),
                                                InferredType::Record(vec![(
                                                    "body".to_string(),
                                                    InferredType::Record(vec![
                                                        (
                                                            "address".to_string(),
                                                            InferredType::Record(vec![
                                                                (
                                                                    "street".to_string(),
                                                                    InferredType::Str,
                                                                ),
                                                                (
                                                                    "city".to_string(),
                                                                    InferredType::Str,
                                                                ),
                                                            ]),
                                                        ),
                                                        ("id".to_string(), InferredType::Str),
                                                        ("name".to_string(), InferredType::Str),
                                                        (
                                                            "titles".to_string(),
                                                            InferredType::List(Box::new(
                                                                InferredType::Str,
                                                            )),
                                                        ),
                                                    ]),
                                                )]),
                                            )),
                                            "body".to_string(),
                                            InferredType::Record(vec![
                                                (
                                                    "address".to_string(),
                                                    InferredType::Record(vec![
                                                        ("street".to_string(), InferredType::Str),
                                                        ("city".to_string(), InferredType::Str),
                                                    ]),
                                                ),
                                                ("id".to_string(), InferredType::Str),
                                                ("name".to_string(), InferredType::Str),
                                                (
                                                    "titles".to_string(),
                                                    InferredType::List(Box::new(InferredType::Str)),
                                                ),
                                            ]),
                                        )),
                                        "titles".to_string(),
                                        InferredType::List(Box::new(InferredType::Str)),
                                    )),
                                    1,
                                    InferredType::Str,
                                )),
                            },
                        ],
                        InferredType::Str,
                    ),
                ],
                InferredType::Str,
            )
        }
    }
}
