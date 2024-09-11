// Copyright 2024 Golem Cloud
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

use crate::{Expr, InferredType, MatchArm};
use std::collections::VecDeque;
use crate::call_type::CallType;

pub fn push_types_down(expr: &mut Expr) -> Result<(), String> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::SelectField(expr, field, inferred_type) => {
                let field_type = inferred_type.clone();
                let record_type = vec![(field.to_string(), field_type)];
                let inferred_record_type = InferredType::Record(record_type);

                expr.add_infer_type_mut(inferred_record_type);
                queue.push_back(expr);
            }

            Expr::SelectIndex(expr, _, inferred_type) => {
                let field_type = inferred_type.clone();
                let inferred_record_type = InferredType::List(Box::new(field_type));
                expr.add_infer_type_mut(inferred_record_type);
                queue.push_back(expr);
            }
            Expr::Cond(cond, then, else_, inferred_type) => {
                then.add_infer_type_mut(inferred_type.clone());
                else_.add_infer_type_mut(inferred_type.clone());

                cond.add_infer_type_mut(InferredType::Bool);
                queue.push_back(cond);
                queue.push_back(then);
                queue.push_back(else_);
            }
            Expr::Not(expr, inferred_type) => {
                expr.add_infer_type_mut(inferred_type.clone());
                queue.push_back(expr);
            }
            Expr::Option(Some(expr), inferred_type) => {
                internal::handle_option(expr, inferred_type)?;
                queue.push_back(expr);
            }

            Expr::Result(Ok(expr), inferred_type) => {
                internal::handle_ok(expr, inferred_type)?;
                queue.push_back(expr);
            }

            Expr::Result(Err(expr), inferred_type) => {
                internal::handle_err(expr, inferred_type)?;
                queue.push_back(expr);
            }

            Expr::PatternMatch(pred, match_arms, inferred_type) => {
                for MatchArm {
                    arm_resolution_expr,
                    arm_pattern,
                } in match_arms
                {
                    let predicate_type = pred.inferred_type();
                    internal::update_arm_pattern_type(arm_pattern, &predicate_type)?;
                    arm_resolution_expr.add_infer_type_mut(inferred_type.clone());
                    queue.push_back(arm_resolution_expr);
                }
            }

            Expr::Tuple(exprs, inferred_type) => {
                internal::handle_tuple(exprs, inferred_type, &mut queue)?;
            }
            Expr::Sequence(expressions, inferred_type) => {
                internal::handle_sequence(expressions, inferred_type, &mut queue)?;
            }

            Expr::Record(expressions, inferred_type) => {
                internal::handle_record(expressions, inferred_type, &mut queue)?;
            }

            // For type push down, if it's variant type or enum type
            Expr::Call(call_type, expressions, inferred_type) => {
                match call_type {
                    CallType::VariantConstructor(name) => {
                        match inferred_type {
                            InferredType::Variant(variant) => {
                                let identified_variant = variant
                                    .iter()
                                    .find(|(variant_name, _)| variant_name == name);

                                if let Some((_name, Some(inner_type))) = identified_variant {
                                    for expr in expressions {
                                        expr.add_infer_type_mut(inner_type.clone());
                                        queue.push_back(expr);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {
                        for expr in expressions {
                            queue.push_back(expr);
                        }
                    }
                }
            }

            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
}

mod internal {
    use crate::type_refinement::precise_types::*;
    use crate::type_refinement::TypeRefinement;
    use crate::{ArmPattern, Expr, InferredType};
    use std::collections::VecDeque;

    pub(crate) fn handle_option(
        inner_expr: &mut Expr,
        outer_inferred_type: &InferredType,
    ) -> Result<(), String> {
        let refined_optional_type = OptionalType::refine(outer_inferred_type)
            .ok_or("Expected optional type".to_string())?;
        let inner_type = refined_optional_type.inner_type();

        inner_expr.add_infer_type_mut(inner_type.clone());
        Ok(())
    }

    pub(crate) fn handle_ok(
        inner_expr: &mut Expr,
        outer_inferred_type: &InferredType,
    ) -> Result<(), String> {
        let refined_ok_type =
            OkType::refine(outer_inferred_type).ok_or("Expected ok type".to_string())?;
        let inner_type = refined_ok_type.inner_type();

        inner_expr.add_infer_type_mut(inner_type.clone());

        Ok(())
    }

    pub(crate) fn handle_err(
        inner_expr: &mut Expr,
        outer_inferred_type: &InferredType,
    ) -> Result<(), String> {
        let refined_err_type =
            ErrType::refine(outer_inferred_type).ok_or("Expected err type".to_string())?;
        let inner_type = refined_err_type.inner_type();

        inner_expr.add_infer_type_mut(inner_type.clone());

        Ok(())
    }

    pub(crate) fn handle_sequence<'a>(
        inner_expressions: &'a mut [Expr],
        outer_inferred_type: &InferredType,
        push_down_queue: &mut VecDeque<&'a mut Expr>,
    ) -> Result<(), String> {
        let refined_list_type =
            ListType::refine(outer_inferred_type).ok_or("Expected list type".to_string())?;
        let inner_type = refined_list_type.inner_type();

        for expr in inner_expressions.iter_mut() {
            expr.add_infer_type_mut(inner_type.clone());
            push_down_queue.push_back(expr);
        }

        Ok(())
    }

    pub(crate) fn handle_tuple<'a>(
        inner_expressions: &'a mut [Expr],
        outer_inferred_type: &InferredType,
        push_down_queue: &mut VecDeque<&'a mut Expr>,
    ) -> Result<(), String> {
        let refined_tuple_type =
            TupleType::refine(outer_inferred_type).ok_or("Expected tuple type".to_string())?;
        let inner_types = refined_tuple_type.inner_types();

        for (expr, typ) in inner_expressions.iter_mut().zip(inner_types) {
            expr.add_infer_type_mut(typ.clone());
            push_down_queue.push_back(expr);
        }

        Ok(())
    }

    pub(crate) fn handle_record<'a>(
        inner_expressions: &'a mut [(String, Box<Expr>)],
        outer_inferred_type: &InferredType,
        push_down_queue: &mut VecDeque<&'a mut Expr>,
    ) -> Result<(), String> {
        let refined_record_type =
            RecordType::refine(outer_inferred_type).ok_or("Expected record type".to_string())?;

        for (field, expr) in inner_expressions {
            let inner_type = refined_record_type.inner_type_by_field(field);
            expr.add_infer_type_mut(inner_type.clone());
            push_down_queue.push_back(expr);
        }

        Ok(())
    }

    pub(crate) fn update_arm_pattern_type(
        arm_pattern: &mut ArmPattern,
        predicate_type: &InferredType,
    ) -> Result<(), String> {
        match arm_pattern {
            ArmPattern::Literal(expr) => {
                expr.add_infer_type_mut(predicate_type.clone());
                expr.push_types_down()?;
            }
            ArmPattern::As(_, pattern) => {
                update_arm_pattern_type(pattern, predicate_type)?;
            }
            ArmPattern::Constructor(constructor_name, patterns) => match predicate_type {
                InferredType::Option(inner_type) => {
                    if constructor_name == "some" || constructor_name == "none" {
                        for pattern in &mut *patterns {
                            update_arm_pattern_type(pattern, inner_type)?;
                        }
                    }
                }
                InferredType::Result { ok, error } => {
                    if constructor_name == "ok" {
                        if let Some(ok_type) = ok {
                            for pattern in &mut *patterns {
                                update_arm_pattern_type(pattern, ok_type)?;
                            }
                        }
                    };
                    if constructor_name == "err" {
                        if let Some(err_type) = error {
                            for pattern in &mut *patterns {
                                update_arm_pattern_type(pattern, err_type)?;
                            }
                        }
                    };
                }
                InferredType::Variant(variant) => {
                    let identified_variant = variant
                        .iter()
                        .find(|(name, _optional_type)| name == constructor_name);

                    if let Some((_name, Some(inner_type))) = identified_variant {
                        for pattern in &mut *patterns {
                            update_arm_pattern_type(pattern, inner_type)?;
                        }
                    }
                }
                _ => {}
            },
            ArmPattern::WildCard => {}
        }

        Ok(())
    }
}

#[cfg(test)]
mod type_push_down_tests {
    use crate::{Expr, InferredType, VariableId};

    #[test]
    fn test_push_down_for_record() {
        let mut expr = Expr::Record(
            vec![("titles".to_string(), Box::new(Expr::identifier("x")))],
            InferredType::AllOf(vec![
                InferredType::Record(vec![("titles".to_string(), InferredType::Unknown)]),
                InferredType::Record(vec![("titles".to_string(), InferredType::U64)]),
            ]),
        );

        expr.push_types_down().unwrap();
        let expected = Expr::Record(
            vec![(
                "titles".to_string(),
                Box::new(Expr::Identifier(
                    VariableId::global("x".to_string()),
                    InferredType::U64,
                )),
            )],
            InferredType::AllOf(vec![
                InferredType::Record(vec![("titles".to_string(), InferredType::Unknown)]),
                InferredType::Record(vec![("titles".to_string(), InferredType::U64)]),
            ]),
        );
        assert_eq!(expr, expected);
    }

    #[test]
    fn test_push_down_for_sequence() {
        let mut expr = Expr::Sequence(
            vec![Expr::identifier("x"), Expr::identifier("y")],
            InferredType::AllOf(vec![
                InferredType::List(Box::new(InferredType::U32)),
                InferredType::List(Box::new(InferredType::U64)),
            ]),
        );

        expr.push_types_down().unwrap();
        let expected = Expr::Sequence(
            vec![
                Expr::Identifier(
                    VariableId::global("x".to_string()),
                    InferredType::AllOf(vec![InferredType::U32, InferredType::U64]),
                ),
                Expr::Identifier(
                    VariableId::global("y".to_string()),
                    InferredType::AllOf(vec![InferredType::U32, InferredType::U64]),
                ),
            ],
            InferredType::AllOf(vec![
                InferredType::List(Box::new(InferredType::U32)),
                InferredType::List(Box::new(InferredType::U64)),
            ]),
        );
        assert_eq!(expr, expected);
    }
}
