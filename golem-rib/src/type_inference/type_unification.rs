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

use crate::call_type::CallType;
use crate::{ArmPattern, Expr, MultipleUnResolvedTypesError, UnResolvedTypesError};

pub fn unify_types(expr: &mut Expr) -> Result<(), MultipleUnResolvedTypesError> {
    let mut queue = vec![];
    queue.push(expr);
    let mut errors: Vec<UnResolvedTypesError> = vec![];

    while let Some(expr) = queue.pop() {
        let expr_copied = expr.clone();

        match expr {
            Expr::Number { inferred_type, .. } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of number: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::Record {
                exprs,
                inferred_type,
                ..
            } => {
                queue.extend(exprs.iter_mut().map(|(_, expr)| &mut **expr));

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of record: {}",
                                    e
                                )),
                        );
                    }
                }
            }
            Expr::Tuple {
                exprs,
                inferred_type,
                ..
            } => {
                queue.extend(exprs.iter_mut());

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of tuple: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::Range {
                range,
                inferred_type,
                ..
            } => {
                for expr in range.get_exprs_mut() {
                    queue.push(expr);
                }

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of range: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::Sequence {
                exprs,
                inferred_type,
                ..
            } => {
                queue.extend(exprs.iter_mut());
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of sequence: {}",
                                    e
                                )),
                        );
                    }
                }
            }
            Expr::Option {
                expr: Some(expr),
                inferred_type,
                ..
            } => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of option: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::Option { inferred_type, .. } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of option: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::Result {
                expr: Ok(expr),
                inferred_type,
                ..
            } => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of result-ok: {}",
                                    e
                                )),
                        );
                    }
                }
            }
            Expr::Result {
                expr: Err(expr),
                inferred_type,
                ..
            } => {
                queue.push(expr);

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of result-err: {}",
                                    e
                                )),
                        );
                    }
                }
            }
            Expr::Cond {
                cond,
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                queue.push(cond);
                queue.push(lhs);
                queue.push(rhs);

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of if-else condition: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::ListComprehension {
                iterable_expr,
                yield_expr,
                inferred_type,
                ..
            } => {
                queue.push(iterable_expr);
                queue.push(yield_expr);

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of list comprehension: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::ListReduce {
                iterable_expr,
                init_value_expr,
                yield_expr,
                inferred_type,
                ..
            } => {
                queue.push(iterable_expr);
                queue.push(init_value_expr);
                queue.push(yield_expr);

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of list aggregation: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::PatternMatch {
                predicate,
                match_arms,
                inferred_type,
                ..
            } => {
                queue.push(predicate);
                for arm in match_arms.iter_mut().rev() {
                    let arm_resolution_expr = &mut *arm.arm_resolution_expr;
                    let arm_pattern: &mut ArmPattern = &mut arm.arm_pattern;
                    internal::push_arm_pattern_expr(arm_pattern, &mut queue);
                    queue.push(arm_resolution_expr);
                }
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of pattern match: {}",
                                    e
                                )),
                        );
                    }
                }
            }
            Expr::Call {
                call_type,
                args,
                inferred_type,
                ..
            } => {
                queue.extend(args.iter_mut());

                match call_type {
                    // We don't care about anything inside instance creation
                    CallType::InstanceCreation(_) => {}
                    // Make sure worker expression in function
                    CallType::Function {
                        worker,
                        function_name,
                    } => {
                        if let Some(worker) = worker {
                            queue.push(worker);
                        }

                        let unified_inferred_type = inferred_type.unify();

                        match unified_inferred_type {
                            Ok(unified_type) => *inferred_type = unified_type,
                            Err(e) => {
                                errors.push(
                                    UnResolvedTypesError::from(&expr_copied, None)
                                        .with_additional_error_detail(format!(
                                            "cannot determine the return type of function {}: {}",
                                            function_name, e
                                        )),
                                );
                            }
                        }
                    }

                    _ => {
                        let unified_inferred_type = inferred_type.unify();

                        match unified_inferred_type {
                            Ok(unified_type) => *inferred_type = unified_type,
                            Err(e) => {
                                errors.push(
                                    UnResolvedTypesError::from(&expr_copied, None)
                                        .with_additional_error_detail(format!(
                                            "cannot determine the type of function return: {}",
                                            e
                                        )),
                                );
                            }
                        }
                    }
                }
            }
            Expr::SelectField {
                expr,
                inferred_type,
                ..
            } => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of field selection: {}",
                                    e
                                )),
                        );
                    }
                }
            }
            Expr::SelectIndex {
                expr,
                inferred_type,
                ..
            } => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of index selection: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::SelectDynamic {
                expr,
                index,
                inferred_type,
                ..
            } => {
                queue.push(expr);
                queue.push(index);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of dynamic field selection: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::Let { expr, .. } => {
                queue.push(expr);
            }
            Expr::Literal { inferred_type, .. } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of literal: {}",
                                    e
                                )),
                        );
                    }
                }
            }
            Expr::Flags { inferred_type, .. } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of flags: {}",
                                    e
                                )),
                        );
                    }
                }
            }
            Expr::Identifier { inferred_type, .. } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of identifier: {}",
                                    e
                                )),
                        );
                    }
                }
            }
            Expr::Boolean { .. } => {}
            Expr::Concat { exprs, .. } => {
                queue.extend(exprs);
            }
            Expr::ExprBlock {
                exprs,
                inferred_type,
                ..
            } => {
                queue.extend(exprs);

                let unified_inferred_type = inferred_type.unify();

                if let Ok(unified_type) = unified_inferred_type {
                    *inferred_type = unified_type
                }
            }

            Expr::Not {
                expr,
                inferred_type,
                ..
            } => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of NOT expression: {}",
                                    e
                                )),
                        );
                    }
                }
            }
            Expr::Unwrap {
                expr,
                inferred_type,
                ..
            } => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::Throw { inferred_type, .. } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::GetTag { inferred_type, .. } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::GreaterThan { lhs, rhs, .. } => {
                queue.push(lhs);
                queue.push(rhs);
            }

            Expr::Plus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => internal::handle_math_op(
                &mut queue,
                lhs,
                rhs,
                inferred_type,
                &mut errors,
                &expr_copied,
            ),

            Expr::Minus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => internal::handle_math_op(
                &mut queue,
                lhs,
                rhs,
                inferred_type,
                &mut errors,
                &expr_copied,
            ),

            Expr::Divide {
                lhs,
                rhs,
                inferred_type,
                ..
            } => internal::handle_math_op(
                &mut queue,
                lhs,
                rhs,
                inferred_type,
                &mut errors,
                &expr_copied,
            ),

            Expr::Multiply {
                lhs,
                rhs,
                inferred_type,
                ..
            } => internal::handle_math_op(
                &mut queue,
                lhs,
                rhs,
                inferred_type,
                &mut errors,
                &expr_copied,
            ),

            Expr::And { lhs, rhs, .. } => {
                queue.push(lhs);
                queue.push(rhs);
            }
            Expr::Or { lhs, rhs, .. } => {
                queue.push(lhs);
                queue.push(rhs);
            }

            Expr::GreaterThanOrEqualTo { lhs, rhs, .. } => {
                queue.push(lhs);
                queue.push(rhs);
            }
            Expr::LessThanOrEqualTo { lhs, rhs, .. } => {
                queue.push(lhs);
                queue.push(rhs);
            }
            Expr::EqualTo { lhs, rhs, .. } => {
                queue.push(lhs);
                queue.push(rhs);
            }
            Expr::LessThan { lhs, rhs, .. } => {
                queue.push(lhs);
                queue.push(rhs);
            }
            Expr::InvokeMethodLazy { .. } => {}
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(MultipleUnResolvedTypesError(errors))
    }
}

