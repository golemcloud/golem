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

use crate::{Expr, InferredType};
use std::collections::VecDeque;

// Initialize a queue with all expr in the tree, with the root node first:
// Example queue:
// [select_field(select_field(a, b), c), select_field(a, b), identifier(a)]
//
// The goal is to assign inferred types to each expression
// in the queue by working with a stack.
//
// Process:
//
// 1. Pop from the back of the queue and push to the front of
//    an inferred type stack, assigning inferred types along the way.
//
// Example Walkthrough:
//
// 1. Pop the back element in the queue to get `identifier(a)`.
//    - Check the `inferred_type_stack` by popping from the front.
//    - If it's `None`, push `identifier(a)`'s inferred type to the stack:
//      - `Record(b -> Record(c -> u64))`.
//
// 2. Pop the back element in the queue again to get `select_field(a, b)`.
//    - Check the `inferred_type_stack`, which now has
//      `Record(b -> Record(c -> u64))` at the front.
//    - Retrieve the type for `b` from `Record(b -> Record(c -> u64))`
//      and push it to the front of the stack.
//
// 3. Pop the final element from the queue: `select_field(select_field(a, b), c)`.
//    - Check the `inferred_type_stack`, which has `Record(c -> u64)` at the front.
//    - Retrieve the type for `c` from `Record(c -> u64)`
//      and push it to the stack as `u64`.
//
// At the end of this process, each expression has an assigned
// inferred type, created by traversing in a queue and stack order.
pub fn type_pull_up(expr: &Expr) -> Result<Expr, String> {
    let mut expr_queue = VecDeque::new();
    internal::make_expr_nodes_queue(expr, &mut expr_queue);

    let mut inferred_type_stack = VecDeque::new();

    while let Some(expr) = expr_queue.pop_back() {
        match expr {
            Expr::Tuple(tuple_elems, current_inferred_type) => {
                internal::handle_tuple(
                    tuple_elems,
                    current_inferred_type,
                    &mut inferred_type_stack,
                );
            }

            expr @ Expr::Identifier(_, _) => {
                inferred_type_stack.push_front(expr.clone());
            }

            expr @ Expr::Flags(_, _) => {
                inferred_type_stack.push_front(expr.clone());
            }

            Expr::SelectField(expr, field, current_inferred_type) => {
                internal::handle_select_field(
                    expr,
                    field,
                    current_inferred_type,
                    &mut inferred_type_stack,
                )?;
            }

            Expr::SelectIndex(expr, index, current_inferred_type) => {
                internal::handle_select_index(
                    expr,
                    index,
                    current_inferred_type,
                    &mut inferred_type_stack,
                )?;
            }

            Expr::Result(Ok(_), current_inferred_type) => {
                internal::handle_result_ok(expr, current_inferred_type, &mut inferred_type_stack);
            }

            Expr::Result(Err(_), current_inferred_type) => {
                internal::handle_result_error(
                    expr,
                    current_inferred_type,
                    &mut inferred_type_stack,
                );
            }

            Expr::Option(Some(expr), current_inferred_type) => {
                internal::handle_option_some(expr, current_inferred_type, &mut inferred_type_stack);
            }

            Expr::Option(None, current_inferred_type) => {
                inferred_type_stack.push_front(Expr::Option(None, current_inferred_type.clone()));
            }

            Expr::Cond(pred, then, else_, current_inferred_type) => {
                internal::handle_if_else(
                    pred,
                    then,
                    else_,
                    current_inferred_type,
                    &mut inferred_type_stack,
                );
            }

            //
            Expr::PatternMatch(predicate, match_arms, current_inferred_type) => {
                internal::handle_pattern_match(
                    predicate,
                    match_arms,
                    current_inferred_type,
                    &mut inferred_type_stack,
                );
            }

            Expr::Concat(exprs, _) => {
                internal::handle_concat(exprs, &mut inferred_type_stack);
            }

            Expr::ExprBlock(exprs, current_inferred_type) => {
                internal::handle_multiple(exprs, current_inferred_type, &mut inferred_type_stack);
            }

            Expr::Not(_, current_inferred_type) => {
                internal::handle_not(expr, current_inferred_type, &mut inferred_type_stack);
            }

            Expr::GreaterThan(left, right, current_inferred_type) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut inferred_type_stack,
                    Expr::GreaterThan,
                );
            }

            Expr::GreaterThanOrEqualTo(left, right, current_inferred_type) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut inferred_type_stack,
                    Expr::GreaterThanOrEqualTo,
                );
            }

            Expr::LessThanOrEqualTo(left, right, current_inferred_type) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut inferred_type_stack,
                    Expr::LessThanOrEqualTo,
                );
            }
            Expr::Plus(left, right, current_inferred_type) => {
                internal::handle_math_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut inferred_type_stack,
                    Expr::Plus,
                );
            }

            Expr::Minus(left, right, current_inferred_type) => {
                internal::handle_math_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut inferred_type_stack,
                    Expr::Minus,
                );
            }

            Expr::Multiply(left, right, current_inferred_type) => {
                internal::handle_math_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut inferred_type_stack,
                    Expr::Multiply,
                );
            }

            Expr::Divide(left, right, current_inferred_type) => {
                internal::handle_math_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut inferred_type_stack,
                    Expr::Divide,
                );
            }

            Expr::EqualTo(left, right, current_inferred_type) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut inferred_type_stack,
                    Expr::EqualTo,
                );
            }

            Expr::LessThan(left, right, current_inferred_type) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut inferred_type_stack,
                    Expr::LessThan,
                );
            }

            Expr::Let(variable_id, typ, expr, inferred_type) => {
                internal::handle_let(
                    variable_id,
                    expr,
                    typ,
                    inferred_type,
                    &mut inferred_type_stack,
                );
            }
            Expr::Sequence(exprs, current_inferred_type) => {
                internal::handle_sequence(exprs, current_inferred_type, &mut inferred_type_stack);
            }
            Expr::Record(expr, inferred_type) => {
                internal::handle_record(expr, inferred_type, &mut inferred_type_stack);
            }
            Expr::Literal(_, _) => {
                inferred_type_stack.push_front(expr.clone());
            }
            Expr::Number(_, _, _) => {
                inferred_type_stack.push_front(expr.clone());
            }
            Expr::Boolean(_, _) => {
                inferred_type_stack.push_front(expr.clone());
            }
            Expr::And(left, right, _) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    &InferredType::Bool,
                    &mut inferred_type_stack,
                    Expr::And,
                );
            }

            Expr::Or(left, right, _) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    &InferredType::Bool,
                    &mut inferred_type_stack,
                    Expr::Or,
                );
            }

            Expr::Call(call_type, exprs, inferred_type) => {
                internal::handle_call(call_type, exprs, inferred_type, &mut inferred_type_stack);
            }

            Expr::Unwrap(expr, inferred_type) => {
                internal::handle_unwrap(expr, inferred_type, &mut inferred_type_stack);
            }

            Expr::Throw(_, _) => {
                inferred_type_stack.push_front(expr.clone());
            }

            Expr::GetTag(_, inferred_type) => {
                internal::handle_get_tag(expr, inferred_type, &mut inferred_type_stack);
            }

            Expr::ListComprehension {
                iterated_variable,
                iterable_expr,
                yield_expr,
                inferred_type,
                ..
            } => {
                internal::handle_list_comprehension(
                    iterated_variable,
                    iterable_expr,
                    yield_expr,
                    inferred_type,
                    &mut inferred_type_stack,
                );
            }

            Expr::ListReduce {
                reduce_variable,
                iterated_variable,
                iterable_expr,
                init_value_expr,
                yield_expr,
                inferred_type,
            } => internal::handle_list_reduce(
                reduce_variable,
                iterated_variable,
                iterable_expr,
                init_value_expr,
                yield_expr,
                inferred_type,
                &mut inferred_type_stack,
            ),
        }
    }

    inferred_type_stack
        .pop_front()
        .ok_or("Failed type inference during pull up".to_string())
}

