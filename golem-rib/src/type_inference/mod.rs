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

pub use call_arguments_inference::*;
pub use enum_inference::*;
pub use errors::*;
pub use expr_visitor::*;
pub use global_input_inference::*;
pub use global_variable_type_binding::*;
pub use identifier_inference::*;
pub use identify_instance_creation::*;
pub use index_selection_type_binding::*;
pub use infer_orphan_literals::*;
pub use inference_fix_point::*;
pub use inferred_expr::*;
pub use instance_type_binding::*;
pub use rib_input_type::*;
pub use rib_output_type::*;
pub use type_annotation_binding::*;
pub use type_hint::*;
pub use type_pull_up::*;
pub use type_push_down::*;
pub use type_reset::*;
pub use type_unification::*;
pub use variable_binding::*;
pub use variant_inference::*;
pub use worker_function_invocation::*;

mod call_arguments_inference;
mod enum_inference;
mod errors;
mod expr_visitor;
mod global_input_inference;
mod global_variable_type_binding;
mod identifier_inference;
mod identify_instance_creation;
mod index_selection_type_binding;
mod infer_orphan_literals;
mod inference_fix_point;
mod inferred_expr;
mod instance_type_binding;
mod rib_input_type;
mod rib_output_type;
mod type_annotation_binding;
mod type_hint;
mod type_pull_up;
mod type_push_down;
mod type_reset;
mod type_unification;
mod variable_binding;
mod variant_inference;
mod worker_function_invocation;

#[cfg(test)]
mod tests {
    use crate::call_type::CallType;
    use crate::type_checker::Path;
    use crate::type_inference::global_variable_type_binding::GlobalVariableTypeSpec;
    use crate::type_inference::tests::test_utils::{
        call, concat, cond, equal_to, expr_block, get_analysed_exports, get_analysed_type_enum,
        get_analysed_type_variant, greater_than, greater_than_or_equal_to, identifier, less_than,
        less_than_or_equal_to, let_binding, number, option, pattern_match, plus, record, result,
        select_dynamic, select_field, sequence, tuple,
    };
    use crate::{
        ArmPattern, DynamicParsedFunctionName, DynamicParsedFunctionReference, Expr,
        FunctionTypeRegistry, InferredExpr, InferredType, MatchArm, Number, ParsedFunctionSite,
        TypeName, VariableId,
    };
    use bigdecimal::BigDecimal;
    use golem_wasm_ast::analysis::analysed_type::{list, str, u64};

    use test_r::test;

    #[test]
    fn test_inference_global_variable_1() {
        let rib_expr = r#"
             let res = foo;
             res
            "#;

        let mut expr = Expr::from_text(rib_expr).unwrap();
        let type_spec =
            GlobalVariableTypeSpec::new("foo", Path::from_elems(vec![]), InferredType::Str);

        let with_type_spec = expr.infer_types(&FunctionTypeRegistry::empty(), &vec![type_spec]);

        assert!(with_type_spec.is_ok());

        let mut new_expr = Expr::from_text(rib_expr).unwrap();
        let without_type_spec = new_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(without_type_spec.is_err())
    }

    #[test]
    fn test_inference_global_variable_2() {
        let rib_expr = r#"
             let res = request.path.user-id;
             let hello: u64 = request.path.number;
             hello
            "#;

        let mut expr = Expr::from_text(rib_expr).unwrap();
        let type_spec = GlobalVariableTypeSpec::new(
            "request",
            Path::from_elems(vec!["path"]),
            InferredType::Str,
        );

        assert!(expr
            .infer_types(&FunctionTypeRegistry::empty(), &vec![type_spec])
            .is_ok());
    }

    #[test]
    fn test_inference_global_variable_3() {
        let rib_expr = r#"
             let res1 = request.path.user-id;
             let res2 = request.headers.name;
             let res3 = request.headers.age;
             "${res1}-${res2}-${res3}"
            "#;

        let mut expr = Expr::from_text(rib_expr).unwrap();
        let type_spec = vec![
            GlobalVariableTypeSpec::new(
                "request",
                Path::from_elems(vec!["path"]),
                InferredType::Str,
            ),
            GlobalVariableTypeSpec::new(
                "request",
                Path::from_elems(vec!["headers"]),
                InferredType::Str,
            ),
        ];

        assert!(expr
            .infer_types(&FunctionTypeRegistry::empty(), &type_spec)
            .is_ok());
    }

