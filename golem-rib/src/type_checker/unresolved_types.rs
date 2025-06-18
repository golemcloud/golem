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

use crate::call_type::{CallType, InstanceCreationType};
use crate::{Expr, UnResolvedTypesError};
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

            Expr::Range {
                range,
                inferred_type,
                ..
            } => {
                let exprs = range.get_exprs();

                for expr in exprs {
                    queue.push_back(expr);
                }

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
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

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }

            Expr::SelectField {
                expr,
                field,
                inferred_type,
                ..
            } => {
                queue.push_back(expr);
                if inferred_type.is_unknown() {
                    return Err(
                        UnResolvedTypesError::from(expr.source_span()).at_field(field.clone())
                    );
                }
            }

            Expr::SelectIndex {
                expr,
                index,
                inferred_type,
                ..
            } => {
                queue.push_back(expr);
                queue.push_back(index);

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }

            Expr::Sequence {
                exprs,
                inferred_type,
                ..
            } => {
                internal::unresolved_types_in_list(exprs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::Record {
                exprs,
                inferred_type,
                ..
            } => {
                internal::unresolved_types_in_record(
                    &exprs
                        .iter()
                        .map(|(k, v)| (k.clone(), v.deref().clone()))
                        .collect(),
                )?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::Tuple { exprs, .. } => {
                internal::unresolved_types_in_tuple(exprs)?;
            }
            Expr::Literal { inferred_type, .. } => {
                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::Number { inferred_type, .. } => {
                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }

            Expr::Flags { inferred_type, .. } => {
                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::Identifier { inferred_type, .. } => {
                if inferred_type.is_unknown() {
                    return Err(
                        UnResolvedTypesError::from(expr.source_span()).with_help_message(
                            format!("make sure `{}` is a valid identifier", expr).as_str(),
                        ),
                    );
                }
            }
            Expr::Boolean { inferred_type, .. } => {
                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::Concat {
                exprs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_concat(exprs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
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
                ..
            } => {
                queue.push_back(expr);

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::GreaterThan {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::And {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::Plus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::Minus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::Multiply {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::Divide {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::Or {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::GreaterThanOrEqualTo {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::LessThanOrEqualTo {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::EqualTo {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::LessThan {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_binary_op(lhs, rhs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::Cond {
                cond,
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_if_condition(cond, lhs, rhs)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
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
                expr: expr0,
                inferred_type,
                ..
            } => {
                if let Some(expr) = expr0 {
                    queue.push_back(expr);
                }

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
            Expr::Result { expr, .. } => internal::unresolved_type_for_result(expr)?,

            Expr::Call {
                call_type,
                args,
                inferred_type,
                ..
            } => {
                for arg in args {
                    queue.push_back(arg);
                }

                let worker_name = call_type.worker_expr();

                if let Some(worker_name) = worker_name {
                    queue.push_back(worker_name);
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
                        InstanceCreationType::WitWorker { worker_name, .. } => {
                            let worker_name = worker_name
                                .as_ref()
                                .map_or("".to_string(), |x| format!(", with worker `{}`", x));
                            format!(
                                "cannot determine the type of instance creation `{}`",
                                worker_name
                            )
                        }
                        InstanceCreationType::WitResource {
                            module,
                            resource_name,
                            ..
                        } => {
                            let worker_name = module
                                .as_ref()
                                .and_then(|x| x.worker_name())
                                .map_or("".to_string(), |x| format!(", with worker `{}`", x));

                            format!(
                                "cannot determine the type of the resource creation `{}`{}",
                                resource_name.resource_name, worker_name
                            )
                        }
                    },
                };

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span())
                        .with_additional_error_detail(additional_message));
                }
            }
            Expr::Unwrap { .. } => {}
            Expr::Throw { .. } => {}
            Expr::GenerateWorkerName { .. } => {}
            Expr::GetTag { .. } => {}
            Expr::ListComprehension {
                iterable_expr,
                yield_expr,
                inferred_type,
                ..
            } => {
                internal::unresolved_type_for_list_comprehension(iterable_expr, yield_expr)?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }

            Expr::Length {
                expr,
                inferred_type,
                ..
            } => {
                queue.push_back(expr);

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }

            Expr::ListReduce {
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
                )?;

                if inferred_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                }
            }
        }
    }

    Ok(())
}

mod internal {
    use crate::type_checker::unresolved_types::check_unresolved_types;
    use crate::{Expr, MatchArm, UnResolvedTypesError};

    pub fn unresolved_types_in_record(
        expr_fields: &Vec<(String, Expr)>,
    ) -> Result<(), UnResolvedTypesError> {
        for (field_name, field_expr) in expr_fields {
            check_unresolved_types(field_expr).map_err(|err| err.at_field(field_name.clone()))?;
        }

        Ok(())
    }

    pub fn unresolved_types_in_tuple(exprs_in_tuple: &[Expr]) -> Result<(), UnResolvedTypesError> {
        for (index, field_expr) in exprs_in_tuple.iter().enumerate() {
            check_unresolved_types(field_expr).map_err(|error| error.at_index(index))?;
        }

        Ok(())
    }

    pub fn unresolved_type_for_concat(expr_fields: &[Expr]) -> Result<(), UnResolvedTypesError> {
        for (index, field_expr) in expr_fields.iter().enumerate() {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() {
                return Err(UnResolvedTypesError::from(field_expr.source_span()).at_index(index));
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
        if left_type.is_unknown() {
            return Err(UnResolvedTypesError::from(left.source_span()));
        } else {
            check_unresolved_types(left)?;
        }

        if right_type.is_unknown() {
            return Err(UnResolvedTypesError::from(right.source_span()));
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
        if iterable_type.is_unknown() {
            return Err(UnResolvedTypesError::from(iterable_expr.source_span()));
        } else {
            check_unresolved_types(iterable_expr)?;
        }

        let yield_expr_type = yield_expr.inferred_type();
        if yield_expr_type.is_unknown() {
            return Err(UnResolvedTypesError::from(yield_expr.source_span()));
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
        if iterable_type.is_unknown() {
            return Err(UnResolvedTypesError::from(iterable_expr.source_span()));
        } else {
            check_unresolved_types(iterable_expr)?;
        }

        let yield_expr_type = yield_expr.inferred_type();
        if yield_expr_type.is_unknown() {
            return Err(UnResolvedTypesError::from(yield_expr.source_span()));
        } else {
            check_unresolved_types(yield_expr)?;
        }

        let init_value_expr_type = init_value_expr.inferred_type();
        if init_value_expr_type.is_unknown() {
            return Err(UnResolvedTypesError::from(init_value_expr.source_span()));
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
        if cond_type.is_unknown() {
            return Err(UnResolvedTypesError::from(cond.source_span()));
        } else {
            check_unresolved_types(cond)?;
        }

        if if_type.is_unknown() {
            return Err(UnResolvedTypesError::from(if_expr.source_span()));
        } else {
            check_unresolved_types(if_expr)?;
        }

        if else_type.is_unknown() {
            return Err(UnResolvedTypesError::from(if_expr.source_span()));
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
        if cond_type.is_unknown() {
            return Err(UnResolvedTypesError::from(cond.source_span()));
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
                if expr_type.is_unknown() {
                    return Err(UnResolvedTypesError::from(expr.source_span()));
                } else {
                    check_unresolved_types(&expr)?;
                }
            }

            let expr = match_arm.clone().arm_resolution_expr;

            let expr_type = expr.inferred_type();
            if expr_type.is_unknown() {
                return Err(UnResolvedTypesError::from(expr.source_span()));
            } else {
                check_unresolved_types(&expr)?;
            }
        }

        Ok(())
    }

    pub fn unresolved_type_for_result(
        ok_err: &Result<Box<Expr>, Box<Expr>>,
    ) -> Result<(), UnResolvedTypesError> {
        let ok_expr = ok_err.as_ref().ok();

        let error_expr = ok_err.as_ref().err();

        if let Some(ok_expr_inner) = ok_expr {
            check_unresolved_types(&ok_expr_inner)?;
        }

        if let Some(error_expr_inner) = error_expr {
            check_unresolved_types(&error_expr_inner)?;
        }

        Ok(())
    }

    pub fn unresolved_types_in_list(expr_fields: &[Expr]) -> Result<(), UnResolvedTypesError> {
        for (index, field_expr) in expr_fields.iter().enumerate() {
            let field_type = field_expr.inferred_type();
            if field_type.is_unknown() {
                return Err(UnResolvedTypesError::from(field_expr.source_span()).at_index(index));
            } else {
                check_unresolved_types(field_expr)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod unresolved_types_tests {
    use crate::{Expr, RibCompiler};
    use test_r::test;

    fn strip_spaces(input: &str) -> String {
        let lines = input.lines();

        let first_line = lines
            .clone()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        let margin_width = first_line.chars().take_while(|c| c.is_whitespace()).count();

        let result = lines
            .map(|line| {
                if line.trim().is_empty() {
                    String::new()
                } else {
                    line[margin_width..].to_string()
                }
            })
            .collect::<Vec<String>>()
            .join("\n");

        result.strip_prefix("\n").unwrap_or(&result).to_string()
    }

    #[test]
    fn test_unresolved_types_identifier() {
        let expr = Expr::from_text("hello").unwrap();
        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let error = r#"
        error in the following rib found at line 1, column 1
        `hello`
        cause: cannot determine the type
        help: try specifying the expected type explicitly
        help: if the issue persists, please review the script for potential type inconsistencies
        help: make sure `hello` is a valid identifier
        "#;

        assert_eq!(error_msg, strip_spaces(error));
    }

    #[test]
    fn test_unresolved_type_nested_record_index() {
        let expr = Expr::from_text("{foo: {a: \"bar\", b: (\"foo\", hello)}}").unwrap();
        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 1, column 29
        `hello`
        cause: cannot determine the type
        unresolved type at path: `foo.b[1]`
        help: try specifying the expected type explicitly
        help: if the issue persists, please review the script for potential type inconsistencies
        help: make sure `hello` is a valid identifier
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_unresolved_type_result_ok() {
        let expr = Expr::from_text("ok(hello)").unwrap();
        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 1, column 4
        `hello`
        cause: cannot determine the type
        help: try specifying the expected type explicitly
        help: if the issue persists, please review the script for potential type inconsistencies
        help: make sure `hello` is a valid identifier
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }

    #[test]
    fn test_unresolved_type_result_err() {
        let expr = Expr::from_text("err(hello)").unwrap();

        let compiler = RibCompiler::default();
        let error_msg = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 1, column 5
        `hello`
        cause: cannot determine the type
        help: try specifying the expected type explicitly
        help: if the issue persists, please review the script for potential type inconsistencies
        help: make sure `hello` is a valid identifier
        "#;

        assert_eq!(error_msg, strip_spaces(expected));
    }
}
