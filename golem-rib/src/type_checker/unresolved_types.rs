use crate::type_checker::UnResolvedTypesError;
use crate::Expr;
use std::collections::VecDeque;
use std::ops::Deref;

pub fn check_unresolved_types(expr: &Expr) -> Result<(), UnResolvedTypesError> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Let(_, _, expr, _) => {
                queue.push_back(expr);
            }
            Expr::SelectField(expr, field, _, inferred_type) => {
                queue.push_back(expr);
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr).at_field(field.clone()));
                }
            }
            Expr::SelectIndex(expr, index, _, inferred_type) => {
                queue.push_back(expr);
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr).at_index(*index));
                }
            }
            Expr::Sequence(exprs, inferred_type) => {
                internal::unresolved_types_in_list(exprs)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::Record(fields, inferred_type) => {
                internal::unresolved_types_in_record(
                    &fields
                        .iter()
                        .map(|(k, v)| (k.clone(), v.deref().clone()))
                        .collect(),
                )?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::Tuple(exprs, _) => {
                internal::unresolved_types_in_tuple(exprs)?;
            }
            Expr::Literal(_, inferred_type) => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::Number(_, _, inferred_type) => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr).with_additional_message(
                        "Number literals must have a type annotation. Example: `1u64`",
                    ));
                }
            }
            Expr::Flags(_, inferred_type) => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::Identifier(_, _, inferred_type) => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr).with_additional_message(
                        format!("`{}` is unknown identifier", expr).as_str(),
                    ));
                }
            }
            Expr::Boolean(_, inferred_type) => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::Concat(exprs, inferred_type) => {
                internal::unresolved_type_for_concat(exprs)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::ExprBlock(exprs, _) => {
                for expr in exprs {
                    queue.push_back(expr);
                }
            }
            Expr::Not(expr, inferred_type) => {
                queue.push_back(expr);
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::GreaterThan(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)?;
            }
            Expr::And(left, right, _) => internal::unresolved_type_for_binary_op(left, right)?,
            Expr::Plus(left, right, _) => internal::unresolved_type_for_binary_op(left, right)?,
            Expr::Minus(left, right, _) => internal::unresolved_type_for_binary_op(left, right)?,
            Expr::Multiply(left, right, _) => internal::unresolved_type_for_binary_op(left, right)?,
            Expr::Divide(left, right, _) => internal::unresolved_type_for_binary_op(left, right)?,
            Expr::Or(left, right, _) => internal::unresolved_type_for_binary_op(left, right)?,
            Expr::GreaterThanOrEqualTo(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)?;
            }
            Expr::LessThanOrEqualTo(left, right, _) => {
                internal::unresolved_type_for_binary_op(left, right)?;
            }
            Expr::EqualTo(left, right, _) => internal::unresolved_type_for_binary_op(left, right)?,
            Expr::LessThan(left, right, _) => internal::unresolved_type_for_binary_op(left, right)?,
            Expr::Cond(cond, left, right, inferred_type) => {
                internal::unresolved_type_for_if_condition(cond, left, right)?;
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            Expr::PatternMatch(cond, arms, _) => {
                internal::unresolved_type_for_pattern_match(cond, arms)?;
            }
            Expr::Option(option, inferred_type) => {
                if let Some(expr) = option {
                    queue.push_back(expr);
                }

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::new(expr));
                }
            }
            expr @ Expr::Result(ok_err, _) => internal::unresolved_type_for_result(ok_err, expr)?,
            Expr::Call(_, args, _) => {
                for arg in args {
                    queue.push_back(arg);
                }
            }
            Expr::Unwrap(_, _) => {}
            Expr::Throw(_, _) => {}
            Expr::GetTag(_, _) => {}
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
        assert_eq!(compile(&expr, &vec![]).unwrap_err().to_string(), "Unable to determine the type of `1` in the record at path `a`. Number literals must have a type annotation. Example: `1u64`");
    }

    #[test]
    fn test_unresolved_type_nested_record() {
        let expr = Expr::from_text("{foo: {a: 1, b: \"hello\"}}").unwrap();
        compile(&expr, &vec![]).unwrap_err();
        assert_eq!(compile(&expr, &vec![]).unwrap_err().to_string(), "Unable to determine the type of `1` in the record at path `foo.a`. Number literals must have a type annotation. Example: `1u64`");
    }

    #[test]
    fn test_unresolved_type_nested_record_index() {
        let expr = Expr::from_text("{foo: {a: \"bar\", b: (\"foo\", hello)}}").unwrap();
        compile(&expr, &vec![]).unwrap_err();
        assert_eq!(
            compile(&expr, &vec![]).unwrap_err().to_string(),
            "Unable to determine the type of `hello` in the record at path `foo.b[1]`. `hello` is unknown identifier. Invalid element in Tuple"
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
