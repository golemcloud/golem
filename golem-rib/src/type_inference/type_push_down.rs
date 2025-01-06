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

use crate::type_inference::type_push_down::internal::{
    handle_list_comprehension, handle_list_reduce,
};
use crate::{Expr, InferredType, MatchArm};
use std::collections::VecDeque;

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
                    internal::update_arm_pattern_type(arm_pattern, &predicate_type, pred)?;
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

            Expr::Call(call_type, expressions, inferred_type) => {
                internal::handle_call(call_type, expressions, inferred_type, &mut queue);
            }

            Expr::ListComprehension {
                iterated_variable,
                iterable_expr,
                yield_expr,
                inferred_type,
            } => {
                handle_list_comprehension(
                    iterated_variable,
                    iterable_expr,
                    yield_expr,
                    inferred_type,
                )?;
                queue.push_back(iterable_expr);
                queue.push_back(yield_expr);
            }

            Expr::ListReduce {
                reduce_variable,
                iterated_variable,
                iterable_expr,
                init_value_expr,
                yield_expr,
                inferred_type,
            } => {
                handle_list_reduce(
                    reduce_variable,
                    iterated_variable,
                    iterable_expr,
                    init_value_expr,
                    yield_expr,
                    inferred_type,
                )?;
                queue.push_back(iterable_expr);
                queue.push_back(init_value_expr);
                queue.push_back(yield_expr);
            }

            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
}

mod internal {
    use crate::call_type::CallType;
    use crate::type_refinement::precise_types::*;
    use crate::type_refinement::TypeRefinement;
    use crate::{ArmPattern, Expr, InferredType, VariableId};
    use std::collections::VecDeque;

    pub(crate) fn handle_list_comprehension(
        variable_id: &mut VariableId,
        iterable_expr: &mut Expr,
        yield_expr: &mut Expr,
        comprehension_result_type: &InferredType,
    ) -> Result<(), String> {
        // If the iterable_expr is List<Y> , the identifier with the same variable name within yield should be Y
        update_yield_expr_in_list_comprehension(
            variable_id,
            &iterable_expr.inferred_type(),
            yield_expr,
        )?;

        // If the outer inferred_type is List<X> this implies, the yield expression should be X
        let refined_list_type = ListType::refine(comprehension_result_type)
            .ok_or("The result of a comprehension should be of type list".to_string())?;

        let inner_type = refined_list_type.inner_type();

        yield_expr.add_infer_type_mut(inner_type.clone());

        Ok(())
    }

    pub(crate) fn handle_list_reduce(
        result_variable_id: &mut VariableId,
        reduce_variable_id: &mut VariableId,
        iterable_expr: &mut Expr,
        init_value_expr: &mut Expr,
        yield_expr: &mut Expr,
        aggregation_result_type: &InferredType,
    ) -> Result<(), String> {
        // If the iterable_expr is List<Y> , the identifier with the same variable name within yield should be Y
        update_yield_expr_in_list_reduce(
            result_variable_id,
            reduce_variable_id,
            iterable_expr,
            yield_expr,
            init_value_expr,
        )?;

        // If the outer_expr is X this implies, the yield expression should be X, and therefore initial expression should be X
        yield_expr.add_infer_type_mut(aggregation_result_type.clone());
        init_value_expr.add_infer_type_mut(aggregation_result_type.clone());

        Ok(())
    }

    fn update_yield_expr_in_list_comprehension(
        variable_id: &mut VariableId,
        iterable_type: &InferredType,
        yield_expr: &mut Expr,
    ) -> Result<(), String> {
        if !iterable_type.is_unknown() {
            let refined_iterable =
                ListType::refine(iterable_type).ok_or("Expected list type".to_string())?;

            let iterable_variable_type = refined_iterable.inner_type();

            let mut queue = VecDeque::new();
            queue.push_back(yield_expr);

            while let Some(expr) = queue.pop_back() {
                match expr {
                    Expr::Identifier(v, existing_inferred_type) => {
                        if let VariableId::ListComprehension(l) = v {
                            if l.name == variable_id.name() {
                                *existing_inferred_type =
                                    existing_inferred_type.merge(iterable_variable_type.clone())
                            }
                        }
                    }
                    _ => expr.visit_children_mut_bottom_up(&mut queue),
                }
            }
        }
        Ok(())
    }
    fn update_yield_expr_in_list_reduce(
        reduce_variable: &mut VariableId,
        iterated_variable: &mut VariableId,
        iterable_expr: &Expr,
        yield_expr: &mut Expr,
        init_value_expr: &mut Expr,
    ) -> Result<(), String> {
        let iterable_inferred_type = iterable_expr.inferred_type();

        if !iterable_expr.inferred_type().is_unknown() {
            let refined_iterable = ListType::refine(&iterable_inferred_type)
                .ok_or("Expected list type".to_string())?;

            let iterable_variable_type = refined_iterable.inner_type();

            let init_value_expr_type = init_value_expr.inferred_type();
            let mut queue = VecDeque::new();
            queue.push_back(yield_expr);

            while let Some(expr) = queue.pop_back() {
                match expr {
                    Expr::Identifier(v, existing_inferred_type) => {
                        if let VariableId::ListComprehension(l) = v {
                            if l.name == iterated_variable.name() {
                                *existing_inferred_type =
                                    existing_inferred_type.merge(iterable_variable_type.clone())
                            }
                        } else if let VariableId::ListReduce(l) = v {
                            if l.name == reduce_variable.name() {
                                *existing_inferred_type =
                                    existing_inferred_type.merge(init_value_expr_type.clone())
                            }
                        }
                    }

                    _ => expr.visit_children_mut_bottom_up(&mut queue),
                }
            }
        }
        Ok(())
    }

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
        let refined_record_type = RecordType::refine(outer_inferred_type).ok_or({
            let inner_expressions = inner_expressions
                .iter()
                .map(|(_, expr)| expr.to_string())
                .collect::<Vec<String>>()
                .join(", ");

            format!("{} is invalid. Expected record", inner_expressions)
        })?;

