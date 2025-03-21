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

use crate::{Expr, ExprVisitor, InferredType};

// Given `f` executes inference, find expr where `f(expr) = expr`
pub fn type_inference_fix_point<F, E>(mut scan_and_infer: F, expr: &mut Expr) -> Result<(), E>
where
    F: FnMut(&mut Expr) -> Result<(), E>,
{
    loop {
        let mut original = expr.clone();

        scan_and_infer(expr)?;

        if compare_expr_types(&mut original, expr) {
            break;
        }
    }

    Ok(())
}

fn compare_expr_types(left: &mut Expr, right: &mut Expr) -> bool {
    let mut left_stack = ExprVisitor::bottom_up(left);
    let mut right_stack = ExprVisitor::bottom_up(right);

    while let (Some(left), Some(right)) = (left_stack.pop_front(), right_stack.pop_front()) {
        if !compare_inferred_types(&left.inferred_type(), &right.inferred_type()) {
            return false;
        }
    }

    left_stack.is_empty() && right_stack.is_empty()
}

fn compare_inferred_types(left: &InferredType, right: &InferredType) -> bool {
    compare_inferred_types_internal(left, right, true)
}

fn compare_inferred_types_internal(left: &InferredType, right: &InferredType, bool: bool) -> bool {
    match (left, right) {
        // AlLOf(AllOf(Str, Int), Unknown)
        (InferredType::AllOf(left), InferredType::AllOf(right)) => {
            left.iter().all(|left| {
                right
                    .iter()
                    .any(|right| compare_inferred_types_internal(left, right, false))
            }) && right.iter().all(|right| {
                left.iter()
                    .any(|left| compare_inferred_types_internal(left, right, false))
            })
        }
        // a precise type is converted to a less precise type, and hence false
        (InferredType::AllOf(left), InferredType::OneOf(right)) => {
            left.iter().all(|left| {
                right
                    .iter()
                    .any(|right| compare_inferred_types_internal(left, right, false))
            }) && right.iter().all(|right| {
                left.iter()
                    .any(|left| compare_inferred_types_internal(left, right, false))
            })
        }
        // Converted a less precise type to a more precise type, and hence false
        (InferredType::OneOf(_), InferredType::AllOf(_)) => false,

        (InferredType::OneOf(left), InferredType::OneOf(right)) => {
            left.iter().all(|left| {
                right
                    .iter()
                    .any(|right| compare_inferred_types_internal(left, right, false))
            }) && right.iter().all(|right| {
                left.iter()
                    .any(|left| compare_inferred_types_internal(left, right, false))
            })
        }

        // More precision this time and therefore false
        (InferredType::AllOf(left), inferred_type) => {
            if bool {
                left.iter()
                    .all(|left| compare_inferred_types_internal(left, inferred_type, true))
            } else {
                left.iter()
                    .any(|left| compare_inferred_types_internal(left, inferred_type, true))
            }
        }

        // Less precision this time and therefore false
        (inferred_type, InferredType::AllOf(right)) => {
            if bool {
                right
                    .iter()
                    .all(|left| compare_inferred_types_internal(left, inferred_type, true))
            } else {
                right
                    .iter()
                    .any(|left| compare_inferred_types_internal(left, inferred_type, true))
            }
        }

        (InferredType::Record(left), InferredType::Record(right)) => {
            left.iter().all(|(key, value)| {
                if let Some(right_value) = right
                    .iter()
                    .find(|(right_key, _)| key == right_key)
                    .map(|(_, value)| value)
                {
                    compare_inferred_types_internal(value, right_value, true)
                } else {
                    true
                }
            }) && right.iter().all(|(key, value)| {
                if let Some(left_value) = left
                    .iter()
                    .find(|(left_key, _)| key == left_key)
                    .map(|(_, value)| value)
                {
                    compare_inferred_types_internal(value, left_value, true)
                } else {
                    true
                }
            })
        }

        (InferredType::Tuple(left), InferredType::Tuple(right)) => left
            .iter()
            .zip(right.iter())
            .all(|(left, right)| compare_inferred_types_internal(left, right, true)),

        (InferredType::List(left), InferredType::List(right)) => {
            compare_inferred_types_internal(left, right, true)
        }

        (InferredType::Option(left), InferredType::Option(right)) => {
            compare_inferred_types_internal(left, right, true)
        }

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
                (Some(left_ok), Some(right_ok)) => {
                    compare_inferred_types_internal(left_ok, right_ok, true)
                }
                (None, None) => true,
                _ => false,
            };

            let error = match (left_error, right_error) {
                (Some(left_error), Some(right_error)) => {
                    compare_inferred_types_internal(left_error, right_error, true)
                }
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

#[cfg(test)]
mod tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::parser::type_name::TypeName;
    use crate::type_inference::inference_fix_point::{compare_expr_types, compare_inferred_types};
    use crate::{Expr, FunctionTypeRegistry, InferredType, VariableId};

    #[test]
    fn test_inferred_type_equality_1() {
        let left = InferredType::Str;
        let right = InferredType::Str;
        assert!(compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_2() {
        let left = InferredType::Unknown;
        let right = InferredType::Unknown;
        assert!(compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_3() {
        let left = InferredType::Unknown;
        let right = InferredType::Str;
        assert!(!compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_4() {
        let left = InferredType::Str;
        let right = InferredType::Unknown;
        assert!(!compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_5() {
        let left = InferredType::Unknown;
        let right = InferredType::AllOf(vec![InferredType::Str]);

        assert!(!compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_6() {
        let left = InferredType::Unknown;
        let right = InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]);

        assert!(!compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_7() {
        let left = InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]);
        let right = InferredType::AllOf(vec![InferredType::AllOf(vec![
            InferredType::Str,
            InferredType::Unknown,
        ])]);

        assert!(compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_8() {
        let left = InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]);
        let right = InferredType::AllOf(vec![
            InferredType::U64,
            InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]),
        ]);

        assert!(!compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_9() {
        let left = InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]);
        let right = InferredType::AllOf(vec![
            InferredType::Str,
            InferredType::Unknown,
            InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]),
        ]);

        assert!(compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_10() {
        let left = InferredType::AllOf(vec![
            InferredType::Str,
            InferredType::Unknown,
            InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]),
        ]);
        let right = InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]);

        assert!(compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_11() {
        let left = InferredType::OneOf(vec![InferredType::U64, InferredType::U32]);
        let right = InferredType::AllOf(vec![
            InferredType::Str,
            InferredType::Unknown,
            InferredType::OneOf(vec![InferredType::U64, InferredType::U32]),
        ]);

        assert!(!compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_12() {
        let left = InferredType::Unknown;
        let right = InferredType::OneOf(vec![
            InferredType::Str,
            InferredType::Unknown,
            InferredType::OneOf(vec![InferredType::U64, InferredType::U32]),
        ]);

        assert!(!compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_expr_comparison_1() {
        let mut left = Expr::identifier_global("x", None);
        let mut right = Expr::identifier_global("x", None);

        assert!(compare_expr_types(&mut left, &mut right));
    }

    #[test]
    fn test_expr_comparison_2() {
        let left = InferredType::AllOf(vec![
            InferredType::Str,
            InferredType::Unknown,
            InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]),
        ]);
        let right = InferredType::AllOf(vec![InferredType::Str, InferredType::Unknown]);

        let mut left = Expr::identifier_global("x", None).merge_inferred_type(left);
        let mut right = Expr::identifier_global("x", None).merge_inferred_type(right);

        assert!(compare_expr_types(&mut left, &mut right));
    }

    #[test]
    fn test_expr_comparison_3() {
        let left = InferredType::Unknown;
        let right = InferredType::OneOf(vec![
            InferredType::Str,
            InferredType::Unknown,
            InferredType::OneOf(vec![InferredType::U64, InferredType::U32]),
        ]);

        let mut left = Expr::identifier_global("x", None).merge_inferred_type(left);
        let mut right = Expr::identifier_global("x", None).merge_inferred_type(right);

        assert!(!compare_expr_types(&mut left, &mut right));
    }

    #[test]
    fn test_expr_comparison_4() {
        let left_identifier = Expr::identifier_global("x", None);
        let right_identifier = Expr::identifier_global("x", None);

        let mut left = Expr::let_binding("x", left_identifier, None);
        let mut right = Expr::let_binding("x", right_identifier, None);

        assert!(compare_expr_types(&mut left, &mut right));
    }

    #[test]
    fn test_expr_comparison_5() {
        let left_identifier = Expr::identifier_global("x", None);
        let right_identifier = Expr::identifier_global("x", None);
        let cond = Expr::greater_than(left_identifier, right_identifier);
        let then_ = Expr::identifier_global("x", None);
        let else_ = Expr::identifier_global("x", None);

        let mut left = Expr::cond(cond.clone(), then_.clone(), else_.clone());
        let mut right = Expr::cond(cond, then_, else_);

        assert!(compare_expr_types(&mut left, &mut right));
    }

    #[test]
    fn test_fix_point() {
        let expr = r#"
        let x: u64 = 1;
        if x == x then x else y
        "#;

        let mut expr = Expr::from_text(expr).unwrap();
        expr.infer_types(&FunctionTypeRegistry::empty(), &vec![])
            .unwrap();
        let expected = Expr::expr_block(vec![
            Expr::let_binding_with_variable_id(
                VariableId::local("x", 0),
                Expr::number_inferred(BigDecimal::from(1), None, InferredType::U64),
                Some(TypeName::U64),
            ),
            Expr::cond(
                Expr::equal_to(
                    Expr::identifier_local("x", 0, None).with_inferred_type(InferredType::U64),
                    Expr::identifier_local("x", 0, None).with_inferred_type(InferredType::U64),
                ),
                Expr::identifier_local("x", 0, None).with_inferred_type(InferredType::U64),
                Expr::identifier_global("y", None).with_inferred_type(InferredType::U64),
            )
            .with_inferred_type(InferredType::U64),
        ])
        .with_inferred_type(InferredType::U64);

        assert_eq!(expr, expected)
    }
}
