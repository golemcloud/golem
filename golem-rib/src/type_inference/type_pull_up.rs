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

use crate::rib_type_error::RibTypeError;
use crate::{CustomError, Expr, ExprVisitor, InferredType};
use std::collections::VecDeque;

pub fn type_pull_up(expr: &mut Expr) -> Result<Expr, RibTypeError> {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_front() {
        match expr {
            Expr::Tuple {
                exprs,
                inferred_type,
                ..
            } => {
                internal::handle_tuple(exprs, inferred_type);
            }

            Expr::Identifier { .. } => {}

            Expr::Flags { .. } => {}

            Expr::InvokeMethodLazy { lhs, method, .. } => {
                let lhs = lhs.to_string();
                return Err(CustomError {
                    expr: expr.clone(),
                    help_message: vec![],
                    message: format!("invalid method invocation `{}.{}`. make sure `{}` is defined and is a valid instance type (i.e, resource or worker)", lhs, method, lhs),
                }.into());
            }

            Expr::SelectField {
                expr,
                field,
                inferred_type,
                ..
            } => {
                internal::handle_select_field(expr, field, inferred_type)?;
            }

            Expr::SelectIndex {
                expr,
                index,
                inferred_type,
                source_span,
                ..
            } => {
                internal::handle_select_index(expr, index, inferred_type)?;
            }

            Expr::Result {
                expr: Ok(_),
                inferred_type,
                ..
            } => {
                internal::handle_result_ok(expr, inferred_type);
            }

            Expr::Result {
                expr: Err(_),
                inferred_type,
                ..
            } => {
                internal::handle_result_error(expr, inferred_type);
            }

            Expr::Option {
                expr: Some(expr),
                inferred_type,
                ..
            } => {
                internal::handle_option_some(expr, inferred_type);
            }

            Expr::Option { .. } => {}

            Expr::Cond {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::handle_if_else(lhs, rhs, inferred_type);
            }

            Expr::PatternMatch {
                match_arms,
                inferred_type,
                ..
            } => {
                internal::handle_pattern_match(match_arms, inferred_type);
            }

            Expr::Concat { .. } => {}

            Expr::ExprBlock {
                exprs,
                inferred_type,
                ..
            } => {
                internal::handle_multiple(exprs, inferred_type);
            }

            Expr::Not { .. } => {}
            Expr::GreaterThan { .. } => {}
            Expr::GreaterThanOrEqualTo { .. } => {}
            Expr::LessThanOrEqualTo { .. } => {}
            Expr::EqualTo { .. } => {}
            Expr::LessThan { .. } => {}

            plus @ Expr::Plus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::handle_math_op(plus, lhs, rhs, inferred_type)?;
            }

            minus @ Expr::Minus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::handle_math_op(minus, lhs, rhs, inferred_type)?;
            }

            multiply @ Expr::Multiply {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::handle_math_op(multiply, lhs, rhs, inferred_type)?;
            }

            divide @ Expr::Divide {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                internal::handle_math_op(divide, lhs, rhs, inferred_type)?;
            }

            Expr::Let { .. } => {}

            Expr::Sequence {
                exprs,
                type_annotation,
                inferred_type,
                source_span,
            } => {
                internal::handle_sequence(
                    exprs,
                    inferred_type,
                    &mut inferred_expr_stack,
                    type_annotation,
                    source_span,
                );
            }

            Expr::Record {
                exprs,
                inferred_type,
                source_span,
                ..
            } => {
                internal::handle_record(
                    exprs,
                    inferred_type,
                    &mut inferred_expr_stack,
                    source_span,
                );
            }
            Expr::Literal { .. } => {
                inferred_expr_stack.push_front(expr.clone());
            }
            Expr::Number { .. } => {
                inferred_expr_stack.push_front(expr.clone());
            }
            Expr::Boolean { .. } => {
                inferred_expr_stack.push_front(expr.clone());
            }
            Expr::And {
                lhs,
                rhs,
                source_span,
                type_annotation,
                ..
            } => {
                internal::handle_comparison_op(
                    lhs,
                    rhs,
                    &InferredType::Bool,
                    &mut inferred_expr_stack,
                    |a, b, c| Expr::And {
                        lhs: a,
                        rhs: b,
                        inferred_type: c,
                        source_span: source_span.clone(),
                        type_annotation: type_annotation.clone(),
                    },
                );
            }

            Expr::Or {
                lhs,
                rhs,
                source_span,
                type_annotation,
                ..
            } => {
                internal::handle_comparison_op(
                    lhs,
                    rhs,
                    &InferredType::Bool,
                    &mut inferred_expr_stack,
                    |a, b, c| Expr::Or {
                        lhs: a,
                        rhs: b,
                        inferred_type: c,
                        source_span: source_span.clone(),
                        type_annotation: type_annotation.clone(),
                    },
                );
            }

            Expr::Call {
                call_type,
                generic_type_parameter,
                args,
                inferred_type,
                source_span,
                ..
            } => {
                internal::handle_call(
                    call_type,
                    generic_type_parameter.clone(),
                    args,
                    inferred_type,
                    &mut inferred_expr_stack,
                    source_span,
                );
            }

            Expr::Unwrap {
                expr,
                inferred_type,
                source_span,
                ..
            } => {
                internal::handle_unwrap(expr, inferred_type, &mut inferred_expr_stack, source_span);
            }

            Expr::Length {
                expr,
                inferred_type,
                source_span,
                ..
            } => {
                internal::handle_length(expr, inferred_type, &mut inferred_expr_stack, source_span);
            }

            Expr::Throw { .. } => {
                inferred_expr_stack.push_front(expr.clone());
            }

            Expr::GetTag {
                expr,
                inferred_type,
                source_span,
                ..
            } => {
                internal::handle_get_tag(
                    expr,
                    inferred_type,
                    &mut inferred_expr_stack,
                    source_span,
                );
            }

            Expr::ListComprehension {
                iterated_variable,
                iterable_expr,
                yield_expr,
                inferred_type,
                source_span,
                ..
            } => {
                internal::handle_list_comprehension(
                    iterated_variable,
                    iterable_expr,
                    yield_expr,
                    inferred_type,
                    &mut inferred_expr_stack,
                    source_span,
                );
            }

            Expr::ListReduce {
                reduce_variable,
                iterated_variable,
                iterable_expr,
                init_value_expr,
                yield_expr,
                inferred_type,
                source_span,
                ..
            } => internal::handle_list_reduce(
                reduce_variable,
                iterated_variable,
                iterable_expr,
                init_value_expr,
                yield_expr,
                inferred_type,
                &mut inferred_expr_stack,
                source_span,
            ),

            Expr::Range {
                range, source_span, ..
            } => {
                internal::handle_range(range, source_span, &mut inferred_expr_stack);
            }
        }
    }

    inferred_expr_stack.pop_front().ok_or(
        CustomError {
            expr: expr.clone(),
            message: "could not infer type".to_string(),
            help_message: vec![],
        }
        .into(),
    )
}

