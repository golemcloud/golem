use crate::type_checker::UnResolvedTypesError;
use crate::Expr;
use std::collections::VecDeque;
use std::ops::Deref;

pub fn check_unresolved_types(expr: &Expr) -> Result<(), UnResolvedTypesError> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Let { expr, .. } => {
                queue.push_back(expr);
            }
            Expr::InvokeMethodLazy {
                lhs,
                args,
                inferred_type,
                ..
            } => {
                queue.push_back(lhs);
                for arg in args {
                    queue.push_back(arg);
                }

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::SelectField {
                expr,
                field,
                inferred_type,
                ..
            } => {
                queue.push_back(expr);
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr).at_field(field.clone()));
                }
            }
            Expr::SelectIndex {
                expr,
                index,
                inferred_type,
                ..
            } => {
                queue.push_back(expr);
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr).at_index(*index));
                }
            }
            Expr::Sequence {
                exprs,
                inferred_type,
                ..
            } => {
                internal::unresolved_types_in_list(exprs)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::Record {
                exprs,
                inferred_type,
            } => {
                internal::unresolved_types_in_record(
                    &exprs
                        .iter()
                        .map(|(k, v)| (k.clone(), v.deref().clone()))
                        .collect(),
                )?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::Tuple { exprs, .. } => {
                internal::unresolved_types_in_tuple(exprs)?;
            }
            Expr::Literal(_, inferred_type) => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::Number { inferred_type, .. } => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr).with_additional_message(
                        "Number literals must have a type annotation. Example: `1: u64`",
                    ));
                }
            }
            Expr::Flags { inferred_type, .. } => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::Identifier { inferred_type, .. } => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr).with_additional_message(
                        format!("`{}` is unknown identifier", expr).as_str(),
                    ));
                }
            }
            Expr::Boolean { inferred_type, .. } => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::Concat {
                exprs,
                inferred_type,
            } => {
                internal::unresolved_type_for_concat(exprs)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::ExprBlock { exprs, .. } => {
                for expr in exprs {
                    queue.push_back(expr);
                }
            }
            Expr::Not {
                expr,
                inferred_type,
            } => {
                queue.push_back(expr);
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::GreaterThan { lhs, rhs, .. } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;
            }
            Expr::And { lhs, rhs, .. } => internal::unresolved_type_for_binary_op(lhs, rhs)?,
            Expr::Plus { lhs, rhs, .. } => internal::unresolved_type_for_binary_op(lhs, rhs)?,
            Expr::Minus { lhs, rhs, .. } => internal::unresolved_type_for_binary_op(lhs, rhs)?,
            Expr::Multiply { lhs, rhs, .. } => internal::unresolved_type_for_binary_op(lhs, rhs)?,
            Expr::Divide { lhs, rhs, .. } => internal::unresolved_type_for_binary_op(lhs, rhs)?,
            Expr::Or { lhs, rhs, .. } => internal::unresolved_type_for_binary_op(lhs, rhs)?,
            Expr::GreaterThanOrEqualTo { lhs, rhs, .. } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;
            }
            Expr::LessThanOrEqualTo { lhs, rhs, .. } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;
            }
            Expr::EqualTo { lhs, rhs, .. } => internal::unresolved_type_for_binary_op(lhs, rhs)?,
            Expr::LessThan { lhs, rhs, .. } => internal::unresolved_type_for_binary_op(lhs, rhs)?,
            Expr::Cond {
                cond,
                lhs,
                rhs,
                inferred_type,
            } => {
                internal::unresolved_type_for_if_condition(cond, lhs, rhs)?;
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::PatternMatch {
                predicate,
                match_arms,
                ..
            } => {
                internal::unresolved_type_for_pattern_match(predicate, match_arms)?;
            }
            Expr::Option {
                expr,
                inferred_type,
                ..
            } => {
                if let Some(expr) = expr {
                    queue.push_back(expr);
                }

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            parent_expr @ Expr::Result { expr, .. } => {
                internal::unresolved_type_for_result(expr, parent_expr)?
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    queue.push_back(arg);
                }
            }
            Expr::Unwrap { .. } => {}
            Expr::Throw { .. } => {}
            Expr::GetTag { .. } => {}
            Expr::ListComprehension {
                iterable_expr,
                yield_expr,
                ..
            } => internal::unresolved_type_for_list_comprehension(iterable_expr, yield_expr)?,
            Expr::ListReduce {
                iterable_expr,
                init_value_expr,
                yield_expr,
                ..
            } => internal::unresolved_type_for_list_aggregation(
                iterable_expr,
                init_value_expr,
                yield_expr,
            )?,
        }
    }

    Ok(())
}

mod internal {
    use crate::type_checker::unresolved_types::check_unresolved_types;
    use crate::type_checker::UnResolvedTypesError;
    use crate::{Expr, MatchArm};
    use std::ops::Deref;

    pub fn unresolved_types_in_record(
        expr_fields: &Vec<(String, Expr)>,
    ) -> Result<(), UnResolvedTypesError> {
        for (field_name, field_expr) in expr_fields {
            check_unresolved_types(field_expr)
                .map_err(|error| error.at_field(field_name.clone()))?;
        }

        Ok(())
    }

