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

use crate::{Expr, ExprVisitor, InferredType, TypeInternal};

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
    compare_inferred_types_internal(left.internal_type(), right.internal_type(), true)
}

fn compare_inferred_types_internal(left: &TypeInternal, right: &TypeInternal, bool: bool) -> bool {
    match (left, right) {
        // AlLOf(AllOf(Str, Int), Unknown)
        (TypeInternal::AllOf(left), TypeInternal::AllOf(right)) => {
            left.iter().all(|left| {
                right.iter().any(|right| {
                    compare_inferred_types_internal(
                        left.internal_type(),
                        right.internal_type(),
                        false,
                    )
                })
            }) && right.iter().all(|right| {
                left.iter().any(|left| {
                    compare_inferred_types_internal(
                        left.internal_type(),
                        right.internal_type(),
                        false,
                    )
                })
            })
        }

        // More precision this time and therefore false
        (TypeInternal::AllOf(left), inferred_type) => {
            if bool {
                left.iter().all(|left| {
                    compare_inferred_types_internal(left.internal_type(), inferred_type, true)
                })
            } else {
                left.iter().any(|left| {
                    compare_inferred_types_internal(left.internal_type(), inferred_type, true)
                })
            }
        }

        // Less precision this time and therefore false
        (inferred_type, TypeInternal::AllOf(right)) => {
            if bool {
                right.iter().all(|left| {
                    compare_inferred_types_internal(left.internal_type(), inferred_type, true)
                })
            } else {
                right.iter().any(|left| {
                    compare_inferred_types_internal(left.internal_type(), inferred_type, true)
                })
            }
        }

        (TypeInternal::Record(left), TypeInternal::Record(right)) => {
            left.iter().all(|(key, value)| {
                if let Some(right_value) = right
                    .iter()
                    .find(|(right_key, _)| key == right_key)
                    .map(|(_, value)| value)
                {
                    compare_inferred_types_internal(
                        value.internal_type(),
                        right_value.internal_type(),
                        true,
                    )
                } else {
                    true
                }
            }) && right.iter().all(|(key, value)| {
                if let Some(left_value) = left
                    .iter()
                    .find(|(left_key, _)| key == left_key)
                    .map(|(_, value)| value)
                {
                    compare_inferred_types_internal(
                        value.internal_type(),
                        left_value.internal_type(),
                        true,
                    )
                } else {
                    true
                }
            })
        }

        (TypeInternal::Tuple(left), TypeInternal::Tuple(right)) => {
            left.iter().zip(right.iter()).all(|(left, right)| {
                compare_inferred_types_internal(left.internal_type(), right.internal_type(), true)
            })
        }

        (TypeInternal::List(left), TypeInternal::List(right)) => {
            compare_inferred_types_internal(left.internal_type(), right.internal_type(), true)
        }

        (TypeInternal::Option(left), TypeInternal::Option(right)) => {
            compare_inferred_types_internal(left.internal_type(), right.internal_type(), true)
        }

        (
            TypeInternal::Result {
                ok: left_ok,
                error: left_error,
            },
            TypeInternal::Result {
                ok: right_ok,
                error: right_error,
            },
        ) => {
            let ok = match (left_ok, right_ok) {
                (Some(left_ok), Some(right_ok)) => compare_inferred_types_internal(
                    left_ok.internal_type(),
                    right_ok.internal_type(),
                    true,
                ),
                (None, None) => true,
                _ => false,
            };

            let error = match (left_error, right_error) {
                (Some(left_error), Some(right_error)) => compare_inferred_types_internal(
                    left_error.internal_type(),
                    right_error.internal_type(),
                    true,
                ),
                (None, None) => true,
                _ => false,
            };

            ok && error
        }

        (TypeInternal::Flags(left), TypeInternal::Flags(right)) => left == right,

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
        let left = InferredType::string();
        let right = InferredType::string();
        assert!(compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_2() {
        let left = InferredType::unknown();
        let right = InferredType::unknown();
        assert!(compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_3() {
        let left = InferredType::unknown();
        let right = InferredType::string();
        assert!(!compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_4() {
        let left = InferredType::string();
        let right = InferredType::unknown();
        assert!(!compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_5() {
        let left = InferredType::unknown();
        let right = InferredType::all_of(vec![InferredType::string()]);

        assert!(!compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_6() {
        let left = InferredType::unknown();
        let right = InferredType::all_of(vec![InferredType::string(), InferredType::unknown()]);

        assert!(!compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_7() {
        let left = InferredType::all_of(vec![InferredType::string(), InferredType::unknown()]);
        let right = InferredType::all_of(vec![InferredType::all_of(vec![
            InferredType::string(),
            InferredType::unknown(),
        ])]);

        assert!(compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_8() {
        let left = InferredType::all_of(vec![InferredType::string(), InferredType::unknown()]);
        let right = InferredType::all_of(vec![
            InferredType::u64(),
            InferredType::all_of(vec![InferredType::string(), InferredType::unknown()]),
        ]);

        assert!(!compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_9() {
        let left = InferredType::all_of(vec![InferredType::string(), InferredType::unknown()]);

        let right = InferredType::all_of(vec![
            InferredType::string(),
            InferredType::unknown(),
            InferredType::all_of(vec![InferredType::string(), InferredType::unknown()]),
        ]);

        assert!(compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_inferred_type_equality_10() {
        let left = InferredType::all_of(vec![
            InferredType::string(),
            InferredType::unknown(),
            InferredType::all_of(vec![InferredType::string(), InferredType::unknown()]),
        ]);

        let right = InferredType::all_of(vec![InferredType::string(), InferredType::unknown()]);

        assert!(compare_inferred_types(&left, &right));
    }

    #[test]
    fn test_expr_comparison_1() {
        let mut left = Expr::identifier_global("x", None);
        let mut right = Expr::identifier_global("x", None);

        assert!(compare_expr_types(&mut left, &mut right));
    }

    #[test]
    fn test_expr_comparison_2() {
        let left = InferredType::all_of(vec![
            InferredType::string(),
            InferredType::unknown(),
            InferredType::all_of(vec![InferredType::string(), InferredType::unknown()]),
        ]);

        let right = InferredType::all_of(vec![InferredType::string(), InferredType::unknown()]);

        let mut left = Expr::identifier_global("x", None).merge_inferred_type(left);
        let mut right = Expr::identifier_global("x", None).merge_inferred_type(right);

        assert!(compare_expr_types(&mut left, &mut right));
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
                Expr::number_inferred(BigDecimal::from(1), None, InferredType::u64()),
                Some(TypeName::U64),
            ),
            Expr::cond(
                Expr::equal_to(
                    Expr::identifier_local("x", 0, None).with_inferred_type(InferredType::u64()),
                    Expr::identifier_local("x", 0, None).with_inferred_type(InferredType::u64()),
                ),
                Expr::identifier_local("x", 0, None).with_inferred_type(InferredType::u64()),
                Expr::identifier_global("y", None).with_inferred_type(InferredType::u64()),
            )
            .with_inferred_type(InferredType::u64()),
        ])
        .with_inferred_type(InferredType::u64());

        assert_eq!(expr, expected)
    }
}
