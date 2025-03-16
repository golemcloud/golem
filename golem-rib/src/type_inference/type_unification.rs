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
use crate::{ArmPattern, Expr, ExprVisitor, MultipleUnResolvedTypesError, UnResolvedTypesError};

pub fn unify_types(expr: &mut Expr) -> Result<(), MultipleUnResolvedTypesError> {
    let mut visitor = ExprVisitor::bottom_up(expr);
    let mut errors: Vec<UnResolvedTypesError> = vec![];

    while let Some(expr) = visitor.pop_front() {
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
                inferred_type,
                ..
            } => {
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
                inferred_type,
                ..
            } => {
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
                inferred_type,
                ..
            } => {
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
                inferred_type,
                ..
            } => {
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
                expr: Some(_),
                inferred_type,
                ..
            } => {
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
                expr: Ok(_),
                inferred_type,
                ..
            } => {
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
                expr: Err(_),
                inferred_type,
                ..
            } => {

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
                inferred_type,
                ..
            } => {

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

            Expr::Length {
                inferred_type,
                ..
            } => {

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(
                            UnResolvedTypesError::from(&expr_copied, None)
                                .with_additional_error_detail(format!(
                                    "cannot determine the type of length function: {}",
                                    e
                                )),
                        );
                    }
                }
            }

            Expr::ListComprehension {
                inferred_type,
                ..
            } => {

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
                inferred_type,
                ..
            } => {

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
                inferred_type,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => {
                        *inferred_type = unified_type;
                    }
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
                inferred_type,
                ..
            } => {
                match call_type {
                    // We don't care about anything inside instance creation
                    CallType::InstanceCreation(_) => {}
                    // Make sure worker expression in function
                    CallType::Function {
                        function_name,
                        ..
                    } => {

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
                inferred_type,
                ..
            } => {
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
                inferred_type,
                ..
            } => {
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
            Expr::Concat { .. } => {}
            Expr::ExprBlock {
                inferred_type,
                ..
            } => {

                let unified_inferred_type = inferred_type.unify();

                if let Ok(unified_type) = unified_inferred_type {
                    *inferred_type = unified_type
                }
            }

            Expr::Not {
                inferred_type,
                ..
            } => {

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
                inferred_type,
                ..
            } => {

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

            Expr::GreaterThan { .. } => {}

            Expr::Plus {
                inferred_type,
                ..
            } => internal::handle_math_op(
                inferred_type,
                &mut errors,
                &expr_copied,
            ),

            Expr::Minus {
                inferred_type,
                ..
            } => internal::handle_math_op(
                inferred_type,
                &mut errors,
                &expr_copied,
            ),

            Expr::Divide {
                inferred_type,
                ..
            } => internal::handle_math_op(
                inferred_type,
                &mut errors,
                &expr_copied,
            ),

            Expr::Multiply {
                inferred_type,
                ..
            } => internal::handle_math_op(
                inferred_type,
                &mut errors,
                &expr_copied,
            ),

            Expr::And { .. } => {}
            Expr::Or { .. } => {}

            Expr::GreaterThanOrEqualTo { .. } => {}
            Expr::LessThanOrEqualTo { .. } => {}
            Expr::EqualTo { .. } => {}
            Expr::LessThan { .. } => {}
            Expr::InvokeMethodLazy { .. } => {}
            Expr::Let { .. } => {}
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
        inferred_type: &mut InferredType,
        errors: &mut Vec<UnResolvedTypesError>,
        expr: &Expr,
    ) {

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
