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

use crate::{Expr, InferredType, MatchArm, Range, VariableId};
use bigdecimal::BigDecimal;
use std::ops::Deref;

pub fn desugar_range_selection(select_from: &Expr, range_expr: &Expr) -> Result<Expr, String> {
    let iterable_expr = VariableId::list_comprehension_identifier("__i__");

    match range_expr {
        Expr::Range { range, .. } => match range {
            Range::Range { .. } | Range::RangeInclusive { .. } => Ok(Expr::list_comprehension(
                iterable_expr.clone(),
                range_expr.clone(),
                Expr::select_index(
                    select_from.clone(),
                    Expr::identifier_with_variable_id(iterable_expr, None),
                ),
            )
            .with_inferred_type(select_from.inferred_type())),

            Range::RangeFrom { from, .. } => {
                let length = VariableId::local("__size__", 0);
                let length_identifier = Expr::identifier_with_variable_id(length.clone(), None)
                    .with_inferred_type(InferredType::u64());
                let index = VariableId::local("__index__", 0);
                let index_identifier = Expr::identifier_with_variable_id(index.clone(), None)
                    .with_inferred_type(InferredType::u64());

                Ok(Expr::expr_block(vec![
                    Expr::let_binding_with_variable_id(
                        length,
                        Expr::length(select_from.clone()),
                        None,
                    ),
                    Expr::let_binding_with_variable_id(
                        index,
                        Expr::minus(
                            length_identifier,
                            Expr::number(BigDecimal::from(1))
                                .with_inferred_type(InferredType::u64()),
                        )
                        .with_inferred_type(InferredType::u64()),
                        None,
                    )
                    .with_inferred_type(InferredType::u64()),
                    Expr::list_comprehension(
                        iterable_expr.clone(),
                        Expr::range(from.deref().clone(), index_identifier),
                        Expr::select_index(
                            select_from.clone(),
                            Expr::identifier_with_variable_id(iterable_expr, None),
                        ),
                    )
                    .with_inferred_type(select_from.inferred_type()),
                ]))
            }
        },

        _ => Err("internal error: expression is not a range for any desugar to apply".to_string()),
    }
}

pub fn desugar_pattern_match(
    pred: &Expr,
    match_arms: &[MatchArm],
    expr_type: InferredType,
) -> Option<Expr> {
    let mut if_else_branches = vec![];

    for match_arm in match_arms.iter() {
        let if_else_branch = internal::IfThenBranch::from_pred_and_match_arm(match_arm, pred);
        if let Some(condition) = if_else_branch {
            if_else_branches.push(condition);
        }
    }

    internal::build_expr_from(if_else_branches).map(|expr| expr.merge_inferred_type(expr_type))
}

mod internal {
    use crate::call_type::CallType;
    use crate::rib_source_span::SourceSpan;
    use crate::{ArmPattern, Expr, InferredType, MatchArm, Number, TypeInternal, VariableId};
    use bigdecimal::{BigDecimal, FromPrimitive};

    pub(crate) fn build_expr_from(if_branches: Vec<IfThenBranch>) -> Option<Expr> {
        if let Some(branch) = if_branches.first() {
            let mut expr = Expr::cond(
                branch.condition.clone(),
                branch.body.clone(),
                Expr::throw("No match found"),
            );

            for branch in if_branches.iter().skip(1).rev() {
                if let Expr::Cond { rhs, .. } = &mut expr {
                    let else_copy = *rhs.clone();
                    *rhs = Box::new(
                        Expr::cond(branch.condition.clone(), branch.body.clone(), else_copy)
                            .merge_inferred_type(branch.body.inferred_type()),
                    );
                }
            }

            Some(expr)
        } else {
            None
        }
    }

    #[derive(Debug, Clone)]
    pub(crate) struct IfThenBranch {
        pub(crate) condition: Expr,
        pub(crate) body: Expr,
    }

    impl IfThenBranch {
        pub(crate) fn from_pred_and_match_arm(
            match_arm: &MatchArm,
            pred: &Expr,
        ) -> Option<IfThenBranch> {
            get_conditions(match_arm, pred, None, pred.inferred_type())
        }
    }

