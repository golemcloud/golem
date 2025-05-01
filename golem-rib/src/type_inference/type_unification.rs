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

use crate::inferred_type::{TypeOrigin, UnificationFailureInternal};
use crate::{Expr, ExprVisitor, InferredType, TypeUnificationError};

pub fn unify_types(expr: &mut Expr) -> Result<(), TypeUnificationError> {
    let mut original_expr = expr.clone();

    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_front() {
        match expr {
            Expr::Number {
                inferred_type,
                number,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::number(number.value.clone()).with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::Record {
                inferred_type,
                exprs,
                source_span,
                ..
            } => {
                let exprs = exprs
                    .iter()
                    .map(|(a, b)| (a.clone(), b.as_ref().clone()))
                    .collect();

                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::record(exprs).with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }
            Expr::Tuple {
                inferred_type,
                exprs,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::tuple(exprs.clone()).with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::Range {
                inferred_type,
                range,
                source_span,
                type_annotation,
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::Range {
                        range: range.clone(),
                        source_span: source_span.clone(),
                        type_annotation: type_annotation.clone(),
                        inferred_type: inferred_type.clone(),
                    },
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::Sequence {
                exprs,
                inferred_type,
                type_annotation,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::sequence(exprs.clone(), type_annotation.clone())
                        .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }
            Expr::Option {
                inferred_type,
                expr,
                source_span,
                type_annotation,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::option(expr.as_deref().cloned())
                        .with_type_annotation_opt(type_annotation.clone())
                        .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::Result {
                expr: Ok(expr),
                inferred_type,
                type_annotation,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::ok(expr.as_ref().clone(), type_annotation.clone())
                        .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::Result {
                expr: Err(expr),
                inferred_type,
                type_annotation,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::err(expr.as_ref().clone(), type_annotation.clone())
                        .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::Cond {
                inferred_type,
                cond,
                lhs,
                rhs,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::cond(
                        cond.as_ref().clone(),
                        lhs.as_ref().clone(),
                        rhs.as_ref().clone(),
                    ),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::Length {
                inferred_type,
                expr,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::length(expr.as_ref().clone()).with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::ListComprehension {
                iterated_variable,
                iterable_expr,
                yield_expr,
                inferred_type,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::list_comprehension(
                        iterated_variable.clone(),
                        iterable_expr.as_ref().clone(),
                        yield_expr.as_ref().clone(),
                    )
                    .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
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
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::list_reduce(
                        reduce_variable.clone(),
                        iterated_variable.clone(),
                        iterable_expr.as_ref().clone(),
                        yield_expr.as_ref().clone(),
                        init_value_expr.as_ref().clone(),
                    )
                    .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::PatternMatch {
                inferred_type,
                predicate,
                match_arms,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::pattern_match(predicate.as_ref().clone(), match_arms.clone())
                        .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }
            Expr::Call {
                call_type,
                args,
                inferred_type,
                source_span,
                type_annotation,
                generic_type_parameter,
            } => {
                let expr = Expr::Call {
                    call_type: call_type.clone(),
                    args: args.clone(),
                    inferred_type: InferredType::unknown(),
                    source_span: source_span.clone(),
                    generic_type_parameter: generic_type_parameter.clone(),
                    type_annotation: type_annotation.clone(),
                };

                let unified_type = unify_inferred_type(&mut original_expr, expr, inferred_type)?;

                *inferred_type = unified_type;
            }
            Expr::SelectField {
                inferred_type,
                expr,
                field,
                source_span,
                type_annotation,
            } => {
                let expr =
                    Expr::select_field(expr.as_ref().clone(), field, type_annotation.clone())
                        .with_source_span(source_span.clone());

                let unified_type = unify_inferred_type(&mut original_expr, expr, inferred_type)?;

                *inferred_type = unified_type;
            }

            Expr::SelectIndex {
                inferred_type,
                expr,
                index,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::select_index(expr.as_ref().clone(), index.as_ref().clone())
                        .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::Let { .. } => {}
            Expr::Literal {
                inferred_type,
                value,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::literal(value.clone()).with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }
            Expr::Flags {
                inferred_type,
                flags,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::flags(flags.clone()).with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }
            Expr::Identifier {
                inferred_type,
                variable_id,
                source_span,
                type_annotation,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::identifier_with_variable_id(variable_id.clone(), type_annotation.clone())
                        .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }
            Expr::Boolean { .. } => {}
            Expr::Concat { .. } => {}
            Expr::ExprBlock {
                inferred_type,
                source_span,
                ..
            } => {
                let unified_inferred_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::expr_block(vec![]).with_source_span(source_span.clone()),
                    inferred_type,
                );

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
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::not(expr.as_ref().clone())
                        .with_source_span(source_span.clone())
                        .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }
            Expr::Unwrap {
                inferred_type,
                expr,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    expr.unwrap().with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::Throw {
                inferred_type,
                message,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::throw(message).with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::GetTag {
                inferred_type,
                expr,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::get_tag(expr.as_ref().clone()).with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::GreaterThan { .. } => {}

            Expr::Plus {
                inferred_type,
                lhs,
                rhs,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::plus(lhs.as_ref().clone(), rhs.as_ref().clone())
                        .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::Minus {
                inferred_type,
                lhs,
                rhs,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::minus(lhs.as_ref().clone(), rhs.as_ref().clone())
                        .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::Divide {
                inferred_type,
                lhs,
                rhs,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::minus(lhs.as_ref().clone(), rhs.as_ref().clone())
                        .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
            }

            Expr::Multiply {
                inferred_type,
                lhs,
                rhs,
                source_span,
                ..
            } => {
                let unified_type = unify_inferred_type(
                    &mut original_expr,
                    Expr::multiply(lhs.as_ref().clone(), rhs.as_ref().clone())
                        .with_source_span(source_span.clone()),
                    inferred_type,
                )?;

                *inferred_type = unified_type;
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

fn unify_inferred_type(
    original_expr: &mut Expr,
    expr: Expr,
    inferred_type: &InferredType,
) -> Result<InferredType, TypeUnificationError> {
    let unification_result = inferred_type.unify();

    match unification_result {
        Ok(unified_type) => Ok(unified_type),
        Err(e) => match e {
            UnificationFailureInternal::TypeMisMatch { expected, found } => {
                let found_origin = found.critical_origin();
                let found_source_span = found_origin.source_span();
                let found_expr = found_source_span
                    .as_ref()
                    .and_then(|span| original_expr.lookup(span));

                let expected_origin = expected.critical_origin();

                let additional_message = match expected_origin {
                    TypeOrigin::PatternMatch(span) => {
                        format!(
                            "expected {} based on pattern match branch at line {} column {}",
                            expected.printable(),
                            span.start_line(),
                            span.start_column()
                        )
                    }
                    TypeOrigin::Default => "".to_string(),
                    TypeOrigin::NoOrigin => "".to_string(),
                    TypeOrigin::Declared(source_span) => {
                        format!(
                            "{} declared at line {} column {}",
                            expected.printable(),
                            source_span.start_line(),
                            source_span.start_column()
                        )
                    }
                    TypeOrigin::OriginatedAt(_) => "".to_string(),
                    TypeOrigin::Multiple(_) => "".to_string(),
                };

                match found_expr {
                    Some(found_expr) => Err(TypeUnificationError::type_mismatch_error(
                        found_expr,
                        None,
                        expected,
                        found,
                        vec![additional_message],
                    )),

                    None => {
                        let ambiguity_message = format!(
                            "conflicting types {}, {}",
                            found.printable(),
                            expected.printable()
                        );
                        Err(TypeUnificationError::unresolved_types_error(
                            expr,
                            None,
                            vec![ambiguity_message],
                        ))
                    }
                }
            }
            UnificationFailureInternal::ConflictingTypes {
                conflicting_types,
                additional_error_detail,
            } => {
                let mut additional_messages = vec![format!(
                    "conflicting types: {}",
                    conflicting_types
                        .iter()
                        .map(|t| t.printable())
                        .collect::<Vec<_>>()
                        .join(", ")
                )];

                additional_messages.extend(additional_error_detail);

                Err(TypeUnificationError::unresolved_types_error(
                    expr,
                    None,
                    additional_messages,
                ))
            }
            UnificationFailureInternal::UnknownType => {
                Err(TypeUnificationError::unresolved_types_error(
                    expr,
                    None,
                    vec!["cannot determine the type".to_string()],
                ))
            }
        },
    }
}
