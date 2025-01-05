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

// Given f executes inference, find expr where f(expr) = expr
pub fn type_inference_fix_point<F, E>(mut scan_and_infer: F, expr: &mut Expr) -> Result<(), E>
where
    F: FnMut(&mut Expr) -> Result<(), E>,
{
    loop {
        let original = expr.clone();

        scan_and_infer(expr)?;

        let terminated = internal::equivalent_exprs(&original, expr);

        if terminated {
            break;
        }
    }

    Ok(())
}

mod internal {
    use crate::type_inference::inference_fix_point::internal;
    use crate::{Expr, InferredType};
    use std::collections::VecDeque;

    pub(crate) fn equivalent_exprs(left: &Expr, right: &Expr) -> bool {
        let mut queue1 = VecDeque::new();
        let mut left_stack = vec![];

        queue1.push_back(left);

        while let Some(expr) = queue1.pop_back() {
            left_stack.push(expr);
            expr.visit_children_bottom_up(&mut queue1);
        }

        let mut queue2 = VecDeque::new();
        queue2.push_back(right);
        let mut right_stack = vec![];

        while let Some(expr) = queue2.pop_back() {
            right_stack.push(expr);
            expr.visit_children_bottom_up(&mut queue2);
        }

        while let Some(left) = left_stack.pop() {
            let right = right_stack.pop();

            if let Some(right) = right {
                if internal::non_equivalent_types(&left.inferred_type(), &right.inferred_type()) {
                    return false;
                }
            }
        }

        true
    }

    pub(crate) fn equivalent_types(left: &InferredType, right: &InferredType) -> bool {
        compare(left, right, true)
    }
    pub(crate) fn non_equivalent_types(left: &InferredType, right: &InferredType) -> bool {
        !equivalent_types(left, right)
    }
    pub(crate) fn compare(left: &InferredType, right: &InferredType, bool: bool) -> bool {
        match (left, right) {
            // AlLOf(AllOf(Str, Int), Unknown)
            (InferredType::AllOf(left), InferredType::AllOf(right)) => {
                left.iter()
                    .all(|left| right.iter().any(|right| compare(left, right, false)))
                    && right
                        .iter()
                        .all(|right| left.iter().any(|left| compare(left, right, false)))
            }
            // a precise type is converted to a less precise type, and hence false
            (InferredType::AllOf(left), InferredType::OneOf(right)) => {
                left.iter()
                    .all(|left| right.iter().any(|right| compare(left, right, false)))
                    && right
                        .iter()
                        .all(|right| left.iter().any(|left| compare(left, right, false)))
            }
            // Converted a less precise type to a more precise type, and hence false
            (InferredType::OneOf(_), InferredType::AllOf(_)) => false,

            (InferredType::OneOf(left), InferredType::OneOf(right)) => {
                left.iter()
                    .all(|left| right.iter().any(|right| compare(left, right, false)))
                    && right
                        .iter()
                        .all(|right| left.iter().any(|left| compare(left, right, false)))
            }

            // More precision this time and therefore false
            (InferredType::AllOf(left), inferred_type) => {
                if bool {
                    left.iter().all(|left| compare(left, inferred_type, true))
                } else {
                    left.iter().any(|left| compare(left, inferred_type, true))
                }
            }

            // Less precision this time and therefore false
            (inferred_type, InferredType::AllOf(right)) => {
                if bool {
                    right.iter().all(|left| compare(left, inferred_type, true))
                } else {
                    right.iter().any(|left| compare(left, inferred_type, true))
                }
            }

            (InferredType::Record(left), InferredType::Record(right)) => {
                left.iter().all(|(key, value)| {
                    if let Some(right_value) = right
                        .iter()
                        .find(|(right_key, _)| key == right_key)
                        .map(|(_, value)| value)
                    {
                        compare(value, right_value, true)
                    } else {
                        true
                    }
                }) && right.iter().all(|(key, value)| {
                    if let Some(left_value) = left
                        .iter()
                        .find(|(left_key, _)| key == left_key)
                        .map(|(_, value)| value)
                    {
                        compare(value, left_value, true)
                    } else {
                        true
                    }
                })
            }

            (InferredType::Tuple(left), InferredType::Tuple(right)) => left
                .iter()
                .zip(right.iter())
                .all(|(left, right)| compare(left, right, true)),

            (InferredType::List(left), InferredType::List(right)) => compare(left, right, true),

            (InferredType::Option(left), InferredType::Option(right)) => compare(left, right, true),

            (
                InferredType::Result {
                    ok: left_ok,
                    error: left_error,
                },
                InferredType::Result {
                    ok: right_ok,
                    error: right_error,
                },
            ) => {
                let ok = match (left_ok, right_ok) {
                    (Some(left_ok), Some(right_ok)) => compare(left_ok, right_ok, true),
                    (None, None) => true,
                    _ => false,
                };

                let error = match (left_error, right_error) {
                    (Some(left_error), Some(right_error)) => compare(left_error, right_error, true),
                    (None, None) => true,
                    _ => false,
                };

                ok && error
            }

            (InferredType::Flags(left), InferredType::Flags(right)) => left == right,

            (InferredType::OneOf(_), _inferred_type) => false,

            (_inferred_type, InferredType::OneOf(_)) => false,

            (left, right) => left == right,
        }
    }
}