mod internal {
    use crate::call_type::{CallType, InstanceCreationType};

    use crate::generic_type_parameter::GenericTypeParameter;
    use crate::rib_source_span::SourceSpan;
    use crate::rib_type_error::RibTypeError;
    use crate::type_inference::type_hint::TypeHint;
    use crate::type_refinement::precise_types::{ListType, RangeType, RecordType};
    use crate::type_refinement::TypeRefinement;
    use crate::{
        ActualType, ExpectedType, Expr, GetTypeHint, InferredNumber, InferredType, MatchArm, Path,
        Range, TypeMismatchError, TypeName, VariableId,
    };

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
        inferred_expr_stack: &mut VecDeque<Expr>,
        source_span: &SourceSpan,
    ) {
        let yield_expr_inferred = inferred_expr_stack
            .pop_front()
            .unwrap_or(current_yield_expr.clone());
        let iterable_expr_inferred = inferred_expr_stack
            .pop_front()
            .unwrap_or(current_iterable_expr.clone());

        let list_expr = InferredType::List(Box::new(yield_expr_inferred.inferred_type()));
        let comprehension_type = current_comprehension_type.merge(list_expr);

        inferred_expr_stack.push_front(
            Expr::list_comprehension_typed(
                variable_id.clone(),
                iterable_expr_inferred,
                yield_expr_inferred,
                comprehension_type,
            )
            .with_source_span(source_span.clone()),
        );
    }

    pub(crate) fn handle_list_reduce(
        reduce_variable: &VariableId,
        iterated_variable: &VariableId,
        iterable_expr: &Expr,
        initial_value_expr: &Expr,
        yield_expr: &Expr,
        reduce_type: &InferredType,
        inferred_expr_stack: &mut VecDeque<Expr>,
        source_span: &SourceSpan,
    ) {
        let new_yield_expr = inferred_expr_stack
            .pop_front()
            .unwrap_or(yield_expr.clone());
        let new_init_value_expr = inferred_expr_stack
            .pop_front()
            .unwrap_or(initial_value_expr.clone());
        let new_iterable_expr = inferred_expr_stack
            .pop_front()
            .unwrap_or(iterable_expr.clone());

        let new_reduce_type = reduce_type.merge(new_init_value_expr.inferred_type());

        inferred_expr_stack.push_front(
            Expr::typed_list_reduce(
                reduce_variable.clone(),
                iterated_variable.clone(),
                new_iterable_expr,
                new_init_value_expr,
                new_yield_expr,
                new_reduce_type,
            )
            .with_source_span(source_span.clone()),
        );
    }

    pub(crate) fn handle_tuple(tuple_elems: &[Expr], current_tuple_type: &mut InferredType) {
        let mut new_inferred_type = vec![];

        for current_tuple_elem in tuple_elems.iter().rev() {
            new_inferred_type.push(current_tuple_elem.inferred_type());
        }

        let new_tuple_type = InferredType::Tuple(new_inferred_type);

        *current_tuple_type = current_tuple_type.merge(new_tuple_type);
    }

    pub(crate) fn handle_select_field(
        select_from: &Expr,
        field: &str,
        current_field_type: &mut InferredType,
    ) -> Result<(), RibTypeError> {
        let selection_field_type = get_inferred_type_of_selected_field(select_from, field)?;

        *current_field_type = current_field_type.merge(selection_field_type);

        Ok(())
    }

    pub(crate) fn handle_select_index(
        select_from: &Expr,
        index: &Expr,
        current_select_index_type: &mut InferredType,
    ) -> Result<(), RibTypeError> {
        let selection_expr_inferred_type = select_from.inferred_type();

        // if select_from is not yet gone through any phase, we cannot guarantee
        // it is a list type, otherwise continue with the assumption that it is a record
        if !selection_expr_inferred_type.is_unknown() {
            let index_type = get_inferred_type_of_selection_dynamic(select_from, index)?;

            *current_select_index_type = current_select_index_type.merge(index_type);
        }

        Ok(())
    }

    pub(crate) fn handle_result_ok(ok_expr: &Expr, current_inferred_type: &mut InferredType) {
        let inferred_type_of_ok_expr = ok_expr.inferred_type();
        let result_type = InferredType::Result {
            ok: Some(Box::new(inferred_type_of_ok_expr)),
            error: None,
        };
        *current_inferred_type = current_inferred_type.merge(result_type);
    }

    pub(crate) fn handle_result_error(error_expr: &Expr, current_inferred_type: &mut InferredType) {
        let inferred_type_of_error_expr = error_expr.inferred_type();
        let result_type = InferredType::Result {
            ok: None,
            error: Some(Box::new(inferred_type_of_error_expr)),
        };

        *current_inferred_type = current_inferred_type.merge(result_type);
    }

    pub(crate) fn handle_option_some(some_expr: &Expr, inferred_type: &mut InferredType) {
        let inferred_type_of_some_expr = some_expr.inferred_type();
        let option_type = InferredType::Option(Box::new(inferred_type_of_some_expr));

        *inferred_type = inferred_type.merge(option_type);
    }

    pub(crate) fn handle_if_else(
        then_expr: &Expr,
        else_expr: &Expr,
        inferred_type: &mut InferredType,
    ) {
        let inferred_type_of_then_expr = then_expr.inferred_type();
        let inferred_type_of_else_expr = else_expr.inferred_type();

        *inferred_type =
            inferred_type.merge(inferred_type_of_then_expr.merge(inferred_type_of_else_expr));
    }

    pub fn handle_pattern_match(current_match_arms: &[MatchArm], inferred_type: &mut InferredType) {
        let mut arm_resolution_inferred_types = vec![];

        for arm in current_match_arms {
            let arm_inferred_type = arm.arm_resolution_expr.inferred_type();
            arm_resolution_inferred_types.push(arm_inferred_type);
        }

        let new_inferred_type = InferredType::all_of(arm_resolution_inferred_types);

        if let Some(new_inferred_type) = new_inferred_type {
            *inferred_type = inferred_type.merge(new_inferred_type)
        }
    }

    pub(crate) fn handle_multiple(expr_block: &Vec<Expr>, inferred_type: &mut InferredType) {
        let new_inferred_type = expr_block.last().map(|x| x.inferred_type());

        if let Some(new_inferred_type) = new_inferred_type {
            *inferred_type = inferred_type.merge(new_inferred_type);
        }
    }

    pub(crate) fn handle_math_op(
        original_math_expr: &Expr,
        lhs: &Expr,
        rhs: &Expr,
        result_type: &mut InferredType,
    ) -> Result<(), TypeMismatchError> {
        // If final result  is not resolved, while both lhs and rhs are resolved
        // then we expect the
        if result_type.un_resolved()
            && !rhs.inferred_type().un_resolved()
            && !lhs.inferred_type().un_resolved()
        {
            let right_number_type = get_number(rhs, original_math_expr)?;
            let left_number_type = get_number(lhs, original_math_expr)?;

            if right_number_type == left_number_type {
                *result_type = result_type.merge(InferredType::from(right_number_type.clone()));
            } else {
                return Err(TypeMismatchError {
                    expr_with_wrong_type: original_math_expr.clone(),
                    parent_expr: None,
                    expected_type: ExpectedType::Hint(TypeHint::Number),
                    actual_type: ActualType::Inferred(InferredType::from(right_number_type)),
                    field_path: Default::default(),
                    additional_error_detail: vec![
                        "type mismatch in mathematical expression: operands have incompatible types. "
                            .to_string(),
                    ],
                });
            }
        }

        Ok(())
    }

    fn get_number(number_expr: &Expr, math_op: &Expr) -> Result<InferredNumber, TypeMismatchError> {
        let rhs_type = number_expr.inferred_type();

        rhs_type.as_number().map_err(|_| TypeMismatchError {
            expr_with_wrong_type: number_expr.clone(),
            parent_expr: Some(math_op.clone()),
            expected_type: ExpectedType::Hint(TypeHint::Number),
            actual_type: ActualType::Inferred(rhs_type),
            field_path: Default::default(),
            additional_error_detail: vec![],
        })
    }

    pub(crate) fn handle_range(
        range: &Range,
        source_span: &SourceSpan,
        inferred_expr_stack: &mut VecDeque<Expr>,
    ) {
        match range {
            Range::Range { from, to } => {
                let right = inferred_expr_stack
                    .pop_front()
                    .unwrap_or(to.deref().clone());
                let left = inferred_expr_stack
                    .pop_front()
                    .unwrap_or(from.deref().clone());

                let new_inferred_type = InferredType::Range {
                    from: Box::new(left.inferred_type()),
                    to: Some(Box::new(right.inferred_type())),
                };
                let new_range = Expr::range(left, right)
                    .with_inferred_type(new_inferred_type)
                    .with_source_span(source_span.clone());

                inferred_expr_stack.push_front(new_range);
            }
            Range::RangeInclusive { from, to } => {
                let right = inferred_expr_stack
                    .pop_front()
                    .unwrap_or(to.deref().clone());
                let left = inferred_expr_stack
                    .pop_front()
                    .unwrap_or(from.deref().clone());

                let new_inferred_type = InferredType::Range {
                    from: Box::new(left.inferred_type()),
                    to: Some(Box::new(right.inferred_type())),
                };

                let new_range = Expr::range_inclusive(left, right)
                    .with_inferred_type(new_inferred_type)
                    .with_source_span(source_span.clone());

                inferred_expr_stack.push_front(new_range);
            }
            Range::RangeFrom { from } => {
                let left = inferred_expr_stack
                    .pop_front()
                    .unwrap_or(from.deref().clone());

                let new_inferred_type = InferredType::Range {
                    from: Box::new(left.inferred_type()),
                    to: None,
                };

                let new_range = Expr::range_from(left)
                    .with_inferred_type(new_inferred_type)
                    .with_source_span(source_span.clone());

                inferred_expr_stack.push_front(new_range);
            }
        }
    }

    pub(crate) fn handle_call(
        call_type: &CallType,
        generic_type_parameter: Option<GenericTypeParameter>,
        arguments: &[Expr],
        inferred_type: &InferredType,
        inferred_expr_stack: &mut VecDeque<Expr>,
        source_span: &SourceSpan,
    ) {
        let mut new_arg_exprs = vec![];

        for expr in arguments.iter().rev() {
            let expr = inferred_expr_stack.pop_front().unwrap_or(expr.clone());
            new_arg_exprs.push(expr);
        }

        new_arg_exprs.reverse();

        match call_type {
            CallType::Function {
                function_name,
                worker,
            } => {
                let new_worker = worker.as_ref().map(|worker| {
                    inferred_expr_stack
                        .pop_front()
                        .unwrap_or(worker.deref().clone())
                });

                let mut function_name = function_name.clone();

                let resource_params = function_name.function.raw_resource_params_mut();

                if let Some(resource_params) = resource_params {
                    let mut new_resource_params = vec![];
                    for expr in resource_params.iter().rev() {
                        let expr = inferred_expr_stack.pop_front().unwrap_or(expr.clone());
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

                let mut worker_in_inferred_type = None;

                if let InferredType::Instance { instance_type } = inferred_type {
                    let worker = instance_type.worker_name();
                    if let Some(worker) = worker {
                        worker_in_inferred_type = Some(
                            inferred_expr_stack
                                .pop_front()
                                .unwrap_or(worker.deref().clone()),
                        )
                    }
                };

                let new_inferred_type = match worker_in_inferred_type {
                    Some(worker) => match inferred_type {
                        InferredType::Instance { instance_type } => {
                            let mut new_instance_type = instance_type.clone();
                            new_instance_type.set_worker_name(worker);

                            InferredType::Instance {
                                instance_type: new_instance_type,
                            }
                        }

                        _ => inferred_type.clone(),
                    },
                    None => inferred_type.clone(),
                };

                // worker in the call type
                let new_call = Expr::call(
                    CallType::Function {
                        function_name,
                        worker: new_worker.map(Box::new),
                    },
                    None,
                    new_arg_exprs,
                )
                .with_inferred_type(new_inferred_type)
                .with_source_span(source_span.clone());

                inferred_expr_stack.push_front(new_call);
            }

            CallType::InstanceCreation(instance_creation) => {
                let worker_name = instance_creation.worker_name();

                if let Some(worker_name) = worker_name {
                    let worker_name = inferred_expr_stack.pop_front().unwrap_or(worker_name);

                    let new_instance_creation = match instance_creation {
                        InstanceCreationType::Worker { .. } => InstanceCreationType::Worker {
                            worker_name: Some(Box::new(worker_name.clone())),
                        },
                        InstanceCreationType::Resource { resource_name, .. } => {
                            InstanceCreationType::Resource {
                                worker_name: Some(Box::new(worker_name.clone())),
                                resource_name: resource_name.clone(),
                            }
                        }
                    };

                    let new_inferred_type = match inferred_type {
                        InferredType::Instance { instance_type } => {
                            instance_type.worker().map(|worker_expr| {
                                let inferred_worker_expr = inferred_expr_stack
                                    .pop_front()
                                    .unwrap_or_else(|| worker_expr.clone());

                                let mut new_instance_type = instance_type.clone();
                                new_instance_type.set_worker_name(inferred_worker_expr);

                                InferredType::Instance {
                                    instance_type: new_instance_type,
                                }
                            })
                        }
                        _ => None,
                    };

                    let new_call = Expr::call(
                        CallType::InstanceCreation(new_instance_creation.clone()),
                        generic_type_parameter,
                        new_arg_exprs,
                    )
                    .with_inferred_type(new_inferred_type.unwrap_or_else(|| inferred_type.clone()))
                    .with_source_span(source_span.clone());
                    inferred_expr_stack.push_front(new_call);
                } else {
                    let new_call = Expr::call(
                        CallType::InstanceCreation(instance_creation.clone()),
                        generic_type_parameter,
                        new_arg_exprs,
                    )
                    .with_inferred_type(inferred_type.clone())
                    .with_source_span(source_span.clone());

                    inferred_expr_stack.push_front(new_call);
                }
            }

            CallType::VariantConstructor(str) => {
                let new_call = Expr::call(
                    CallType::VariantConstructor(str.clone()),
                    None,
                    new_arg_exprs,
                )
                .with_inferred_type(inferred_type.clone())
                .with_source_span(source_span.clone());
                inferred_expr_stack.push_front(new_call);
            }

            CallType::EnumConstructor(str) => {
                let new_call =
                    Expr::call(CallType::EnumConstructor(str.clone()), None, new_arg_exprs)
                        .with_inferred_type(inferred_type.clone())
                        .with_source_span(source_span.clone());
                inferred_expr_stack.push_front(new_call);
            }
        }
    }

    pub(crate) fn handle_length(
        original_length_expr: &Expr,
        current_length_type: &InferredType,
        inferred_expr_stack: &mut VecDeque<Expr>,
        source_span: &SourceSpan,
    ) {
        let expr = inferred_expr_stack
            .pop_front()
            .unwrap_or(original_length_expr.clone());
        let new_length = Expr::length(expr)
            .with_inferred_type(current_length_type.clone())
            .with_source_span(source_span.clone());
        inferred_expr_stack.push_front(new_length);
    }

    pub(crate) fn handle_unwrap(
        expr: &Expr,
        current_inferred_type: &InferredType,
        inferred_expr_stack: &mut VecDeque<Expr>,
        source_span: &SourceSpan,
    ) {
        let expr = inferred_expr_stack.pop_front().unwrap_or(expr.clone());
        let new_unwrap = expr
            .unwrap()
            .with_inferred_type(current_inferred_type.merge(expr.inferred_type()))
            .with_source_span(source_span.clone());
        inferred_expr_stack.push_front(new_unwrap);
    }

    pub(crate) fn handle_get_tag(
        expr: &Expr,
        current_inferred_type: &InferredType,
        inferred_expr_stack: &mut VecDeque<Expr>,
        source_span: &SourceSpan,
    ) {
        let expr = inferred_expr_stack.pop_front().unwrap_or(expr.clone());
        let new_get_tag = Expr::get_tag(expr.clone())
            .with_inferred_type(current_inferred_type.merge(expr.inferred_type()))
            .with_source_span(source_span.clone());
        inferred_expr_stack.push_front(new_get_tag);
    }

    pub(crate) fn handle_sequence(
        current_expr_list: &[Expr],
        current_inferred_type: &InferredType,
        inferred_expr_stack: &mut VecDeque<Expr>,
        type_annotation: &Option<TypeName>,
        source_span: &SourceSpan,
    ) {
        let mut new_exprs = vec![];

        for expr in current_expr_list.iter().rev() {
            let expr = inferred_expr_stack.pop_front().unwrap_or(expr.clone());
            new_exprs.push(expr);
        }

        new_exprs.reverse();

        let new_sequence = {
            if let Some(first_expr) = new_exprs.clone().first() {
                Expr::sequence(new_exprs, type_annotation.clone())
                    .with_inferred_type(
                        current_inferred_type
                            .clone()
                            .merge(InferredType::List(Box::new(first_expr.inferred_type()))),
                    )
                    .with_source_span(source_span.clone())
            } else {
                Expr::sequence(new_exprs, type_annotation.clone())
                    .with_inferred_type(current_inferred_type.clone())
                    .with_source_span(source_span.clone())
            }
        };

        inferred_expr_stack.push_front(new_sequence);
    }

    pub(crate) fn handle_record(
        current_expr_list: &[(String, Box<Expr>)],
        current_inferred_type: &InferredType,
        inferred_expr_stack: &mut VecDeque<Expr>,
        source_span: &SourceSpan,
    ) {
        let mut ordered_types = vec![];
        let mut new_exprs = vec![];

        for (field, expr) in current_expr_list.iter().rev() {
            let expr: Expr = inferred_expr_stack
                .pop_front()
                .unwrap_or(expr.deref().clone());
            ordered_types.push((field.clone(), expr.inferred_type()));
            new_exprs.push((field.clone(), expr.clone()));
        }

        new_exprs.reverse();
        ordered_types.reverse();

        let new_record_type = InferredType::Record(ordered_types);

        let merged_record_type = current_inferred_type.merge(new_record_type);

        let new_record = Expr::record(new_exprs.to_vec())
            .with_inferred_type(merged_record_type)
            .with_source_span(source_span.clone());
        inferred_expr_stack.push_front(new_record);
    }

    pub(crate) fn get_inferred_type_of_selected_field(
        select_from: &Expr,
        field: &str,
    ) -> Result<InferredType, RibTypeError> {
        let select_from_inferred_type = select_from.inferred_type();
        let refined_record = RecordType::refine(&select_from_inferred_type).ok_or({
            TypeMismatchError {
                expr_with_wrong_type: select_from.clone(),
                parent_expr: None,
                expected_type: ExpectedType::Hint(TypeHint::Record(None)),
                actual_type: ActualType::Inferred(select_from_inferred_type.clone()),
                field_path: Path::default(),
                additional_error_detail: vec![format!(
                    "cannot select {} from {} since it is not a record type. Found: {}",
                    field,
                    select_from,
                    select_from_inferred_type.get_type_hint()
                )],
            }
        })?;

        Ok(refined_record.inner_type_by_name(field))
    }

    fn get_inferred_type_of_selection_dynamic(
        select_from: &Expr,
        index: &Expr,
    ) -> Result<InferredType, RibTypeError> {
        let select_from_type = select_from.inferred_type();
        let select_index_type = index.inferred_type();

        let refined_list = ListType::refine(&select_from_type).ok_or({
            TypeMismatchError {
                expr_with_wrong_type: select_from.clone(),
                parent_expr: None,
                expected_type: ExpectedType::Hint(TypeHint::List(None)),
                actual_type: ActualType::Inferred(select_from_type.clone()),
                field_path: Default::default(),
                additional_error_detail: vec![format!(
                    "cannot get index {} from {} since it is not a list type. Found: {}",
                    index,
                    select_from,
                    select_from_type.get_type_hint()
                )],
            }
        })?;

        let list_type = refined_list.inner_type();

        if select_index_type.contains_only_number() {
            Ok(list_type)
        } else {
            let range = RangeType::refine(&select_index_type).ok_or({
                TypeMismatchError {
                    expr_with_wrong_type: select_from.clone(),
                    parent_expr: None,
                    expected_type: ExpectedType::Hint(TypeHint::Number),
                    actual_type: ActualType::Inferred(select_index_type.clone()),
                    field_path: Default::default(),
                    additional_error_detail: vec![format!(
                        "cannot get index {} from {} since it is neither a number type or a range type. found: {}",
                        index, select_from, select_index_type.get_type_hint()
                    )],
                }
            })?;

            Ok(InferredType::List(Box::new(list_type)))
        }
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
        ArmPattern, Expr, ExprVisitor, FunctionTypeRegistry, InferredType, MatchArm, VariableId,
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
            Expr::identifier_global("foo", None).merge_inferred_type(InferredType::Record(vec![(
                "foo".to_string(),
                InferredType::Record(vec![("bar".to_string(), InferredType::U64)]),
            )]));
        let select_expr = Expr::select_field(record_identifier, "foo", None);
        let expr = Expr::select_field(select_expr, "bar", None);
        let new_expr = expr.pull_types_up().unwrap();
        assert_eq!(new_expr.inferred_type(), InferredType::U64);
    }

    #[test]
    pub fn test_pull_up_for_select_index() {
        let identifier = Expr::identifier_global("foo", None)
            .merge_inferred_type(InferredType::List(Box::new(InferredType::U64)));
        let expr = Expr::select_index(identifier.clone(), Expr::number(BigDecimal::from(0)));
        let new_expr = expr.pull_types_up().unwrap();
        let expected = Expr::select_index(identifier, Expr::number(BigDecimal::from(0)))
            .merge_inferred_type(InferredType::U64);
        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_sequence() {
        let elems = vec![
            Expr::number_inferred(BigDecimal::from(1), None, InferredType::U64),
            Expr::number_inferred(BigDecimal::from(2), None, InferredType::U64),
        ];

        let expr = Expr::sequence(elems.clone(), None).with_inferred_type(InferredType::Unknown);
        let new_expr = expr.pull_types_up().unwrap();

        assert_eq!(
            new_expr,
            Expr::sequence(elems, None)
                .with_inferred_type(InferredType::List(Box::new(InferredType::U64)))
        );
    }

    #[test]
    pub fn test_pull_up_for_tuple() {
        let expr = Expr::tuple(vec![
            Expr::literal("foo"),
            Expr::number_inferred(BigDecimal::from(1), None, InferredType::U64),
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
                Expr::number_inferred(BigDecimal::from(1), None, InferredType::U64),
            ),
            (
                "bar".to_string(),
                Expr::number_inferred(BigDecimal::from(2), None, InferredType::U32),
            ),
        ];
        let expr = Expr::record(elems.clone()).with_inferred_type(InferredType::Record(vec![
            ("foo".to_string(), InferredType::Unknown),
            ("bar".to_string(), InferredType::Unknown),
        ]));
        let new_expr = expr.pull_types_up().unwrap();

        assert_eq!(
            new_expr,
            Expr::record(elems).with_inferred_type(InferredType::AllOf(vec![
                InferredType::Record(vec![
                    ("foo".to_string(), InferredType::U64),
                    ("bar".to_string(), InferredType::U32)
                ]),
                InferredType::Record(vec![
                    ("foo".to_string(), InferredType::Unknown),
                    ("bar".to_string(), InferredType::Unknown)
                ])
            ]))
        );
    }

    #[test]
    pub fn test_pull_up_for_concat() {
        let expr = Expr::concat(vec![Expr::literal("foo"), Expr::literal("bar")]);
        let new_expr = expr.pull_types_up().unwrap();
        let expected = Expr::concat(vec![Expr::literal("foo"), Expr::literal("bar")])
            .with_inferred_type(InferredType::Str);
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
        let inner1 = Expr::identifier_global("foo", None)
            .merge_inferred_type(InferredType::List(Box::new(InferredType::U64)));

        let select_index1 = Expr::select_index(inner1.clone(), Expr::number(BigDecimal::from(0)));
        let select_index2 = Expr::select_index(inner1, Expr::number(BigDecimal::from(1)));

        let inner2 = Expr::identifier_global("bar", None)
            .merge_inferred_type(InferredType::List(Box::new(InferredType::U64)));

        let select_index3 = Expr::select_index(inner2.clone(), Expr::number(BigDecimal::from(0)));
        let select_index4 = Expr::select_index(inner2, Expr::number(BigDecimal::from(1)));

        let expr = Expr::cond(
            Expr::greater_than(select_index1.clone(), select_index2.clone()),
            select_index3.clone(),
            select_index4.clone(),
        );

        let new_expr = expr.pull_types_up().unwrap();
        let expected = Expr::cond(
            Expr::greater_than(
                Expr::select_index(
                    Expr::identifier_global("foo", None)
                        .with_inferred_type(InferredType::List(Box::new(InferredType::U64))),
                    Expr::number(BigDecimal::from(0)),
                )
                .with_inferred_type(InferredType::U64),
                Expr::select_index(
                    Expr::identifier_global("foo", None)
                        .with_inferred_type(InferredType::List(Box::new(InferredType::U64))),
                    Expr::number(BigDecimal::from(1)),
                )
                .with_inferred_type(InferredType::U64),
            )
            .with_inferred_type(InferredType::Bool),
            Expr::select_index(
                Expr::identifier_global("bar", None)
                    .with_inferred_type(InferredType::List(Box::new(InferredType::U64))),
                Expr::number(BigDecimal::from(0)),
            )
            .with_inferred_type(InferredType::U64),
            Expr::select_index(
                Expr::identifier_global("bar", None)
                    .with_inferred_type(InferredType::List(Box::new(InferredType::U64))),
                Expr::number(BigDecimal::from(1)),
            )
            .with_inferred_type(InferredType::U64),
        )
        .with_inferred_type(InferredType::U64);
        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_greater_than() {
        let inner =
            Expr::identifier_global("foo", None).merge_inferred_type(InferredType::Record(vec![
                ("bar".to_string(), InferredType::Str),
                ("baz".to_string(), InferredType::U64),
            ]));

        let select_field1 = Expr::select_field(inner.clone(), "bar", None);
        let select_field2 = Expr::select_field(inner, "baz", None);
        let expr = Expr::greater_than(select_field1.clone(), select_field2.clone());

        let new_expr = expr.pull_types_up().unwrap();

        let expected = Expr::greater_than(
            select_field1.merge_inferred_type(InferredType::Str),
            select_field2.merge_inferred_type(InferredType::U64),
        )
        .merge_inferred_type(InferredType::Bool);
        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_greater_than_or_equal_to() {
        let inner = Expr::identifier_global("foo", None)
            .merge_inferred_type(InferredType::List(Box::new(InferredType::U64)));

        let select_index1 = Expr::select_index(inner.clone(), Expr::number(BigDecimal::from(0)));
        let select_index2 = Expr::select_index(inner, Expr::number(BigDecimal::from(1)));
        let expr = Expr::greater_than_or_equal_to(select_index1.clone(), select_index2.clone());

        let new_expr = expr.pull_types_up().unwrap();

        let expected = Expr::greater_than_or_equal_to(
            select_index1.merge_inferred_type(InferredType::U64),
            select_index2.merge_inferred_type(InferredType::U64),
        )
        .merge_inferred_type(InferredType::Bool);
        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_less_than_or_equal_to() {
        let record_type = InferredType::Record(vec![
            ("bar".to_string(), InferredType::Str),
            ("baz".to_string(), InferredType::U64),
        ]);

        let inner = Expr::identifier_global("foo", None)
            .merge_inferred_type(InferredType::List(Box::new(record_type.clone())));

        let select_field_from_first = Expr::select_field(
            Expr::select_index(inner.clone(), Expr::number(BigDecimal::from(0))),
            "bar",
            None,
        );
        let select_field_from_second = Expr::select_field(
            Expr::select_index(inner.clone(), Expr::number(BigDecimal::from(1))),
            "baz",
            None,
        );
        let expr = Expr::less_than_or_equal_to(
            select_field_from_first.clone(),
            select_field_from_second.clone(),
        );

        let new_expr = expr.pull_types_up().unwrap();

        let new_select_field_from_first = Expr::select_field(
            Expr::select_index(inner.clone(), Expr::number(BigDecimal::from(0)))
                .merge_inferred_type(record_type.clone()),
            "bar",
            None,
        )
        .merge_inferred_type(InferredType::Str);

        let new_select_field_from_second = Expr::select_field(
            Expr::select_index(inner.clone(), Expr::number(BigDecimal::from(1)))
                .merge_inferred_type(record_type),
            "baz",
            None,
        )
        .merge_inferred_type(InferredType::U64);

        let expected =
            Expr::less_than_or_equal_to(new_select_field_from_first, new_select_field_from_second)
                .merge_inferred_type(InferredType::Bool);

        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_equal_to() {
        let expr = Expr::equal_to(
            Expr::number(BigDecimal::from(1)),
            Expr::number(BigDecimal::from(2)),
        );
        let new_expr = expr.pull_types_up().unwrap();
        assert_eq!(new_expr.inferred_type(), InferredType::Bool);
    }

    #[test]
    pub fn test_pull_up_for_less_than() {
        let expr = Expr::less_than(
            Expr::number(BigDecimal::from(1)),
            Expr::number(BigDecimal::from(2)),
        );
        let new_expr = expr.pull_types_up().unwrap();
        assert_eq!(new_expr.inferred_type(), InferredType::Bool);
    }

    #[test]
    pub fn test_pull_up_for_call() {
        let expr = Expr::call_worker_function(
            DynamicParsedFunctionName::parse("global_fn").unwrap(),
            None,
            None,
            vec![Expr::number(BigDecimal::from(1))],
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
        expr.infer_types_initial_phase(&function_registry, &vec![])
            .unwrap();
        expr.infer_all_identifiers();
        let new_expr = expr.pull_types_up().unwrap();

        let expected = Expr::expr_block(vec![
            Expr::let_binding_with_variable_id(
                VariableId::local("input", 0),
                Expr::record(vec![
                    (
                        "foo".to_string(),
                        Expr::literal("afs").with_inferred_type(InferredType::Str),
                    ),
                    (
                        "bar".to_string(),
                        Expr::literal("al").with_inferred_type(InferredType::Str),
                    ),
                ])
                .with_inferred_type(InferredType::Record(vec![
                    ("foo".to_string(), InferredType::Str),
                    ("bar".to_string(), InferredType::Str),
                ])),
                None,
            ),
            Expr::call(
                CallType::function_without_worker(DynamicParsedFunctionName {
                    site: PackagedInterface {
                        namespace: "golem".to_string(),
                        package: "it".to_string(),
                        interface: "api".to_string(),
                        version: None,
                    },
                    function: IndexedResourceMethod {
                        resource: "cart".to_string(),
                        resource_params: vec![Expr::select_field(
                            Expr::identifier_local("input", 0, None).with_inferred_type(
                                InferredType::Record(vec![
                                    ("foo".to_string(), InferredType::Str),
                                    ("bar".to_string(), InferredType::Str),
                                ]),
                            ),
                            "foo",
                            None,
                        )
                        .with_inferred_type(InferredType::Str)],
                        method: "checkout".to_string(),
                    },
                }),
                None,
                vec![],
            ),
        ]);

        assert_eq!(new_expr, expected);
    }

    #[test]
    pub fn test_pull_up_for_unwrap() {
        let mut number = Expr::number(BigDecimal::from(1));
        number.with_inferred_type_mut(InferredType::F64);
        let expr = Expr::option(Some(number)).unwrap();
        let expr = expr.pull_types_up().unwrap();
        assert_eq!(
            expr.inferred_type(),
            InferredType::Option(Box::new(InferredType::F64))
        );
    }

    #[test]
    pub fn test_pull_up_for_tag() {
        let mut number = Expr::number(BigDecimal::from(1));
        number.with_inferred_type_mut(InferredType::F64);
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
                Expr::identifier_global("foo", None).merge_inferred_type(InferredType::Record(
                    vec![("bar".to_string(), InferredType::Str)],
                )),
                "bar",
                None,
            ),
            vec![
                MatchArm {
                    arm_pattern: ArmPattern::Constructor(
                        "cons1".to_string(),
                        vec![ArmPattern::Literal(Box::new(Expr::select_field(
                            Expr::identifier_global("foo", None).merge_inferred_type(
                                InferredType::Record(vec![("bar".to_string(), InferredType::Str)]),
                            ),
                            "bar",
                            None,
                        )))],
                    ),
                    arm_resolution_expr: Box::new(Expr::select_field(
                        Expr::identifier_global("baz", None).merge_inferred_type(
                            InferredType::Record(vec![("qux".to_string(), InferredType::Str)]),
                        ),
                        "qux",
                        None,
                    )),
                },
                MatchArm {
                    arm_pattern: ArmPattern::Constructor(
                        "cons2".to_string(),
                        vec![ArmPattern::Literal(Box::new(Expr::select_field(
                            Expr::identifier_global("quux", None).merge_inferred_type(
                                InferredType::Record(vec![(
                                    "corge".to_string(),
                                    InferredType::Str,
                                )]),
                            ),
                            "corge",
                            None,
                        )))],
                    ),
                    arm_resolution_expr: Box::new(Expr::select_field(
                        Expr::identifier_global("grault", None).merge_inferred_type(
                            InferredType::Record(vec![("garply".to_string(), InferredType::Str)]),
                        ),
                        "garply",
                        None,
                    )),
                },
            ],
        );
        let new_expr = expr.pull_types_up().unwrap();
        let expected = internal::expected_pattern_match();
        assert_eq!(new_expr, expected);
    }

    fn pull_up(expr: &mut Expr) {
        let mut expr = ExprVisitor::bottom_up(expr);
        while let Some(expr) = expr.pop_front() {
            match expr {
                Expr::Identifier {
                    variable_id,
                    inferred_type,
                    source_span,
                    ..
                } => {
                    *inferred_type = InferredType::U32;
                }

                Expr::Sequence {
                    exprs,
                    inferred_type,
                    source_span,
                    type_annotation,
                    ..
                } => {
                    let mut new_inferred_type = vec![];
                    for expr in exprs.iter() {
                        let inferred_type = expr.inferred_type();
                        new_inferred_type.push(inferred_type);
                    }

                    *inferred_type = InferredType::List(Box::new(new_inferred_type[0].clone()));
                }

                Expr::Tuple {
                    exprs,
                    inferred_type,
                    source_span,
                    ..
                } => {
                    let mut new_inferred_type = vec![];
                    for expr in exprs.iter() {
                        let inferred_type = expr.inferred_type();
                        new_inferred_type.push(inferred_type);
                    }

                    *inferred_type = InferredType::Tuple(new_inferred_type);
                }

                Expr::ExprBlock {
                    exprs,
                    inferred_type,
                    source_span,
                    ..
                } => {
                    let new_inferred_type = exprs
                        .last()
                        .map_or(InferredType::Unknown, |last_expr| last_expr.inferred_type());

                    *inferred_type = new_inferred_type
                }

                _ => {}
            }
        }
    }

    #[test]
    fn test_tuple_pull_up() {
        let expr = Expr::sequence(
            vec![
                Expr::identifier_global("foo", None),
                Expr::identifier_global("bar", None),
            ],
            None,
        );

        let mut block = Expr::expr_block(vec![expr.clone(), expr.clone()]);

        pull_up(&mut block);

        dbg!(block);
        assert!(false)
    }

    mod internal {
        use crate::{ArmPattern, Expr, InferredType, MatchArm};

        pub(crate) fn expected_pattern_match() -> Expr {
            Expr::pattern_match(
                Expr::select_field(
                    Expr::identifier_global("foo", None).with_inferred_type(InferredType::Record(
                        vec![("bar".to_string(), InferredType::Str)],
                    )),
                    "bar",
                    None,
                )
                .with_inferred_type(InferredType::Str),
                vec![
                    MatchArm {
                        arm_pattern: ArmPattern::Constructor(
                            "cons1".to_string(),
                            vec![ArmPattern::Literal(Box::new(
                                Expr::select_field(
                                    Expr::identifier_global("foo", None).with_inferred_type(
                                        InferredType::Record(vec![(
                                            "bar".to_string(),
                                            InferredType::Str,
                                        )]),
                                    ),
                                    "bar",
                                    None,
                                )
                                .with_inferred_type(InferredType::Str),
                            ))],
                        ),
                        arm_resolution_expr: Box::new(
                            Expr::select_field(
                                Expr::identifier_global("baz", None).with_inferred_type(
                                    InferredType::Record(vec![(
                                        "qux".to_string(),
                                        InferredType::Str,
                                    )]),
                                ),
                                "qux",
                                None,
                            )
                            .with_inferred_type(InferredType::Str),
                        ),
                    },
                    MatchArm {
                        arm_pattern: ArmPattern::Constructor(
                            "cons2".to_string(),
                            vec![ArmPattern::Literal(Box::new(
                                Expr::select_field(
                                    Expr::identifier_global("quux", None).with_inferred_type(
                                        InferredType::Record(vec![(
                                            "corge".to_string(),
                                            InferredType::Str,
                                        )]),
                                    ),
                                    "corge",
                                    None,
                                )
                                .with_inferred_type(InferredType::Str),
                            ))],
                        ),
                        arm_resolution_expr: Box::new(
                            Expr::select_field(
                                Expr::identifier_global("grault", None).with_inferred_type(
                                    InferredType::Record(vec![(
                                        "garply".to_string(),
                                        InferredType::Str,
                                    )]),
                                ),
                                "garply",
                                None,
                            )
                            .with_inferred_type(InferredType::Str),
                        ),
                    },
                ],
            )
            .with_inferred_type(InferredType::Str)
        }
    }
}
