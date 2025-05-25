// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{Expr, InferredType};
use std::collections::VecDeque;

pub fn bind_type_annotations(expr: &mut Expr) {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::Let {
                type_annotation,
                expr: rhs,
                source_span,
                ..
            } => {
                if let Some(type_annotation) = type_annotation {
                    let new_inferred_type =
                        InferredType::from(&*type_annotation).declared_at(source_span.clone());

                    rhs.with_inferred_type_mut(new_inferred_type)
                }

                queue.push_back(rhs);
            }

            expr => {
                let source_span = expr.source_span();
                let type_annotation = expr.type_annotation();

                if let Some(type_annotation) = type_annotation {
                    let new_inferred_type =
                        InferredType::from(type_annotation).declared_at(source_span);

                    expr.with_inferred_type_mut(new_inferred_type);
                }

                expr.visit_expr_nodes_lazy(&mut queue)
            }
        }
    }
}

#[cfg(test)]
mod type_binding_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use super::*;
    use crate::parser::type_name::TypeName;
    use crate::{ArmPattern, MatchArm, VariableId};

    #[test]
    fn test_bind_type_in_let() {
        let expr_str = r#"
            let x: u64 = 1
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::let_binding(
            "x",
            Expr::number_inferred(BigDecimal::from(1), None, InferredType::u64()),
            Some(TypeName::U64),
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_bind_type_in_option() {
        let expr_str = r#"
            some(1): option<u64>
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::option(Some(Expr::number_inferred(
            BigDecimal::from(1),
            None,
            InferredType::s32(),
        )))
        .with_type_annotation(TypeName::Option(Box::new(TypeName::U64)))
        .with_inferred_type(InferredType::option(InferredType::u64()));

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_bind_type_in_result_1() {
        // Data associated with both success and error case
        let expr_str = r#"
            ok(1): result<u64, string>
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::ok(
            Expr::number_inferred(BigDecimal::from(1), None, InferredType::s32()),
            Some(TypeName::Result {
                ok: Some(Box::new(TypeName::U64)),
                error: Some(Box::new(TypeName::Str)),
            }),
        )
        .with_inferred_type(InferredType::result(
            Some(InferredType::u64()),
            Some(InferredType::string()),
        ));

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_bind_type_in_result_2() {
        // Data associated with only success case
        let expr_str = r#"
            ok(1): result<u64>
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::ok(
            Expr::number(BigDecimal::from(1)),
            Some(TypeName::Result {
                ok: Some(Box::new(TypeName::U64)),
                error: None,
            }),
        )
        .with_inferred_type(InferredType::result(Some(InferredType::u64()), None));

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_bind_type_in_result_3() {
        // Data associated with only error case
        let expr_str = r#"
            err(1): result<_, u64>
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::err(
            Expr::number(BigDecimal::from(1)),
            Some(TypeName::Result {
                ok: None,
                error: Some(Box::new(TypeName::U64)),
            }),
        )
        .with_inferred_type(InferredType::result(None, Some(InferredType::u64())));

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_bind_type_in_result_4() {
        // Don't care the data associated with either case
        let expr_str = r#"
            ok(1): result
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();
        expr.bind_type_annotations();

        let expected = Expr::ok(
            Expr::number(BigDecimal::from(1)),
            Some(TypeName::Result {
                ok: None,
                error: None,
            }),
        )
        .with_inferred_type(InferredType::result(None, None));

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_bind_type_select_field() {
        let expr_str = r#"
            foo.bar.baz: u32
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::select_field(
            Expr::select_field(
                Expr::identifier_with_variable_id(VariableId::Global("foo".to_string()), None),
                "bar",
                None,
            ),
            "baz",
            Some(TypeName::U32),
        )
        .with_inferred_type(InferredType::u32());

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_bind_type_select_index() {
        let expr_str = r#"
            foo.bar.baz[1]: u32
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::select_index(
            Expr::select_field(
                Expr::select_field(
                    Expr::identifier_with_variable_id(VariableId::Global("foo".to_string()), None),
                    "bar",
                    None,
                ),
                "baz",
                None,
            ),
            Expr::number(BigDecimal::from(1)),
        )
        .with_type_annotation(TypeName::U32)
        .with_inferred_type(InferredType::u32());

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_bind_type_on_both_sides() {
        let expr_str = r#"
            let x: u64 = 1u64
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::let_binding_with_variable_id(
            VariableId::global("x".to_string()),
            Expr::number_inferred(
                BigDecimal::from(1),
                Some(TypeName::U64),
                InferredType::u64(),
            ),
            Some(TypeName::U64),
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_binding_in_block() {
        let expr_str = r#"
            let x = {
              let y: u64 = 1;
              y
            }
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::let_binding_with_variable_id(
            VariableId::global("x".to_string()),
            Expr::expr_block(vec![
                Expr::let_binding_with_variable_id(
                    VariableId::global("y".to_string()),
                    Expr::number_inferred(BigDecimal::from(1), None, InferredType::u64()),
                    Some(TypeName::U64),
                ),
                Expr::identifier_with_variable_id(VariableId::global("y".to_string()), None),
            ])
            .with_inferred_type(InferredType::unknown()),
            None,
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_binding_in_match_expr() {
        let expr_str = r#"
            match x {
              a => 2u64
            }
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::pattern_match(
            Expr::identifier_with_variable_id(VariableId::global("x".to_string()), None),
            vec![MatchArm {
                arm_pattern: ArmPattern::Literal(Box::new(Expr::identifier_with_variable_id(
                    VariableId::global("a".to_string()),
                    None,
                ))),
                arm_resolution_expr: Box::new(Expr::number_inferred(
                    BigDecimal::from(2),
                    Some(TypeName::U64),
                    InferredType::u64(),
                )),
            }],
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_binding_in_if_else() {
        let expr_str = r#"
            if x then
              1u64
            else
              2u64
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::cond(
            Expr::identifier_with_variable_id(VariableId::global("x".to_string()), None),
            Expr::number_inferred(
                BigDecimal::from(1),
                Some(TypeName::U64),
                InferredType::u64(),
            ),
            Expr::number_inferred(
                BigDecimal::from(2),
                Some(TypeName::U64),
                InferredType::u64(),
            ),
        );

        assert_eq!(expr, expected);
    }
}
