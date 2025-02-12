use crate::type_checker::Path;
use crate::{Expr, InferredType, VariableId};
use std::collections::VecDeque;

// The goal is to be able to specify the types associated with an identifier.
// i.e, `a.*` is always `Str`, or `a.b.*` is always `Str`, or `a.b.c` is always `Str`
// This can be represented using `GlobalVariableTypeSpec { a, vec![], Str }`, `GlobalVariableTypeSpec {a, b, Str}`  and
// `GlobalVariableTypeSpec {a, vec[b, c], Str}` respectively
// If you specify completely opposite types to be default, you will get a compilation error.
#[derive(Clone, Debug)]
pub struct GlobalVariableTypeSpec {
    pub variable_id: VariableId,
    pub path: Path,
    pub inferred_type: InferredType,
}

//
// Algorithm:
// We initially create queue of immutable Expr (to be able to push mutable version has to do into reference count logic in Rust)
// and then push it to an intermediate stack and recreate the Expr. This is similar to `type_pull_up` phase.
// This is verbose but will make the algorithm quite easy to handle.

// Any other way of non-recursive way of overriding values requires RefCell. i.e,
// get a mutable expr, and send each mutable node into a queue, and then read these
// mutable expr and mutate it elsewhere requires Rc with RefCell in Rust. We
// decide from the beginning to keep the Expr tree as simple as possible with no Rc or RefCell structures
// just for 1 or 2 phases of compilation.
//
// Steps:
//  // Pre-process
//  Initialize a queue with all expsr in the tree, with the root node first:
//  Example queue:
//  [select_field(select_field(a, b), c), select_field(a, b), identifier(a)]
//
// Example Walkthrough: Given `GlobalVariableTypeSpec { a, vec[b, c], Str]`
//
// 1. Pop the back element in the queue to get `identifier(a)`.
//    - Check the `temp_stack` by popping from the front.
//    - If it's `None`, push `identifier(a)`'s to the stack.
//
// 2. Pop the back element in the queue again to get `select_field(a, b)`.
//    - Check the `temp_stack`, which now has
//      `(identifier(a), true)` at the front. We pop it out.
//    - Given `b` in the current is part of the path, and given path is not empty now,
//      push (select_field(identifier(a), b), true) back to stack (by this time stack has only 1 elem)
//
// 3. Pop the final element from the queue: `select_field(select_field(a, b), c)`.
//    - Check the `temp_stack`, which has `select_field(identifier(a), b), true) ` at the front.
//    - Given flag is true, and given c is also path (and the path has no more elements)
//      push (select_field((select_field(identifier(a), b), c, InferredType::Str)), false)
//      where false indicates loop break
//
//  The same algorithm above is tweaked even if users specified partial paths. Example:
//  Everything under `a.b` (regardless of the existence of c and d) at their leafs follow the default type

pub fn bind_global_variables_type(
    expr: &Expr,
    type_pecs: &Vec<GlobalVariableTypeSpec>,
) -> Result<Expr, String> {
    let mut result_expr = expr.clone();

    for spec in type_pecs {
        result_expr = bind_with_type_spec(&result_expr, spec)?;
    }

    Ok(result_expr)
}

