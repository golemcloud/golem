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

use crate::Expr;
use std::collections::VecDeque;

pub(crate) fn bind_type_annotations(expr: &mut Expr) {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Let {
                type_annotation,
                expr,
                ..
            } => {
                if let Some(type_name) = type_annotation {
                    expr.with_inferred_type_mut(type_name.clone().into());
                }
                queue.push_back(expr);
            }

            Expr::Number {
                type_annotation,
                inferred_type,
                ..
            } => {
                if let Some(type_name) = type_annotation {
                    *inferred_type = type_name.clone().into();
                }
            }

            Expr::SelectField {
                expr,
                type_annotation,
                inferred_type,
                ..
            } => {
                if let Some(type_name) = type_annotation {
                    *inferred_type = type_name.clone().into();
                }
                queue.push_back(expr);
            }

            Expr::SelectIndex {
                expr,
                type_annotation,
                inferred_type,
                ..
            } => {
                if let Some(type_name) = type_annotation {
                    *inferred_type = type_name.clone().into();
                }
                queue.push_back(expr);
            }

            Expr::SelectDynamic {
                expr,
                index,
                type_annotation,
                inferred_type,
                ..
            } => {
                if let Some(type_name) = type_annotation {
                    *inferred_type = type_name.clone().into();
                }
                queue.push_back(expr);
                queue.push_back(index);
            }

            Expr::Identifier {
                type_annotation,
                inferred_type,
                ..
            } => {
                if let Some(type_name) = type_annotation {
                    *inferred_type = type_name.clone().into();
                }
            }

            Expr::Option {
                expr,
                type_annotation,
                inferred_type,
                ..
            } => {
                if let Some(type_name) = type_annotation {
                    *inferred_type = type_name.clone().into();
                }

                if let Some(expr) = expr {
                    queue.push_back(expr);
                }
            }

            Expr::Result {
                expr,
                type_annotation,
                inferred_type,
                ..
            } => {
                if let Some(type_name) = type_annotation {
                    *inferred_type = type_name.clone().into();
                }

                match expr {
                    Ok(expr) => queue.push_back(expr),
                    Err(expr) => queue.push_back(expr),
                }
            }

            Expr::Sequence {
                exprs,
                type_annotation,
                inferred_type,
                ..
            } => {
                if let Some(type_name) = type_annotation {
                    *inferred_type = type_name.clone().into();
                }

                for expr in exprs {
                    queue.push_back(expr);
                }
            }

            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }
}

#[cfg(test)]
mod type_binding_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use super::*;
    use crate::parser::type_name::TypeName;
    use crate::{ArmPattern, InferredType, MatchArm, VariableId};

    #[test]
    fn test_bind_type_in_let() {
        let expr_str = r#"
            let x: u64 = 1
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::let_binding(
            "x",
            Expr::number(BigDecimal::from(1), None, InferredType::U64),
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

        let expected = Expr::option_with_type_annotation(
            Some(Expr::number(
                BigDecimal::from(1),
                None,
                InferredType::number(),
            )),
            TypeName::Option(Box::new(TypeName::U64)),
        )
        .with_inferred_type(InferredType::Option(Box::new(InferredType::U64)));

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
            Expr::number(BigDecimal::from(1), None, InferredType::number()),
            Some(TypeName::Result {
                ok: Some(Box::new(TypeName::U64)),
                error: Some(Box::new(TypeName::Str)),
            }),
        )
        .with_inferred_type(InferredType::Result {
            ok: Some(Box::new(InferredType::U64)),
            error: Some(Box::new(InferredType::Str)),
        });

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
            Expr::number(BigDecimal::from(1), None, InferredType::number()),
            Some(TypeName::Result {
                ok: Some(Box::new(TypeName::U64)),
                error: None,
            }),
        )
        .with_inferred_type(InferredType::Result {
            ok: Some(Box::new(InferredType::U64)),
            error: None,
        });

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
            Expr::number(BigDecimal::from(1), None, InferredType::number()),
            Some(TypeName::Result {
                ok: None,
                error: Some(Box::new(TypeName::U64)),
            }),
        )
        .with_inferred_type(InferredType::Result {
            ok: None,
            error: Some(Box::new(InferredType::U64)),
        });

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
            Expr::number(BigDecimal::from(1), None, InferredType::number()),
            Some(TypeName::Result {
                ok: None,
                error: None,
            }),
        )
        .with_inferred_type(InferredType::Result {
            ok: None,
            error: None,
        });

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
        .with_inferred_type(InferredType::U32);

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_bind_type_select_index() {
        let expr_str = r#"
            foo.bar.baz[1]: u32
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_type_annotations();

        let expected = Expr::select_dynamic(
            Expr::select_field(
                Expr::select_field(
                    Expr::identifier_with_variable_id(VariableId::Global("foo".to_string()), None),
                    "bar",
                    None,
                ),
                "baz",
                None,
            ),
            Expr::number(BigDecimal::from(1), None, InferredType::number()),
            Some(TypeName::U32),
        )
        .with_inferred_type(InferredType::U32);

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
            Expr::number(BigDecimal::from(1), Some(TypeName::U64), InferredType::U64),
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
                    Expr::number(BigDecimal::from(1), None, InferredType::U64),
                    Some(TypeName::U64),
                ),
                Expr::identifier_with_variable_id(VariableId::global("y".to_string()), None),
            ])
            .with_inferred_type(InferredType::Unknown),
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
                arm_resolution_expr: Box::new(Expr::number(
                    BigDecimal::from(2),
                    Some(TypeName::U64),
                    InferredType::U64,
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
            Expr::number(BigDecimal::from(1), Some(TypeName::U64), InferredType::U64),
            Expr::number(BigDecimal::from(2), Some(TypeName::U64), InferredType::U64),
        );

        assert_eq!(expr, expected);
    }
}
