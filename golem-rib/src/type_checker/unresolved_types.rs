use crate::type_checker::UnResolvedTypesError;
use crate::Expr;
use golem_wasm_ast::analysis::AnalysedType;
use std::collections::VecDeque;
use std::ops::Deref;

pub fn check_unresolved_types(expr: &Expr) -> Result<(), UnResolvedTypesError> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        let error = match expr {
            Expr::Let(_, _, _, _) => Ok(()),
            Expr::SelectField(expr, field, inferred_type) => {
                queue.push_back(expr);
                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr).at_field(field.clone()))
                } else {
                    Ok(())
                }
            }
            Expr::SelectIndex(expr, index, inferred_type) => {
                queue.push_back(expr);
                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr).at_index(*index))
                } else {
                    Ok(())
                }
            }
            Expr::Sequence(exprs, inferred_type) => internal::unresolved_types_in_list(&exprs),
            Expr::Record(field, inferred_type) => {
                internal::unresolved_types_in_record(
                    &field
                        .iter()
                        .map(|(k, v)| (k.clone(), v.deref().clone()))
                        .collect(),
                )?;

                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr))
                } else {
                    Ok(())
                }
            }
            Expr::Tuple(exprs, inferred_type) => {
                internal::unresolved_types_in_tuple(&exprs)?;

                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr))
                } else {
                    Ok(())
                }
            }
            Expr::Literal(_, inferred_type) => {
                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr))
                } else {
                    Ok(())
                }
            }
            Expr::Number(_, _, inferred_type) => {
                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr))
                } else {
                    Ok(())
                }
            }
            Expr::Flags(_, inferred_type) => {
                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr))
                } else {
                    Ok(())
                }
            }
            Expr::Identifier(_, inferred_type) => {
                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr))
                } else {
                    Ok(())
                }
            }
            Expr::Boolean(_, inferred_type) => {
                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr))
                } else {
                    Ok(())
                }
            }
            Expr::Concat(exprs, inferred_type) => {
                internal::unresolved_type_for_concat(&exprs)?;

                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr))
                } else {
                    Ok(())
                }
            }
            Expr::Multiple(exprs, inferred_type) => {
                for expr in exprs {
                    queue.push_back(expr);
                }

                Ok(())
            }
            Expr::Not(expr, inferred_type) => {
                queue.push_back(expr);
                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr))
                } else {
                    Ok(())
                }
            }
            Expr::GreaterThan(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)
            }
            Expr::And(left, right, _) => internal::unresolved_type_for_binary_op(left, right),
            Expr::Or(left, right, _) => internal::unresolved_type_for_binary_op(left, right),
            Expr::GreaterThanOrEqualTo(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)
            }
            Expr::LessThanOrEqualTo(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)
            }
            Expr::EqualTo(left, right, _) => internal::unresolved_type_for_binary_op(left, right),
            Expr::LessThan(left, right, _) => internal::unresolved_type_for_binary_op(left, right),
            Expr::Cond(cond, left, right, inferred_type) => {
                internal::unresolved_type_for_if_condition(cond, left, right)?;
                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr))
                } else {
                    Ok(())
                }
            }
            Expr::PatternMatch(cond, arms, inferred_type) => {
                internal::unresolved_type_for_pattern_match(cond, arms)?;
                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr))
                } else {
                    Ok(())
                }
            }
            Expr::Option(option, inferred_type) => {
                if let Some(expr) = option {
                    queue.push_back(expr);
                }

                if inferred_type.un_resolved() {
                    Err(UnResolvedTypesError::new(expr))
                } else {
                    Ok(())
                }
            }
            Expr::Result(ok_err, _) => internal::unresolved_type_for_result(ok_err),
            Expr::Call(_, _, _) => Ok(()),
            Expr::Unwrap(_, _) => Ok(()),
            Expr::Throw(_, _) => Ok(()),
            Expr::GetTag(_, _) => Ok(()),
        };

        if let Err(error) = error {
            return Err(error);
        }
    }

    Ok(())
}

mod internal {
    use crate::type_checker::{check_unresolved_types, UnResolvedTypesError};
    use crate::{Expr, MatchArm};
    use std::ops::Deref;