fn bind_with_type_spec(expr: &Expr, type_spec: &GlobalVariableTypeSpec) -> Result<Expr, String> {
    let mut path = type_spec.path.clone();

    let mut expr_queue = VecDeque::new();

    internal::make_expr_nodes_queue(expr, &mut expr_queue);

    let mut temp_stack = VecDeque::new();

    while let Some(expr) = expr_queue.pop_back() {
        match expr {
            expr @ Expr::Identifier(variable_id, type_name, _) => {
                if variable_id == &type_spec.variable_id {
                    if path.is_empty() {
                        let continue_traverse = matches!(expr_queue.back(), Some(Expr::SelectField(inner, _, _, _)) if inner.as_ref() == expr);

                        if continue_traverse {
                            temp_stack.push_front((expr.clone(), true));
                        } else {
                            temp_stack.push_front((
                                Expr::Identifier(
                                    variable_id.clone(),
                                    type_name.clone(),
                                    type_spec.inferred_type.clone(),
                                ),
                                false,
                            ));
                        }
                    } else {
                        temp_stack.push_front((expr.clone(), true));
                    }
                } else {
                    temp_stack.push_front((expr.clone(), false));
                }
            }

            outer @ Expr::SelectField(inner_expr, field, type_name, current_inferred_type) => {
                let continue_search = matches!(expr_queue.back(), Some(Expr::SelectField(inner, _, _, _)) if inner.as_ref() == outer);

                internal::handle_select_field(
                    inner_expr,
                    field,
                    continue_search,
                    current_inferred_type,
                    &mut temp_stack,
                    &mut path,
                    &type_spec.inferred_type,
                    type_name,
                )?;
            }

            Expr::Tuple(tuple_elems, current_inferred_type) => {
                internal::handle_tuple(tuple_elems, current_inferred_type, &mut temp_stack);
            }

            expr @ Expr::Flags(_, _) => {
                temp_stack.push_front((expr.clone(), false));
            }

            Expr::SelectIndex(expr, index, type_name, current_inferred_type) => {
                internal::handle_select_index(
                    expr,
                    index,
                    current_inferred_type,
                    &mut temp_stack,
                    type_name,
                )?;
            }

            Expr::Result(Ok(_), type_name, current_inferred_type) => {
                internal::handle_result_ok(expr, current_inferred_type, &mut temp_stack, type_name);
            }

            Expr::Result(Err(_), type_name, current_inferred_type) => {
                internal::handle_result_error(
                    expr,
                    current_inferred_type,
                    &mut temp_stack,
                    type_name,
                );
            }

            Expr::Option(Some(expr), type_name, current_inferred_type) => {
                internal::handle_option_some(expr, current_inferred_type, &mut temp_stack, type_name);
            }

            Expr::Option(None, type_name, current_inferred_type) => {
                temp_stack.push_front((
                    Expr::Option(None, type_name.clone(), current_inferred_type.clone()),
                    false,
                ));
            }

            Expr::Cond(pred, then, else_, current_inferred_type) => {
                internal::handle_if_else(pred, then, else_, current_inferred_type, &mut temp_stack);
            }

            //
            Expr::PatternMatch(predicate, match_arms, current_inferred_type) => {
                internal::handle_pattern_match(
                    predicate,
                    match_arms,
                    current_inferred_type,
                    &mut temp_stack,
                );
            }

            Expr::Concat(exprs, _) => {
                internal::handle_concat(exprs, &mut temp_stack);
            }

            Expr::ExprBlock(exprs, current_inferred_type) => {
                internal::handle_multiple(exprs, current_inferred_type, &mut temp_stack);
            }

            Expr::Not(_, current_inferred_type) => {
                internal::handle_not(expr, current_inferred_type, &mut temp_stack);
            }

            Expr::GreaterThan(left, right, current_inferred_type) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut temp_stack,
                    Expr::GreaterThan,
                );
            }

            Expr::GreaterThanOrEqualTo(left, right, current_inferred_type) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut temp_stack,
                    Expr::GreaterThanOrEqualTo,
                );
            }

            Expr::LessThanOrEqualTo(left, right, current_inferred_type) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut temp_stack,
                    Expr::LessThanOrEqualTo,
                );
            }
            Expr::Plus(left, right, current_inferred_type) => {
                internal::handle_math_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut temp_stack,
                    Expr::Plus,
                );
            }

            Expr::Minus(left, right, current_inferred_type) => {
                internal::handle_math_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut temp_stack,
                    Expr::Minus,
                );
            }

            Expr::Multiply(left, right, current_inferred_type) => {
                internal::handle_math_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut temp_stack,
                    Expr::Multiply,
                );
            }

            Expr::Divide(left, right, current_inferred_type) => {
                internal::handle_math_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut temp_stack,
                    Expr::Divide,
                );
            }

            Expr::EqualTo(left, right, current_inferred_type) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut temp_stack,
                    Expr::EqualTo,
                );
            }

            Expr::LessThan(left, right, current_inferred_type) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    current_inferred_type,
                    &mut temp_stack,
                    Expr::LessThan,
                );
            }

            Expr::Let(variable_id, typ, expr, inferred_type) => {
                internal::handle_let(variable_id, expr, typ, inferred_type, &mut temp_stack);
            }
            Expr::Sequence(exprs, current_inferred_type) => {
                internal::handle_sequence(exprs, current_inferred_type, &mut temp_stack);
            }
            Expr::Record(expr, inferred_type) => {
                internal::handle_record(expr, inferred_type, &mut temp_stack);
            }
            Expr::Literal(_, _) => {
                temp_stack.push_front((expr.clone(), false));
            }
            Expr::Number(_, _, _) => {
                temp_stack.push_front((expr.clone(), false));
            }
            Expr::Boolean(_, _) => {
                temp_stack.push_front((expr.clone(), false));
            }
            Expr::And(left, right, _) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    &InferredType::Bool,
                    &mut temp_stack,
                    Expr::And,
                );
            }

            Expr::Or(left, right, _) => {
                internal::handle_comparison_op(
                    left,
                    right,
                    &InferredType::Bool,
                    &mut temp_stack,
                    Expr::Or,
                );
            }

            Expr::Call(call_type, exprs, inferred_type) => {
                internal::handle_call(call_type, exprs, inferred_type, &mut temp_stack);
            }

            Expr::Unwrap(expr, inferred_type) => {
                internal::handle_unwrap(expr, inferred_type, &mut temp_stack);
            }

            Expr::Throw(_, _) => {
                temp_stack.push_front((expr.clone(), false));
            }

            Expr::GetTag(_, inferred_type) => {
                internal::handle_get_tag(expr, inferred_type, &mut temp_stack);
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
                    &mut temp_stack,
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
                &mut temp_stack,
            ),
        }
    }

    temp_stack
        .pop_front()
        .map(|x| x.0)
        .ok_or("Failed type inference during pull up".to_string())
}

