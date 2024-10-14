use std::collections::VecDeque;
use std::ops::Deref;
use crate::Expr;

pub fn check_unresolved_types(expr: &Expr) -> Result<(), String> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Let(_, _, _, _) => {}
            Expr::SelectField(expr, field, inferred_type) => {
                queue.push_back(expr);
                if inferred_type.un_resolved() {
                    return Err(format!("Un-resolved type for field {}", field));
                }
            }
            Expr::SelectIndex(expr, index, inferred_type) => {
                queue.push_back(expr);
                if inferred_type.un_resolved() {
                    return Err(format!("Un-resolved type for index {}", index));
                }
            }
            Expr::Sequence(exprs, inferred_type) => {
                internal::unresolved_types_in_list(&exprs)?;
            }
            Expr::Record(field, inferred_type) => {
                internal::unresolved_types_in_record(
                    &field
                        .iter()
                        .map(|(k, v)| (k.clone(), v.deref().clone()))
                        .collect(),
                )?;

                if inferred_type.un_resolved() {
                    return Err("Un-resolved type for record".to_string());
                }
            }
            Expr::Tuple(exprs, inferreed_type) => {
                internal::unresolved_types_in_tuple(&exprs)?;

                if inferreed_type.un_resolved() {
                    return Err("Un-resolved type for tuple".to_string());
                }
            }
            Expr::Literal(str, inferred_type) => {
                if inferred_type.un_resolved() {
                    return Err(format!("Un-resolved type for literal {}", str));
                }
            }
            Expr::Number(number, _, inferred_type) => {
                if inferred_type.un_resolved() {
                    return Err(format!("Un-resolved type for number {}", number));
                }
            }
            Expr::Flags(flags, inferred_type) => {
                if inferred_type.un_resolved() {
                    return Err(format!("Un-resolved type for flags {:?}", flags));
                }
            }
            Expr::Identifier(identifier, inferred_type) => {
                if inferred_type.un_resolved() {
                    return Err(format!("Un-resolved type for identifier {}", identifier));
                }
            }
            Expr::Boolean(bool, inferred_type) => {
                if inferred_type.un_resolved() {
                    return Err(format!("Un-resolved type for boolean {}", bool));
                }
            }
            Expr::Concat(exprs, inferred_type) => {
                if let Err(msg) = internal::unresolved_type_for_concat(&exprs) {
                    return Err(msg);
                }
                if inferred_type.un_resolved() {
                    return Err("Un-resolved type for concat".to_string());
                }
            }
            Expr::Multiple(exprs, inferred_type) => {
                for expr in exprs {
                    queue.push_back(expr);
                }
                if inferred_type.un_resolved() {
                    return Err("Un-resolved type for multiple".to_string());
                }
            }
            Expr::Not(expr, inferred_type) => {
                queue.push_back(expr);
                if inferred_type.un_resolved() {
                    return Err("Un-resolved type for not".to_string());
                }
            }
            Expr::GreaterThan(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)?;
            }
            Expr::And(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)?;
            }
            Expr::Or(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)?;
            }
            Expr::GreaterThanOrEqualTo(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)?;
            }
            Expr::LessThanOrEqualTo(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)?;
            }
            Expr::EqualTo(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)?;
            }
            Expr::LessThan(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)?;
            }
            Expr::Cond(cond, left, right, inferred_type) => {
                internal::unresolved_type_for_if_condition(cond, left, right)?;
                if inferred_type.un_resolved() {
                    return Err("Un-resolved type for if condition".to_string());
                }
            }
            Expr::PatternMatch(cond, arms, inferred_type) => {
                internal::unresolved_type_for_pattern_match(cond, arms)?;
                if inferred_type.un_resolved() {
                    return Err("Un-resolved type for pattern match".to_string());
                }
            }
            Expr::Option(option, inferred_type) => {
                if let Some(expr) = option {
                    queue.push_back(expr);
                }

                if inferred_type.un_resolved() {
                    return Err("Un-resolved type for option".to_string());
                }
            }
            Expr::Result(ok_err, _) => {
                internal::unresolved_type_for_result(ok_err)?;
            }
            Expr::Call(_, _, _) => {}
            Expr::Unwrap(_, _) => {}
            Expr::Throw(_, _) => {}
            Expr::GetTag(_, _) => {}
        }
    }

    Ok(())
}