mod internal {
    use crate::call_type::CallType;

    use crate::type_refinement::precise_types::{ListType, RecordType};
    use crate::type_refinement::TypeRefinement;
    use crate::{Expr, InferredType, MatchArm, VariableId};
    use std::collections::VecDeque;
    use std::ops::Deref;

    pub(crate) fn make_expr_nodes_queue<'a>(expr: &'a Expr, expr_queue: &mut VecDeque<&'a Expr>) {
        let mut stack = VecDeque::new();

        stack.push_back(expr);

        while let Some(current_expr) = stack.pop_back() {
            expr_queue.push_back(current_expr);

            current_expr.visit_children_bottom_up(&mut stack)
        }
    }

    pub(crate) fn handle_list_comprehension(
        variable_id: &VariableId,
        current_iterable_expr: &Expr,
        current_yield_expr: &Expr,
        current_comprehension_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let yield_expr_inferred = inferred_type_stack
            .pop_front()
            .unwrap_or(current_yield_expr.clone());
        let iterable_expr_inferred = inferred_type_stack
            .pop_front()
            .unwrap_or(current_iterable_expr.clone());

        let list_expr = InferredType::List(Box::new(yield_expr_inferred.inferred_type()));
        let comprehension_type = current_comprehension_type.merge(list_expr);

        inferred_type_stack.push_front(Expr::typed_list_comprehension(
            variable_id.clone(),
            iterable_expr_inferred,
            yield_expr_inferred,
            comprehension_type,
        ))
    }

    pub(crate) fn handle_list_reduce(
        reduce_variable: &VariableId,
        iterated_variable: &VariableId,
        iterable_expr: &Expr,
        initial_value_expr: &Expr,
        yield_expr: &Expr,
        reduce_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let new_yield_expr = inferred_type_stack
            .pop_front()
            .unwrap_or(yield_expr.clone());
        let new_init_value_expr = inferred_type_stack
            .pop_front()
            .unwrap_or(initial_value_expr.clone());
        let new_iterable_expr = inferred_type_stack
            .pop_front()
            .unwrap_or(iterable_expr.clone());

        let new_reduce_type = reduce_type.merge(new_init_value_expr.inferred_type());

        inferred_type_stack.push_front(Expr::typed_list_reduce(
            reduce_variable.clone(),
            iterated_variable.clone(),
            new_iterable_expr,
            new_init_value_expr,
            new_yield_expr,
            new_reduce_type,
        ))
    }

    pub(crate) fn handle_tuple(
        tuple_elems: &[Expr],
        current_tuple_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let mut new_tuple_elems = vec![];

        for current_tuple_elem in tuple_elems.iter().rev() {
            let pulled_up_type = inferred_type_stack.pop_front();
            let new_tuple_elem = pulled_up_type.unwrap_or(current_tuple_elem.clone());
            new_tuple_elems.push(new_tuple_elem);
        }

        new_tuple_elems.reverse();

        let new_tuple_type =
            InferredType::Tuple(new_tuple_elems.iter().map(|x| x.inferred_type()).collect());

        let merged_tuple_type = current_tuple_type.merge(new_tuple_type);
        let new_tuple = Expr::Tuple(new_tuple_elems, merged_tuple_type);
        inferred_type_stack.push_front(new_tuple);
    }

    pub(crate) fn handle_select_field(
        original_selection_expr: &Expr,
        field: &str,
        current_field_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) -> Result<(), String> {
        let expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_selection_expr.clone());
        let select_from_expr_type = expr.inferred_type();
        let selection_field_type =
            get_inferred_type_of_selected_field(field, &select_from_expr_type)?;

        let new_select_field = Expr::SelectField(
            Box::new(expr.clone()),
            field.to_string(),
            current_field_type.merge(selection_field_type),
        );

        inferred_type_stack.push_front(new_select_field);

        Ok(())
    }

    pub fn handle_select_index(
        original_selection_expr: &Expr,
        index: &usize,
        current_index_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) -> Result<(), String> {
        let expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_selection_expr.clone());
        let inferred_type_of_selection_expr = expr.inferred_type();
        let list_type =
            get_inferred_type_of_selection_index(*index, &inferred_type_of_selection_expr)?;
        let new_select_index = Expr::SelectIndex(
            Box::new(expr.clone()),
            *index,
            current_index_type.merge(list_type),
        );
        inferred_type_stack.push_front(new_select_index);

        Ok(())
    }

    pub(crate) fn handle_result_ok(
        original_ok_expr: &Expr,
        current_ok_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let ok_expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_ok_expr.clone());
        let inferred_type_of_ok_expr = ok_expr.inferred_type();
        let result_type = InferredType::Result {
            ok: Some(Box::new(inferred_type_of_ok_expr)),
            error: None,
        };
        let new_result = Expr::Result(
            Ok(Box::new(ok_expr.clone())),
            current_ok_type.merge(result_type),
        );
        inferred_type_stack.push_front(new_result);
    }

    pub(crate) fn handle_result_error(
        original_error_expr: &Expr,
        current_error_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_error_expr.clone());
        let inferred_type_of_error_expr = expr.inferred_type();
        let result_type = InferredType::Result {
            ok: None,
            error: Some(Box::new(inferred_type_of_error_expr)),
        };
        let new_result = Expr::Result(
            Err(Box::new(expr.clone())),
            current_error_type.merge(result_type),
        );
        inferred_type_stack.push_front(new_result);
    }

    pub(crate) fn handle_option_some(
        original_some_expr: &Expr,
        current_some_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_some_expr.clone());
        let inferred_type_of_some_expr = expr.inferred_type();
        let option_type = InferredType::Option(Box::new(inferred_type_of_some_expr));
        let new_option = Expr::Option(
            Some(Box::new(expr.clone())),
            current_some_type.merge(option_type),
        );
        inferred_type_stack.push_front(new_option);
    }

    pub(crate) fn handle_if_else(
        original_predicate: &Expr,
        original_then_expr: &Expr,
        original_else_expr: &Expr,
        current_inferred_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let else_expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_else_expr.clone());
        let then_expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_then_expr.clone());
        let cond_expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_predicate.clone());
        let inferred_type_of_then_expr = then_expr.inferred_type();
        let inferred_type_of_else_expr = else_expr.inferred_type();

        let new_type = current_inferred_type
            .merge(inferred_type_of_then_expr.merge(inferred_type_of_else_expr));

        let new_expr = Expr::Cond(
            Box::new(cond_expr),
            Box::new(then_expr.clone()),
            Box::new(else_expr.clone()),
            new_type,
        );

        inferred_type_stack.push_front(new_expr);
    }

    pub fn handle_pattern_match(
        predicate: &Expr,
        current_match_arms: &[MatchArm],
        current_inferred_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let mut new_resolutions = vec![];
        let mut new_arm_patterns = vec![];
        for un_inferred_match_arm in current_match_arms.iter().rev() {
            let arm_resolution = inferred_type_stack
                .pop_front()
                .unwrap_or(un_inferred_match_arm.arm_resolution_expr.deref().clone());

            let mut arm_pattern = un_inferred_match_arm.arm_pattern.clone();
            let mut current_arm_pattern_exprs = arm_pattern.get_expr_literals_mut();

            let mut new_arm_pattern_exprs = vec![];

            for _ in &current_arm_pattern_exprs {
                let arm_expr = inferred_type_stack.pop_front();
                new_arm_pattern_exprs.push(arm_expr)
            }
            new_arm_pattern_exprs.reverse();

            current_arm_pattern_exprs
                .iter_mut()
                .zip(new_arm_pattern_exprs.iter())
                .for_each(|(arm_expr, new_expr_opt)| {
                    if let Some(new_expr) = new_expr_opt {
                        **arm_expr = Box::new(new_expr.clone());
                    }
                });

            new_resolutions.push(arm_resolution);
            new_arm_patterns.push(arm_pattern);
        }

        let inferred_types = new_resolutions
            .iter()
            .map(|expr| expr.inferred_type())
            .collect::<Vec<_>>();

        let new_inferred_type = InferredType::all_of(inferred_types);

        let mut new_match_arms = new_arm_patterns
            .iter()
            .zip(new_resolutions.iter())
            .map(|(arm_pattern, arm_resolution)| crate::MatchArm {
                arm_pattern: arm_pattern.clone(),
                arm_resolution_expr: Box::new(arm_resolution.clone()),
            })
            .collect::<Vec<_>>();

        new_match_arms.reverse();

        let new_type = if let Some(new_inferred_type) = new_inferred_type {
            current_inferred_type.merge(new_inferred_type)
        } else {
            current_inferred_type.clone()
        };

        let pred = inferred_type_stack.pop_front().unwrap_or(predicate.clone());

        let new_expr = Expr::PatternMatch(Box::new(pred.clone()), new_match_arms, new_type);

        inferred_type_stack.push_front(new_expr);
    }

    pub(crate) fn handle_concat(exprs: &Vec<Expr>, inferred_type_stack: &mut VecDeque<Expr>) {
        let mut new_exprs = vec![];
        for expr in exprs {
            let expr = inferred_type_stack.pop_front().unwrap_or(expr.clone());
            new_exprs.push(expr);
        }

        new_exprs.reverse();

        let new_concat = Expr::Concat(new_exprs, InferredType::Str);
        inferred_type_stack.push_front(new_concat);
    }

    pub(crate) fn handle_multiple(
        current_expr_list: &Vec<Expr>,
        current_inferred_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let mut new_exprs = vec![];
        for _ in current_expr_list {
            let expr = inferred_type_stack.pop_front();
            if let Some(expr) = expr {
                new_exprs.push(expr);
            } else {
                break;
            }
        }

        new_exprs.reverse();

        let new_inferred_type = if let Some(last_expr) = new_exprs.last() {
            last_expr.inferred_type()
        } else {
            InferredType::Unknown
        };

        let new_multiple =
            Expr::ExprBlock(new_exprs, current_inferred_type.merge(new_inferred_type));
        inferred_type_stack.push_front(new_multiple);
    }

    pub(crate) fn handle_not(
        original_not_expr: &Expr,
        current_not_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_not_expr.clone());
        let new_not = Expr::Not(Box::new(expr), current_not_type.clone());
        inferred_type_stack.push_front(new_not);
    }

    pub(crate) fn handle_math_op<F>(
        original_left_expr: &Expr,
        original_right_expr: &Expr,
        result_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
        f: F,
    ) where
        F: Fn(Box<Expr>, Box<Expr>, InferredType) -> Expr,
    {
        let right_expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_right_expr.clone());
        let left_expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_left_expr.clone());

        let right_expr_type = right_expr.inferred_type();
        let left_expr_type = left_expr.inferred_type();
        let new_result_type = result_type.merge(right_expr_type).merge(left_expr_type);

        let new_math_op = f(
            Box::new(left_expr),
            Box::new(right_expr),
            new_result_type.clone(),
        );

        inferred_type_stack.push_front(new_math_op);
    }

    pub(crate) fn handle_comparison_op<F>(
        original_left_expr: &Expr,
        original_right_expr: &Expr,
        result_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
        f: F,
    ) where
        F: Fn(Box<Expr>, Box<Expr>, InferredType) -> Expr,
    {
        let right_expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_right_expr.clone());
        let left_expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_left_expr.clone());
        let new_binary = f(
            Box::new(left_expr),
            Box::new(right_expr),
            result_type.clone(),
        );
        inferred_type_stack.push_front(new_binary);
    }

    pub(crate) fn handle_call(
        call_type: &CallType,
        arguments: &[Expr],
        inferred_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let mut new_arg_exprs = vec![];

        for expr in arguments.iter().rev() {
            let expr = inferred_type_stack.pop_front().unwrap_or(expr.clone());
            new_arg_exprs.push(expr);
        }

        new_arg_exprs.reverse();

        match call_type {
            CallType::Function(fun_name) => {
                let mut function_name = fun_name.clone();

                let resource_params = function_name.function.raw_resource_params_mut();

                if let Some(resource_params) = resource_params {
                    let mut new_resource_params = vec![];
                    for expr in resource_params.iter().rev() {
                        let expr = inferred_type_stack.pop_front().unwrap_or(expr.clone());
                        new_resource_params.push(expr);
                    }

                    new_resource_params.reverse();

                    resource_params
                        .iter_mut()
                        .zip(new_resource_params.iter())
                        .for_each(|(param, new_expr)| {
                            *param = new_expr.clone();
                        });
                }

                let new_call = Expr::Call(
                    CallType::Function(function_name),
                    new_arg_exprs,
                    inferred_type.clone(),
                );
                inferred_type_stack.push_front(new_call);
            }

            CallType::VariantConstructor(str) => {
                let new_call = Expr::Call(
                    CallType::VariantConstructor(str.clone()),
                    new_arg_exprs,
                    inferred_type.clone(),
                );
                inferred_type_stack.push_front(new_call);
            }

            CallType::EnumConstructor(str) => {
                let new_call = Expr::Call(
                    CallType::EnumConstructor(str.clone()),
                    new_arg_exprs,
                    inferred_type.clone(),
                );
                inferred_type_stack.push_front(new_call);
            }
        }
    }

    pub(crate) fn handle_unwrap(
        expr: &Expr,
        current_inferred_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let expr = inferred_type_stack.pop_front().unwrap_or(expr.clone());
        let new_unwrap = Expr::Unwrap(
            Box::new(expr.clone()),
            current_inferred_type.merge(expr.inferred_type()),
        );
        inferred_type_stack.push_front(new_unwrap);
    }

    pub(crate) fn handle_get_tag(
        expr: &Expr,
        current_inferred_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let expr = inferred_type_stack.pop_front().unwrap_or(expr.clone());
        let new_get_tag = Expr::GetTag(
            Box::new(expr.clone()),
            current_inferred_type.merge(expr.inferred_type()),
        );
        inferred_type_stack.push_front(new_get_tag);
    }

    pub(crate) fn handle_let(
        original_variable_id: &VariableId,
        original_expr: &Expr,
        optional_type: &Option<crate::parser::type_name::TypeName>,
        current_inferred_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let expr = inferred_type_stack
            .pop_front()
            .unwrap_or(original_expr.clone());
        let new_let = Expr::Let(
            original_variable_id.clone(),
            optional_type.clone(),
            Box::new(expr),
            current_inferred_type.clone(),
        );
        inferred_type_stack.push_front(new_let);
    }

    pub(crate) fn handle_sequence(
        current_expr_list: &[Expr],
        current_inferred_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let mut new_exprs = vec![];

        for expr in current_expr_list.iter().rev() {
            let expr = inferred_type_stack.pop_front().unwrap_or(expr.clone());
            new_exprs.push(expr);
        }

        new_exprs.reverse();

        let new_sequence = {
            if let Some(first_expr) = new_exprs.clone().first() {
                Expr::Sequence(
                    new_exprs,
                    current_inferred_type
                        .clone()
                        .merge(InferredType::List(Box::new(first_expr.inferred_type()))),
                )
            } else {
                Expr::Sequence(new_exprs, current_inferred_type.clone())
            }
        };

        inferred_type_stack.push_front(new_sequence);
    }

    pub(crate) fn handle_record(
        current_expr_list: &[(String, Box<Expr>)],
        current_inferred_type: &InferredType,
        inferred_type_stack: &mut VecDeque<Expr>,
    ) {
        let mut ordered_types = vec![];
        let mut new_exprs = vec![];

        for (field, expr) in current_expr_list.iter().rev() {
            let expr: Expr = inferred_type_stack
                .pop_front()
                .unwrap_or(expr.deref().clone());
            ordered_types.push((field.clone(), expr.inferred_type()));
            new_exprs.push((field.clone(), Box::new(expr.clone())));
        }

        new_exprs.reverse();
        ordered_types.reverse();

        let new_record_type = InferredType::Record(ordered_types);

        let merged_record_type = current_inferred_type.merge(new_record_type);

        let new_record = Expr::Record(new_exprs.to_vec(), merged_record_type);
        inferred_type_stack.push_front(new_record);
    }

    pub(crate) fn get_inferred_type_of_selected_field(
        select_field: &str,
        select_from_type: &InferredType,
    ) -> Result<InferredType, String> {
        let refined_record = RecordType::refine(select_from_type).ok_or(format!(
            "Cannot select {} since it is not a record type. Found: {:?}",
            select_field, select_from_type
        ))?;

        Ok(refined_record.inner_type_by_name(select_field))
    }

    pub(crate) fn get_inferred_type_of_selection_index(
        selected_index: usize,
        select_from_type: &InferredType,
    ) -> Result<InferredType, String> {
        let refined_list = ListType::refine(select_from_type).ok_or(format!(
            "Cannot get index {} since it is not a list type. Found: {:?}",
            selected_index, select_from_type
        ))?;

        Ok(refined_list.inner_type())
    }
}