    // Match arms are converted to if-else conditions, with the help of instruction called `GetTag`.
    // We rely on the actual constructor names, and for that reason we pattern ok, err, option, none
    // having a precise control. For example, if it's none, there is no need to check the tag of predicate and
    // and arm-literal, as it is a direct boolean expression.
    fn get_conditions(
        match_arm: &MatchArm,
        pred_expr: &Expr,
        tag: Option<Expr>,
        inferred_type_of_pred: InferredType,
    ) -> Option<IfThenBranch> {
        let arm_pattern = &match_arm.arm_pattern;
        let resolution = &match_arm.arm_resolution_expr;

        // match x {
        // some(some(x)) => "hello"
        // }
        match arm_pattern {
            ArmPattern::Literal(arm_pattern_expr) => {
                handle_literal(arm_pattern_expr, pred_expr, resolution, tag)
            }

            ArmPattern::Constructor(constructor_name, arm_patterns) => hande_constructor(
                pred_expr,
                constructor_name,
                arm_patterns,
                resolution,
                inferred_type_of_pred,
                tag,
            ),

            ArmPattern::TupleConstructor(arm_patterns) => hande_constructor(
                pred_expr,
                "tuple",
                arm_patterns,
                resolution,
                inferred_type_of_pred,
                tag,
            ),

            ArmPattern::ListConstructor(arm_patterns) => hande_constructor(
                pred_expr,
                "list",
                arm_patterns,
                resolution,
                inferred_type_of_pred,
                tag,
            ),

            ArmPattern::RecordConstructor(field_arm_pattern_collection) => {
                handle_record_constructor(
                    pred_expr,
                    field_arm_pattern_collection,
                    resolution,
                    inferred_type_of_pred,
                    tag,
                )
            }

            ArmPattern::As(name, inner_pattern) => handle_as_pattern(
                name,
                inner_pattern,
                pred_expr,
                resolution,
                tag,
                inferred_type_of_pred,
            ),

            ArmPattern::WildCard => {
                let branch = IfThenBranch {
                    condition: tag.unwrap_or(Expr::boolean(true)),
                    body: resolution.as_ref().clone(),
                };
                Some(branch)
            }
        }
    }

    fn handle_literal(
        arm_pattern_expr: &Expr,
        pred_expr: &Expr,
        resolution: &Expr,
        tag: Option<Expr>,
    ) -> Option<IfThenBranch> {
        match arm_pattern_expr {
            Expr::Identifier {
                variable_id,
                inferred_type,
                ..
            } => {
                let assign_var = Expr::let_binding_with_variable_id(
                    variable_id.clone(),
                    pred_expr.clone(),
                    None,
                )
                .with_inferred_type(inferred_type.clone());

                let block = Expr::expr_block(vec![assign_var, resolution.clone()]);

                let branch = IfThenBranch {
                    condition: tag.unwrap_or(Expr::boolean(true)),
                    body: block,
                };
                Some(branch)
            }

            Expr::Call {
                call_type: CallType::EnumConstructor(name),
                ..
            } => {
                let cond = if let Some(t) = tag {
                    Expr::and(
                        t,
                        Expr::equal_to(Expr::get_tag(pred_expr.clone()), Expr::literal(name)),
                    )
                } else {
                    Expr::equal_to(Expr::get_tag(pred_expr.clone()), Expr::literal(name))
                };

                let branch = IfThenBranch {
                    condition: cond,
                    body: resolution.clone(),
                };
                Some(branch)
            }

            _ => {
                let branch = IfThenBranch {
                    condition: Expr::equal_to(pred_expr.clone(), arm_pattern_expr.clone()),
                    body: resolution.clone(),
                };

                Some(branch)
            }
        }
    }