        for (field, expr) in inner_expressions {
            let inner_type = refined_record_type.inner_type_by_name(field);
            expr.add_infer_type_mut(inner_type.clone());
            push_down_queue.push_back(expr);
        }

        Ok(())
    }

    pub(crate) fn handle_call<'a>(
        call_type: &CallType,
        expressions: &'a mut Vec<Expr>,
        inferred_type: &InferredType,
        queue: &mut VecDeque<&'a mut Expr>,
    ) {
        match call_type {
            // For CallType::Enum, there are no argument expressions
            // For CallType::Function, there is no type available to push down to arguments, as it is invalid
            // to push down the return type of function to its arguments.
            // For variant constructor, the type of the arguments are present in the return type of the call
            // and should be pushed down to arguments
            CallType::VariantConstructor(name) => {
                if let InferredType::Variant(variant) = inferred_type {
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
            }
            _ => {
                for expr in expressions {
                    queue.push_back(expr);
                }
            }
        }
    }

    pub(crate) fn update_arm_pattern_type(
        arm_pattern: &mut ArmPattern,
        predicate_type: &InferredType,
        original_predicate: &Expr,
    ) -> Result<(), String> {
        match arm_pattern {
            ArmPattern::Literal(expr) => {
                expr.add_infer_type_mut(predicate_type.clone());
                //expr.push_types_down()?;
            }
            ArmPattern::As(_, pattern) => {
                update_arm_pattern_type(pattern, predicate_type, original_predicate)?;
            }

            ArmPattern::Constructor(constructor_name, patterns) => {
                if constructor_name == "some" || constructor_name == "none" {
                    let resolved = OptionalType::refine(predicate_type).ok_or(format!(
                        "Invalid pattern match. Cannot match {} to {}",
                        original_predicate, constructor_name
                    ))?;

                    let inner = resolved.inner_type();

                    for pattern in patterns {
                        update_arm_pattern_type(pattern, &inner, original_predicate)?;
                    }
                } else if constructor_name == "ok" {
                    let resolved = OkType::refine(predicate_type);

                    match resolved {
                        Some(resolved) => {
                            let inner = resolved.inner_type();

                            for pattern in patterns {
                                update_arm_pattern_type(pattern, &inner, original_predicate)?;
                            }
                        }

                        None => {
                            let refined_type = ErrType::refine(predicate_type);

                            match refined_type {
                                Some(_) => {}
                                None => {
                                    return Err(format!(
                                        "Invalid pattern match. Cannot match {} to ok",
                                        original_predicate
                                    ));
                                }
                            }
                        }
                    }
                } else if constructor_name == "err" {
                    let resolved = ErrType::refine(predicate_type);

                    match resolved {
                        Some(resolved) => {
                            let inner = resolved.inner_type();

                            for pattern in patterns {
                                update_arm_pattern_type(pattern, &inner, original_predicate)?;
                            }
                        }

                        None => {
                            let refined_type = OkType::refine(predicate_type);

                            match refined_type {
                                Some(_) => {}
                                None => {
                                    return Err(format!(
                                        "Invalid pattern match. Cannot match {} to err",
                                        original_predicate
                                    ));
                                }
                            }
                        }
                    }
                } else if let Some(variant_type) = VariantType::refine(predicate_type) {
                    let variant_arg_type = variant_type.inner_type_by_name(constructor_name);
                    for pattern in patterns {
                        update_arm_pattern_type(pattern, &variant_arg_type, original_predicate)?;
                    }
                }
            }

            ArmPattern::TupleConstructor(patterns) => {
                let tuple_type = TupleType::refine(predicate_type).ok_or(format!(
                    "Invalid pattern match. Cannot match {} to tuple",
                    original_predicate
                ))?;
                let inner_types = tuple_type.inner_types();

                if patterns.len() == inner_types.len() {
                    for (pattern, inner_type) in patterns.iter_mut().zip(inner_types) {
                        update_arm_pattern_type(pattern, &inner_type, original_predicate)?;
                    }
                } else {
                    return Err(format!("Mismatch in number of elements in tuple pattern match. Expected {}, Actual: {}", inner_types.len(), patterns.len()));
                }
            }

            ArmPattern::ListConstructor(patterns) => {
                let list_type = ListType::refine(predicate_type).ok_or(format!(
                    "Invalid pattern match. Cannot match {} to list",
                    original_predicate
                ))?;

                let list_elem_type = list_type.inner_type();

                for pattern in &mut *patterns {
                    update_arm_pattern_type(pattern, &list_elem_type, original_predicate)?;
                }
            }

            ArmPattern::RecordConstructor(fields) => {
                let record_type = RecordType::refine(predicate_type).ok_or(format!(
                    "Invalid pattern match. Cannot match {} to record",
                    original_predicate
                ))?;

                for (field, pattern) in fields {
                    let type_of_field = record_type.inner_type_by_name(field);
                    update_arm_pattern_type(pattern, &type_of_field, original_predicate)?;
                }
            }

            ArmPattern::WildCard => {}
        }

        Ok(())
    }
}

#[cfg(test)]
mod type_push_down_tests {
    use test_r::test;

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