#[cfg(test)]
mod type_pull_up_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::call_type::CallType;
    use crate::function_name::DynamicParsedFunctionName;
    use crate::DynamicParsedFunctionReference::IndexedResourceMethod;
    use crate::ParsedFunctionSite::PackagedInterface;
    use crate::{
        ArmPattern, Expr, FunctionTypeRegistry, InferredType, MatchArm, Number, VariableId,
    };

    #[test]
    pub fn test_pull_up_identifier() {
        let expr = "foo";
        let mut expr = Expr::from_text(expr).unwrap();
        expr.add_infer_type_mut(InferredType::Str);
        let new_expr = expr.pull_types_up().unwrap();
        assert_eq!(new_expr.inferred_type(), InferredType::Str);
    }

    #[test]
    pub fn test_pull_up_for_select_field() {
        let record_identifier =
            Expr::identifier("foo").add_infer_type(InferredType::Record(vec![(
                "foo".to_string(),
                InferredType::Record(vec![("bar".to_string(), InferredType::U64)]),
            )]));
        let select_expr = Expr::select_field(record_identifier, "foo");
        let expr = Expr::select_field(select_expr, "bar");
        let new_expr = expr.pull_types_up().unwrap();
        assert_eq!(new_expr.inferred_type(), InferredType::U64);
    }

    #[test]
    pub fn test_pull_up_for_select_index() {
        let identifier =
            Expr::identifier("foo").add_infer_type(InferredType::List(Box::new(InferredType::U64)));
        let expr = Expr::select_index(identifier.clone(), 0);
        let new_expr = expr.pull_types_up().unwrap();
        let expected = Expr::select_index(identifier, 0).add_infer_type(InferredType::U64);
        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_sequence() {
        let elems = vec![
            Expr::Number(
                Number {
                    value: BigDecimal::from(1),
                },
                None,
                InferredType::U64,
            ),
            Expr::Number(
                Number {
                    value: BigDecimal::from(2),
                },
                None,
                InferredType::U64,
            ),
        ];

        let expr = Expr::Sequence(elems.clone(), InferredType::Unknown);
        let new_expr = expr.pull_types_up().unwrap();

        assert_eq!(
            new_expr,
            Expr::Sequence(elems, InferredType::List(Box::new(InferredType::U64)))
        );
    }

    #[test]
    pub fn test_pull_up_for_tuple() {
        let expr = Expr::tuple(vec![
            Expr::literal("foo"),
            Expr::Number(
                Number {
                    value: BigDecimal::from(1),
                },
                None,
                InferredType::U64,
            ),
        ]);
        let new_expr = expr.pull_types_up().unwrap();
        assert_eq!(
            new_expr.inferred_type(),
            InferredType::Tuple(vec![InferredType::Str, InferredType::U64])
        );
    }

    #[test]
    pub fn test_pull_up_for_record() {
        let elems = vec![
            (
                "foo".to_string(),
                Box::new(Expr::Number(
                    Number {
                        value: BigDecimal::from(1),
                    },
                    None,
                    InferredType::U64,
                )),
            ),
            (
                "bar".to_string(),
                Box::new(Expr::Number(
                    Number {
                        value: BigDecimal::from(2),
                    },
                    None,
                    InferredType::U32,
                )),
            ),
        ];
        let expr = Expr::Record(
            elems.clone(),
            InferredType::Record(vec![
                ("foo".to_string(), InferredType::Unknown),
                ("bar".to_string(), InferredType::Unknown),
            ]),
        );
        let new_expr = expr.pull_types_up().unwrap();

        assert_eq!(
            new_expr,
            Expr::Record(
                elems,
                InferredType::AllOf(vec![
                    InferredType::Record(vec![
                        ("foo".to_string(), InferredType::U64),
                        ("bar".to_string(), InferredType::U32)
                    ]),
                    InferredType::Record(vec![
                        ("foo".to_string(), InferredType::Unknown),
                        ("bar".to_string(), InferredType::Unknown)
                    ])
                ])
            )
        );
    }

    #[test]
    pub fn test_pull_up_for_concat() {
        let expr = Expr::concat(vec![Expr::literal("foo"), Expr::literal("bar")]);
        let new_expr = expr.pull_types_up().unwrap();
        let expected = Expr::Concat(
            vec![Expr::literal("foo"), Expr::literal("bar")],
            InferredType::Str,
        );
        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_not() {
        let expr = Expr::not(Expr::boolean(true));
        let new_expr = expr.pull_types_up().unwrap();
        assert_eq!(new_expr.inferred_type(), InferredType::Bool);
    }

    #[test]
    pub fn test_pull_up_if_else() {
        let inner1 =
            Expr::identifier("foo").add_infer_type(InferredType::List(Box::new(InferredType::U64)));

        let select_index1 = Expr::select_index(inner1.clone(), 0);
        let select_index2 = Expr::select_index(inner1, 1);

        let inner2 =
            Expr::identifier("bar").add_infer_type(InferredType::List(Box::new(InferredType::U64)));

        let select_index3 = Expr::select_index(inner2.clone(), 0);
        let select_index4 = Expr::select_index(inner2, 1);

        let expr = Expr::cond(
            Expr::greater_than(select_index1.clone(), select_index2.clone()),
            select_index3.clone(),
            select_index4.clone(),
        );

        let new_expr = expr.pull_types_up().unwrap();
        let expected = Expr::Cond(
            Box::new(Expr::GreaterThan(
                Box::new(Expr::SelectIndex(
                    Box::new(Expr::Identifier(
                        VariableId::global("foo".to_string()),
                        InferredType::List(Box::new(InferredType::U64)),
                    )),
                    0,
                    InferredType::U64,
                )),
                Box::new(Expr::SelectIndex(
                    Box::new(Expr::Identifier(
                        VariableId::global("foo".to_string()),
                        InferredType::List(Box::new(InferredType::U64)),
                    )),
                    1,
                    InferredType::U64,
                )),
                InferredType::Bool,
            )),
            Box::new(Expr::SelectIndex(
                Box::new(Expr::Identifier(
                    VariableId::global("bar".to_string()),
                    InferredType::List(Box::new(InferredType::U64)),
                )),
                0,
                InferredType::U64,
            )),
            Box::new(Expr::SelectIndex(
                Box::new(Expr::Identifier(
                    VariableId::global("bar".to_string()),
                    InferredType::List(Box::new(InferredType::U64)),
                )),
                1,
                InferredType::U64,
            )),
            InferredType::U64,
        );
        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_greater_than() {
        let inner = Expr::identifier("foo").add_infer_type(InferredType::Record(vec![
            ("bar".to_string(), InferredType::Str),
            ("baz".to_string(), InferredType::U64),
        ]));

        let select_field1 = Expr::select_field(inner.clone(), "bar");
        let select_field2 = Expr::select_field(inner, "baz");
        let expr = Expr::greater_than(select_field1.clone(), select_field2.clone());

        let new_expr = expr.pull_types_up().unwrap();

        let expected = Expr::greater_than(
            select_field1.add_infer_type(InferredType::Str),
            select_field2.add_infer_type(InferredType::U64),
        )
        .add_infer_type(InferredType::Bool);
        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_greater_than_or_equal_to() {
        let inner =
            Expr::identifier("foo").add_infer_type(InferredType::List(Box::new(InferredType::U64)));

        let select_index1 = Expr::select_index(inner.clone(), 0);
        let select_index2 = Expr::select_index(inner, 1);
        let expr = Expr::greater_than_or_equal_to(select_index1.clone(), select_index2.clone());

        let new_expr = expr.pull_types_up().unwrap();

        let expected = Expr::greater_than_or_equal_to(
            select_index1.add_infer_type(InferredType::U64),
            select_index2.add_infer_type(InferredType::U64),
        )
        .add_infer_type(InferredType::Bool);
        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_less_than_or_equal_to() {
        let record_type = InferredType::Record(vec![
            ("bar".to_string(), InferredType::Str),
            ("baz".to_string(), InferredType::U64),
        ]);

        let inner = Expr::identifier("foo")
            .add_infer_type(InferredType::List(Box::new(record_type.clone())));

        let select_field_from_first =
            Expr::select_field(Expr::select_index(inner.clone(), 0), "bar");
        let select_field_from_second =
            Expr::select_field(Expr::select_index(inner.clone(), 1), "baz");
        let expr = Expr::less_than_or_equal_to(
            select_field_from_first.clone(),
            select_field_from_second.clone(),
        );

        let new_expr = expr.pull_types_up().unwrap();

        let new_select_field_from_first = Expr::select_field(
            Expr::select_index(inner.clone(), 0).add_infer_type(record_type.clone()),
            "bar",
        )
        .add_infer_type(InferredType::Str);

        let new_select_field_from_second = Expr::select_field(
            Expr::select_index(inner.clone(), 1).add_infer_type(record_type),
            "baz",
        )
        .add_infer_type(InferredType::U64);

        let expected =
            Expr::less_than_or_equal_to(new_select_field_from_first, new_select_field_from_second)
                .add_infer_type(InferredType::Bool);

        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_equal_to() {
        let expr = Expr::equal_to(
            Expr::untyped_number(BigDecimal::from(1)),
            Expr::untyped_number(BigDecimal::from(2)),
        );
        let new_expr = expr.pull_types_up().unwrap();
        assert_eq!(new_expr.inferred_type(), InferredType::Bool);
    }

    #[test]
    pub fn test_pull_up_for_less_than() {
        let expr = Expr::less_than(
            Expr::untyped_number(BigDecimal::from(1)),
            Expr::untyped_number(BigDecimal::from(2)),
        );
        let new_expr = expr.pull_types_up().unwrap();
        assert_eq!(new_expr.inferred_type(), InferredType::Bool);
    }

    #[test]
    pub fn test_pull_up_for_call() {
        let expr = Expr::call(
            DynamicParsedFunctionName::parse("global_fn").unwrap(),
            vec![Expr::untyped_number(BigDecimal::from(1))],
        );
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::Unknown);
    }

    #[test]
    pub fn test_pull_up_for_dynamic_call() {
        let rib = r#"
           let input = { foo: "afs", bar: "al" };
           golem:it/api.{cart(input.foo).checkout}()
        "#;

        let mut expr = Expr::from_text(rib).unwrap();
        let function_registry = FunctionTypeRegistry::empty();
        expr.infer_types_initial_phase(&function_registry).unwrap();
        expr.infer_all_identifiers().unwrap();
        let new_expr = expr.pull_types_up().unwrap();

        let expected = Expr::ExprBlock(
            vec![
                Expr::Let(
                    VariableId::local("input", 0),
                    None,
                    Box::new(Expr::Record(
                        vec![
                            (
                                "foo".to_string(),
                                Box::new(Expr::Literal("afs".to_string(), InferredType::Str)),
                            ),
                            (
                                "bar".to_string(),
                                Box::new(Expr::Literal("al".to_string(), InferredType::Str)),
                            ),
                        ],
                        InferredType::Record(vec![
                            ("foo".to_string(), InferredType::Str),
                            ("bar".to_string(), InferredType::Str),
                        ]),
                    )),
                    InferredType::Unknown,
                ),
                Expr::Call(
                    CallType::Function(DynamicParsedFunctionName {
                        site: PackagedInterface {
                            namespace: "golem".to_string(),
                            package: "it".to_string(),
                            interface: "api".to_string(),
                            version: None,
                        },
                        function: IndexedResourceMethod {
                            resource: "cart".to_string(),
                            resource_params: vec![Expr::SelectField(
                                Box::new(Expr::Identifier(
                                    VariableId::local("input", 0),
                                    InferredType::Record(vec![
                                        ("foo".to_string(), InferredType::Str),
                                        ("bar".to_string(), InferredType::Str),
                                    ]),
                                )),
                                "foo".to_string(),
                                InferredType::Str,
                            )],
                            method: "checkout".to_string(),
                        },
                    }),
                    vec![],
                    InferredType::Unknown,
                ),
            ],
            InferredType::Unknown,
        );

        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_unwrap() {
        let mut number = Expr::untyped_number(BigDecimal::from(1));
        number.override_type_type_mut(InferredType::F64);
        let expr = Expr::option(Some(number)).unwrap();
        let expr = expr.pull_types_up().unwrap();
        assert_eq!(
            expr.inferred_type(),
            InferredType::Option(Box::new(InferredType::F64))
        );
    }

    #[test]
    pub fn test_pull_up_for_tag() {
        let mut number = Expr::untyped_number(BigDecimal::from(1));
        number.override_type_type_mut(InferredType::F64);
        let expr = Expr::get_tag(Expr::option(Some(number)));
        let expr = expr.pull_types_up().unwrap();
        assert_eq!(
            expr.inferred_type(),
            InferredType::Option(Box::new(InferredType::F64))
        );
    }

    #[test]
    pub fn test_pull_up_for_pattern_match() {
        let expr = Expr::pattern_match(
            Expr::select_field(
                Expr::identifier("foo").add_infer_type(InferredType::Record(vec![(
                    "bar".to_string(),
                    InferredType::Str,
                )])),
                "bar",
            ),
            vec![
                MatchArm {
                    arm_pattern: ArmPattern::Constructor(
                        "cons1".to_string(),
                        vec![ArmPattern::Literal(Box::new(Expr::SelectField(
                            Box::new(Expr::identifier("foo").add_infer_type(InferredType::Record(
                                vec![("bar".to_string(), InferredType::Str)],
                            ))),
                            "bar".to_string(),
                            InferredType::Unknown,
                        )))],
                    ),
                    arm_resolution_expr: Box::new(Expr::SelectField(
                        Box::new(Expr::identifier("baz").add_infer_type(InferredType::Record(
                            vec![("qux".to_string(), InferredType::Str)],
                        ))),
                        "qux".to_string(),
                        InferredType::Unknown,
                    )),
                },
                MatchArm {
                    arm_pattern: ArmPattern::Constructor(
                        "cons2".to_string(),
                        vec![ArmPattern::Literal(Box::new(Expr::SelectField(
                            Box::new(Expr::identifier("quux").add_infer_type(
                                InferredType::Record(vec![(
                                    "corge".to_string(),
                                    InferredType::Str,
                                )]),
                            )),
                            "corge".to_string(),
                            InferredType::Unknown,
                        )))],
                    ),
                    arm_resolution_expr: Box::new(Expr::SelectField(
                        Box::new(
                            Expr::identifier("grault").add_infer_type(InferredType::Record(vec![
                                ("garply".to_string(), InferredType::Str),
                            ])),
                        ),
                        "garply".to_string(),
                        InferredType::Unknown,
                    )),
                },
            ],
        );
        let new_expr = expr.pull_types_up().unwrap();
        let expected = internal::expected_pattern_match();
        assert_eq!(new_expr, expected);
    }

    mod internal {
        use crate::{ArmPattern, Expr, InferredType, MatchArm, VariableId};

        pub(crate) fn expected_pattern_match() -> Expr {
            Expr::PatternMatch(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Identifier(
                        VariableId::global("foo".to_string()),
                        InferredType::Record(vec![("bar".to_string(), InferredType::Str)]),
                    )),
                    "bar".to_string(),
                    InferredType::Str,
                )),
                vec![
                    MatchArm {
                        arm_pattern: ArmPattern::Constructor(
                            "cons1".to_string(),
                            vec![ArmPattern::Literal(Box::new(Expr::SelectField(
                                Box::new(Expr::Identifier(
                                    VariableId::global("foo".to_string()),
                                    InferredType::Record(vec![(
                                        "bar".to_string(),
                                        InferredType::Str,
                                    )]),
                                )),
                                "bar".to_string(),
                                InferredType::Str,
                            )))],
                        ),
                        arm_resolution_expr: Box::new(Expr::SelectField(
                            Box::new(Expr::Identifier(
                                VariableId::global("baz".to_string()),
                                InferredType::Record(vec![("qux".to_string(), InferredType::Str)]),
                            )),
                            "qux".to_string(),
                            InferredType::Str,
                        )),
                    },
                    MatchArm {
                        arm_pattern: ArmPattern::Constructor(
                            "cons2".to_string(),
                            vec![ArmPattern::Literal(Box::new(Expr::SelectField(
                                Box::new(Expr::Identifier(
                                    VariableId::global("quux".to_string()),
                                    InferredType::Record(vec![(
                                        "corge".to_string(),
                                        InferredType::Str,
                                    )]),
                                )),
                                "corge".to_string(),
                                InferredType::Str,
                            )))],
                        ),
                        arm_resolution_expr: Box::new(Expr::SelectField(
                            Box::new(Expr::Identifier(
                                VariableId::global("grault".to_string()),
                                InferredType::Record(vec![(
                                    "garply".to_string(),
                                    InferredType::Str,
                                )]),
                            )),
                            "garply".to_string(),
                            InferredType::Str,
                        )),
                    },
                ],
                InferredType::Str,
            )
        }
    }
}