    fn handle_record_constructor(
        pred_expr: &Expr,
        bind_patterns: &[(String, ArmPattern)],
        resolution: &Expr,
        pred_expr_inferred_type: InferredType,
        tag: Option<Expr>,
    ) -> Option<IfThenBranch> {
        match pred_expr_inferred_type.internal_type() {
            TypeInternal::Record(field_and_types) => {
                // Resolution body is a list of expressions which grows (maybe with some let bindings)
                // as we recursively iterate over the bind patterns
                // where bind patterns are {name: x, age: _, address : _ } in the case of `match record { {name: x, age: _, address : _ } ) =>`
                // These will exist prior to the original resolution of a successful record match.
                let mut resolution_body = vec![];

                // The conditions keep growing as we recursively iterate over the bind patterns
                // and there are multiple conditions (if condition) for each element in the record.
                let mut conditions = vec![];

                // We assume pred-expr can be queried by field using Expr::select_field, and we pick each element in the bind pattern
                // to get the corresponding expr in pred-expr and keep recursively iterating until the record is completed.
                // However, there is no resolution body for each of this iteration, so we use an empty expression
                // and finally push the original resolution body once we fully build the conditions.
                for (field, arm_pattern) in bind_patterns.iter() {
                    let new_pred = Expr::select_field(pred_expr.clone(), field, None);
                    let new_pred_type = field_and_types
                        .iter()
                        .find(|(f, _)| f == field)
                        .map(|(_, t)| t.clone())
                        .unwrap_or(InferredType::unknown());

                    let branch = get_conditions(
                        &MatchArm::new(arm_pattern.clone(), Expr::empty_expr()),
                        &new_pred,
                        None,
                        new_pred_type.clone(),
                    );

                    if let Some(x) = branch {
                        conditions.push(x.condition);
                        resolution_body.push(x.body)
                    }
                }

                resolution_body.push(resolution.clone());

                let and_cond = Expr::and_combine(conditions);

                and_cond.map(|c| IfThenBranch {
                    condition: {
                        if let Some(t) = tag {
                            Expr::and(t, c)
                        } else {
                            c
                        }
                    },
                    body: Expr::expr_block(resolution_body),
                })
            }

            _ => None,
        }
    }

