use crate::{ArmPattern, Expr};

pub fn unify_types(expr: &mut Expr) -> Result<(), Vec<String>> {
    let mut queue = vec![];
    queue.push(expr);
    let mut errors = vec![];

    while let Some(expr) = queue.pop() {
        let expr_str = &mut expr.to_string();

        match expr {
            Expr::Number(_, inferred_type) => {
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.extend(e);
                    }
                }
            }

            Expr::Record(vec, inferred_type) => {
                queue.extend(vec.iter_mut().map(|(_, expr)| &mut **expr));

                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of record {}", expr_str));
                        errors.extend(e);
                    }
                }
            }
            Expr::Tuple(vec, inferred_type) => {
                queue.extend(vec.iter_mut());

                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of tuple {}", expr_str));
                        errors.extend(e);
                    }
                }
            }
            Expr::Sequence(vec, inferred_type) => {
                queue.extend(vec.iter_mut());
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of sequence {}",
                            expr_str
                        ));
                        errors.extend(e);
                    }
                }
            }
            Expr::Option(Some(expr), inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of option {}", expr_str));
                        errors.extend(e);
                    }
                }
            }

            Expr::Option(None, inferred_type) => {
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of option {}", expr_str));
                        errors.extend(e);
                    }
                }
            }

            Expr::Result(Ok(expr), inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of `result::ok` {}",
                            expr_str
                        ));
                        errors.extend(e);
                    }
                }
            }
            Expr::Result(Err(expr), inferred_type) => {
                queue.push(expr);

                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of `result::err` {}",
                            expr_str
                        ));
                        errors.extend(e);
                    }
                }
            }
            Expr::Cond(cond, then, else_, inferred_type) => {
                queue.push(cond);
                queue.push(then);
                queue.push(else_);

                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of condition expression {}",
                            expr_str
                        ));
                        errors.extend(e);
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
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of pattern match expression {}",
                            expr_str
                        ));
                        errors.extend(e);
                    }
                }
            }
            Expr::Call(function_call, vec, inferred_type) => {
                queue.extend(vec.iter_mut());

                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of function return {}",
                            function_call
                        ));
                        errors.extend(e);
                    }
                }
            }
            Expr::SelectField(expr, _, inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of field selection {}",
                            expr_str
                        ));
                        errors.extend(e);
                    }
                }
            }
            Expr::SelectIndex(expr, _, inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of index selection {}",
                            expr_str
                        ));
                        errors.extend(e);
                    }
                }
            }

            Expr::Let(_, expr, inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of let binding {}",
                            expr_str
                        ));
                        errors.extend(e);
                    }
                }
            }
            Expr::Literal(_, inferred_type) => {
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.extend(e);
                    }
                }
            }
            Expr::Flags(_, inferred_type) => {
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of flags {}", expr_str));
                        errors.extend(e);
                    }
                }
            }
            Expr::Identifier(_, inferred_type) => {
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of identifier {}",
                            expr_str
                        ));
                        errors.extend(e);
                    }
                }
            }
            Expr::Boolean(_, _) => {}
            Expr::Concat(exprs, _) => {
                queue.extend(exprs);
            }
            Expr::Multiple(expr, inferred_type) => {
                queue.extend(expr);

                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!(
                            "Unable to resolve the type of code block {}",
                            expr_str
                        ));
                        errors.extend(e);
                    }
                }
            }
            Expr::Not(expr, inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of {}", expr_str));
                        errors.extend(e);
                    }
                }
            }
            Expr::Unwrap(expr, inferred_type) => {
                queue.push(expr);
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of {}", expr_str));
                        errors.extend(e);
                    }
                }
            }
            Expr::Throw(_, inferred_type) => {
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of {}", expr_str));
                        errors.extend(e);
                    }
                }
            }

            Expr::Tag(_, inferred_type) => {
                let unified_inferred_type = inferred_type.unify_types();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        errors.push(format!("Unable to resolve the type of {}", expr_str));
                        errors.extend(e);
                    }
                }
            }

            Expr::GreaterThan(left, right, _) => {
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
    use crate::{ArmPattern, Expr};

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
            ArmPattern::WildCard => {}
        }
    }
}