mod internal {
    use crate::type_checker::{check_unresolved_types};
    use crate::{Expr, MatchArm};

    pub fn unresolved_types_in_record(expr_fields: &Vec<(String, Expr)>) -> Result<(), String> {
        for (field_name, field_expr) in expr_fields {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err(format!(
                    "Un-inferred type for field `{}` in record",
                    field_name
                ));
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_types_in_tuple(expr_fields: &Vec<Expr>) -> Result<(), String> {
        for field_expr in expr_fields {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err("Un-inferred type for tuple item".to_string());
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_type_for_concat(expr_fields: &Vec<Expr>) -> Result<(), String> {
        for field_expr in expr_fields {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err("Un-inferred type for concat item".to_string());
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_type_for_binary_op(left: &Expr, right: &Expr) -> Result<(), String> {
        let left_type = left.inferred_type();
        let right_type = right.inferred_type();
        if left_type.is_unknown() || left_type.is_one_of() {
            return Err("Un-inferred type for left operand".to_string());
        } else {
            check_unresolved_types(left)?;
        }

        if right_type.is_unknown() || right_type.is_one_of() {
            return Err("Un-inferred type for right operand".to_string());
        } else {
            check_unresolved_types(right)?;
        }

        Ok(())
    }

    pub fn unresolved_type_for_if_condition(
        cond: &Expr,
        if_expr: &Expr,
        else_expr: &Expr,
    ) -> Result<(), String> {
        let cond_type = cond.inferred_type();
        let if_type = if_expr.inferred_type();
        let else_type = else_expr.inferred_type();
        if cond_type.is_unknown() || cond_type.is_one_of() {
            return Err("Un-inferred type for condition".to_string());
        } else {
            check_unresolved_types(cond)?;
        }

        if if_type.is_unknown() || if_type.is_one_of() {
            return Err("Un-inferred type for if branch".to_string());
        } else {
            check_unresolved_types(if_expr)?;
        }

        if else_type.is_unknown() || else_type.is_one_of() {
            return Err("Un-inferred type for else branch".to_string());
        } else {
            check_unresolved_types(else_expr)?;
        }

        Ok(())
    }

    pub fn unresolved_type_for_pattern_match(
        cond: &Expr,
        match_arms: &Vec<MatchArm>,
    ) -> Result<(), String> {
        let cond_type = cond.inferred_type();
        if cond_type.is_unknown() || cond_type.is_one_of() {
            return Err("Un-inferred type for condition".to_string());
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
                    return Err("Un-inferred type for pattern match expression".to_string());
                } else {
                    check_unresolved_types(&expr)?;
                }
            }

            let expr = match_arm.clone().arm_resolution_expr;

            let expr_type = expr.inferred_type();
            if expr_type.is_unknown() || expr_type.is_one_of() {
                return Err("Un-inferred type for pattern match resolution expression".to_string());
            } else {
                check_unresolved_types(&expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_type_for_result(ok_err: &Result<Box<Expr>, Box<Expr>>) -> Result<(), String> {
        let ok_expr = ok_err.clone().ok();
        let error_expr = ok_err.clone().err();
        if let Some(ok_expr) = ok_expr {
            let ok_type = ok_expr.inferred_type();
            if ok_type.un_resolved() {
                return Err("Un-inferred type for ok branch".to_string());
            } else {
                check_unresolved_types(&ok_expr)?;
            }
        }

        if let Some(error_expr) = error_expr {
            let error_type = error_expr.inferred_type();
            if error_type.un_resolved() {
                return Err("Un-inferred type for error branch".to_string());
            } else {
                check_unresolved_types(&error_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_types_in_list(expr_fields: &Vec<Expr>) -> Result<(), String> {
        for (index, field_expr) in expr_fields.iter().enumerate() {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err(format!("Un-inferred type for list at index {}", index));
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_types_in_variant(
        expr_fields: &Vec<(String, Option<Expr>)>,
    ) -> Result<(), String> {
        for (_, field_expr) in expr_fields {
            if let Some(field_expr) = field_expr {
                let field_type = field_expr.inferred_type();
                if field_type.is_unknown() || field_type.is_one_of() {
                    return Err("Un-inferred type for variant case".to_string());
                } else {
                    check_unresolved_types(field_expr)?;
                }
            }
        }

        Ok(())
    }
}