    fn hande_constructor(
        pred_expr: &Expr,
        constructor_name: &str,
        bind_patterns: &[ArmPattern],
        resolution: &Expr,
        pred_expr_inferred_type: InferredType,
        tag: Option<Expr>,
    ) -> Option<IfThenBranch> {
        match pred_expr_inferred_type.internal_type() {
            TypeInternal::Variant(variant) => {
                let arg_pattern_opt = bind_patterns.first();

                let inner_type = &variant
                    .iter()
                    .find(|(case_name, _)| case_name == constructor_name);
                let inner_variant_arg_type = inner_type.and_then(|(_, typ)| typ.clone());

                let cond = if let Some(t) = tag {
                    Expr::and(
                        t,
                        Expr::equal_to(
                            Expr::get_tag(pred_expr.clone()),
                            Expr::literal(constructor_name),
                        ),
                    )
                } else {
                    Expr::equal_to(
                        Expr::get_tag(pred_expr.clone()),
                        Expr::literal(constructor_name),
                    )
                };
                match (arg_pattern_opt, inner_variant_arg_type) {
                    (None, None) => None,
                    (Some(pattern), Some(inferred_type)) => get_conditions(
                        &MatchArm::new(pattern.clone(), resolution.clone()),
                        &pred_expr.unwrap(),
                        Some(cond),
                        inferred_type,
                    ),
                    _ => None,
                }
            }

            TypeInternal::Option(inner) if constructor_name == "some" => {
                let cond = if let Some(t) = tag {
                    Expr::and(
                        t,
                        Expr::equal_to(
                            Expr::get_tag(pred_expr.clone()),
                            Expr::literal(constructor_name),
                        ),
                    )
                } else {
                    Expr::equal_to(
                        Expr::get_tag(pred_expr.clone()),
                        Expr::literal(constructor_name),
                    )
                };

                match bind_patterns.first() {
                    Some(pattern) => get_conditions(
                        &MatchArm::new(pattern.clone(), resolution.clone()),
                        &pred_expr.unwrap(),
                        Some(cond),
                        inner.clone(),
                    ),
                    _ => None,
                }
            }

            TypeInternal::Option(_) if constructor_name == "none" => {
                let cond = if let Some(t) = tag {
                    Expr::and(
                        t,
                        Expr::equal_to(
                            Expr::get_tag(pred_expr.clone()),
                            Expr::literal(constructor_name),
                        ),
                    )
                } else {
                    Expr::equal_to(
                        Expr::get_tag(pred_expr.clone()),
                        Expr::literal(constructor_name),
                    )
                };

                Some(IfThenBranch {
                    condition: cond,
                    body: resolution.clone(),
                })
            }

            TypeInternal::Result { ok, error } => {
                let inner_variant_arg_type = if constructor_name == "ok" {
                    ok
                } else if constructor_name == "err" {
                    error
                } else {
                    return None;
                };

                let cond = if let Some(t) = tag {
                    Expr::and(
                        t,
                        Expr::equal_to(
                            Expr::get_tag(pred_expr.clone()),
                            Expr::literal(constructor_name),
                        ),
                    )
                } else {
                    Expr::equal_to(
                        Expr::get_tag(pred_expr.clone()),
                        Expr::literal(constructor_name),
                    )
                };

                match bind_patterns.first() {
                    Some(pattern) => get_conditions(
                        &MatchArm::new(pattern.clone(), resolution.clone()),
                        &pred_expr.unwrap(),
                        Some(cond),
                        inner_variant_arg_type
                            .clone()
                            .unwrap_or(InferredType::unknown())
                            .clone(),
                    ),

                    _ => None,
                }
            }

            TypeInternal::Tuple(inferred_types) => {
                // Resolution body is a list of expressions which grows (may be with some let bindings)
                // as we recursively iterate over the bind patterns
                // where bind patterns are x, _, y in the case of `match tuple_variable { (x, _, y)) =>`
                // These will exist prior to the original resolution of a successful tuple match.
                let mut resolution_body = vec![];

                // The conditions keep growing as we recursively iterate over the bind patterns
                // and there are multiple conditions (if condition) for each element in the tuple
                let mut conditions = vec![];

                // We assume pred-expr is indexed (i.e, tuple is indexed), and we pick each element in the bind pattern
                // and get the corresponding expr in pred-expr and keep recursively iterating until the tuple is completed.
                // However there is no resolution body for each of this iteration, so we use an empty expression
                // and finally push the original resolution body once we fully build the conditions.
                for (index, arm_pattern) in bind_patterns.iter().enumerate() {
                    let new_pred = Expr::select_index(
                        pred_expr.clone(),
                        Expr::Number {
                            number: Number {
                                value: BigDecimal::from_usize(index).unwrap(),
                            },
                            type_annotation: None,
                            inferred_type: InferredType::u64(),
                            source_span: SourceSpan::default(),
                        },
                    );
                    let new_pred_type = inferred_types
                        .get(index)
                        .cloned()
                        .unwrap_or(InferredType::unknown());

                    let branch = get_conditions(
                        &MatchArm::new(arm_pattern.clone(), Expr::empty_expr()),
                        &new_pred,
                        None,
                        new_pred_type.clone(),
                    );

                    if let Some(x) = branch {
                        conditions.push(x.condition);
                        resolution_body.push(x.body)
                    }
                }

                resolution_body.push(resolution.clone());

                let and_cond = Expr::and_combine(conditions);

                and_cond.map(|c| IfThenBranch {
                    condition: {
                        if let Some(t) = tag {
                            Expr::and(t, c)
                        } else {
                            c
                        }
                    },
                    body: Expr::expr_block(resolution_body),
                })
            }

            TypeInternal::List(inferred_type) => {
                // Resolution body is a list of expressions which grows (may be with some let bindings)
                // as we recursively iterate over the bind patterns
                // where bind patterns are x, _, y in the case of `match list_ { [x, _, y]) =>`
                // These will exist prior to the original resolution of a successful list match.
                let mut resolution_body = vec![];

                // The conditions keep growing as we recursively iterate over the bind patterns
                // and there are multiple conditions (if condition) for each element in the list
                let mut conditions = vec![];

                // We assume pred-expr is indexed (i.e, list is indexed), and we pick each element in the bind pattern
                // and get the corresponding expr in pred-expr and keep recursively iterating until the list is completed.
                // However there is no resolution body for each of this iteration, so we use an empty expression
                // and finally push the original resolution body once we fully build the conditions.
                for (index, arm_pattern) in bind_patterns.iter().enumerate() {
                    let new_pred = Expr::select_index(
                        pred_expr.clone(),
                        Expr::Number {
                            number: Number {
                                value: BigDecimal::from_usize(index).unwrap(),
                            },
                            type_annotation: None,
                            inferred_type: InferredType::u64(),
                            source_span: SourceSpan::default(),
                        },
                    );
                    let new_pred_type = inferred_type.clone();

                    let branch = get_conditions(
                        &MatchArm::new(arm_pattern.clone(), Expr::empty_expr()),
                        &new_pred,
                        None,
                        new_pred_type.clone(),
                    );

                    if let Some(x) = branch {
                        conditions.push(x.condition);
                        resolution_body.push(x.body)
                    }
                }

                resolution_body.push(resolution.clone());

                let and_cond = Expr::and_combine(conditions);

                and_cond.map(|c| IfThenBranch {
                    condition: {
                        if let Some(t) = tag {
                            Expr::and(t, c)
                        } else {
                            c
                        }
                    },
                    body: Expr::expr_block(resolution_body),
                })
            }

            TypeInternal::Unknown => Some(IfThenBranch {
                condition: Expr::boolean(false),
                body: resolution.clone(),
            }),
            _ => None,
        }
    }