mod internal {
    use crate::{ArmPattern, Expr, InferredType, UnResolvedTypesError};

    pub(crate) fn handle_math_op<'a>(
        queue: &mut Vec<&'a mut Expr>,
        left: &'a mut Expr,
        right: &'a mut Expr,
        inferred_type: &mut InferredType,
        errors: &mut Vec<UnResolvedTypesError>,
        expr: &Expr,
    ) {
        queue.push(left);
        queue.push(right);
        let unified_inferred_type = inferred_type.unify();

        match unified_inferred_type {
            Ok(unified_type) => *inferred_type = unified_type,
            Err(e) => {
                errors.push(
                    UnResolvedTypesError::from(expr, None).with_additional_error_detail(format!(
                        "cannot determine the type of math operation: {}",
                        e
                    )),
                );
            }
        }
    }

    // Push any existence of expr in arm patterns to queue
    pub(crate) fn push_arm_pattern_expr<'a>(
        arm_pattern: &'a mut ArmPattern,
        queue: &mut Vec<&'a mut Expr>,
    ) {
        match arm_pattern {
            ArmPattern::Literal(expr) => {
                queue.push(expr);
            }
            ArmPattern::As(_, pattern) => {
                push_arm_pattern_expr(pattern, queue);
            }
            ArmPattern::Constructor(_, patterns) => {
                for pattern in patterns {
                    push_arm_pattern_expr(pattern, queue);
                }
            }

            ArmPattern::TupleConstructor(patterns) => {
                for pattern in patterns {
                    push_arm_pattern_expr(pattern, queue);
                }
            }

            ArmPattern::ListConstructor(patterns) => {
                for pattern in patterns {
                    push_arm_pattern_expr(pattern, queue);
                }
            }

            ArmPattern::RecordConstructor(fields) => {
                for (_, pattern) in fields {
                    push_arm_pattern_expr(pattern, queue);
                }
            }

            ArmPattern::WildCard => {}
        }
    }
}