    #[test]
    fn test_inference_inline_type_annotation() {
        let mut old = Expr::from_text(r#"1u32"#).unwrap();

        let result = old.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_ok());

        // We inline the type of foo.bar.baz with u32 (over-riding what's given in the type spec)
        let mut new = Expr::from_text(r#"1: u32"#).unwrap();
        let result = new.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_inline_type_annotation_1() {
        let mut invalid_rib_expr = Expr::from_text(r#"foo.bar.baz[0] + 1u32"#).unwrap();

        let result = invalid_rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_err());

        // We inline the type of foo.bar.baz with u32 (over-riding what's given in the type spec)
        let mut valid_rib_expr = Expr::from_text(r#"foo.bar.baz[0]: u32 + 1u32"#).unwrap();
        let result = valid_rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_inline_type_annotation_2() {
        let type_spec =
            GlobalVariableTypeSpec::new("foo", Path::from_elems(vec!["bar"]), InferredType::Str);

        // by default foo.bar.* will be inferred to be a string (given the above type spec) and
        // foo.bar.baz + 1u32 should fail compilation since we are adding string with a u32.
        let mut invalid_rib_expr = Expr::from_text(r#"foo.bar.baz + 1u32"#).unwrap();

        let result =
            invalid_rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![type_spec.clone()]);

        assert!(result.is_err());

        // We inline the type of foo.bar.baz with u32 (over-riding what's given in the type spec)
        let mut valid_rib_expr = Expr::from_text(r#"foo.bar.baz: u32 + 1u32"#).unwrap();
        let result =
            valid_rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![type_spec.clone()]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_inline_type_annotation_3() {
        let type_spec =
            GlobalVariableTypeSpec::new("foo", Path::from_elems(vec![]), InferredType::Str);

        // by default foo will be inferred to be a string (given the above type spec) and
        // foo + 1u32 should fail compilation since we are adding string with a u32.
        let mut invalid_rib_expr = Expr::from_text(r#"foo + 1u32"#).unwrap();

        let result =
            invalid_rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![type_spec.clone()]);

        assert!(result.is_err());

        // We inline the type of foo identifier with u32 (over-riding what's given in the type spec)
        let mut valid_rib_expr = Expr::from_text(r#"foo: u32 + 1u32"#).unwrap();
        let result =
            valid_rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![type_spec.clone()]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_inline_type_annotation_4() {
        // Even if 1 is not specified with a specific number type, it should be inferred as u64
        let mut rib_expr = Expr::from_text(r#"some(1): option<u64>"#).unwrap();

        let result = rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_inline_type_annotation_6() {
        // Even if 1 is not specified with a specific number type, it should be inferred as u64
        let mut rib_expr = Expr::from_text(r#"some(1): option<option<u64>>"#).unwrap();

        let result = rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_err());
    }

    #[test]
    fn test_inference_inline_type_annotation_7() {
        let mut valid_rib_expr = Expr::from_text(r#"ok(1): result<u64, string>"#).unwrap();
        let result = valid_rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_inline_type_annotation_8() {
        let mut valid_rib_expr = Expr::from_text(r#"ok(1): result<u64>"#).unwrap();
        let result = valid_rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_inline_type_annotation_9() {
        let mut valid_rib_expr = Expr::from_text(r#"err(1): result<_, u64>"#).unwrap();
        let result = valid_rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_inline_type_annotation_10() {
        let mut valid_rib_expr = Expr::from_text(r#"err(1): result"#).unwrap();
        let result = valid_rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_inline_type_annotation_11() {
        let mut valid_rib_expr = Expr::from_text(r#"[1, 2]: list<u64>"#).unwrap();
        let result = valid_rib_expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_standalone_literals_1() {
        let mut expr = Expr::from_text(r#"err(1)"#).unwrap();

        let result = expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_standalone_literals_2() {
        let mut expr = Expr::from_text(r#"ok(1)"#).unwrap();

        let result = expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_standalone_literals_3() {
        let mut expr = Expr::from_text(r#"[1, 2]"#).unwrap();

        let result = expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);

        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_standalone_literals_4() {
        let mut expr = Expr::from_text("{foo: {status: 200, b: \"hello\"}}").unwrap();
        let result = expr.infer_types(&FunctionTypeRegistry::empty(), &vec![]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_simple_let_binding_type_inference() {
        let rib_expr = r#"
          let x = 1;
          foo(x)
        "#;

        let function_type_registry = test_utils::get_function_type_registry();

        let mut expr = Expr::from_text(rib_expr).unwrap();

        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let let_binding = let_binding(
            VariableId::local("x", 0),
            None,
            number(
                Number {
                    value: BigDecimal::from(1),
                },
                None,
                InferredType::U64,
            ), // The number in let expression is identified to be a U64
        );

        let call_expr = call(
            CallType::function_without_worker(DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            None,
            vec![identifier(
                VariableId::local("x", 0),
                None,
                InferredType::U64, // Variable identified to be a U64
            )],
            InferredType::Sequence(vec![]),
        );

        let expected = expr_block(vec![let_binding, call_expr], InferredType::Sequence(vec![]));

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_multiple_let_binding_expressions() {
        let rib_expr = r#"
          let x = 1;
          let y = 2;
          foo(x);
          baz(y)
        "#;

        let function_type_registry = test_utils::get_function_type_registry();

        let mut expr = Expr::from_text(rib_expr).unwrap();

        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let let_binding1 = let_binding(
            VariableId::local("x", 0),
            None,
            number(
                Number {
                    value: BigDecimal::from(1),
                },
                None,
                InferredType::U64,
            ),
        );

        let let_binding2 = let_binding(
            VariableId::local("y", 0),
            None,
            number(
                Number {
                    value: BigDecimal::from(2),
                },
                None,
                InferredType::U32,
            ),
        );

        let call_expr1 = call(
            CallType::function_without_worker(DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            None,
            vec![identifier(
                VariableId::local("x", 0),
                None,
                InferredType::U64,
            )],
            InferredType::Sequence(vec![]),
        );

        let call_expr2 = call(
            CallType::function_without_worker(DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "baz".to_string(),
                },
            }),
            None,
            vec![identifier(
                VariableId::local("y", 0),
                None,
                InferredType::U32,
            )],
            InferredType::Sequence(vec![]),
        );

        let expected = expr_block(
            vec![let_binding1, let_binding2, call_expr1, call_expr2],
            InferredType::Sequence(vec![]),
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_number_literal() {
        let rib_expr = r#"
          let x: u64 = 1;
          x

          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    Some(TypeName::U64),
                    number(
                        Number {
                            value: BigDecimal::from(1),
                        },
                        None,
                        InferredType::U64,
                    ),
                ),
                identifier(VariableId::local("x", 0), None, InferredType::U64),
            ],
            InferredType::U64,
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_string_literal() {
        let rib_expr = r#"
          let x = "1";
          x

          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(VariableId::local("x", 0), None, Expr::literal("1")),
                identifier(VariableId::local("x", 0), None, InferredType::Str),
            ],
            InferredType::Str,
        );

        assert_eq!(expr, expected);
    }

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

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    Some(TypeName::U64),
                    number(
                        Number {
                            value: BigDecimal::from(1),
                        },
                        None,
                        InferredType::U64,
                    ),
                ),
                let_binding(
                    VariableId::local("y", 0),
                    Some(TypeName::U64),
                    number(
                        Number {
                            value: BigDecimal::from(2),
                        },
                        None,
                        InferredType::U64,
                    ),
                ),
                greater_than(
                    identifier(VariableId::local("x", 0), None, InferredType::U64),
                    identifier(VariableId::local("y", 0), None, InferredType::U64),
                    InferredType::Bool,
                ),
                greater_than_or_equal_to(
                    identifier(VariableId::local("x", 0), None, InferredType::U64),
                    identifier(VariableId::local("y", 0), None, InferredType::U64),
                    InferredType::Bool,
                ),
                less_than(
                    identifier(VariableId::local("x", 0), None, InferredType::U64),
                    identifier(VariableId::local("y", 0), None, InferredType::U64),
                    InferredType::Bool,
                ),
                less_than_or_equal_to(
                    identifier(VariableId::local("x", 0), None, InferredType::U64),
                    identifier(VariableId::local("y", 0), None, InferredType::U64),
                    InferredType::Bool,
                ),
                equal_to(
                    identifier(VariableId::local("x", 0), None, InferredType::U64),
                    identifier(VariableId::local("y", 0), None, InferredType::U64),
                    InferredType::Bool,
                ),
            ],
            InferredType::Bool,
        );

        assert_eq!(expr, expected);
    }

    #[test]
    async fn test_inference_enum_construction_and_pattern_match() {
        let input_enum_type = get_analysed_type_enum(vec!["foo", "bar", "foo-bar"]);

        let output_enum_type = get_analysed_type_enum(vec!["success", "failure", "in-progress"]);

        let component_metadata = get_analysed_exports(
            "process",
            vec![
                input_enum_type.clone(),
                input_enum_type.clone(),
                input_enum_type.clone(),
                str(),
            ],
            output_enum_type.clone(),
        );

        let expr = r#"
              let user: string = request.body.user-id;
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

        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = test_utils::expected_expr_for_enum_test();

        assert_eq!(expr, expected);
    }

    #[test]
    async fn test_inference_variant_construction_and_pattern_match() {
        let input_variant_type = get_analysed_type_variant(vec![
            ("foo", Some(u64())),
            ("bar-baz", Some(str())),
            ("foo-bar", None),
        ]);

        let output_variant_type = get_analysed_type_variant(vec![
            ("success", Some(u64())),
            ("in-progress", Some(str())),
            ("failure", None),
        ]);

        let component_metadata = get_analysed_exports(
            "process",
            vec![
                input_variant_type.clone(),
                input_variant_type.clone(),
                input_variant_type.clone(),
                u64(),
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
                bar-baz(n1) => "y bar ${n1}",
                foo-bar => "y foo-bar"
              };

              let z = match query3 {
                foo(n) => "z foo ${n}",
                bar-baz(n1) => "z bar ${n1}",
                foo-bar => "z foo-bar"
              };

              { x: x, y: y, z: z }

            "#;

        let function_type_registry =
            FunctionTypeRegistry::from_export_metadata(&component_metadata);

        let mut expr = Expr::from_text(expr).unwrap();

        let result = expr.infer_types(&function_type_registry, &vec![]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_concat() {
        let rib_expr = r#"
          let x = "1";
          let y = "2";
          "${x}${y}"
          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(VariableId::local("x", 0), None, Expr::literal("1")),
                let_binding(VariableId::local("y", 0), None, Expr::literal("2")),
                concat(
                    vec![
                        identifier(VariableId::local("x", 0), None, InferredType::Str),
                        identifier(VariableId::local("y", 0), None, InferredType::Str),
                    ],
                    InferredType::Str,
                ),
            ],
            InferredType::Str,
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_boolean_literal() {
        let rib_expr = r#"
          let x = true;
          x

          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(VariableId::local("x", 0), None, Expr::boolean(true)),
                identifier(VariableId::local("x", 0), None, InferredType::Bool),
            ],
            InferredType::Bool,
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_cond() {
        let rib_expr = r#"
          let x: u64 = 1;
          let y: u64 = 2;
          let res1 = "foo";
          let res2 = "bar";
          if x > y then res1 else res2
          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    Some(TypeName::U64),
                    number(
                        Number {
                            value: BigDecimal::from(1),
                        },
                        None,
                        InferredType::U64,
                    ),
                ),
                let_binding(
                    VariableId::local("y", 0),
                    Some(TypeName::U64),
                    number(
                        Number {
                            value: BigDecimal::from(2),
                        },
                        None,
                        InferredType::U64,
                    ),
                ),
                let_binding(VariableId::local("res1", 0), None, Expr::literal("foo")),
                let_binding(VariableId::local("res2", 0), None, Expr::literal("bar")),
                cond(
                    greater_than(
                        identifier(VariableId::local("x", 0), None, InferredType::U64),
                        identifier(VariableId::local("y", 0), None, InferredType::U64),
                        InferredType::Bool,
                    ),
                    identifier(VariableId::local("res1", 0), None, InferredType::Str),
                    identifier(VariableId::local("res2", 0), None, InferredType::Str),
                    InferredType::Str,
                ),
            ],
            InferredType::Str,
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_identifier() {
        let rib_expr = r#"
          let x = "1";
          let y = x;
          y

          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(VariableId::local("x", 0), None, Expr::literal("1")),
                let_binding(
                    VariableId::local("y", 0),
                    None,
                    identifier(VariableId::local("x", 0), None, InferredType::Str),
                ),
                identifier(VariableId::local("y", 0), None, InferredType::Str),
            ],
            InferredType::Str,
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_identifier_multiple_re_assign() {
        let rib_expr = r#"
          let x: u64 = 1;
          let y = x;
          let z = y;
          z

          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    Some(TypeName::U64),
                    number(
                        Number {
                            value: BigDecimal::from(1),
                        },
                        None,
                        InferredType::U64,
                    ),
                ),
                let_binding(
                    VariableId::local("y", 0),
                    None,
                    identifier(VariableId::local("x", 0), None, InferredType::U64),
                ),
                let_binding(
                    VariableId::local("z", 0),
                    None,
                    identifier(VariableId::local("y", 0), None, InferredType::U64),
                ),
                identifier(VariableId::local("z", 0), None, InferredType::U64),
            ],
            InferredType::U64,
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_list() {
        let rib_expr = r#"
          let x: list<u64> = [1, 2, 3];
          x

          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    Some(TypeName::List(Box::new(TypeName::U64))),
                    sequence(
                        vec![
                            number(
                                Number {
                                    value: BigDecimal::from(1),
                                },
                                None,
                                InferredType::U64,
                            ),
                            number(
                                Number {
                                    value: BigDecimal::from(2),
                                },
                                None,
                                InferredType::U64,
                            ),
                            number(
                                Number {
                                    value: BigDecimal::from(3),
                                },
                                None,
                                InferredType::U64,
                            ),
                        ],
                        None,
                        InferredType::List(Box::new(InferredType::U64)),
                    ),
                ),
                identifier(
                    VariableId::local("x", 0),
                    None,
                    InferredType::List(Box::new(InferredType::U64)),
                ),
            ],
            InferredType::List(Box::new(InferredType::U64)),
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_select_index() {
        let rib_expr = r#"
          let x: list<u64> = [1, 2, 3];
          x[0]

          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    Some(TypeName::List(Box::new(TypeName::U64))),
                    sequence(
                        vec![
                            number(
                                Number {
                                    value: BigDecimal::from(1),
                                },
                                None,
                                InferredType::U64,
                            ),
                            number(
                                Number {
                                    value: BigDecimal::from(2),
                                },
                                None,
                                InferredType::U64,
                            ),
                            number(
                                Number {
                                    value: BigDecimal::from(3),
                                },
                                None,
                                InferredType::U64,
                            ),
                        ],
                        None,
                        InferredType::List(Box::new(InferredType::U64)),
                    ),
                ),
                select_dynamic(
                    identifier(
                        VariableId::local("x", 0),
                        None,
                        InferredType::List(Box::new(InferredType::U64)),
                    ),
                    number(
                        Number {
                            value: BigDecimal::from(0),
                        },
                        None,
                        InferredType::U64,
                    ),
                    None,
                    InferredType::U64,
                ),
            ],
            InferredType::U64,
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_select_field() {
        let rib_expr = r#"
          let n: u64 = 1;
          let x = { foo : n };
          x.foo

          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("n", 0),
                    Some(TypeName::U64),
                    number(
                        Number {
                            value: BigDecimal::from(1),
                        },
                        None,
                        InferredType::U64,
                    ),
                ),
                let_binding(
                    VariableId::local("x", 0),
                    None,
                    record(
                        vec![(
                            "foo".to_string(),
                            identifier(VariableId::local("n", 0), None, InferredType::U64),
                        )],
                        InferredType::Record(vec![("foo".to_string(), InferredType::U64)]),
                    ),
                ),
                select_field(
                    identifier(
                        VariableId::local("x", 0),
                        None,
                        InferredType::Record(vec![("foo".to_string(), InferredType::U64)]),
                    ),
                    "foo".to_string(),
                    None,
                    InferredType::U64,
                ),
            ],
            InferredType::U64,
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_tuple() {
        let rib_expr = r#"
          let x: tuple<u64, string> = (1, "2");
          x

          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    Some(TypeName::Tuple(vec![TypeName::U64, TypeName::Str])),
                    tuple(
                        vec![
                            number(
                                Number {
                                    value: BigDecimal::from(1),
                                },
                                None,
                                InferredType::U64,
                            ),
                            Expr::literal("2"),
                        ],
                        InferredType::Tuple(vec![InferredType::U64, InferredType::Str]),
                    ),
                ),
                identifier(
                    VariableId::local("x", 0),
                    None,
                    InferredType::Tuple(vec![InferredType::U64, InferredType::Str]),
                ),
            ],
            InferredType::Tuple(vec![InferredType::U64, InferredType::Str]),
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_variable_conflict_case() {
        let expr_str = r#"
              let y: u64 = 1;
              let z = some(y);

              match z {
                 some(z) => y,
                 none => 0u64
              }
            "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.infer_types(&FunctionTypeRegistry::empty(), &vec![])
            .unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("y", 0),
                    Some(TypeName::U64),
                    number(
                        Number {
                            value: BigDecimal::from(1),
                        },
                        None,
                        InferredType::U64,
                    ),
                ),
                let_binding(
                    VariableId::local("z", 0),
                    None,
                    option(
                        Some(identifier(
                            VariableId::local("y", 0),
                            None,
                            InferredType::U64,
                        )),
                        None,
                        InferredType::Option(Box::new(InferredType::U64)),
                    ),
                ),
                pattern_match(
                    identifier(
                        VariableId::local("z", 0),
                        None,
                        InferredType::Option(Box::new(InferredType::U64)),
                    ),
                    vec![
                        MatchArm::new(
                            ArmPattern::constructor(
                                "some",
                                vec![ArmPattern::literal(identifier(
                                    VariableId::match_identifier("z".to_string(), 1),
                                    None,
                                    InferredType::U64,
                                ))],
                            ),
                            identifier(VariableId::local("y", 0), None, InferredType::U64),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor("none", vec![]),
                            number(
                                Number {
                                    value: BigDecimal::from(0),
                                },
                                Some(TypeName::U64),
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

    #[test]
    fn test_inference_simple_pattern_match() {
        let rib_expr = r#"
                let x = 1;
                let y = 2;

                match x {
                  1 => foo(x),
                  2 => baz(y)
                }"#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let let_binding1 = let_binding(
            VariableId::local("x", 0),
            None,
            number(
                Number {
                    value: BigDecimal::from(1),
                },
                None,
                InferredType::U64,
            ),
        );

        let let_binding2 = let_binding(
            VariableId::local("y", 0),
            None,
            number(
                Number {
                    value: BigDecimal::from(2),
                },
                None,
                InferredType::U32,
            ),
        );

        let match_expr_expected = pattern_match(
            identifier(VariableId::local("x", 0), None, InferredType::U64),
            vec![
                MatchArm::new(
                    ArmPattern::Literal(Box::new(number(
                        Number {
                            value: BigDecimal::from(1),
                        },
                        None,
                        InferredType::U64,
                    ))),
                    call(
                        CallType::function_without_worker(DynamicParsedFunctionName {
                            site: ParsedFunctionSite::Global,
                            function: DynamicParsedFunctionReference::Function {
                                function: "foo".to_string(),
                            },
                        }),
                        None,
                        vec![identifier(
                            VariableId::local("x", 0),
                            None,
                            InferredType::U64,
                        )],
                        InferredType::Sequence(vec![]),
                    ),
                ),
                MatchArm::new(
                    ArmPattern::Literal(Box::new(number(
                        Number {
                            value: BigDecimal::from(2),
                        },
                        None,
                        InferredType::U64, // because predicate is u64
                    ))),
                    call(
                        CallType::function_without_worker(DynamicParsedFunctionName {
                            site: ParsedFunctionSite::Global,
                            function: DynamicParsedFunctionReference::Function {
                                function: "baz".to_string(),
                            },
                        }),
                        None,
                        vec![identifier(
                            VariableId::local("y", 0),
                            None,
                            InferredType::U32,
                        )],
                        InferredType::Sequence(vec![]),
                    ),
                ),
            ],
            InferredType::Sequence(vec![]),
        );

        let expected = expr_block(
            vec![let_binding1, let_binding2, match_expr_expected],
            InferredType::Sequence(vec![]),
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_pattern_match_with_result() {
        let rib_expr = r#"
              let x: u64 = 1;

              match err(x) {
                err(_) => none,
                ok(_) => some(some(x))
              }
            "#;

        let function_type_registry = test_utils::get_function_type_registry();

        let mut expr = Expr::from_text(rib_expr).unwrap();

        let result = expr.infer_types(&function_type_registry, &vec![]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_inference_pattern_match_with_option() {
        let expr_str = r#"
              let x: u64 = 1;
              let y: u64 = 2;
              match some(x) {
                some(x) => some(some(x)),
                none => some(some(y))
              }
            "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.infer_types(&FunctionTypeRegistry::empty(), &vec![])
            .unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    Some(TypeName::U64),
                    number(
                        Number {
                            value: BigDecimal::from(1),
                        },
                        None,
                        InferredType::U64,
                    ),
                ),
                let_binding(
                    VariableId::local("y", 0),
                    Some(TypeName::U64),
                    number(
                        Number {
                            value: BigDecimal::from(2),
                        },
                        None,
                        InferredType::U64,
                    ),
                ),
                pattern_match(
                    option(
                        Some(identifier(
                            VariableId::local("x", 0),
                            None,
                            InferredType::U64,
                        )),
                        None,
                        InferredType::Option(Box::new(InferredType::U64)),
                    ),
                    vec![
                        MatchArm::new(
                            ArmPattern::Constructor(
                                "some".to_string(),
                                vec![ArmPattern::Literal(Box::new(identifier(
                                    VariableId::match_identifier("x".to_string(), 1),
                                    None,
                                    InferredType::U64,
                                )))],
                            ),
                            option(
                                Some(option(
                                    Some(identifier(
                                        VariableId::match_identifier("x".to_string(), 1),
                                        None,
                                        InferredType::U64,
                                    )),
                                    None,
                                    InferredType::Option(Box::new(InferredType::U64)),
                                )),
                                None,
                                InferredType::Option(Box::new(InferredType::Option(Box::new(
                                    InferredType::U64,
                                )))),
                            ),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor("none", vec![]),
                            option(
                                Some(option(
                                    Some(identifier(
                                        VariableId::local("y", 0),
                                        None,
                                        InferredType::U64,
                                    )),
                                    None,
                                    InferredType::Option(Box::new(InferredType::U64)),
                                )),
                                None,
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
    fn test_inference_pattern_match_with_record() {
        let expr_str = r#"
              let x = { foo : "bar" };
              match some(x) {
                some(x) => x,
                none => { foo : "baz" }
              }
            "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.infer_types(&FunctionTypeRegistry::empty(), &vec![])
            .unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    None,
                    record(
                        vec![("foo".to_string(), Expr::literal("bar"))],
                        InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                    ),
                ),
                pattern_match(
                    option(
                        Some(identifier(
                            VariableId::local("x", 0),
                            None,
                            InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                        )),
                        None,
                        InferredType::Option(Box::new(InferredType::Record(vec![(
                            "foo".to_string(),
                            InferredType::Str,
                        )]))),
                    ),
                    vec![
                        MatchArm::new(
                            ArmPattern::Constructor(
                                "some".to_string(),
                                vec![ArmPattern::Literal(Box::new(identifier(
                                    VariableId::match_identifier("x".to_string(), 1),
                                    None,
                                    InferredType::Record(vec![(
                                        "foo".to_string(),
                                        InferredType::Str,
                                    )]),
                                )))],
                            ),
                            identifier(
                                VariableId::match_identifier("x".to_string(), 1),
                                None,
                                InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                            ),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor("none", vec![]),
                            record(
                                vec![("foo".to_string(), Expr::literal("baz"))],
                                InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                            ),
                        ),
                    ],
                    InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                ),
            ],
            InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
        );

        assert_eq!(expr, expected)
    }

    #[test]
    fn test_inference_pattern_match_with_record_with_select_field() {
        let expr_str = r#"
              let x = { foo : "bar" };
              match some(x) {
                some(x) => x.foo,
                none => "baz"
              }
            "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.infer_types(&FunctionTypeRegistry::empty(), &vec![])
            .unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    None,
                    record(
                        vec![("foo".to_string(), Expr::literal("bar"))],
                        InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                    ),
                ),
                pattern_match(
                    option(
                        Some(identifier(
                            VariableId::local("x", 0),
                            None,
                            InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                        )),
                        None,
                        InferredType::Option(Box::new(InferredType::Record(vec![(
                            "foo".to_string(),
                            InferredType::Str,
                        )]))),
                    ),
                    vec![
                        MatchArm::new(
                            ArmPattern::constructor(
                                "some",
                                vec![ArmPattern::literal(identifier(
                                    VariableId::match_identifier("x".to_string(), 1),
                                    None,
                                    InferredType::Record(vec![(
                                        "foo".to_string(),
                                        InferredType::Str,
                                    )]),
                                ))],
                            ),
                            select_field(
                                identifier(
                                    VariableId::match_identifier("x".to_string(), 1),
                                    None,
                                    InferredType::Record(vec![(
                                        "foo".to_string(),
                                        InferredType::Str,
                                    )]),
                                ),
                                "foo".to_string(),
                                None,
                                InferredType::Str,
                            ),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor("none", vec![]),
                            Expr::literal("baz"),
                        ),
                    ],
                    InferredType::Str,
                ),
            ],
            InferredType::Str,
        );

        assert_eq!(expr, expected)
    }

    #[test]
    fn test_inference_pattern_match_with_record_with_select_index() {
        let expr_str = r#"
              let x = { foo : "bar" };
              let y: list<u64> = [1, 2, 3];

              match some(x) {
                some(x) => x.foo,
                none => "baz"
              };

              match some(y) {
                 some(y) => y[0],
                 none => 0: u64
              }
            "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.infer_types(&FunctionTypeRegistry::empty(), &vec![])
            .unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    None,
                    record(
                        vec![("foo".to_string(), Expr::literal("bar"))],
                        InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                    ),
                ),
                let_binding(
                    VariableId::local("y", 0),
                    Some(TypeName::List(Box::new(TypeName::U64))),
                    sequence(
                        vec![
                            number(
                                Number {
                                    value: BigDecimal::from(1),
                                },
                                None,
                                InferredType::U64,
                            ),
                            number(
                                Number {
                                    value: BigDecimal::from(2),
                                },
                                None,
                                InferredType::U64,
                            ),
                            number(
                                Number {
                                    value: BigDecimal::from(3),
                                },
                                None,
                                InferredType::U64,
                            ),
                        ],
                        None,
                        InferredType::List(Box::new(InferredType::U64)),
                    ),
                ),
                pattern_match(
                    option(
                        Some(identifier(
                            VariableId::local("x", 0),
                            None,
                            InferredType::Record(vec![("foo".to_string(), InferredType::Str)]),
                        )),
                        None,
                        InferredType::Option(Box::new(InferredType::Record(vec![(
                            "foo".to_string(),
                            InferredType::Str,
                        )]))),
                    ),
                    vec![
                        MatchArm::new(
                            ArmPattern::Constructor(
                                "some".to_string(),
                                vec![ArmPattern::Literal(Box::new(identifier(
                                    VariableId::match_identifier("x".to_string(), 1),
                                    None,
                                    InferredType::Record(vec![(
                                        "foo".to_string(),
                                        InferredType::Str,
                                    )]),
                                )))],
                            ),
                            select_field(
                                identifier(
                                    VariableId::match_identifier("x".to_string(), 1),
                                    None,
                                    InferredType::Record(vec![(
                                        "foo".to_string(),
                                        InferredType::Str,
                                    )]),
                                ),
                                "foo".to_string(),
                                None,
                                InferredType::Str,
                            ),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor("none", vec![]),
                            Expr::literal("baz"),
                        ),
                    ],
                    InferredType::Str,
                ),
                pattern_match(
                    option(
                        Some(identifier(
                            VariableId::local("y", 0),
                            None,
                            InferredType::List(Box::new(InferredType::U64)),
                        )),
                        None,
                        InferredType::Option(Box::new(InferredType::List(Box::new(
                            InferredType::U64,
                        )))),
                    ),
                    vec![
                        MatchArm::new(
                            ArmPattern::Constructor(
                                "some".to_string(),
                                vec![ArmPattern::Literal(Box::new(identifier(
                                    VariableId::match_identifier("y".to_string(), 3),
                                    None,
                                    InferredType::List(Box::new(InferredType::U64)),
                                )))],
                            ),
                            select_dynamic(
                                identifier(
                                    VariableId::match_identifier("y".to_string(), 3),
                                    None,
                                    InferredType::List(Box::new(InferredType::U64)),
                                ),
                                number(
                                    Number {
                                        value: BigDecimal::from(0),
                                    },
                                    None,
                                    InferredType::U64,
                                ),
                                None,
                                InferredType::U64,
                            ),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor("none", vec![]),
                            number(
                                Number {
                                    value: BigDecimal::from(0),
                                },
                                Some(TypeName::U64),
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

    #[test]
    fn test_inference_option() {
        let rib_expr = r#"
          let x: option<u64> = some(1);
          x

          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    Some(TypeName::Option(Box::new(TypeName::U64))),
                    option(
                        Some(number(
                            Number {
                                value: BigDecimal::from(1),
                            },
                            None,
                            InferredType::U64,
                        )),
                        None,
                        InferredType::Option(Box::new(InferredType::U64)),
                    ),
                ),
                identifier(
                    VariableId::local("x", 0),
                    None,
                    InferredType::Option(Box::new(InferredType::U64)),
                ),
            ],
            InferredType::Option(Box::new(InferredType::U64)),
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_option_nested_type_inference() {
        let rib_expr = r#"
          let x: option<u64> = some(1);
          let y = some(x);
          y

          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    Some(TypeName::Option(Box::new(TypeName::U64))),
                    option(
                        Some(number(
                            Number {
                                value: BigDecimal::from(1),
                            },
                            None,
                            InferredType::U64,
                        )),
                        None,
                        InferredType::Option(Box::new(InferredType::U64)),
                    ),
                ),
                let_binding(
                    VariableId::local("y", 0),
                    None,
                    option(
                        Some(identifier(
                            VariableId::local("x", 0),
                            None,
                            InferredType::Option(Box::new(InferredType::U64)),
                        )),
                        None,
                        InferredType::Option(Box::new(InferredType::Option(Box::new(
                            InferredType::U64,
                        )))),
                    ),
                ),
                identifier(
                    VariableId::local("y", 0),
                    None,
                    InferredType::Option(Box::new(InferredType::Option(Box::new(
                        InferredType::U64,
                    )))),
                ),
            ],
            InferredType::Option(Box::new(InferredType::Option(Box::new(InferredType::U64)))),
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_record_type_inference() {
        let rib_expr = r#"
          let number: u64 = 1;
          let x = { foo : number };
          x

          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("number", 0),
                    Some(TypeName::U64),
                    number(
                        Number {
                            value: BigDecimal::from(1),
                        },
                        None,
                        InferredType::U64,
                    ),
                ),
                let_binding(
                    VariableId::local("x", 0),
                    None,
                    record(
                        vec![(
                            "foo".to_string(),
                            identifier(VariableId::local("number", 0), None, InferredType::U64),
                        )],
                        InferredType::Record(vec![("foo".to_string(), InferredType::U64)]),
                    ),
                ),
                identifier(
                    VariableId::local("x", 0),
                    None,
                    InferredType::Record(vec![("foo".to_string(), InferredType::U64)]),
                ),
            ],
            InferredType::Record(vec![("foo".to_string(), InferredType::U64)]),
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_record_identifier() {
        let rib_expr = r#"
          let x: u64 = if true then 1u64 else 20u64;
          let y = {
             let z = {x: x};
             z
          };
          y
          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    Some(TypeName::U64),
                    cond(
                        Expr::boolean(true),
                        number(
                            Number {
                                value: BigDecimal::from(1),
                            },
                            Some(TypeName::U64),
                            InferredType::U64,
                        ),
                        number(
                            Number {
                                value: BigDecimal::from(20),
                            },
                            Some(TypeName::U64),
                            InferredType::U64,
                        ),
                        InferredType::U64,
                    ),
                ),
                let_binding(
                    VariableId::local("y", 0),
                    None,
                    expr_block(
                        vec![
                            let_binding(
                                VariableId::local("z", 0),
                                None,
                                record(
                                    vec![(
                                        "x".to_string(),
                                        identifier(
                                            VariableId::local("x", 0),
                                            None,
                                            InferredType::U64,
                                        ),
                                    )],
                                    InferredType::Record(vec![(
                                        "x".to_string(),
                                        InferredType::U64,
                                    )]),
                                ),
                            ),
                            identifier(
                                VariableId::local("z", 0),
                                None,
                                InferredType::Record(vec![("x".to_string(), InferredType::U64)]),
                            ),
                        ],
                        InferredType::Record(vec![("x".to_string(), InferredType::U64)]),
                    ),
                ),
                identifier(
                    VariableId::local("y", 0),
                    None,
                    InferredType::Record(vec![("x".to_string(), InferredType::U64)]),
                ),
            ],
            InferredType::Record(vec![("x".to_string(), InferredType::U64)]),
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_record_select_with_function_call() {
        let request_body_type = test_utils::get_analysed_type_record(vec![
            ("id".to_string(), str()),
            ("name".to_string(), str()),
            ("titles".to_string(), list(str())),
            (
                "address".to_string(),
                test_utils::get_analysed_type_record(vec![
                    ("street".to_string(), str()),
                    ("city".to_string(), str()),
                ]),
            ),
        ]);

        let worker_response = test_utils::create_none(&str());

        let request_type = test_utils::get_analysed_type_record(vec![(
            "body".to_string(),
            request_body_type.clone(),
        )]);

        let return_type = golem_wasm_ast::analysis::analysed_type::option(worker_response.typ);

        let component_metadata =
            get_analysed_exports("foo", vec![request_type.clone()], return_type);

        let expr_str = r#"
              let x = { body : { id: "bId", name: "bName", titles: request.body.titles, address: request.body.address } };
              let result = foo(x);
              match result {  some(value) => "personal-id", none =>  x.body.titles[1] }
            "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        let function_type_registry =
            FunctionTypeRegistry::from_export_metadata(&component_metadata);

        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = test_utils::expected_expr_for_select_index();

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_inference_list_aggregation() {
        let rib_expr = r#"
           let ages: list<u64> = [1, 2, 3];
           reduce z, a in ages from 0u64 {
              let result = z + a;
              yield result;
           }
        "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let inferred_expr =
            InferredExpr::from_expr(expr, &FunctionTypeRegistry::empty(), &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("ages", 0),
                    Some(TypeName::List(Box::new(TypeName::U64))),
                    sequence(
                        vec![
                            Expr::number_inferred(BigDecimal::from(1), None, InferredType::U64),
                            Expr::number_inferred(BigDecimal::from(2), None, InferredType::U64),
                            Expr::number_inferred(BigDecimal::from(3), None, InferredType::U64),
                        ],
                        None,
                        InferredType::List(Box::new(InferredType::U64)),
                    ),
                ),
                Expr::typed_list_reduce(
                    VariableId::list_reduce_identifier("z"),
                    VariableId::list_comprehension_identifier("a"),
                    identifier(
                        VariableId::local("ages", 0),
                        None,
                        InferredType::List(Box::new(InferredType::U64)),
                    ),
                    Expr::number_inferred(
                        BigDecimal::from(0),
                        Some(TypeName::U64),
                        InferredType::U64,
                    ),
                    expr_block(
                        vec![
                            let_binding(
                                VariableId::local("result", 0),
                                None,
                                plus(
                                    identifier(
                                        VariableId::list_reduce_identifier("z"),
                                        None,
                                        InferredType::U64,
                                    ),
                                    identifier(
                                        VariableId::list_comprehension_identifier("a"),
                                        None,
                                        InferredType::U64,
                                    ),
                                    InferredType::U64,
                                ),
                            ),
                            identifier(VariableId::local("result", 0), None, InferredType::U64),
                        ],
                        InferredType::U64,
                    ),
                    InferredType::U64,
                ),
            ],
            InferredType::U64,
        );

        assert_eq!(inferred_expr.get_expr(), &expected);
    }

    #[test]
    fn test_inference_list_comprehension() {
        let rib_expr = r#"
          let x = ["foo", "bar"];

          for i in x {
            yield i;
          }

          "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let inferred_expr =
            InferredExpr::from_expr(expr, &FunctionTypeRegistry::empty(), &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("x", 0),
                    None,
                    sequence(
                        vec![Expr::literal("foo"), Expr::literal("bar")],
                        None,
                        InferredType::List(Box::new(InferredType::Str)),
                    ),
                ),
                Expr::list_comprehension_typed(
                    VariableId::list_comprehension_identifier("i"),
                    identifier(
                        VariableId::local("x", 0),
                        None,
                        InferredType::List(Box::new(InferredType::Str)),
                    ),
                    expr_block(
                        vec![identifier(
                            VariableId::list_comprehension_identifier("i"),
                            None,
                            InferredType::Str,
                        )],
                        InferredType::Str,
                    ),
                    InferredType::List(Box::new(InferredType::Str)),
                ),
            ],
            InferredType::List(Box::new(InferredType::Str)),
        );

        assert_eq!(inferred_expr.get_expr(), &expected);
    }

    #[test]
    fn test_inference_result() {
        let rib_expr = r#"
          let p = err("foo");
          let q = ok("bar");
          { a : p, b: q }
          "#;

        let function_type_registry = test_utils::get_function_type_registry();
        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let expected = expr_block(
            vec![
                let_binding(
                    VariableId::local("p", 0),
                    None,
                    result(
                        Err(Expr::literal("foo")),
                        None,
                        InferredType::Result {
                            ok: Some(Box::new(InferredType::Unknown)),
                            error: Some(Box::new(InferredType::Str)),
                        },
                    ),
                ),
                let_binding(
                    VariableId::local("q", 0),
                    None,
                    result(
                        Ok(Expr::literal("bar")),
                        None,
                        InferredType::Result {
                            ok: Some(Box::new(InferredType::Str)),
                            error: Some(Box::new(InferredType::Unknown)),
                        },
                    ),
                ),
                record(
                    vec![
                        (
                            "a".to_string(),
                            identifier(
                                VariableId::local("p", 0),
                                None,
                                InferredType::Result {
                                    ok: Some(Box::new(InferredType::Unknown)),
                                    error: Some(Box::new(InferredType::Str)),
                                },
                            ),
                        ),
                        (
                            "b".to_string(),
                            identifier(
                                VariableId::local("q", 0),
                                None,
                                InferredType::Result {
                                    ok: Some(Box::new(InferredType::Str)),
                                    error: Some(Box::new(InferredType::Unknown)),
                                },
                            ),
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

    mod test_utils {
        use crate::call_type::CallType;
        use crate::function_name::{DynamicParsedFunctionName, DynamicParsedFunctionReference};
        use crate::generic_type_parameter::GenericTypeParameter;
        use crate::parser::type_name::TypeName;
        use crate::rib_source_span::SourceSpan;
        use crate::{
            ArmPattern, Expr, FunctionTypeRegistry, InferredType, MatchArm, MatchIdentifier,
            Number, ParsedFunctionSite, VariableId,
        };
        use bigdecimal::BigDecimal;
        use golem_wasm_ast::analysis::analysed_type::u64;
        use golem_wasm_ast::analysis::TypeVariant;
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            AnalysedType, NameOptionTypePair, NameTypePair, TypeEnum, TypeRecord, TypeU32,
        };
        use golem_wasm_rpc::{Value, ValueAndType};

        pub(crate) fn result(
            expr: Result<Expr, Expr>,
            type_annotation: Option<TypeName>,
            inferred_type: InferredType,
        ) -> Expr {
            Expr::Result {
                expr: expr.map(Box::new).map_err(Box::new),
                type_annotation,
                inferred_type,
                source_span: SourceSpan::default(),
            }
        }
        pub(crate) fn plus(lhs: Expr, rhs: Expr, inferred_type: InferredType) -> Expr {
            Expr::Plus {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }

        pub(crate) fn pattern_match(
            predicate: Expr,
            match_arms: Vec<MatchArm>,
            inferred_type: InferredType,
        ) -> Expr {
            Expr::PatternMatch {
                predicate: Box::new(predicate),
                match_arms,
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }
        pub(crate) fn literal(value: String, inferred_type: InferredType) -> Expr {
            Expr::Literal {
                value,
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }
        pub(crate) fn tuple(exprs: Vec<Expr>, inferred_type: InferredType) -> Expr {
            Expr::Tuple {
                exprs,
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }
        pub(crate) fn option(
            expr: Option<Expr>,
            type_annotation: Option<TypeName>,
            inferred_type: InferredType,
        ) -> Expr {
            Expr::Option {
                expr: expr.map(Box::new),
                type_annotation,
                inferred_type,
                source_span: SourceSpan::default(),
            }
        }

        pub(crate) fn select_field(
            expr: Expr,
            field: String,
            type_annotation: Option<TypeName>,
            inferred_type: InferredType,
        ) -> Expr {
            Expr::SelectField {
                expr: Box::new(expr),
                field,
                type_annotation,
                inferred_type,
                source_span: SourceSpan::default(),
            }
        }

        pub(crate) fn select_dynamic(
            expr: Expr,
            index: Expr,
            type_annotation: Option<TypeName>,
            inferred_type: InferredType,
        ) -> Expr {
            Expr::SelectIndex {
                expr: Box::new(expr),
                index: Box::new(index),
                type_annotation,
                inferred_type,
                source_span: SourceSpan::default(),
            }
        }

        pub(crate) fn sequence(
            exprs: Vec<Expr>,
            type_annotation: Option<TypeName>,
            inferred_type: InferredType,
        ) -> Expr {
            Expr::Sequence {
                exprs,
                type_annotation,
                inferred_type,
                source_span: SourceSpan::default(),
            }
        }

        pub(crate) fn cond(cond: Expr, lhs: Expr, rhs: Expr, inferred_type: InferredType) -> Expr {
            Expr::Cond {
                cond: Box::new(cond),
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }
        pub(crate) fn greater_than(lhs: Expr, rhs: Expr, inferred_type: InferredType) -> Expr {
            Expr::GreaterThan {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }

        pub(crate) fn greater_than_or_equal_to(
            lhs: Expr,
            rhs: Expr,
            inferred_type: InferredType,
        ) -> Expr {
            Expr::GreaterThanOrEqualTo {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }

        pub(crate) fn less_than(lhs: Expr, rhs: Expr, inferred_type: InferredType) -> Expr {
            Expr::LessThan {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }

        pub(crate) fn less_than_or_equal_to(
            lhs: Expr,
            rhs: Expr,
            inferred_type: InferredType,
        ) -> Expr {
            Expr::LessThanOrEqualTo {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }

        pub(crate) fn equal_to(lhs: Expr, rhs: Expr, inferred_type: InferredType) -> Expr {
            Expr::EqualTo {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }

        pub(crate) fn concat(exprs: Vec<Expr>, inferred_type: InferredType) -> Expr {
            Expr::Concat {
                exprs,
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }
        pub(crate) fn call(
            call_type: CallType,
            generic_type_parameter: Option<GenericTypeParameter>,
            args: Vec<Expr>,
            inferred_type: InferredType,
        ) -> Expr {
            Expr::Call {
                call_type,
                generic_type_parameter,
                args,
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }
        pub(crate) fn number(
            value: Number,
            type_annotation: Option<TypeName>,
            inferred_type: InferredType,
        ) -> Expr {
            Expr::Number {
                number: value,
                type_annotation,
                inferred_type,
                source_span: SourceSpan::default(),
            }
        }
        pub(crate) fn identifier(
            variable_id: VariableId,
            type_annotation: Option<TypeName>,
            inferred_type: InferredType,
        ) -> Expr {
            Expr::Identifier {
                variable_id,
                type_annotation,
                inferred_type,
                source_span: SourceSpan::default(),
            }
        }
        pub(crate) fn record(exprs: Vec<(String, Expr)>, inferred_type: InferredType) -> Expr {
            Expr::Record {
                exprs: exprs
                    .iter()
                    .map(|(k, v)| (k.clone(), Box::new(v.clone())))
                    .collect(),
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }

        pub(crate) fn let_binding(
            variable_id: VariableId,
            type_annotation: Option<TypeName>,
            expr: Expr,
        ) -> Expr {
            Expr::Let {
                variable_id,
                type_annotation,
                expr: Box::new(expr),
                inferred_type: InferredType::Tuple(vec![]),
                source_span: SourceSpan::default(),
            }
        }

        pub(crate) fn expr_block(exprs: Vec<Expr>, inferred_type: InferredType) -> Expr {
            Expr::ExprBlock {
                exprs,
                inferred_type,
                source_span: SourceSpan::default(),
                type_annotation: None,
            }
        }

        pub(crate) fn get_function_type_registry() -> FunctionTypeRegistry {
            let metadata = vec![
                AnalysedExport::Function(AnalysedFunction {
                    name: "foo".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "my_parameter".to_string(),
                        typ: u64(),
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

        pub(crate) fn create_none(typ: &AnalysedType) -> ValueAndType {
            ValueAndType::new(
                Value::Option(None),
                golem_wasm_ast::analysis::analysed_type::option(typ.clone()),
            )
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
            expr_block(
                vec![
                    let_binding(
                        VariableId::local("user", 0),
                        Some(TypeName::Str),
                        select_field(
                            select_field(
                                identifier(
                                    VariableId::global("request".to_string()),
                                    None,
                                    InferredType::Record(vec![(
                                        "body".to_string(),
                                        InferredType::Record(vec![(
                                            "user-id".to_string(),
                                            InferredType::Str,
                                        )]),
                                    )]),
                                ),
                                "body".to_string(),
                                None,
                                InferredType::Record(vec![(
                                    "user-id".to_string(),
                                    InferredType::Str,
                                )]),
                            ),
                            "user-id".to_string(),
                            None,
                            InferredType::Str,
                        ),
                    ),
                    let_binding(
                        VariableId::local("query1", 0),
                        None,
                        call(
                            CallType::EnumConstructor("foo".to_string()),
                            None,
                            vec![],
                            InferredType::Enum(vec![
                                "foo".to_string(),
                                "bar".to_string(),
                                "foo-bar".to_string(),
                            ]),
                        ),
                    ),
                    let_binding(
                        VariableId::local("query2", 0),
                        None,
                        call(
                            CallType::EnumConstructor("bar".to_string()),
                            None,
                            vec![],
                            InferredType::Enum(vec![
                                "foo".to_string(),
                                "bar".to_string(),
                                "foo-bar".to_string(),
                            ]),
                        ),
                    ),
                    let_binding(
                        VariableId::local("query3", 0),
                        None,
                        call(
                            CallType::EnumConstructor("foo-bar".to_string()),
                            None,
                            vec![],
                            InferredType::Enum(vec![
                                "foo".to_string(),
                                "bar".to_string(),
                                "foo-bar".to_string(),
                            ]),
                        ),
                    ),
                    let_binding(
                        VariableId::local("result", 0),
                        None,
                        call(
                            CallType::function_without_worker(DynamicParsedFunctionName {
                                site: ParsedFunctionSite::Global,
                                function: DynamicParsedFunctionReference::Function {
                                    function: "process".to_string(),
                                },
                            }),
                            None,
                            vec![
                                identifier(
                                    VariableId::local("query1", 0),
                                    None,
                                    InferredType::Enum(vec![
                                        "foo".to_string(),
                                        "bar".to_string(),
                                        "foo-bar".to_string(),
                                    ]),
                                ),
                                identifier(
                                    VariableId::local("query2", 0),
                                    None,
                                    InferredType::Enum(vec![
                                        "foo".to_string(),
                                        "bar".to_string(),
                                        "foo-bar".to_string(),
                                    ]),
                                ),
                                identifier(
                                    VariableId::local("query3", 0),
                                    None,
                                    InferredType::Enum(vec![
                                        "foo".to_string(),
                                        "bar".to_string(),
                                        "foo-bar".to_string(),
                                    ]),
                                ),
                                identifier(VariableId::local("user", 0), None, InferredType::Str),
                            ],
                            InferredType::Enum(vec![
                                "success".to_string(),
                                "failure".to_string(),
                                "in-progress".to_string(),
                            ]),
                        ),
                    ),
                    let_binding(
                        VariableId::local("x", 0),
                        None,
                        pattern_match(
                            identifier(
                                VariableId::local("result", 0),
                                None,
                                InferredType::Enum(vec![
                                    "success".to_string(),
                                    "failure".to_string(),
                                    "in-progress".to_string(),
                                ]),
                            ),
                            vec![
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(call(
                                        CallType::EnumConstructor("success".to_string()),
                                        None,
                                        vec![],
                                        InferredType::Enum(vec![
                                            "success".to_string(),
                                            "failure".to_string(),
                                            "in-progress".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(concat(
                                        vec![
                                            literal("success ".to_string(), InferredType::Str),
                                            identifier(
                                                VariableId::local("user", 0),
                                                None,
                                                InferredType::Str,
                                            ),
                                        ],
                                        InferredType::Str,
                                    )),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(call(
                                        CallType::EnumConstructor("failure".to_string()),
                                        None,
                                        vec![],
                                        InferredType::Enum(vec![
                                            "success".to_string(),
                                            "failure".to_string(),
                                            "in-progress".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(concat(
                                        vec![
                                            literal("failed ".to_string(), InferredType::Str),
                                            identifier(
                                                VariableId::local("user", 0),
                                                None,
                                                InferredType::Str,
                                            ),
                                        ],
                                        InferredType::Str,
                                    )),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(call(
                                        CallType::EnumConstructor("in-progress".to_string()),
                                        None,
                                        vec![],
                                        InferredType::Enum(vec![
                                            "success".to_string(),
                                            "failure".to_string(),
                                            "in-progress".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(literal(
                                        "in-progress".to_string(),
                                        InferredType::Str,
                                    )),
                                },
                            ],
                            InferredType::Str,
                        ),
                    ),
                    let_binding(
                        VariableId::local("y", 0),
                        None,
                        pattern_match(
                            identifier(
                                VariableId::local("query2", 0),
                                None,
                                InferredType::Enum(vec![
                                    "foo".to_string(),
                                    "bar".to_string(),
                                    "foo-bar".to_string(),
                                ]),
                            ),
                            vec![
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(call(
                                        CallType::EnumConstructor("foo".to_string()),
                                        None,
                                        vec![],
                                        InferredType::Enum(vec![
                                            "foo".to_string(),
                                            "bar".to_string(),
                                            "foo-bar".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(concat(
                                        vec![
                                            literal("y foo ".to_string(), InferredType::Str),
                                            identifier(
                                                VariableId::local("user", 0),
                                                None,
                                                InferredType::Str,
                                            ),
                                        ],
                                        InferredType::Str,
                                    )),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(call(
                                        CallType::EnumConstructor("bar".to_string()),
                                        None,
                                        vec![],
                                        InferredType::Enum(vec![
                                            "foo".to_string(),
                                            "bar".to_string(),
                                            "foo-bar".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(concat(
                                        vec![
                                            literal("y bar ".to_string(), InferredType::Str),
                                            identifier(
                                                VariableId::local("user", 0),
                                                None,
                                                InferredType::Str,
                                            ),
                                        ],
                                        InferredType::Str,
                                    )),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(call(
                                        CallType::EnumConstructor("foo-bar".to_string()),
                                        None,
                                        vec![],
                                        InferredType::Enum(vec![
                                            "foo".to_string(),
                                            "bar".to_string(),
                                            "foo-bar".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(literal(
                                        "y foo-bar".to_string(),
                                        InferredType::Str,
                                    )),
                                },
                            ],
                            InferredType::Str,
                        ),
                    ),
                    let_binding(
                        VariableId::local("z", 0),
                        None,
                        pattern_match(
                            identifier(
                                VariableId::local("query3", 0),
                                None,
                                InferredType::Enum(vec![
                                    "foo".to_string(),
                                    "bar".to_string(),
                                    "foo-bar".to_string(),
                                ]),
                            ),
                            vec![
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(call(
                                        CallType::EnumConstructor("foo".to_string()),
                                        None,
                                        vec![],
                                        InferredType::Enum(vec![
                                            "foo".to_string(),
                                            "bar".to_string(),
                                            "foo-bar".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(concat(
                                        vec![
                                            literal("z foo ".to_string(), InferredType::Str),
                                            identifier(
                                                VariableId::local("user", 0),
                                                None,
                                                InferredType::Str,
                                            ),
                                        ],
                                        InferredType::Str,
                                    )),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(call(
                                        CallType::EnumConstructor("bar".to_string()),
                                        None,
                                        vec![],
                                        InferredType::Enum(vec![
                                            "foo".to_string(),
                                            "bar".to_string(),
                                            "foo-bar".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(concat(
                                        vec![
                                            literal("z bar ".to_string(), InferredType::Str),
                                            identifier(
                                                VariableId::local("user", 0),
                                                None,
                                                InferredType::Str,
                                            ),
                                        ],
                                        InferredType::Str,
                                    )),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::Literal(Box::new(call(
                                        CallType::EnumConstructor("foo-bar".to_string()),
                                        None,
                                        vec![],
                                        InferredType::Enum(vec![
                                            "foo".to_string(),
                                            "bar".to_string(),
                                            "foo-bar".to_string(),
                                        ]),
                                    ))),
                                    arm_resolution_expr: Box::new(literal(
                                        "z foo-bar".to_string(),
                                        InferredType::Str,
                                    )),
                                },
                            ],
                            InferredType::Str,
                        ),
                    ),
                    record(
                        vec![
                            (
                                "x".to_string(),
                                identifier(VariableId::local("x", 0), None, InferredType::Str),
                            ),
                            (
                                "y".to_string(),
                                identifier(VariableId::local("y", 0), None, InferredType::Str),
                            ),
                            (
                                "z".to_string(),
                                identifier(VariableId::local("z", 0), None, InferredType::Str),
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
            expr_block(
                vec![
                    let_binding(
                        VariableId::local("x", 0),
                        None,
                        record(
                            vec![(
                                "body".to_string(),
                                record(
                                    vec![
                                        ("id".to_string(), Expr::literal("bId")),
                                        ("name".to_string(), Expr::literal("bName")),
                                        (
                                            "titles".to_string(),
                                            select_field(
                                                select_field(
                                                    identifier(
                                                        VariableId::global("request".to_string()),
                                                        None,
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
                                                    ),
                                                    "body".to_string(),
                                                    None,
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
                                                ),
                                                "titles".to_string(),
                                                None,
                                                InferredType::List(Box::new(InferredType::Str)),
                                            ),
                                        ),
                                        (
                                            "address".to_string(),
                                            select_field(
                                                select_field(
                                                    identifier(
                                                        VariableId::global("request".to_string()),
                                                        None,
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
                                                    ),
                                                    "body".to_string(),
                                                    None,
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
                                                ),
                                                "address".to_string(),
                                                None,
                                                InferredType::Record(vec![
                                                    ("street".to_string(), InferredType::Str),
                                                    ("city".to_string(), InferredType::Str),
                                                ]),
                                            ),
                                        ),
                                    ],
                                    InferredType::Record(vec![
                                        ("id".to_string(), InferredType::Str),
                                        ("name".to_string(), InferredType::Str),
                                        (
                                            "titles".to_string(),
                                            InferredType::List(Box::new(InferredType::Str)),
                                        ),
                                        (
                                            "address".to_string(),
                                            InferredType::Record(vec![
                                                ("street".to_string(), InferredType::Str),
                                                ("city".to_string(), InferredType::Str),
                                            ]),
                                        ),
                                    ]),
                                ),
                            )],
                            InferredType::Record(vec![(
                                "body".to_string(),
                                InferredType::Record(vec![
                                    ("id".to_string(), InferredType::Str),
                                    ("name".to_string(), InferredType::Str),
                                    (
                                        "titles".to_string(),
                                        InferredType::List(Box::new(InferredType::Str)),
                                    ),
                                    (
                                        "address".to_string(),
                                        InferredType::Record(vec![
                                            ("street".to_string(), InferredType::Str),
                                            ("city".to_string(), InferredType::Str),
                                        ]),
                                    ),
                                ]),
                            )]),
                        ),
                    ),
                    let_binding(
                        VariableId::local("result", 0),
                        None,
                        call(
                            CallType::function_without_worker(DynamicParsedFunctionName {
                                site: ParsedFunctionSite::Global,
                                function: DynamicParsedFunctionReference::Function {
                                    function: "foo".to_string(),
                                },
                            }),
                            None,
                            vec![identifier(
                                VariableId::local("x", 0),
                                None,
                                InferredType::Record(vec![(
                                    "body".to_string(),
                                    InferredType::Record(vec![
                                        ("id".to_string(), InferredType::Str),
                                        ("name".to_string(), InferredType::Str),
                                        (
                                            "titles".to_string(),
                                            InferredType::List(Box::new(InferredType::Str)),
                                        ),
                                        (
                                            "address".to_string(),
                                            InferredType::Record(vec![
                                                ("street".to_string(), InferredType::Str),
                                                ("city".to_string(), InferredType::Str),
                                            ]),
                                        ),
                                    ]),
                                )]),
                            )],
                            InferredType::Option(Box::new(InferredType::Option(Box::new(
                                InferredType::Str,
                            )))),
                        ),
                    ),
                    pattern_match(
                        identifier(
                            VariableId::local("result", 0),
                            None,
                            InferredType::Option(Box::new(InferredType::Option(Box::new(
                                InferredType::Str,
                            )))),
                        ),
                        vec![
                            MatchArm {
                                arm_pattern: ArmPattern::constructor(
                                    "some",
                                    vec![ArmPattern::literal(identifier(
                                        VariableId::MatchIdentifier(MatchIdentifier::new(
                                            "value".to_string(),
                                            1,
                                        )),
                                        None,
                                        InferredType::Option(Box::new(InferredType::Str)),
                                    ))],
                                ),
                                arm_resolution_expr: Box::new(Expr::literal("personal-id")),
                            },
                            MatchArm {
                                arm_pattern: ArmPattern::constructor("none", vec![]),
                                arm_resolution_expr: Box::new(select_dynamic(
                                    select_field(
                                        select_field(
                                            identifier(
                                                VariableId::local("x", 0),
                                                None,
                                                InferredType::Record(vec![(
                                                    "body".to_string(),
                                                    InferredType::Record(vec![
                                                        ("id".to_string(), InferredType::Str),
                                                        ("name".to_string(), InferredType::Str),
                                                        (
                                                            "titles".to_string(),
                                                            InferredType::List(Box::new(
                                                                InferredType::Str,
                                                            )),
                                                        ),
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
                                                    ]),
                                                )]),
                                            ),
                                            "body".to_string(),
                                            None,
                                            InferredType::Record(vec![
                                                ("id".to_string(), InferredType::Str),
                                                ("name".to_string(), InferredType::Str),
                                                (
                                                    "titles".to_string(),
                                                    InferredType::List(Box::new(InferredType::Str)),
                                                ),
                                                (
                                                    "address".to_string(),
                                                    InferredType::Record(vec![
                                                        ("street".to_string(), InferredType::Str),
                                                        ("city".to_string(), InferredType::Str),
                                                    ]),
                                                ),
                                            ]),
                                        ),
                                        "titles".to_string(),
                                        None,
                                        InferredType::List(Box::new(InferredType::Str)),
                                    ),
                                    Expr::number_inferred(
                                        BigDecimal::from(1),
                                        None,
                                        InferredType::U64,
                                    ),
                                    None,
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