mod internal {
    use crate::call_type::CallType;

    use crate::type_checker::{Path, PathElem};
    use crate::{Expr, InferredType, MatchArm, TypeName, VariableId};
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
        temp_stack: &mut VecDeque<(Expr, bool)>,
    ) {
        let yield_expr_inferred = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(current_yield_expr.clone());
        let iterable_expr_inferred = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(current_iterable_expr.clone());

        temp_stack.push_front((
            Expr::typed_list_comprehension(
                variable_id.clone(),
                iterable_expr_inferred,
                yield_expr_inferred,
                current_comprehension_type.clone(),
            ),
            false,
        ))
    }

    pub(crate) fn handle_list_reduce(
        reduce_variable: &VariableId,
        iterated_variable: &VariableId,
        iterable_expr: &Expr,
        initial_value_expr: &Expr,
        yield_expr: &Expr,
        reduce_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
    ) {
        let new_yield_expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(yield_expr.clone());
        let new_init_value_expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(initial_value_expr.clone());
        let new_iterable_expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(iterable_expr.clone());

        let new_reduce_type = reduce_type.merge(new_init_value_expr.inferred_type());

        temp_stack.push_front((
            Expr::typed_list_reduce(
                reduce_variable.clone(),
                iterated_variable.clone(),
                new_iterable_expr,
                new_init_value_expr,
                new_yield_expr,
                new_reduce_type,
            ),
            false,
        ))
    }

    pub(crate) fn handle_tuple(
        tuple_elems: &[Expr],
        current_tuple_type: &InferredType,
        result_expr_queue: &mut VecDeque<(Expr, bool)>,
    ) {
        let mut new_tuple_elems = vec![];

        for current_tuple_elem in tuple_elems.iter().rev() {
            let pulled_up_type = result_expr_queue.pop_front();
            let new_tuple_elem = pulled_up_type
                .map(|x| x.0)
                .unwrap_or(current_tuple_elem.clone());
            new_tuple_elems.push(new_tuple_elem);
        }

        new_tuple_elems.reverse();

        // Reform tuple
        let new_tuple = Expr::Tuple(new_tuple_elems, current_tuple_type.clone());
        result_expr_queue.push_front((new_tuple, false));
    }

    pub(crate) fn handle_select_field(
        original_selection_expr: &Expr,
        field: &str,
        continue_search: bool,
        current_field_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        path: &mut Path,
        override_type: &InferredType,
        type_name: &Option<TypeName>,
    ) -> Result<(), String> {
        let (expr, part_of_path) = temp_stack
            .pop_front()
            .unwrap_or((original_selection_expr.clone(), false));

        if part_of_path {
            match path.current() {
                Some(PathElem::Field(name)) if name == field => path.progress(),
                Some(PathElem::Field(_)) => {}
                Some(PathElem::Index(_)) => {}
                None => {}
            }

            if path.is_empty() {
                let new_type = if continue_search {
                    current_field_type.clone()
                } else {
                    current_field_type.merge(override_type.clone())
                };

                temp_stack.push_front((
                    Expr::SelectField(
                        Box::new(expr.clone()),
                        field.to_string(),
                        type_name.clone(),
                        new_type,
                    ),
                    continue_search,
                ));
            } else {
                temp_stack.push_front((
                    Expr::SelectField(
                        Box::new(expr.clone()),
                        field.to_string(),
                        type_name.clone(),
                        current_field_type.clone(),
                    ),
                    true,
                ));
            }
        } else {
            temp_stack.push_front((
                Expr::SelectField(
                    Box::new(expr.clone()),
                    field.to_string(),
                    type_name.clone(),
                    current_field_type.clone(),
                ),
                false,
            ));
        }

        Ok(())
    }

    pub fn handle_select_index(
        original_selection_expr: &Expr,
        index: &usize,
        current_index_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        type_name: &Option<TypeName>,
    ) -> Result<(), String> {
        let expr = temp_stack
            .pop_front()
            .unwrap_or((original_selection_expr.clone(), false));

        let new_select_index = Expr::SelectIndex(
            Box::new(expr.0.clone()),
            *index,
            type_name.clone(),
            current_index_type.clone(),
        );
        temp_stack.push_front((new_select_index, false));

        Ok(())
    }

    pub(crate) fn handle_result_ok(
        original_ok_expr: &Expr,
        current_ok_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        type_name: &Option<TypeName>,
    ) {
        let ok_expr = temp_stack
            .pop_front()
            .unwrap_or((original_ok_expr.clone(), false));

        let new_result = Expr::Result(
            Ok(Box::new(ok_expr.0.clone())),
            type_name.clone(),
            current_ok_type.clone(),
        );
        temp_stack.push_front((new_result, true));
    }

    pub(crate) fn handle_result_error(
        original_error_expr: &Expr,
        current_error_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        type_name: &Option<TypeName>,
    ) {
        let expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(original_error_expr.clone());

        let new_result = Expr::Result(
            Err(Box::new(expr.clone())),
            type_name.clone(),
            current_error_type.clone(),
        );

        temp_stack.push_front((new_result, false));
    }

    pub(crate) fn handle_option_some(
        original_some_expr: &Expr,
        current_some_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        type_name: &Option<TypeName>,
    ) {
        let expr = temp_stack
            .pop_front()
            .unwrap_or((original_some_expr.clone(), false));
        let new_option = Expr::Option(
            Some(Box::new(expr.0.clone())),
            type_name.clone(),
            current_some_type.clone(),
        );
        temp_stack.push_front((new_option, false));
    }

    pub(crate) fn handle_if_else(
        original_predicate: &Expr,
        original_then_expr: &Expr,
        original_else_expr: &Expr,
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
    ) {
        let else_expr = temp_stack
            .pop_front()
            .unwrap_or((original_else_expr.clone(), false));
        let then_expr = temp_stack
            .pop_front()
            .unwrap_or((original_then_expr.clone(), false));
        let cond_expr = temp_stack
            .pop_front()
            .unwrap_or((original_predicate.clone(), false));

        let new_expr = Expr::Cond(
            Box::new(cond_expr.0),
            Box::new(then_expr.0.clone()),
            Box::new(else_expr.0.clone()),
            current_inferred_type.clone(),
        );

        temp_stack.push_front((new_expr, false));
    }

    pub fn handle_pattern_match(
        predicate: &Expr,
        current_match_arms: &[MatchArm],
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
    ) {
        let mut new_resolutions = vec![];
        let mut new_arm_patterns = vec![];
        for un_inferred_match_arm in current_match_arms.iter().rev() {
            let arm_resolution = temp_stack
                .pop_front()
                .map(|x| x.0)
                .unwrap_or(un_inferred_match_arm.arm_resolution_expr.deref().clone());

            let mut arm_pattern = un_inferred_match_arm.arm_pattern.clone();
            let current_arm_pattern_exprs = arm_pattern.get_expr_literals_mut();

            let mut new_arm_pattern_exprs = vec![];

            for _ in &current_arm_pattern_exprs {
                let arm_expr = temp_stack.pop_front().map(|x| x.0);
                new_arm_pattern_exprs.push(arm_expr)
            }
            new_arm_pattern_exprs.reverse();

            new_resolutions.push(arm_resolution);
            new_arm_patterns.push(arm_pattern);
        }

        let mut new_match_arms = new_arm_patterns
            .iter()
            .zip(new_resolutions.iter())
            .map(|(arm_pattern, arm_resolution)| crate::MatchArm {
                arm_pattern: arm_pattern.clone(),
                arm_resolution_expr: Box::new(arm_resolution.clone()),
            })
            .collect::<Vec<_>>();

        new_match_arms.reverse();

        let pred = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(predicate.clone());

        let new_expr = Expr::PatternMatch(
            Box::new(pred.clone()),
            new_match_arms,
            current_inferred_type.clone(),
        );

        temp_stack.push_front((new_expr, false));
    }

    pub(crate) fn handle_concat(exprs: &Vec<Expr>, temp_stack: &mut VecDeque<(Expr, bool)>) {
        let mut new_exprs = vec![];
        for expr in exprs {
            let expr = temp_stack.pop_front().map(|x| x.0).unwrap_or(expr.clone());
            new_exprs.push(expr);
        }

        new_exprs.reverse();

        let new_concat = Expr::Concat(new_exprs, InferredType::Str);
        temp_stack.push_front((new_concat, false));
    }

    pub(crate) fn handle_multiple(
        current_expr_list: &Vec<Expr>,
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
    ) {
        let mut new_exprs = vec![];
        for _ in current_expr_list {
            let expr = temp_stack.pop_front();
            if let Some(expr) = expr {
                new_exprs.push(expr.0);
            } else {
                break;
            }
        }

        new_exprs.reverse();

        let new_multiple = Expr::ExprBlock(new_exprs, current_inferred_type.clone());
        temp_stack.push_front((new_multiple, false));
    }

    pub(crate) fn handle_not(
        original_not_expr: &Expr,
        current_not_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
    ) {
        let expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(original_not_expr.clone());
        let new_not = Expr::Not(Box::new(expr), current_not_type.clone());
        temp_stack.push_front((new_not, false));
    }

    pub(crate) fn handle_math_op<F>(
        original_left_expr: &Expr,
        original_right_expr: &Expr,
        result_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        f: F,
    ) where
        F: Fn(Box<Expr>, Box<Expr>, InferredType) -> Expr,
    {
        let right_expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(original_right_expr.clone());
        let left_expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(original_left_expr.clone());

        let right_expr_type = right_expr.inferred_type();
        let left_expr_type = left_expr.inferred_type();
        let new_result_type = result_type.merge(right_expr_type).merge(left_expr_type);

        let new_math_op = f(
            Box::new(left_expr),
            Box::new(right_expr),
            new_result_type.clone(),
        );

        temp_stack.push_front((new_math_op, false));
    }

    pub(crate) fn handle_comparison_op<F>(
        original_left_expr: &Expr,
        original_right_expr: &Expr,
        result_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        f: F,
    ) where
        F: Fn(Box<Expr>, Box<Expr>, InferredType) -> Expr,
    {
        let right_expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(original_right_expr.clone());
        let left_expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(original_left_expr.clone());

        let new_binary = f(
            Box::new(left_expr),
            Box::new(right_expr),
            result_type.clone(),
        );
        temp_stack.push_front((new_binary, false));
    }

    pub(crate) fn handle_call(
        call_type: &CallType,
        arguments: &[Expr],
        inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
    ) {
        let mut new_arg_exprs = vec![];

        // retrieving all argument from the stack
        for expr in arguments.iter().rev() {
            let expr = temp_stack.pop_front().map(|x| x.0).unwrap_or(expr.clone());
            new_arg_exprs.push(expr);
        }

        new_arg_exprs.reverse();

        match call_type {
            CallType::Function(fun_name) => {
                let mut function_name = fun_name.clone();

                // The resource params in the function name was also in stack and need to be retrieved back
                let resource_params = function_name.function.raw_resource_params_mut();

                if let Some(resource_params) = resource_params {
                    let mut new_resource_params = vec![];
                    for expr in resource_params.iter().rev() {
                        let expr = temp_stack.pop_front().map(|x| x.0).unwrap_or(expr.clone());
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
                temp_stack.push_front((new_call, false));
            }

            CallType::VariantConstructor(str) => {
                let new_call = Expr::Call(
                    CallType::VariantConstructor(str.clone()),
                    new_arg_exprs,
                    inferred_type.clone(),
                );
                temp_stack.push_front((new_call, false));
            }

            CallType::EnumConstructor(str) => {
                let new_call = Expr::Call(
                    CallType::EnumConstructor(str.clone()),
                    new_arg_exprs,
                    inferred_type.clone(),
                );
                temp_stack.push_front((new_call, false));
            }
        }
    }

    pub(crate) fn handle_unwrap(
        expr: &Expr,
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
    ) {
        let expr = temp_stack.pop_front().map(|x| x.0).unwrap_or(expr.clone());
        let new_unwrap = Expr::Unwrap(Box::new(expr.clone()), current_inferred_type.clone());
        temp_stack.push_front((new_unwrap, false));
    }

    pub(crate) fn handle_get_tag(
        expr: &Expr,
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
    ) {
        let expr = temp_stack.pop_front().map(|x| x.0).unwrap_or(expr.clone());
        let new_get_tag = Expr::GetTag(Box::new(expr.clone()), current_inferred_type.clone());
        temp_stack.push_front((new_get_tag, false));
    }

    pub(crate) fn handle_let(
        original_variable_id: &VariableId,
        original_expr: &Expr,
        optional_type: &Option<crate::parser::type_name::TypeName>,
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
    ) {
        let expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(original_expr.clone());
        let new_let = Expr::Let(
            original_variable_id.clone(),
            optional_type.clone(),
            Box::new(expr),
            current_inferred_type.clone(),
        );
        temp_stack.push_front((new_let, false));
    }

    pub(crate) fn handle_sequence(
        current_expr_list: &[Expr],
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
    ) {
        let mut new_exprs = vec![];

        for expr in current_expr_list.iter().rev() {
            let expr = temp_stack.pop_front().map(|x| x.0).unwrap_or(expr.clone());
            new_exprs.push(expr);
        }

        new_exprs.reverse();

        let expr = Expr::Sequence(new_exprs, current_inferred_type.clone());

        temp_stack.push_front((expr, false));
    }

    pub(crate) fn handle_record(
        current_expr_list: &[(String, Box<Expr>)],
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
    ) {
        let mut new_exprs = vec![];

        for (field, expr) in current_expr_list.iter().rev() {
            let expr: Expr = temp_stack
                .pop_front()
                .map(|x| x.0)
                .unwrap_or(expr.deref().clone());
            new_exprs.push((field.clone(), Box::new(expr.clone())));
        }

        new_exprs.reverse();

        let new_record = Expr::Record(new_exprs.to_vec(), current_inferred_type.clone());
        temp_stack.push_front((new_record, false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FunctionTypeRegistry, Id, TypeName};
    use test_r::test;

    #[test]
    fn test_override_types_1() {
        let expr = Expr::from_text(
            r#"
            foo
        "#,
        )
        .unwrap();

        let type_spec = GlobalVariableTypeSpec {
            variable_id: VariableId::global("foo".to_string()),
            path: Path::default(),
            inferred_type: InferredType::Str,
        };

        let result = expr.bind_global_variables_type(&vec![type_spec]).unwrap();

        let expected = Expr::Identifier(
            VariableId::global("foo".to_string()),
            None,
            InferredType::Str,
        );

        assert_eq!(result, expected);
    }

    // Be able to
    #[test]
    fn test_override_types_2() {
        let expr = Expr::from_text(
            r#"
            foo.bar.baz
        "#,
        )
        .unwrap();

        let type_spec = GlobalVariableTypeSpec {
            variable_id: VariableId::global("foo".to_string()),
            path: Path::from_elems(vec!["bar"]),
            inferred_type: InferredType::Str,
        };

        let result = expr.bind_global_variables_type(&vec![type_spec]).unwrap();

        let expected = Expr::SelectField(
            Box::new(Expr::select_field(Expr::identifier("foo"), "bar")),
            "baz".to_string(),
            None,
            InferredType::Str,
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_override_types_3() {
        let expr = Expr::from_text(
            r#"
            foo.bar.baz
        "#,
        )
        .unwrap();

        let type_spec = GlobalVariableTypeSpec {
            variable_id: VariableId::global("foo".to_string()),
            path: Path::from_elems(vec!["bar", "baz"]),
            inferred_type: InferredType::Str,
        };

        let result = expr.bind_global_variables_type(&vec![type_spec]).unwrap();

        let expected = Expr::SelectField(
            Box::new(Expr::select_field(Expr::identifier("foo"), "bar")),
            "baz".to_string(),
            None,
            InferredType::Str,
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_override_types_4() {
        let expr = Expr::from_text(
            r#"
            foo.bar.baz
        "#,
        )
        .unwrap();

        let type_spec = GlobalVariableTypeSpec {
            variable_id: VariableId::global("foo".to_string()),
            path: Path::default(),
            inferred_type: InferredType::Str,
        };

        let result = expr.bind_global_variables_type(&vec![type_spec]).unwrap();

        let expected = Expr::SelectField(
            Box::new(Expr::select_field(Expr::identifier("foo"), "bar")),
            "baz".to_string(),
            None,
            InferredType::Str,
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_override_types_5() {
        let mut expr = Expr::from_text(
            r#"
             let res = foo.bar.user-id;
             let hello: u64 = foo.bar.number;
             hello
        "#,
        )
        .unwrap();

        let type_spec = GlobalVariableTypeSpec {
            variable_id: VariableId::global("foo".to_string()),
            path: Path::from_elems(vec!["bar"]),
            inferred_type: InferredType::Str,
        };

        expr.infer_types(&FunctionTypeRegistry::empty(), &vec![type_spec])
            .unwrap();

        let expected = Expr::ExprBlock(
            vec![
                Expr::Let(
                    VariableId::Local("res".to_string(), Some(Id(0))),
                    None,
                    Box::new(Expr::SelectField(
                        Box::new(Expr::SelectField(
                            Box::new(Expr::Identifier(
                                VariableId::Global("foo".to_string()),
                                None,
                                InferredType::Record(vec![(
                                    "bar".to_string(),
                                    InferredType::Record(vec![
                                        ("number".to_string(), InferredType::U64),
                                        ("user-id".to_string(), InferredType::Str),
                                    ]),
                                )]),
                            )),
                            "bar".to_string(),
                            None,
                            InferredType::Record(vec![
                                ("number".to_string(), InferredType::U64),
                                ("user-id".to_string(), InferredType::Str),
                            ]),
                        )),
                        "user-id".to_string(),
                        None,
                        InferredType::Str,
                    )),
                    InferredType::Unknown,
                ),
                Expr::Let(
                    VariableId::Local("hello".to_string(), Some(Id(0))),
                    Some(TypeName::U64),
                    Box::new(Expr::SelectField(
                        Box::new(Expr::SelectField(
                            Box::new(Expr::Identifier(
                                VariableId::Global("foo".to_string()),
                                None,
                                InferredType::Record(vec![(
                                    "bar".to_string(),
                                    InferredType::Record(vec![
                                        ("number".to_string(), InferredType::U64),
                                        ("user-id".to_string(), InferredType::Str),
                                    ]),
                                )]),
                            )),
                            "bar".to_string(),
                            None,
                            InferredType::Record(vec![
                                ("number".to_string(), InferredType::U64),
                                ("user-id".to_string(), InferredType::Str),
                            ]),
                        )),
                        "number".to_string(),
                        None,
                        InferredType::U64,
                    )),
                    InferredType::Unknown,
                ),
                Expr::Identifier(
                    VariableId::Local("hello".to_string(), Some(Id(0))),
                    None,
                    InferredType::U64,
                ),
            ],
            InferredType::U64,
        );

        assert_eq!(expr, expected);
    }
}
