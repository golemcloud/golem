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

pub(crate) fn bind_type(expr: &mut Expr) {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Let(_, optional_type_name, expr, _) => {
                if let Some(type_name) = optional_type_name {
                    internal::override_type(expr, type_name.clone().into());
                }
                queue.push_back(expr);
            }

            Expr::Number(_, optional_type_name, inferred_type) => {
                if let Some(type_name) = optional_type_name {
                    *inferred_type = type_name.clone().into();
                }
            }

            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }
}

mod internal {
    use crate::{Expr, InferredType};

    pub(crate) fn override_type(expr: &mut Expr, new_type: InferredType) {
        match expr {
            Expr::Identifier(_, inferred_type)
            | Expr::Let(_, _, _, inferred_type)
            | Expr::SelectField(_, _, inferred_type)
            | Expr::SelectIndex(_, _, inferred_type)
            | Expr::Sequence(_, inferred_type)
            | Expr::Record(_, inferred_type)
            | Expr::Tuple(_, inferred_type)
            | Expr::Literal(_, inferred_type)
            | Expr::Number(_, _, inferred_type)
            | Expr::Flags(_, inferred_type)
            | Expr::Boolean(_, inferred_type)
            | Expr::Concat(_, inferred_type)
            | Expr::ExprBlock(_, inferred_type)
            | Expr::Not(_, inferred_type)
            | Expr::GreaterThan(_, _, inferred_type)
            | Expr::GreaterThanOrEqualTo(_, _, inferred_type)
            | Expr::LessThanOrEqualTo(_, _, inferred_type)
            | Expr::EqualTo(_, _, inferred_type)
            | Expr::LessThan(_, _, inferred_type)
            | Expr::Plus(_, _, inferred_type)
            | Expr::Minus(_, _, inferred_type)
            | Expr::Divide(_, _, inferred_type)
            | Expr::Multiply(_, _, inferred_type)
            | Expr::Cond(_, _, _, inferred_type)
            | Expr::PatternMatch(_, _, inferred_type)
            | Expr::Option(_, inferred_type)
            | Expr::Result(_, inferred_type)
            | Expr::Unwrap(_, inferred_type)
            | Expr::Throw(_, inferred_type)
            | Expr::GetTag(_, inferred_type)
            | Expr::And(_, _, inferred_type)
            | Expr::Or(_, _, inferred_type)
            | Expr::ListComprehension { inferred_type, .. }
            | Expr::ListReduce { inferred_type, .. }
            | Expr::Call(_, _, inferred_type) => {
                *inferred_type = new_type;
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
    use crate::{ArmPattern, InferredType, MatchArm, Number, VariableId};

    #[test]
    fn test_bind_type() {
        let expr_str = r#"
            let x: u64 = 1
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_types();

        let expected = Expr::Let(
            VariableId::global("x".to_string()),
            Some(TypeName::U64),
            Box::new(Expr::Number(
                Number {
                    value: BigDecimal::from(1),
                },
                None,
                InferredType::U64,
            )),
            InferredType::Unknown,
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_bind_type_on_both_sides() {
        let expr_str = r#"
            let x: u64 = 1u64
        "#;

        let mut expr = Expr::from_text(expr_str).unwrap();

        expr.bind_types();

        let expected = Expr::Let(
            VariableId::global("x".to_string()),
            Some(TypeName::U64),
            Box::new(Expr::Number(
                Number {
                    value: BigDecimal::from(1),
                },
                Some(TypeName::U64),
                InferredType::U64,
            )),
            InferredType::Unknown,
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

        expr.bind_types();

        let expected = Expr::Let(
            VariableId::global("x".to_string()),
            None,
            Box::new(Expr::ExprBlock(
                vec![
                    Expr::Let(
                        VariableId::global("y".to_string()),
                        Some(TypeName::U64),
                        Box::new(Expr::Number(
                            Number {
                                value: BigDecimal::from(1),
                            },
                            None,
                            InferredType::U64,
                        )),
                        InferredType::Unknown,
                    ),
                    Expr::Identifier(VariableId::global("y".to_string()), InferredType::Unknown),
                ],
                InferredType::Unknown,
            )),
            InferredType::Unknown,
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

        expr.bind_types();

        let expected = Expr::PatternMatch(
            Box::new(Expr::Identifier(
                VariableId::global("x".to_string()),
                InferredType::Unknown,
            )),
            vec![MatchArm {
                arm_pattern: ArmPattern::Literal(Box::new(Expr::Identifier(
                    VariableId::global("a".to_string()),
                    InferredType::Unknown,
                ))),
                arm_resolution_expr: Box::new(Expr::Number(
                    Number {
                        value: BigDecimal::from(2),
                    },
                    Some(TypeName::U64),
                    InferredType::U64,
                )),
            }],
            InferredType::Unknown,
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

        expr.bind_types();

        let expected = Expr::Cond(
            Box::new(Expr::Identifier(
                VariableId::global("x".to_string()),
                InferredType::Unknown,
            )),
            Box::new(Expr::Number(
                Number {
                    value: BigDecimal::from(1),
                },
                Some(TypeName::U64),
                InferredType::U64,
            )),
            Box::new(Expr::Number(
                Number {
                    value: BigDecimal::from(2),
                },
                Some(TypeName::U64),
                InferredType::U64,
            )),
            InferredType::Unknown,
        );

        assert_eq!(expr, expected);
    }
}