    fn handle_as_pattern(
        name: &str,
        inner_pattern: &ArmPattern,
        pred_expr: &Expr,
        resolution: &Expr,
        tag: Option<Expr>,
        pred_expr_inferred_type: InferredType,
    ) -> Option<IfThenBranch> {
        let binding = Expr::let_binding_with_variable_id(
            VariableId::global(name.to_string()),
            pred_expr.clone(),
            None,
        )
        .with_inferred_type(pred_expr.inferred_type());

        let block = Expr::expr_block(vec![binding, resolution.clone()]);
        get_conditions(
            &MatchArm::new(inner_pattern.clone(), block),
            pred_expr,
            tag,
            pred_expr_inferred_type,
        )
    }
}

#[cfg(test)]
mod desugar_tests {
    use test_r::test;

    use crate::compiler::desugar::desugar_tests::expectations::expected_condition_with_identifiers;
    use crate::type_registry::FunctionTypeRegistry;
    use crate::Expr;
    use golem_wasm_ast::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedType, TypeU32, TypeU64,
    };
    use std::ops::Deref;

    use super::*;

    fn get_function_type_registry() -> FunctionTypeRegistry {
        let metadata = vec![
            AnalysedExport::Function(AnalysedFunction {
                name: "foo".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "my_parameter".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                }],
                results: vec![],
            }),
            AnalysedExport::Function(AnalysedFunction {
                name: "baz".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "my_parameter".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                }],
                results: vec![],
            }),
        ];
        FunctionTypeRegistry::from_export_metadata(&metadata)
    }

    #[test]
    fn test_desugar_pattern_match_with_identifiers() {
        let rib_expr = r#"
          let x: option<u64> = some(1);
          match x {
            some(x) => x,
            none => 1u64
          }
        "#;

        let function_type_registry = get_function_type_registry();

        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_types(&function_type_registry, &vec![]).unwrap();

        let desugared_expr = match internal::last_expr(&expr) {
            Expr::PatternMatch {
                predicate,
                match_arms,
                ..
            } => {
                desugar_pattern_match(predicate.deref(), &match_arms, expr.inferred_type()).unwrap()
            }
            _ => panic!("Expected a match expression"),
        };

        assert_eq!(desugared_expr, expected_condition_with_identifiers());
    }

    mod internal {
        use crate::Expr;

        pub(crate) fn last_expr(expr: &Expr) -> Expr {
            match expr {
                Expr::ExprBlock { exprs, .. } => exprs.last().unwrap().clone(),
                _ => expr.clone(),
            }
        }
    }
    mod expectations {
        use crate::{Expr, InferredType, TypeName, VariableId};
        use bigdecimal::BigDecimal;
        pub(crate) fn expected_condition_with_identifiers() -> Expr {
            Expr::cond(
                Expr::equal_to(
                    Expr::get_tag(
                        Expr::identifier_with_variable_id(VariableId::local("x", 0), None)
                            .with_inferred_type(InferredType::option(InferredType::u64())),
                    ),
                    Expr::literal("some"),
                )
                .with_inferred_type(InferredType::bool()),
                Expr::expr_block(vec![
                    Expr::let_binding_with_variable_id(
                        VariableId::match_identifier("x".to_string(), 1),
                        Expr::identifier_with_variable_id(VariableId::local("x", 0), None)
                            .with_inferred_type(InferredType::option(InferredType::u64()))
                            .unwrap(),
                        None,
                    )
                    .with_inferred_type(InferredType::u64()),
                    Expr::identifier_with_variable_id(
                        VariableId::match_identifier("x".to_string(), 1),
                        None,
                    )
                    .with_inferred_type(InferredType::u64()),
                ])
                .with_inferred_type(InferredType::u64()),
                Expr::cond(
                    Expr::equal_to(
                        Expr::get_tag(
                            Expr::identifier_with_variable_id(VariableId::local("x", 0), None)
                                .with_inferred_type(InferredType::option(InferredType::u64())),
                        ),
                        Expr::literal("none"),
                    )
                    .with_inferred_type(InferredType::bool()),
                    Expr::number_inferred(
                        BigDecimal::from(1),
                        Some(TypeName::U64),
                        InferredType::u64(),
                    ),
                    Expr::throw("No match found"),
                )
                .with_inferred_type(InferredType::u64()),
            )
            .with_inferred_type(InferredType::u64())
        }
    }
}
