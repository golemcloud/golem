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

use crate::{Expr, ExprVisitor, InferredType, TypeInternal, UnResolvedTypesError};

pub fn unify_types(expr: &mut Expr) -> Result<(), UnResolvedTypesError> {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_front() {
        match expr {
            Expr::Number {
                inferred_type,
                number,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        return Err(UnResolvedTypesError::from(
                            &Expr::number(number.value.clone())
                                .with_source_span(source_span.clone()),
                            None,
                        )
                        .with_additional_error_detail(format!("invalid number, {}", e)));
                    }
                }
            }

            Expr::Record {
                inferred_type,
                exprs,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let exprs = exprs
                            .iter()
                            .map(|(a, b)| (a.clone(), b.as_ref().clone()))
                            .collect();

                        return Err(UnResolvedTypesError::from(
                            &Expr::record(exprs).with_source_span(source_span.clone()),
                            None,
                        )
                        .with_additional_error_detail(format!("invalid record, {}", e)));
                    }
                }
            }
            Expr::Tuple {
                inferred_type,
                exprs,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        return Err(UnResolvedTypesError::from(
                            &Expr::tuple(exprs.clone()).with_source_span(source_span.clone()),
                            None,
                        )
                        .with_additional_error_detail(format!("invalid tuple, {}", e)))
                    }
                }
            }

            Expr::Range {
                inferred_type,
                range,
                source_span,
                type_annotation,
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        return Err(UnResolvedTypesError::from(
                            &Expr::Range {
                                range: range.clone(),
                                source_span: source_span.clone(),
                                type_annotation: type_annotation.clone(),
                                inferred_type: inferred_type.clone(),
                            },
                            None,
                        )
                        .with_additional_error_detail(format!("invalid range, {}", e)))
                    }
                }
            }

            Expr::Sequence {
                exprs,
                inferred_type,
                type_annotation,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        return Err(UnResolvedTypesError::from(
                            &Expr::sequence(exprs.clone(), type_annotation.clone())
                                .with_source_span(source_span.clone()),
                            None,
                        )
                        .with_additional_error_detail(format!("invalid sequence, {}", e)));
                    }
                }
            }
            Expr::Option {
                inferred_type,
                expr,
                source_span,
                type_annotation,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        return Err(UnResolvedTypesError::from(
                            &Expr::option(expr.as_deref().cloned())
                                .with_type_annotation_opt(type_annotation.clone())
                                .with_source_span(source_span.clone()),
                            None,
                        )
                        .with_additional_error_detail(format!("invalid option, {}", e)));
                    }
                }
            }

            Expr::Result {
                expr: Ok(expr),
                inferred_type,
                type_annotation,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        return Err(UnResolvedTypesError::from(
                            &Expr::ok(expr.as_ref().clone(), type_annotation.clone())
                                .with_source_span(source_span.clone()),
                            None,
                        )
                        .with_additional_error_detail(format!("invalid result-ok, {}", e)));
                    }
                }
            }

            Expr::Result {
                expr: Err(expr),
                inferred_type,
                type_annotation,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        return Err(UnResolvedTypesError::from(
                            &Expr::err(expr.as_ref().clone(), type_annotation.clone())
                                .with_source_span(source_span.clone()),
                            None,
                        )
                        .with_additional_error_detail(format!("invalid result-err, {}", e)));
                    }
                }
            }

            Expr::Cond {
                inferred_type,
                cond,
                lhs,
                rhs,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        return Err(UnResolvedTypesError::from(
                            &Expr::cond(
                                cond.as_ref().clone(),
                                lhs.as_ref().clone(),
                                rhs.as_ref().clone(),
                            )
                            .with_source_span(source_span.clone()),
                            None,
                        )
                        .with_additional_error_detail(format!(
                            "invalid if-else condition, {}",
                            e
                        )));
                    }
                }
            }

            Expr::Length {
                inferred_type,
                expr,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        return Err(UnResolvedTypesError::from(
                            &Expr::length(expr.as_ref().clone())
                                .with_source_span(source_span.clone()),
                            None,
                        )
                        .with_additional_error_detail(format!("invalid length function, {}", e)));
                    }
                }
            }

            Expr::ListComprehension {
                iterated_variable,
                iterable_expr,
                yield_expr,
                inferred_type,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        return Err(UnResolvedTypesError::from(
                            &Expr::list_comprehension(
                                iterated_variable.clone(),
                                iterable_expr.as_ref().clone(),
                                yield_expr.as_ref().clone(),
                            )
                            .with_source_span(source_span.clone()),
                            None,
                        )
                        .with_additional_error_detail(format!(
                            "invalid list comprehension, {}",
                            e
                        )));
                    }
                }
            }

            Expr::ListReduce {
                inferred_type,
                reduce_variable,
                iterated_variable,
                iterable_expr,
                yield_expr,
                init_value_expr,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        return Err(UnResolvedTypesError::from(
                            &Expr::list_reduce(
                                reduce_variable.clone(),
                                iterated_variable.clone(),
                                iterable_expr.as_ref().clone(),
                                yield_expr.as_ref().clone(),
                                init_value_expr.as_ref().clone(),
                            )
                            .with_source_span(source_span.clone()),
                            None,
                        )
                        .with_additional_error_detail(format!("invalid list aggregation, {}", e)));
                    }
                }
            }

            Expr::PatternMatch {
                inferred_type,
                predicate,
                match_arms,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => {
                        *inferred_type = unified_type;
                    }
                    Err(e) => {
                        return Err(UnResolvedTypesError::from(
                            &Expr::pattern_match(predicate.as_ref().clone(), match_arms.clone())
                                .with_source_span(source_span.clone()),
                            None,
                        )
                        .with_additional_error_detail(format!("invalid pattern match, {}", e)));
                    }
                }
            }
            Expr::Call {
                call_type,
                args,
                inferred_type,
                source_span,
                type_annotation,
                generic_type_parameter,
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr = Expr::Call {
                            call_type: call_type.clone(),
                            args: args.clone(),
                            inferred_type: InferredType::unknown(),
                            source_span: source_span.clone(),
                            generic_type_parameter: generic_type_parameter.clone(),
                            type_annotation: type_annotation.clone(),
                        };

                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!(
                                "invalid function call, {}",
                                e
                            )));
                    }
                }
            }
            Expr::SelectField {
                inferred_type,
                expr,
                field,
                source_span,
                type_annotation,
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr = Expr::select_field(
                            expr.as_ref().clone(),
                            field,
                            type_annotation.clone(),
                        )
                        .with_source_span(source_span.clone());
                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!(
                                "invalid field selection, {}",
                                e
                            )));
                    }
                }
            }

            Expr::SelectIndex {
                inferred_type,
                expr,
                index,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr =
                            Expr::select_index(expr.as_ref().clone(), index.as_ref().clone())
                                .with_source_span(source_span.clone());
                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!(
                                "invalid dynamic field selection, {}",
                                e
                            )));
                    }
                }
            }

            Expr::Let { .. } => {}
            Expr::Literal {
                inferred_type,
                value,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr =
                            Expr::literal(value.clone()).with_source_span(source_span.clone());
                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!("invalid literal, {}", e)));
                    }
                }
            }
            Expr::Flags {
                inferred_type,
                flags,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr = Expr::flags(flags.clone()).with_source_span(source_span.clone());
                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!("invalid flags, {}", e)));
                    }
                }
            }
            Expr::Identifier {
                inferred_type,
                variable_id,
                source_span,
                type_annotation,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr = Expr::identifier_with_variable_id(
                            variable_id.clone(),
                            type_annotation.clone(),
                        )
                        .with_source_span(source_span.clone());
                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!("invalid identifier, {}", e)));
                    }
                }
            }
            Expr::Boolean { .. } => {}
            Expr::Concat { .. } => {}
            Expr::ExprBlock { inferred_type, .. } => {
                let unified_inferred_type = inferred_type.unify();

                if let Ok(unified_type) = unified_inferred_type {
                    *inferred_type = unified_type
                }
            }

            Expr::Not {
                inferred_type,
                expr,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr =
                            Expr::not(expr.as_ref().clone()).with_source_span(source_span.clone());

                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!(
                                "invalid NOT expression, {}",
                                e
                            )));
                    }
                }
            }
            Expr::Unwrap {
                inferred_type,
                expr,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr = expr.unwrap().with_source_span(source_span.clone());

                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!(
                                "cannot determine the type, {}",
                                e
                            )));
                    }
                }
            }

            Expr::Throw {
                inferred_type,
                message,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr = Expr::throw(message).with_source_span(source_span.clone());

                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!(
                                "cannot determine the type, {}",
                                e
                            )));
                    }
                }
            }

            Expr::GetTag {
                inferred_type,
                expr,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr = Expr::get_tag(expr.as_ref().clone())
                            .with_source_span(source_span.clone());
                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!(
                                "cannot determine the type, {}",
                                e
                            )));
                    }
                }
            }

            Expr::GreaterThan { .. } => {}

            Expr::Plus {
                inferred_type,
                lhs,
                rhs,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr = Expr::plus(lhs.as_ref().clone(), rhs.as_ref().clone())
                            .with_source_span(source_span.clone());
                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!(
                                "invalid plus expression, {}",
                                e
                            )));
                    }
                }
            }

            Expr::Minus {
                inferred_type,
                lhs,
                rhs,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr = Expr::minus(lhs.as_ref().clone(), rhs.as_ref().clone())
                            .with_source_span(source_span.clone());
                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!(
                                "invalid plus expression, {}",
                                e
                            )));
                    }
                }
            }

            Expr::Divide {
                inferred_type,
                lhs,
                rhs,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr = Expr::divide(lhs.as_ref().clone(), rhs.as_ref().clone())
                            .with_source_span(source_span.clone());
                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!(
                                "invalid plus expression, {}",
                                e
                            )));
                    }
                }
            }

            Expr::Multiply {
                inferred_type,
                lhs,
                rhs,
                source_span,
                ..
            } => {
                let unified_inferred_type = inferred_type.unify();

                match unified_inferred_type {
                    Ok(unified_type) => *inferred_type = unified_type,
                    Err(e) => {
                        let expr = Expr::multiply(lhs.as_ref().clone(), rhs.as_ref().clone())
                            .with_source_span(source_span.clone());
                        return Err(UnResolvedTypesError::from(&expr, None)
                            .with_additional_error_detail(format!(
                                "invalid plus expression, {}",
                                e
                            )));
                    }
                }
            }

            Expr::And { .. } => {}
            Expr::Or { .. } => {}

            Expr::GreaterThanOrEqualTo { .. } => {}
            Expr::LessThanOrEqualTo { .. } => {}
            Expr::EqualTo { .. } => {}
            Expr::LessThan { .. } => {}
            Expr::InvokeMethodLazy { .. } => {}
        }
    }

    Ok(())
}