    pub fn unresolved_types_in_tuple(expr_fields: &[Expr]) -> Result<(), UnResolvedTypesError> {
        for (index, field_expr) in expr_fields.iter().enumerate() {
            check_unresolved_types(field_expr).map_err(|error| {
                error
                    .at_index(index)
                    .with_additional_message("Invalid element in Tuple")
            })?;
        }

        Ok(())
    }

    pub fn unresolved_type_for_concat(expr_fields: &[Expr]) -> Result<(), UnResolvedTypesError> {
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

    pub fn unresolved_type_for_list_comprehension(
        iterable_expr: &Expr,
        yield_expr: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        let iterable_type = iterable_expr.inferred_type();
        if iterable_type.un_resolved() {
            return Err(UnResolvedTypesError::new(iterable_expr));
        } else {
            check_unresolved_types(iterable_expr)?;
        }

        let yield_expr_type = yield_expr.inferred_type();
        if yield_expr_type.un_resolved() {
            return Err(UnResolvedTypesError::new(yield_expr));
        } else {
            check_unresolved_types(yield_expr)?;
        }

        Ok(())
    }

    pub fn unresolved_type_for_list_aggregation(
        iterable_expr: &Expr,
        yield_expr: &Expr,
        init_value_expr: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        let iterable_type = iterable_expr.inferred_type();
        if iterable_type.un_resolved() {
            return Err(UnResolvedTypesError::new(iterable_expr));
        } else {
            check_unresolved_types(iterable_expr)?;
        }

        let yield_expr_type = yield_expr.inferred_type();
        if yield_expr_type.un_resolved() {
            return Err(UnResolvedTypesError::new(yield_expr));
        } else {
            check_unresolved_types(yield_expr)?;
        }

        let init_value_expr_type = init_value_expr.inferred_type();
        if init_value_expr_type.un_resolved() {
            return Err(UnResolvedTypesError::new(init_value_expr));
        } else {
            check_unresolved_types(init_value_expr)?;
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
                .cloned()
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
        parent_expr: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        let ok_expr = ok_err.clone().ok();
        let error_expr = ok_err.clone().err();
        if let Some(ok_expr_inner) = ok_expr.clone() {
            check_unresolved_types(&ok_expr_inner)
                .map_err(|error| error.with_parent_expr(parent_expr))?;
        }

        if let Some(error_expr_inner) = error_expr.clone() {
            check_unresolved_types(&error_expr_inner)
                .map_err(|error| error.with_parent_expr(parent_expr))?;
        }

        Ok(())
    }

    pub fn unresolved_types_in_list(expr_fields: &[Expr]) -> Result<(), UnResolvedTypesError> {
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
}

#[cfg(test)]
mod unresolved_types_tests {
    use crate::{compile, Expr};
    use test_r::test;

    #[test]
    fn test_unresolved_types_identifier() {
        let expr = Expr::from_text("hello").unwrap();
        compile(&expr, &vec![]).unwrap_err();
        assert_eq!(
            compile(&expr, &vec![]).unwrap_err().to_string(),
            "Unable to determine the type of `hello`. `hello` is unknown identifier"
        );
    }

    #[test]
    fn test_unresolved_type_record() {
        let expr = Expr::from_text("{a: 1, b: \"hello\"}").unwrap();
        compile(&expr, &vec![]).unwrap_err();
        assert_eq!(compile(&expr, &vec![]).unwrap_err().to_string(), "Unable to determine the type of `1` in `a`. Number literals must have a type annotation. Example: `1: u64`");
    }

    #[test]
    fn test_unresolved_type_nested_record() {
        let expr = Expr::from_text("{foo: {a: 1, b: \"hello\"}}").unwrap();
        compile(&expr, &vec![]).unwrap_err();
        assert_eq!(compile(&expr, &vec![]).unwrap_err().to_string(), "Unable to determine the type of `1` in `foo.a`. Number literals must have a type annotation. Example: `1: u64`");
    }

    #[test]
    fn test_unresolved_type_nested_record_index() {
        let expr = Expr::from_text("{foo: {a: \"bar\", b: (\"foo\", hello)}}").unwrap();
        compile(&expr, &vec![]).unwrap_err();
        assert_eq!(
            compile(&expr, &vec![]).unwrap_err().to_string(),
            "Unable to determine the type of `hello` in `foo.b[1]`. `hello` is unknown identifier. Invalid element in Tuple"
        );
    }

    #[test]
    fn test_unresolved_type_result_ok() {
        let expr = Expr::from_text("ok(hello)").unwrap();
        assert_eq!(
            compile(&expr, &vec![]).unwrap_err().to_string(),
            "Unable to determine the type of `hello` in ok(hello). `hello` is unknown identifier"
        );
    }

    #[test]
    fn test_unresolved_type_result_err() {
        let expr = Expr::from_text("err(hello)").unwrap();
        assert_eq!(
            compile(&expr, &vec![]).unwrap_err().to_string(),
            "Unable to determine the type of `hello` in err(hello). `hello` is unknown identifier"
        );
    }
}
