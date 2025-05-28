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

use crate::inferred_type::UnificationFailureInternal;
use crate::rib_source_span::SourceSpan;
use crate::{Expr, ExprVisitor, InferredType, TypeUnificationError};

pub fn unify_types(expr: &mut Expr) -> Result<(), TypeUnificationError> {
    let original_expr = expr.clone();
    let mut visitor = ExprVisitor::bottom_up(expr);

    // Pop front to get the innermost expression first that may have caused the type mismatch.
    while let Some(sub_expr) = visitor.pop_front() {
        match sub_expr {
            Expr::Let { .. } => {}
            Expr::Boolean { .. } => {}
            Expr::Concat { .. } => {}
            Expr::GreaterThan { .. } => {}
            Expr::And { .. } => {}
            Expr::Or { .. } => {}
            Expr::GreaterThanOrEqualTo { .. } => {}
            Expr::LessThanOrEqualTo { .. } => {}
            Expr::EqualTo { .. } => {}
            Expr::LessThan { .. } => {}
            Expr::InvokeMethodLazy { .. } => {}
            sub_expr => {
                unify_inferred_type(&original_expr, sub_expr)?;
            }
        }
    }

    Ok(())
}

fn unify_inferred_type(
    original_expr: &Expr,
    sub_expr: &mut Expr,
) -> Result<InferredType, TypeUnificationError> {
    let unification_result = sub_expr.inferred_type().unify();

    match unification_result {
        Ok(unified_type) => {
            sub_expr.with_inferred_type_mut(unified_type.clone());
            Ok(unified_type)
        }
        Err(e) => match e {
            UnificationFailureInternal::TypeMisMatch { left, right } => Err(
                get_type_unification_error_from_mismatch(original_expr, sub_expr, left, right),
            ),

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
                    sub_expr.clone(),
                    None,
                    additional_messages,
                ))
            }

            UnificationFailureInternal::UnknownType => {
                Err(TypeUnificationError::unresolved_types_error(
                    sub_expr.clone(),
                    None,
                    vec!["cannot determine the type".to_string()],
                ))
            }
        },
    }
}

fn get_type_unification_error_from_mismatch(
    rib: &Expr,
    expr_unified: &Expr,
    left: InferredType,
    right: InferredType,
) -> TypeUnificationError {
    let left_default = left.origin.is_default();
    let right_default = right.origin.is_default();

    let left_declared = left.origin.is_declared();
    let right_declared = right.origin.is_declared();

    let left_expr = left
        .source_span()
        .and_then(|span| rib.lookup(&span).map(|expr| (span, expr)));

    let right_expr = right
        .source_span()
        .and_then(|span| rib.lookup(&span).map(|expr| (span, expr)));

    match (left_expr, right_expr) {
        (Some((_, left_expr)), Some((right_span, right_expr))) => {
            let mut additional_error_detail = vec![format!(
                "expected type {} based on expression `{}` found at line {} column {}",
                right.printable(),
                right_expr,
                right_span.start_line(),
                right_span.start_column()
            )];

            additional_error_detail.extend(get_error_detail(
                &right_expr,
                &right,
                right_declared,
                right_default,
            ));
            additional_error_detail.extend(get_error_detail(
                &left_expr,
                &left,
                left_declared,
                left_default,
            ));

            TypeUnificationError::type_mismatch_error(
                left_expr.clone(),
                None,
                right,
                left,
                additional_error_detail,
            )
        }

        (Some((_, left_expr)), None) => {
            let additional_error_detail =
                get_error_detail(&left_expr, &left, left_declared, left_default);

            TypeUnificationError::type_mismatch_error(
                left_expr.clone(),
                None,
                right,
                left,
                additional_error_detail,
            )
        }

        (None, Some((_, right_expr))) => {
            let additional_error_detail =
                get_error_detail(&right_expr, &right, right_declared, right_default);

            TypeUnificationError::type_mismatch_error(
                right_expr.clone(),
                None,
                left,
                right,
                additional_error_detail,
            )
        }

        (None, None) => {
            let additional_messages = vec![format!(
                "conflicting types: {}, {}",
                left.printable(),
                right.printable()
            )];

            TypeUnificationError::unresolved_types_error(
                expr_unified.clone(),
                None,
                additional_messages,
            )
        }
    }
}

fn get_error_detail(
    expr: &Expr,
    inferred_type: &InferredType,
    declared: Option<&SourceSpan>,
    is_default: bool,
) -> Vec<String> {
    let mut details = vec![];

    if let Some(span) = declared {
        details.push(format!(
            "the type of `{}` is declared as `{}` at line {} column {}",
            expr,
            inferred_type.printable(),
            span.start_line(),
            span.start_column()
        ));
    } else if is_default {
        details.push(format!(
            "the expression `{}` is inferred as `{}` by default",
            expr,
            inferred_type.printable()
        ));
    }

    details
}