#[cfg(test)]
mod tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::parser::type_name::TypeName;
    use crate::type_inference::inference_fix_point::internal::{
        equivalent_exprs, equivalent_types, non_equivalent_types,
    };
    use crate::{Expr, FunctionTypeRegistry, InferredType, Number, VariableId};

    #[test]
    fn test_inferred_type_equality_1() {
        let left = InferredType::Str;
        let right = InferredType::Str;
        assert!(equivalent_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_2() {
        let left = InferredType::Unknown;
        let right = InferredType::Unknown;
        assert!(equivalent_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_3() {
        let left = InferredType::Unknown;
        let right = InferredType::Str;
        assert!(!equivalent_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_4() {
        let left = InferredType::Str;
        let right = InferredType::Unknown;
        assert!(!equivalent_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_5() {
        let left = InferredType::Unknown;
        let right = InferredType::AllOf(vec![InferredType::Str]);

        assert!(!equivalent_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_6() {
        let left = InferredType::Unknown;
        let right = InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]);

        assert!(non_equivalent_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_7() {
        let left = InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]);
        let right = InferredType::AllOf(vec![InferredType::AllOf(vec![
            InferredType::Str,
            InferredType::Unknown,
        ])]);

        assert!(equivalent_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_8() {
        let left = InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]);
        let right = InferredType::AllOf(vec![
            InferredType::U64,
            InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]),
        ]);

        assert!(non_equivalent_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_9() {
        let left = InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]);
        let right = InferredType::AllOf(vec![
            InferredType::Str,
            InferredType::Unknown,
            InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]),
        ]);

        assert!(equivalent_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_10() {
        let left = InferredType::AllOf(vec![
            InferredType::Str,
            InferredType::Unknown,
            InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]),
        ]);
        let right = InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]);

        assert!(equivalent_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_11() {
        let left = InferredType::OneOf(vec![InferredType::U64, InferredType::U32]);
        let right = InferredType::AllOf(vec![
            InferredType::Str,
            InferredType::Unknown,
            InferredType::OneOf(vec![InferredType::U64, InferredType::U32]),
        ]);

        assert!(non_equivalent_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_12() {
        let left = InferredType::Unknown;
        let right = InferredType::OneOf(vec![
            InferredType::Str,
            InferredType::Unknown,
            InferredType::OneOf(vec![InferredType::U64, InferredType::U32]),
        ]);

        assert!(non_equivalent_types(&left, &right));
    }

    #[test]
    fn test_expr_comparison_1() {
        let left = Expr::identifier("x");
        let right = Expr::identifier("x");

        assert!(equivalent_exprs(&left, &right));
    }

    #[test]
    fn test_expr_comparison_2() {
        let left = InferredType::AllOf(vec![
            InferredType::Str,
            InferredType::Unknown,
            InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]),
        ]);
        let right = InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]);

        let left = Expr::identifier("x").add_infer_type(left);
        let right = Expr::identifier("x").add_infer_type(right);

        assert!(equivalent_exprs(&left, &right));
    }

    #[test]
    fn test_expr_comparison_3() {
        let left = InferredType::Unknown;
        let right = InferredType::OneOf(vec![
            InferredType::Str,
            InferredType::Unknown,
            InferredType::OneOf(vec![InferredType::U64, InferredType::U32]),
        ]);

        let left = Expr::identifier("x").add_infer_type(left);
        let right = Expr::identifier("x").add_infer_type(right);

        assert!(!equivalent_exprs(&left, &right));
    }

    #[test]
    fn test_expr_comparison_4() {
        let left_identifier = Expr::identifier("x");
        let right_identifier = Expr::identifier("x");

        let left = Expr::let_binding("x", left_identifier);
        let right = Expr::let_binding("x", right_identifier);

        assert!(equivalent_exprs(&left, &right));
    }

    #[test]
    fn test_expr_comparison_5() {
        let left_identifier = Expr::identifier("x");
        let right_identifier = Expr::identifier("x");
        let cond = Expr::greater_than(left_identifier, right_identifier);
        let then_ = Expr::identifier("x");
        let else_ = Expr::identifier("x");

        let left = Expr::cond(cond.clone(), then_.clone(), else_.clone());
        let right = Expr::cond(cond, then_, else_);

        assert!(equivalent_exprs(&left, &right));
    }

    #[test]
    fn test_fix_point() {
        let expr = r#"
        let x: u64 = 1;
        if x == x then x else y
        "#;

        let mut expr = Expr::from_text(expr).unwrap();
        expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();
        let expected = Expr::ExprBlock(
            vec![
                Expr::Let(
                    VariableId::local("x", 0),
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
                Expr::Cond(
                    Box::new(Expr::EqualTo(
                        Box::new(Expr::Identifier(
                            VariableId::local("x", 0),
                            InferredType::U64,
                        )),
                        Box::new(Expr::Identifier(
                            VariableId::local("x", 0),
                            InferredType::U64,
                        )),
                        InferredType::Bool,
                    )),
                    Box::new(Expr::Identifier(
                        VariableId::local("x", 0),
                        InferredType::U64,
                    )),
                    Box::new(Expr::Identifier(
                        VariableId::global("y".to_string()),
                        InferredType::U64,
                    )),
                    InferredType::U64,
                ),
            ],
            InferredType::U64,
        );

        assert_eq!(expr, expected)
    }
}
