use crate::call_type::{CallType, InstanceCreationType};
use crate::type_checker::UnResolvedTypesError;
use crate::Expr;
use std::collections::VecDeque;
use std::ops::Deref;

struct QueuedExpr<'a> {
    expr: &'a Expr,
    parent: Option<&'a Expr>,
}

impl<'a> QueuedExpr<'a> {
    pub fn new(expr: &'a Expr, parent: &'a Expr) -> Self {
        QueuedExpr {
            expr,
            parent: Some(parent),
        }
    }
}

pub fn check_unresolved_types(expr: &Expr) -> Result<(), UnResolvedTypesError> {
    let mut queue = VecDeque::new();
    queue.push_back(QueuedExpr { expr, parent: None });

    while let Some(queued_expr) = queue.pop_back() {
        let expr = queued_expr.expr;

        // Parent of `outer_expr`s below
        let parent = queued_expr.parent.cloned();

        match expr {
            outer_expr @ Expr::Let { expr, .. } => {
                queue.push_back(QueuedExpr::new(expr, outer_expr));
            }
            outer_expr @ Expr::InvokeMethodLazy {
                lhs,
                args,
                inferred_type,
                ..
            } => {
                queue.push_back(QueuedExpr::new(lhs, outer_expr));

                for arg in args {
                    queue.push_back(QueuedExpr::new(arg, outer_expr));
                }

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::SelectField {
                expr,
                field,
                inferred_type,
                ..
            } => {
                queue.push_back(QueuedExpr::new(expr, outer_expr));
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent).at_field(field.clone()));
                }
            }
            outer_expr @ Expr::SelectIndex {
                expr,
                index,
                inferred_type,
                ..
            } => {
                queue.push_back(QueuedExpr::new(expr, outer_expr));
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent).at_index(*index));
                }
            }
            outer_expr @ Expr::Sequence {
                exprs,
                inferred_type,
                ..
            } => {
                internal::unresolved_types_in_list(exprs, outer_expr)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::Record {
                exprs,
                inferred_type,
                ..
            } => {
                internal::unresolved_types_in_record(
                    &exprs
                        .iter()
                        .map(|(k, v)| (k.clone(), v.deref().clone()))
                        .collect(),
                    outer_expr,
                )?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::Tuple { exprs, .. } => {
                internal::unresolved_types_in_tuple(exprs, outer_expr)?;
            }
            Expr::Literal { inferred_type, .. } => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            Expr::Number { inferred_type, .. } => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent)
                        .with_additional_error_detail(
                        "possible types: u64, u32, u16, u8, i64, i32, i16, i8, f64, f32"
                    ));
                }
            }

            Expr::Flags { inferred_type, .. } => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            Expr::Identifier { inferred_type, .. } => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent)
                        .with_additional_error_detail(
                            format!("`{}` is unknown identifier", expr).as_str(),
                        ));
                }
            }
            Expr::Boolean { inferred_type, .. } => {
                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::Concat {
                exprs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_concat(exprs, outer_expr)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::ExprBlock { exprs, .. } => {
                for expr in exprs {
                    queue.push_back(QueuedExpr::new(expr, outer_expr));
                }
            }
            outer_expr @ Expr::Not {
                expr,
                inferred_type,
                ..
            } => {
                queue.push_back(QueuedExpr::new(expr, outer_expr));

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::GreaterThan {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs, outer_expr)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::And {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs, outer_expr)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::Plus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs, outer_expr)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::Minus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs, outer_expr)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::Multiply {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs, outer_expr)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::Divide {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs, outer_expr)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::Or {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs, outer_expr)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::GreaterThanOrEqualTo { lhs, rhs, .. } => {
                internal::unresolved_type_for_binary_op(lhs, rhs, outer_expr)?;

                if outer_expr.inferred_type().un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::LessThanOrEqualTo {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs, outer_expr)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::EqualTo {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs, outer_expr)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::LessThan { lhs, rhs, .. } => {
                internal::unresolved_type_for_binary_op(lhs, rhs, outer_expr)?;

                if outer_expr.inferred_type().un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::Cond {
                cond,
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_if_condition(cond, lhs, rhs, outer_expr)?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::PatternMatch {
                predicate,
                match_arms,
                ..
            } => {
                internal::unresolved_type_for_pattern_match(predicate, match_arms, outer_expr)?;
            }
            outer_expr @ Expr::Option {
                expr: expr0,
                inferred_type,
                ..
            } => {
                if let Some(expr) = expr0 {
                    queue.push_back(QueuedExpr::new(expr, outer_expr));
                }

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::Result { expr, .. } => {
                internal::unresolved_type_for_result(expr, outer_expr)?
            }

            outer_expr @ Expr::Call {
                call_type,
                args,
                inferred_type,
                ..
            } => {
                for arg in args {
                    queue.push_back(QueuedExpr::new(arg, outer_expr));
                }

                let worker_name = call_type.worker_expr();

                if let Some(worker_name) = worker_name {
                    queue.push_back(QueuedExpr::new(worker_name, outer_expr));
                }

                let additional_message = match call_type {
                    CallType::Function { function_name, .. } => {
                        format!(
                            "cannot determine the return type of the function `{}`",
                            function_name
                        )
                    }
                    CallType::VariantConstructor(name) => {
                        format!(
                            "cannot determine the type of the variant constructor `{}`",
                            name
                        )
                    }
                    CallType::EnumConstructor(name) => {
                        format!(
                            "cannot determine the type of the enum constructor `{}`",
                            name
                        )
                    }
                    CallType::InstanceCreation(instance) => match instance {
                        InstanceCreationType::Worker { worker_name } => {
                            let worker_name = worker_name
                                .as_ref()
                                .map_or("".to_string(), |x| format!(", with worker `{}`", x));
                            format!(
                                "cannot determine the type of instance creation `{}`",
                                worker_name
                            )
                        }
                        InstanceCreationType::Resource {
                            worker_name,
                            resource_name,
                        } => {
                            let worker_name = worker_name
                                .as_ref()
                                .map_or("".to_string(), |x| format!(", with worker `{}`", x));
                            format!(
                                "cannot determine the type of the resource creation `{}`{}",
                                resource_name.resource_name, worker_name
                            )
                        }
                    },
                };

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent)
                        .with_additional_error_detail(additional_message));
                }
            }
            Expr::Unwrap { .. } => {}
            Expr::Throw { .. } => {}
            Expr::GetTag { .. } => {}
            outer_expr @ Expr::ListComprehension {
                iterable_expr,
                yield_expr,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_list_comprehension(
                    iterable_expr,
                    yield_expr,
                    outer_expr,
                )?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
            outer_expr @ Expr::ListReduce {
                iterable_expr,
                init_value_expr,
                yield_expr,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_list_aggregation(
                    iterable_expr,
                    init_value_expr,
                    yield_expr,
                    outer_expr,
                )?;

                if inferred_type.un_resolved() {
                    return Err(UnResolvedTypesError::from(expr, parent));
                }
            }
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
        original_expr: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        for (field_name, field_expr) in expr_fields {
            check_unresolved_types(field_expr).map_err(|err| {
                err.at_field(field_name.clone())
                    .with_parent_expr(original_expr)
            })?;
        }

        Ok(())
    }

    pub fn unresolved_types_in_tuple(
        exprs_in_tuple: &[Expr],
        original_expr: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        for (index, field_expr) in exprs_in_tuple.iter().enumerate() {
            check_unresolved_types(field_expr).map_err(|error| {
                error
                    .at_index(index)
                    .with_parent_expr(original_expr)
                    .with_additional_error_detail("Invalid element in Tuple")
            })?;
        }

        Ok(())
    }

    pub fn unresolved_type_for_concat(
        expr_fields: &[Expr],
        original_expr: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        for (index, field_expr) in expr_fields.iter().enumerate() {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err(
                    UnResolvedTypesError::from(field_expr, Some(original_expr.clone()))
                        .at_index(index),
                );
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_type_for_binary_op(
        left: &Expr,
        right: &Expr,
        original_expr: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        let left_type = left.inferred_type();
        let right_type = right.inferred_type();
        if left_type.un_resolved() {
            return Err(UnResolvedTypesError::from(left, Some(original_expr.clone())));
        } else {
            check_unresolved_types(left)?;
        }

        if right_type.un_resolved() {
            return Err(UnResolvedTypesError::from(
                right,
                Some(original_expr.clone()),
            ));
        } else {
            check_unresolved_types(right)?;
        }

        Ok(())
    }

    pub fn unresolved_type_for_list_comprehension(
        iterable_expr: &Expr,
        yield_expr: &Expr,
        original_expr: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        let iterable_type = iterable_expr.inferred_type();
        if iterable_type.un_resolved() {
            return Err(UnResolvedTypesError::from(
                iterable_expr,
                Some(original_expr.clone()),
            ));
        } else {
            check_unresolved_types(iterable_expr)?;
        }

        let yield_expr_type = yield_expr.inferred_type();
        if yield_expr_type.un_resolved() {
            return Err(UnResolvedTypesError::from(
                yield_expr,
                Some(original_expr.clone()),
            ));
        } else {
            check_unresolved_types(yield_expr)?;
        }

        Ok(())
    }

    pub fn unresolved_type_for_list_aggregation(
        iterable_expr: &Expr,
        yield_expr: &Expr,
        init_value_expr: &Expr,
        outer_expr: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        let iterable_type = iterable_expr.inferred_type();
        if iterable_type.un_resolved() {
            return Err(UnResolvedTypesError::from(
                iterable_expr,
                Some(outer_expr.clone()),
            ));
        } else {
            check_unresolved_types(iterable_expr)?;
        }

        let yield_expr_type = yield_expr.inferred_type();
        if yield_expr_type.un_resolved() {
            return Err(UnResolvedTypesError::from(
                yield_expr,
                Some(outer_expr.clone()),
            ));
        } else {
            check_unresolved_types(yield_expr)?;
        }

        let init_value_expr_type = init_value_expr.inferred_type();
        if init_value_expr_type.un_resolved() {
            return Err(UnResolvedTypesError::from(
                init_value_expr,
                Some(outer_expr.clone()),
            ));
        } else {
            check_unresolved_types(init_value_expr)?;
        }

        Ok(())
    }

    pub fn unresolved_type_for_if_condition(
        cond: &Expr,
        if_expr: &Expr,
        else_expr: &Expr,
        outer_expr: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        let cond_type = cond.inferred_type();
        let if_type = if_expr.inferred_type();
        let else_type = else_expr.inferred_type();
        if cond_type.un_resolved() {
            return Err(UnResolvedTypesError::from(cond, Some(outer_expr.clone())));
        } else {
            check_unresolved_types(cond)?;
        }

        if if_type.un_resolved() {
            return Err(UnResolvedTypesError::from(if_expr, Some(outer_expr.clone())));
        } else {
            check_unresolved_types(if_expr)?;
        }

        if else_type.un_resolved() {
            return Err(UnResolvedTypesError::from(if_expr, Some(outer_expr.clone())));
        } else {
            check_unresolved_types(else_expr)?;
        }

        Ok(())
    }

    pub fn unresolved_type_for_pattern_match(
        cond: &Expr,
        match_arms: &Vec<MatchArm>,
        outer_expr: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        let cond_type = cond.inferred_type();
        if cond_type.is_unknown() || cond_type.is_one_of() {
            return Err(UnResolvedTypesError::from(cond, Some(outer_expr.clone())));
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
                    return Err(UnResolvedTypesError::from(&expr, Some(outer_expr.clone())));
                } else {
                    check_unresolved_types(&expr)?;
                }
            }

            let expr = match_arm.clone().arm_resolution_expr;

            let expr_type = expr.inferred_type();
            if expr_type.un_resolved() {
                return Err(UnResolvedTypesError::from(
                    expr.deref(),
                    Some(outer_expr.clone()),
                ));
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

    pub fn unresolved_types_in_list(
        expr_fields: &[Expr],
        parent: &Expr,
    ) -> Result<(), UnResolvedTypesError> {
        for (index, field_expr) in expr_fields.iter().enumerate() {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() || field_type.is_one_of() {
                return Err(
                    UnResolvedTypesError::from(field_expr, Some(parent.clone())).at_index(index)
                );
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
            "cannot determine the type of `hello`. `hello` is unknown identifier"
        );
    }

    #[test]
    fn test_unresolved_type_record() {
        let expr = Expr::from_text("{a: 1, b: \"hello\"}").unwrap();
        let error = compile(&expr, &vec![]).unwrap_err();

        let expected = r#"
cannot determine the type of the following rib expression at line 1, column 5
`1`
found within:
`{a: 1, b: "hello"}`
unrecognized type at field: `a`
possible types: u64, u32, u16, u8, i64, i32, i16, i8, f64, f32
help: consider specifying the type explicitly. Examples: `1: u64`, `person.age: u8`
help: or specify the type in let binding. Example: let numbers: list<u8> = [1, 2, 3]
"#.strip_prefix("\n").unwrap();
        assert_eq!(error,expected);
    }

    #[test]
    fn test_unresolved_type_nested_record() {
        let expr = Expr::from_text("{foo: {a: 1, b: \"hello\"}}").unwrap();
        compile(&expr, &vec![]).unwrap_err();
        assert_eq!(compile(&expr, &vec![]).unwrap_err().to_string(), "cannot determine the type of `1` in `foo.a`. Number literals must have a type annotation. Example: `1: u64`");
    }

    #[test]
    fn test_unresolved_type_nested_record_index() {
        let expr = Expr::from_text("{foo: {a: \"bar\", b: (\"foo\", hello)}}").unwrap();
        compile(&expr, &vec![]).unwrap_err();
        assert_eq!(
            compile(&expr, &vec![]).unwrap_err().to_string(),
            "cannot determine the type of `hello` in `foo.b[1]`. `hello` is unknown identifier. Invalid element in Tuple"
        );
    }

    #[test]
    fn test_unresolved_type_result_ok() {
        let expr = Expr::from_text("ok(hello)").unwrap();
        assert_eq!(
            compile(&expr, &vec![]).unwrap_err().to_string(),
            "cannot determine the type of `hello` in ok(hello). `hello` is unknown identifier"
        );
    }

    #[test]
    fn test_unresolved_type_result_err() {
        let expr = Expr::from_text("err(hello)").unwrap();
        assert_eq!(
            compile(&expr, &vec![]).unwrap_err().to_string(),
            "cannot determine the type of `hello` in err(hello). `hello` is unknown identifier"
        );
    }
}
