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

use crate::{ArmPattern, Expr};

pub fn unify_types(expr: &mut Expr) -> Result<(), Vec<String>> {
    let mut queue = vec![];
    queue.push(expr);
    let mut errors = vec![];

    while let Some(expr) = queue.pop() {
        let expr_str = &mut expr.to_string();

        match expr {
            Expr::Number(_, _, inferred_type) => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(e);
                    }
                }
            }

            Expr::Record(vec, inferred_type) => {
                queue.extend(vec.iter_mut().map(|(_, expr)| &mut **expr));

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of record {}", expr_str));
                        errors.push(e);
                    }
                }
            }
            Expr::Tuple(vec, inferred_type) => {
                queue.extend(vec.iter_mut());

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of tuple {}", expr_str));
                        errors.push(e);
                    }
                }
            }
            Expr::Sequence(vec, inferred_type) => {
                queue.extend(vec.iter_mut());
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of sequence {}",
                            expr_str
                        ));
                        errors.push(e);
                    }
                }
            }
            Expr::Option(Some(expr), inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of option {}", expr_str));
                        errors.push(e);
                    }
                }
            }

            Expr::Option(None, inferred_type) => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of option {}", expr_str));
                        errors.push(e);
                    }
                }
            }

            Expr::Result(Ok(expr), inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of `result::ok` {}",
                            expr_str
                        ));
                        errors.push(e);
                    }
                }
            }
            Expr::Result(Err(expr), inferred_type) => {
                queue.push(expr);

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of `result::err` {}",
                            expr_str
                        ));
                        errors.push(e);
                    }
                }
            }
            Expr::Cond(cond, then, else_, inferred_type) => {
                queue.push(cond);
                queue.push(then);
                queue.push(else_);

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of condition expression {}",
                            expr_str
                        ));
                        errors.push(e);
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
                        errors.push(format!(
                            "Unable to resolve the type of list comprehension {}",
                            expr_str
                        ));

                        errors.push(e)
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
                        errors.push(format!(
                            "Unable to resolve the type of list aggregation {}",
                            expr_str
                        ));

                        errors.push(e)
                    }
                }
            }

            Expr::PatternMatch(expr, arms, inferred_type) => {
                queue.push(expr);
                for arm in arms.iter_mut().rev() {
                    let arm_resolution_expr = &mut *arm.arm_resolution_expr;
                    let arm_pattern: &mut ArmPattern = &mut arm.arm_pattern;
                    internal::push_arm_pattern_expr(arm_pattern, &mut queue);
                    queue.push(arm_resolution_expr);
                }
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of pattern match expression {}",
                            expr_str
                        ));
                        errors.push(e);
                    }
                }
            }
            Expr::Call(function_call, vec, inferred_type) => {
                queue.extend(vec.iter_mut());

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of function return {}",
                            function_call
                        ));
                        errors.push(e);
                    }
                }
            }
            Expr::SelectField(expr, _, inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of field selection {}",
                            expr_str
                        ));
                        errors.push(e);
                    }
                }
            }
            Expr::SelectIndex(expr, _, inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of index selection {}",
                            expr_str
                        ));
                        errors.push(e);
                    }
                }
            }

            Expr::Let(_, _, expr, _) => {
                queue.push(expr);
            }
            Expr::Literal(_, inferred_type) => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(e);
                    }
                }
            }
            Expr::Flags(_, inferred_type) => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of flags {}", expr_str));
                        errors.push(e);
                    }
                }
            }
            Expr::Identifier(_, inferred_type) => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of identifier {:?}",
                            expr
                        ));
                        errors.push(e);
                    }
                }
            }
            Expr::Boolean(_, _) => {}
            Expr::Concat(exprs, _) => {
                queue.extend(exprs);
            }
            Expr::ExprBlock(expr, inferred_type) => {
                queue.extend(expr);

                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(e);
                    }
                }
            }

            Expr::Not(expr, inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of {}", expr_str));
                        errors.push(e);
                    }
                }
            }
            Expr::Unwrap(expr, inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of {}", expr_str));
                        errors.push(e);
                    }
                }
            }

            Expr::Throw(_, inferred_type) => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of {}", expr_str));
                        errors.push(e);
                    }
                }
            }

            Expr::GetTag(_, inferred_type) => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of {}", expr_str));
                        errors.push(e);
                    }
                }
            }

            Expr::GreaterThan(left, right, _) => {
                queue.push(left);
                queue.push(right);
            }

            Expr::Plus(left, right, inferred_type) => internal::handle_math_op(
                &mut queue,
                left,
                right,
                inferred_type,
                &mut errors,
                expr_str,
            ),

            Expr::Minus(left, right, inferred_type) => internal::handle_math_op(
                &mut queue,
                left,
                right,
                inferred_type,
                &mut errors,
                expr_str,
            ),

            Expr::Divide(left, right, inferred_type) => internal::handle_math_op(
                &mut queue,
                left,
                right,
                inferred_type,
                &mut errors,
                expr_str,
            ),

            Expr::Multiply(left, right, inferred_type) => internal::handle_math_op(
                &mut queue,
                left,
                right,
                inferred_type,
                &mut errors,
                expr_str,
            ),

            Expr::And(left, right, _) => {
                queue.push(left);
                queue.push(right);
            }
            Expr::Or(left, right, _) => {
                queue.push(left);
                queue.push(right);
            }

            Expr::GreaterThanOrEqualTo(left, right, _) => {
                queue.push(left);
                queue.push(right);
            }
            Expr::LessThanOrEqualTo(left, right, _) => {
                queue.push(left);
                queue.push(right);
            }
            Expr::EqualTo(left, right, _) => {
                queue.push(left);
                queue.push(right);
            }
            Expr::LessThan(left, right, _) => {
                queue.push(left);
                queue.push(right);
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

mod internal {
    use crate::{ArmPattern, Expr, InferredType};

    pub(crate) fn handle_math_op<'a>(
        queue: &mut Vec<&'a mut Expr>,
        left: &'a mut Expr,
        right: &'a mut Expr,
        inferred_type: &mut InferredType,
        errors: &mut Vec<String>,
        expr_str: &str,
    ) {
        queue.push(left);
        queue.push(right);
        let unified_inferred_type = inferred_type.unify();

        match unified_inferred_type {
            Ok(unified_type) => *inferred_type = unified_type,
            Err(e) => {
                errors.push(format!("Unable to resolve the type of {}", expr_str));
                errors.push(e);
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