    pub fn unresolved_types_in_record(
        expr_fields: &Vec<(String, Expr)>,
    ) -> Result<(), UnResolvedTypesError> {
        for (field_name, field_expr) in expr_fields {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err(UnResolvedTypesError::new(field_expr).at_field(field_name.clone()));
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_types_in_tuple(expr_fields: &Vec<Expr>) -> Result<(), UnResolvedTypesError> {
        for (index, field_expr) in expr_fields.iter().enumerate() {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err(UnResolvedTypesError::new(field_expr).at_index(index));
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_type_for_concat(expr_fields: &Vec<Expr>) -> Result<(), UnResolvedTypesError> {
        for (index, field_expr) in expr_fields.iter().enumerate() {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err(UnResolvedTypesError::new(field_expr).at_index(index));
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_type_for_binary_op(
        left: &Expr,
        right: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        let left_type = left.inferred_type();
        let right_type = right.inferred_type();
        if left_type.un_resolved() {
            return Err(UnResolvedTypesError::new(left));
        } else {
            check_unresolved_types(left)?;
        }

        if right_type.un_resolved() {
            return Err(UnResolvedTypesError::new(right));
        } else {
            check_unresolved_types(right)?;
        }

        Ok(())
    }

    pub fn unresolved_type_for_if_condition(
        cond: &Expr,
        if_expr: &Expr,
        else_expr: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        let cond_type = cond.inferred_type();
        let if_type = if_expr.inferred_type();
        let else_type = else_expr.inferred_type();
        if cond_type.un_resolved() {
            return Err(UnResolvedTypesError::new(cond));
        } else {
            check_unresolved_types(cond)?;
        }

        if if_type.un_resolved() {
            return Err(UnResolvedTypesError::new(if_expr));
        } else {
            check_unresolved_types(if_expr)?;
        }

        if else_type.un_resolved() {
            return Err(UnResolvedTypesError::new(if_expr));
        } else {
            check_unresolved_types(else_expr)?;
        }

        Ok(())
    }

    pub fn unresolved_type_for_pattern_match(
        cond: &Expr,
        match_arms: &Vec<MatchArm>,
    ) -> Result<(), UnResolvedTypesError> {
        let cond_type = cond.inferred_type();
        if cond_type.is_unknown() || cond_type.is_one_of() {
            return Err(UnResolvedTypesError::new(cond));
        } else {
            check_unresolved_types(cond)?;
        }

        for match_arm in match_arms {
            let exprs: Vec<Expr> = match_arm
                .arm_pattern
                .clone()
                .get_expr_literals()
                .into_iter()
                .map(|x| x.clone())
                .collect();

            for expr in exprs {
                let expr_type = expr.inferred_type();
                if expr_type.is_unknown() || expr_type.is_one_of() {
                    return Err(UnResolvedTypesError::new(&expr));
                } else {
                    check_unresolved_types(&expr)?;
                }
            }

            let expr = match_arm.clone().arm_resolution_expr;

            let expr_type = expr.inferred_type();
            if expr_type.un_resolved() {
                return Err(UnResolvedTypesError::new(expr.deref()));
            } else {
                check_unresolved_types(&expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_type_for_result(
        ok_err: &Result<Box<Expr>, Box<Expr>>,
    ) -> Result<(), UnResolvedTypesError> {
        let ok_expr = ok_err.clone().ok();
        let error_expr = ok_err.clone().err();
        if let Some(ok_expr) = ok_expr {
            let ok_type = ok_expr.inferred_type();
            if ok_type.un_resolved() {
                return Err(UnResolvedTypesError::new(ok_expr.deref()));
            } else {
                check_unresolved_types(&ok_expr)?;
            }
        }

        if let Some(error_expr) = error_expr {
            let error_type = error_expr.inferred_type();
            if error_type.un_resolved() {
                return Err(UnResolvedTypesError::new(error_expr.deref()));
            } else {
                check_unresolved_types(&error_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_types_in_list(expr_fields: &Vec<Expr>) -> Result<(), UnResolvedTypesError> {
        for (index, field_expr) in expr_fields.iter().enumerate() {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err(UnResolvedTypesError::new(field_expr).at_index(index));
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_types_in_variant(
        expr_fields: &Vec<(String, Option<Expr>)>,
    ) -> Result<(), UnResolvedTypesError> {
        for (_, field_expr) in expr_fields {
            if let Some(field_expr) = field_expr {
                let field_type = field_expr.inferred_type();
                if field_type.is_unknown() || field_type.is_one_of() {
                    return Err(UnResolvedTypesError::new(field_expr));
                } else {
                    check_unresolved_types(field_expr)?;
                }
            }
        }

        Ok(())
    }
}
