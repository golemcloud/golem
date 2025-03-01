use crate::rib_compilation_error::RibCompilationError;
use crate::type_checker::Path;
use crate::{Expr, InferredType, VariableId};

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

pub fn bind_global_variable_types(
    expr: &Expr,
    type_pecs: &Vec<GlobalVariableTypeSpec>,
) -> Result<Expr, RibCompilationError> {
    let mut result_expr = expr.clone();

    for spec in type_pecs {
        result_expr = internal::bind_global_variable_types(&result_expr, spec)?;
    }

    Ok(result_expr)
}

mod internal {
    use crate::call_type::{CallType, InstanceCreationType};

    use crate::generic_type_parameter::GenericTypeParameter;
    use crate::rib_compilation_error::RibCompilationError;
    use crate::rib_source_span::SourceSpan;
    use crate::type_checker::{Path, PathElem};
    use crate::{
        CustomError, Expr, GlobalVariableTypeSpec, InferredType, MatchArm, Range, TypeName,
        VariableId,
    };
    use std::collections::VecDeque;
    use std::ops::Deref;

    pub(crate) fn bind_global_variable_types(
        expr: &Expr,
        type_spec: &GlobalVariableTypeSpec,
    ) -> Result<Expr, RibCompilationError> {
        let mut path = type_spec.path.clone();

        let mut expr_queue = VecDeque::new();

        make_expr_nodes_queue(expr, &mut expr_queue);

        let mut temp_stack = VecDeque::new();

        while let Some(expr) = expr_queue.pop_back() {
            match expr {
                expr @ Expr::Identifier {
                    variable_id,
                    type_annotation,
                    source_span,
                    ..
                } => {
                    if variable_id == &type_spec.variable_id {
                        if path.is_empty() {
                            let continue_traverse = matches!(expr_queue.back(), Some(Expr::SelectField { expr: expr0, .. }) if expr0.as_ref() == expr);

                            if continue_traverse {
                                temp_stack.push_front((expr.clone(), true));
                            } else {
                                temp_stack.push_front((
                                    Expr::Identifier {
                                        variable_id: variable_id.clone(),
                                        type_annotation: type_annotation.clone(),
                                        inferred_type: type_spec.inferred_type.clone(),
                                        source_span: source_span.clone(),
                                    },
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

                outer @ Expr::SelectField {
                    expr,
                    field,
                    type_annotation,
                    inferred_type,
                    source_span,
                } => {
                    let continue_search = matches!(expr_queue.back(), Some(Expr::SelectField { expr: expr0, ..}) if expr0.as_ref() == outer);

                    handle_select_field(
                        expr,
                        field,
                        continue_search,
                        inferred_type,
                        &mut temp_stack,
                        &mut path,
                        &type_spec.inferred_type,
                        type_annotation,
                        source_span,
                    )?;
                }

                Expr::Tuple {
                    exprs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_tuple(
                        exprs,
                        inferred_type,
                        &mut temp_stack,
                        source_span,
                        type_annotation,
                    );
                }

                expr @ Expr::Flags { .. } => {
                    temp_stack.push_front((expr.clone(), false));
                }

                Expr::SelectIndex {
                    expr,
                    index,
                    type_annotation,
                    inferred_type,
                    source_span,
                } => {
                    handle_select_index(
                        expr,
                        index,
                        inferred_type,
                        &mut temp_stack,
                        type_annotation,
                        source_span,
                    )?;
                }

                Expr::SelectDynamic {
                    expr,
                    index,
                    type_annotation,
                    inferred_type,
                    source_span,
                } => {
                    handle_select_dynamic(
                        expr,
                        index,
                        inferred_type,
                        &mut temp_stack,
                        type_annotation,
                        source_span,
                    )?;
                }

                Expr::Result {
                    expr: Ok(_),
                    type_annotation,
                    inferred_type,
                    source_span,
                } => {
                    handle_result_ok(
                        expr,
                        inferred_type,
                        &mut temp_stack,
                        type_annotation,
                        source_span,
                    );
                }

                Expr::Result {
                    expr: Err(_),
                    type_annotation,
                    inferred_type,
                    source_span,
                } => {
                    handle_result_error(
                        expr,
                        inferred_type,
                        &mut temp_stack,
                        type_annotation,
                        source_span,
                    );
                }

                Expr::Option {
                    expr: Some(expr),
                    type_annotation,
                    inferred_type,
                    source_span,
                } => {
                    handle_option_some(
                        expr,
                        inferred_type,
                        &mut temp_stack,
                        type_annotation,
                        source_span,
                    );
                }

                Expr::Option {
                    type_annotation,
                    inferred_type,
                    source_span,
                    ..
                } => {
                    temp_stack.push_front((
                        Expr::Option {
                            expr: None,
                            type_annotation: type_annotation.clone(),
                            inferred_type: inferred_type.clone(),
                            source_span: source_span.clone(),
                        },
                        false,
                    ));
                }

                Expr::Cond {
                    cond,
                    lhs,
                    rhs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_if_else(
                        cond,
                        lhs,
                        rhs,
                        inferred_type,
                        &mut temp_stack,
                        source_span,
                        type_annotation,
                    );
                }

                //
                Expr::PatternMatch {
                    predicate,
                    match_arms,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_pattern_match(
                        predicate,
                        match_arms,
                        inferred_type,
                        &mut temp_stack,
                        source_span,
                        type_annotation,
                    );
                }

                Expr::Concat {
                    exprs,
                    source_span,
                    type_annotation,
                    ..
                } => {
                    handle_concat(exprs, &mut temp_stack, source_span, type_annotation);
                }

                Expr::ExprBlock {
                    exprs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_multiple(
                        exprs,
                        inferred_type,
                        &mut temp_stack,
                        source_span,
                        type_annotation,
                    );
                }

                Expr::Not {
                    inferred_type,
                    source_span,
                    type_annotation,
                    ..
                } => {
                    handle_not(
                        expr,
                        inferred_type,
                        &mut temp_stack,
                        source_span,
                        type_annotation,
                    );
                }

                Expr::GreaterThan {
                    lhs,
                    rhs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_comparison_op(lhs, rhs, inferred_type, &mut temp_stack, |a, b, c| {
                        Expr::GreaterThan {
                            lhs: a,
                            rhs: b,
                            inferred_type: c,
                            source_span: source_span.clone(),
                            type_annotation: type_annotation.clone(),
                        }
                    });
                }

                Expr::GreaterThanOrEqualTo {
                    lhs,
                    rhs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_comparison_op(lhs, rhs, inferred_type, &mut temp_stack, |a, b, c| {
                        Expr::GreaterThanOrEqualTo {
                            lhs: a,
                            rhs: b,
                            inferred_type: c,
                            source_span: source_span.clone(),
                            type_annotation: type_annotation.clone(),
                        }
                    });
                }

                Expr::LessThanOrEqualTo {
                    lhs,
                    rhs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_comparison_op(lhs, rhs, inferred_type, &mut temp_stack, |a, b, c| {
                        Expr::LessThanOrEqualTo {
                            lhs: a,
                            rhs: b,
                            inferred_type: c,
                            source_span: source_span.clone(),
                            type_annotation: type_annotation.clone(),
                        }
                    });
                }
                Expr::Plus {
                    lhs,
                    rhs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_math_op(lhs, rhs, inferred_type, &mut temp_stack, |a, b, c| {
                        Expr::Plus {
                            lhs: a,
                            rhs: b,
                            inferred_type: c,
                            source_span: source_span.clone(),
                            type_annotation: type_annotation.clone(),
                        }
                    });
                }

                Expr::Minus {
                    lhs,
                    rhs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_math_op(lhs, rhs, inferred_type, &mut temp_stack, |a, b, c| {
                        Expr::Minus {
                            lhs: a,
                            rhs: b,
                            inferred_type: c,
                            source_span: source_span.clone(),
                            type_annotation: type_annotation.clone(),
                        }
                    });
                }

                Expr::Multiply {
                    lhs,
                    rhs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_math_op(lhs, rhs, inferred_type, &mut temp_stack, |a, b, c| {
                        Expr::Multiply {
                            lhs: a,
                            rhs: b,
                            inferred_type: c,
                            source_span: source_span.clone(),
                            type_annotation: type_annotation.clone(),
                        }
                    });
                }

                Expr::Divide {
                    lhs,
                    rhs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_math_op(lhs, rhs, inferred_type, &mut temp_stack, |a, b, c| {
                        Expr::Divide {
                            lhs: a,
                            rhs: b,
                            inferred_type: c,
                            source_span: source_span.clone(),
                            type_annotation: type_annotation.clone(),
                        }
                    });
                }

                Expr::EqualTo {
                    lhs,
                    rhs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_comparison_op(lhs, rhs, inferred_type, &mut temp_stack, |a, b, c| {
                        Expr::EqualTo {
                            lhs: a,
                            rhs: b,
                            inferred_type: c,
                            source_span: source_span.clone(),
                            type_annotation: type_annotation.clone(),
                        }
                    });
                }

                Expr::LessThan {
                    lhs,
                    rhs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_comparison_op(lhs, rhs, inferred_type, &mut temp_stack, |a, b, c| {
                        Expr::LessThan {
                            lhs: a,
                            rhs: b,
                            inferred_type: c,
                            source_span: source_span.clone(),
                            type_annotation: type_annotation.clone(),
                        }
                    });
                }

                Expr::Let {
                    variable_id,
                    type_annotation,
                    expr,
                    inferred_type,
                    source_span,
                } => {
                    handle_let(
                        variable_id,
                        expr,
                        type_annotation,
                        inferred_type,
                        &mut temp_stack,
                        source_span,
                    );
                }
                Expr::Sequence {
                    exprs,
                    type_annotation,
                    inferred_type,
                    source_span,
                    ..
                } => {
                    handle_sequence(
                        exprs,
                        inferred_type,
                        &mut temp_stack,
                        type_annotation,
                        source_span,
                    );
                }
                Expr::Record {
                    exprs,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_record(
                        exprs,
                        inferred_type,
                        &mut temp_stack,
                        source_span,
                        type_annotation,
                    );
                }
                Expr::Literal { .. } => {
                    temp_stack.push_front((expr.clone(), false));
                }
                Expr::Number { .. } => {
                    temp_stack.push_front((expr.clone(), false));
                }
                Expr::Boolean { .. } => {
                    temp_stack.push_front((expr.clone(), false));
                }
                Expr::And {
                    lhs,
                    rhs,
                    source_span,
                    type_annotation,
                    ..
                } => {
                    handle_comparison_op(
                        lhs,
                        rhs,
                        &InferredType::Bool,
                        &mut temp_stack,
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
                    handle_comparison_op(
                        lhs,
                        rhs,
                        &InferredType::Bool,
                        &mut temp_stack,
                        |a, b, c| Expr::Or {
                            lhs: a,
                            rhs: b,
                            inferred_type: c,
                            source_span: source_span.clone(),
                            type_annotation: type_annotation.clone(),
                        },
                    );
                }

                Expr::InvokeMethodLazy {
                    lhs,
                    method,
                    generic_type_parameter,
                    args,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => handle_invoke_method(
                    lhs,
                    method,
                    args,
                    generic_type_parameter.clone(),
                    inferred_type,
                    &mut temp_stack,
                    source_span,
                    type_annotation,
                ),

                Expr::Call {
                    call_type,
                    generic_type_parameter,
                    args,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_call(
                        call_type,
                        args,
                        generic_type_parameter,
                        inferred_type,
                        &mut temp_stack,
                        source_span,
                        type_annotation,
                    );
                }

                Expr::Unwrap {
                    expr,
                    inferred_type,
                    source_span,
                    ..
                } => {
                    handle_unwrap(expr, inferred_type, &mut temp_stack, source_span);
                }

                Expr::Throw { .. } => {
                    temp_stack.push_front((expr.clone(), false));
                }

                Expr::GetTag {
                    expr,
                    inferred_type,
                    source_span,
                    ..
                } => {
                    handle_get_tag(expr, inferred_type, &mut temp_stack, source_span);
                }

                Expr::ListComprehension {
                    iterated_variable,
                    iterable_expr,
                    yield_expr,
                    inferred_type,
                    source_span,
                    ..
                } => {
                    handle_list_comprehension(
                        iterated_variable,
                        iterable_expr,
                        yield_expr,
                        inferred_type,
                        &mut temp_stack,
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
                    type_annotation,
                } => handle_list_reduce(
                    reduce_variable,
                    iterated_variable,
                    iterable_expr,
                    init_value_expr,
                    yield_expr,
                    inferred_type,
                    &mut temp_stack,
                    source_span,
                    type_annotation,
                ),

                Expr::Range {
                    range,
                    inferred_type,
                    source_span,
                    type_annotation,
                } => {
                    handle_range(
                        range,
                        source_span,
                        inferred_type.clone(),
                        &mut temp_stack,
                        type_annotation,
                    );
                }
            }
        }

        temp_stack
            .pop_front()
            .map(|x| x.0)
            .ok_or(CustomError::new(expr, "failed to bind global variable types").into())
    }

    fn make_expr_nodes_queue<'a>(expr: &'a Expr, expr_queue: &mut VecDeque<&'a Expr>) {
        let mut stack = VecDeque::new();

        stack.push_back(expr);

        while let Some(current_expr) = stack.pop_back() {
            expr_queue.push_back(current_expr);

            current_expr.visit_children_bottom_up(&mut stack)
        }
    }

    fn handle_list_comprehension(
        variable_id: &VariableId,
        current_iterable_expr: &Expr,
        current_yield_expr: &Expr,
        current_comprehension_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
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
            )
            .with_source_span(source_span.clone()),
            false,
        ))
    }

    fn handle_list_reduce(
        reduce_variable: &VariableId,
        iterated_variable: &VariableId,
        iterable_expr: &Expr,
        initial_value_expr: &Expr,
        yield_expr: &Expr,
        reduce_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
        type_annotation: &Option<TypeName>,
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
            )
            .with_type_annotation_opt(type_annotation.clone())
            .with_source_span(source_span.clone()),
            false,
        ))
    }

    fn handle_tuple(
        tuple_elems: &[Expr],
        current_tuple_type: &InferredType,
        result_expr_queue: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
        type_name: &Option<TypeName>,
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
        let new_tuple = Expr::tuple(new_tuple_elems)
            .with_inferred_type(current_tuple_type.clone())
            .with_source_span(source_span.clone())
            .with_type_annotation_opt(type_name.clone());
        result_expr_queue.push_front((new_tuple, false));
    }

    fn handle_select_field(
        original_selection_expr: &Expr,
        field: &str,
        continue_search: bool,
        current_field_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        path: &mut Path,
        override_type: &InferredType,
        type_name: &Option<TypeName>,
        source_span: &SourceSpan,
    ) -> Result<(), RibCompilationError> {
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
                    Expr::select_field(expr.clone(), field, type_name.clone())
                        .with_inferred_type(new_type)
                        .with_source_span(source_span.clone()),
                    continue_search,
                ));
            } else {
                temp_stack.push_front((
                    Expr::select_field(expr.clone(), field, type_name.clone())
                        .with_inferred_type(current_field_type.clone())
                        .with_source_span(source_span.clone()),
                    true,
                ));
            }
        } else {
            temp_stack.push_front((
                Expr::select_field(expr.clone(), field, type_name.clone())
                    .with_inferred_type(current_field_type.clone())
                    .with_source_span(source_span.clone()),
                false,
            ));
        }

        Ok(())
    }

    pub fn handle_select_dynamic(
        original_selection_expr: &Expr,
        index: &Expr,
        current_index_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        type_name: &Option<TypeName>,
        source_span: &SourceSpan,
    ) -> Result<(), RibCompilationError> {
        let index = temp_stack.pop_front().unwrap_or((index.clone(), false));

        let expr = temp_stack
            .pop_front()
            .unwrap_or((original_selection_expr.clone(), false));

        let new_select_index = Expr::SelectDynamic {
            expr: Box::new(expr.0.clone()),
            index: Box::new(index.0.clone()),
            type_annotation: type_name.clone(),
            inferred_type: current_index_type.clone(),
            source_span: source_span.clone(),
        };

        temp_stack.push_front((new_select_index, false));

        Ok(())
    }

    pub fn handle_select_index(
        original_selection_expr: &Expr,
        index: &usize,
        current_index_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        type_name: &Option<TypeName>,
        source_span: &SourceSpan,
    ) -> Result<(), RibCompilationError> {
        let expr = temp_stack
            .pop_front()
            .unwrap_or((original_selection_expr.clone(), false));

        let new_select_index = Expr::SelectIndex {
            expr: Box::new(expr.0.clone()),
            index: *index,
            type_annotation: type_name.clone(),
            inferred_type: current_index_type.clone(),
            source_span: source_span.clone(),
        };
        temp_stack.push_front((new_select_index, false));

        Ok(())
    }

    fn handle_result_ok(
        original_ok_expr: &Expr,
        current_ok_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        type_name: &Option<TypeName>,
        source_span: &SourceSpan,
    ) {
        let ok_expr = temp_stack
            .pop_front()
            .unwrap_or((original_ok_expr.clone(), false));

        let new_result = Expr::Result {
            expr: Ok(Box::new(ok_expr.0.clone())),
            type_annotation: type_name.clone(),
            inferred_type: current_ok_type.clone(),
            source_span: source_span.clone(),
        };
        temp_stack.push_front((new_result, true));
    }

    fn handle_result_error(
        original_error_expr: &Expr,
        current_error_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        type_name: &Option<TypeName>,
        source_span: &SourceSpan,
    ) {
        let expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(original_error_expr.clone());

        let new_result = Expr::Result {
            expr: Err(Box::new(expr.clone())),
            type_annotation: type_name.clone(),
            inferred_type: current_error_type.clone(),
            source_span: source_span.clone(),
        };

        temp_stack.push_front((new_result, false));
    }

    fn handle_option_some(
        original_some_expr: &Expr,
        current_some_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        type_name: &Option<TypeName>,
        source_span: &SourceSpan,
    ) {
        let expr = temp_stack
            .pop_front()
            .unwrap_or((original_some_expr.clone(), false));
        let new_option = Expr::Option {
            expr: Some(Box::new(expr.0.clone())),
            type_annotation: type_name.clone(),
            inferred_type: current_some_type.clone(),
            source_span: source_span.clone(),
        };
        temp_stack.push_front((new_option, false));
    }

    fn handle_if_else(
        original_predicate: &Expr,
        original_then_expr: &Expr,
        original_else_expr: &Expr,
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
        type_annotation: &Option<TypeName>,
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

        let new_expr = Expr::Cond {
            cond: Box::new(cond_expr.0),
            lhs: Box::new(then_expr.0.clone()),
            rhs: Box::new(else_expr.0.clone()),
            inferred_type: current_inferred_type.clone(),
            source_span: source_span.clone(),
            type_annotation: type_annotation.clone(),
        };

        temp_stack.push_front((new_expr, false));
    }

    pub fn handle_pattern_match(
        predicate: &Expr,
        current_match_arms: &[MatchArm],
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
        type_annotation: &Option<TypeName>,
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

        let new_expr = Expr::PatternMatch {
            predicate: Box::new(pred.clone()),
            match_arms: new_match_arms,
            inferred_type: current_inferred_type.clone(),
            source_span: source_span.clone(),
            type_annotation: type_annotation.clone(),
        };

        temp_stack.push_front((new_expr, false));
    }

    fn handle_concat(
        exprs: &Vec<Expr>,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
        type_annotation: &Option<TypeName>,
    ) {
        let mut new_exprs = vec![];
        for expr in exprs {
            let expr = temp_stack.pop_front().map(|x| x.0).unwrap_or(expr.clone());
            new_exprs.push(expr);
        }

        new_exprs.reverse();

        let new_concat = Expr::Concat {
            exprs: new_exprs,
            inferred_type: InferredType::Str,
            source_span: source_span.clone(),
            type_annotation: type_annotation.clone(),
        };
        temp_stack.push_front((new_concat, false));
    }

    fn handle_multiple(
        current_expr_list: &Vec<Expr>,
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
        type_annotation: &Option<TypeName>,
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

        let new_multiple = Expr::ExprBlock {
            exprs: new_exprs,
            inferred_type: current_inferred_type.clone(),
            source_span: source_span.clone(),
            type_annotation: type_annotation.clone(),
        };
        temp_stack.push_front((new_multiple, false));
    }

    fn handle_not(
        original_not_expr: &Expr,
        current_not_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
        type_annotation: &Option<TypeName>,
    ) {
        let expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(original_not_expr.clone());
        let new_not = Expr::Not {
            expr: Box::new(expr),
            inferred_type: current_not_type.clone(),
            source_span: source_span.clone(),
            type_annotation: type_annotation.clone(),
        };
        temp_stack.push_front((new_not, false));
    }

    fn handle_math_op<F>(
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

    fn handle_comparison_op<F>(
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

    fn handle_invoke_method(
        original_lhs_expr: &Expr,
        method_name: &str,
        args: &[Expr],
        generic_type_parameter: Option<GenericTypeParameter>,
        inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
        type_annotation: &Option<TypeName>,
    ) {
        let mut new_arg_exprs = vec![];

        for expr in args.iter().rev() {
            let expr = temp_stack.pop_front().map(|x| x.0).unwrap_or(expr.clone());
            new_arg_exprs.push(expr);
        }

        new_arg_exprs.reverse();

        let new_lhs_expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(original_lhs_expr.clone());

        if let InferredType::Instance { instance_type } = inferred_type {
            if let Some(worker_expr) = instance_type.worker() {
                let new_worker_expr = temp_stack
                    .pop_front()
                    .map(|x| x.0)
                    .unwrap_or(worker_expr.clone());

                let mut new_instance_type = instance_type.clone();
                new_instance_type.set_worker_name(new_worker_expr.clone());

                let new_call = Expr::InvokeMethodLazy {
                    lhs: Box::new(new_lhs_expr),
                    method: method_name.to_string(),
                    generic_type_parameter,
                    args: new_arg_exprs,
                    inferred_type: InferredType::Instance {
                        instance_type: new_instance_type,
                    },
                    source_span: source_span.clone(),
                    type_annotation: type_annotation.clone(),
                };

                temp_stack.push_front((new_call, false));
            } else {
                let new_call = Expr::InvokeMethodLazy {
                    lhs: Box::new(new_lhs_expr),
                    method: method_name.to_string(),
                    generic_type_parameter,
                    args: new_arg_exprs,
                    inferred_type: inferred_type.clone(),
                    source_span: source_span.clone(),
                    type_annotation: type_annotation.clone(),
                };

                temp_stack.push_front((new_call, false));
            }
        } else {
            let new_call = Expr::InvokeMethodLazy {
                lhs: Box::new(new_lhs_expr),
                method: method_name.to_string(),
                generic_type_parameter,
                args: new_arg_exprs,
                inferred_type: inferred_type.clone(),
                source_span: source_span.clone(),
                type_annotation: type_annotation.clone(),
            };

            temp_stack.push_front((new_call, false));
        }
    }

    fn handle_call(
        call_type: &CallType,
        arguments: &[Expr],
        generic_type_parameter: &Option<GenericTypeParameter>,
        inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
        type_annotation: &Option<TypeName>,
    ) {
        let mut new_arg_exprs = vec![];

        // retrieving all argument from the stack
        for expr in arguments.iter().rev() {
            let expr = temp_stack.pop_front().map(|x| x.0).unwrap_or(expr.clone());
            new_arg_exprs.push(expr);
        }

        new_arg_exprs.reverse();

        match call_type {
            CallType::InstanceCreation(instance_creation) => {
                let worker_name = instance_creation.worker_name();

                if let Some(worker_name) = worker_name {
                    let worker_name = temp_stack.pop_front().map(|x| x.0).unwrap_or(worker_name);

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

                    let new_call = Expr::Call {
                        call_type: CallType::InstanceCreation(new_instance_creation.clone()),
                        generic_type_parameter: generic_type_parameter.clone(),
                        args: new_arg_exprs,
                        inferred_type: inferred_type.clone(),
                        source_span: source_span.clone(),
                        type_annotation: type_annotation.clone(),
                    };
                    temp_stack.push_front((new_call, false));
                } else {
                    let new_call = Expr::Call {
                        call_type: CallType::InstanceCreation(instance_creation.clone()),
                        generic_type_parameter: generic_type_parameter.clone(),
                        args: new_arg_exprs,
                        inferred_type: inferred_type.clone(),
                        source_span: source_span.clone(),
                        type_annotation: type_annotation.clone(),
                    };

                    temp_stack.push_front((new_call, false));
                }
            }

            CallType::Function {
                function_name,
                worker,
            } => {
                let mut function_name = function_name.clone();

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

                let mut worker_in_inferred_type = None;

                if let InferredType::Instance { instance_type } = inferred_type {
                    let worker = instance_type.worker_name();
                    if let Some(worker) = worker {
                        worker_in_inferred_type = Some(
                            temp_stack
                                .pop_front()
                                .map(|x| x.0)
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
                let new_call = if let Some(worker) = worker {
                    let worker = temp_stack
                        .pop_front()
                        .map(|x| x.0)
                        .unwrap_or(worker.deref().clone());

                    Expr::Call {
                        call_type: CallType::Function {
                            function_name,
                            worker: Some(Box::new(worker)),
                        },
                        generic_type_parameter: generic_type_parameter.clone(),
                        args: new_arg_exprs,
                        inferred_type: new_inferred_type,
                        source_span: source_span.clone(),
                        type_annotation: type_annotation.clone(),
                    }
                } else {
                    Expr::Call {
                        call_type: CallType::Function {
                            function_name,
                            worker: None,
                        },
                        generic_type_parameter: generic_type_parameter.clone(),
                        args: new_arg_exprs,
                        inferred_type: new_inferred_type,
                        source_span: source_span.clone(),
                        type_annotation: type_annotation.clone(),
                    }
                };

                temp_stack.push_front((new_call, false));
            }

            CallType::VariantConstructor(str) => {
                let new_call = Expr::Call {
                    call_type: CallType::VariantConstructor(str.clone()),
                    generic_type_parameter: None,
                    args: new_arg_exprs,
                    inferred_type: inferred_type.clone(),
                    source_span: source_span.clone(),
                    type_annotation: type_annotation.clone(),
                };
                temp_stack.push_front((new_call, false));
            }

            CallType::EnumConstructor(str) => {
                let new_call = Expr::Call {
                    call_type: CallType::EnumConstructor(str.clone()),
                    generic_type_parameter: None,
                    args: new_arg_exprs,
                    inferred_type: inferred_type.clone(),
                    source_span: source_span.clone(),
                    type_annotation: type_annotation.clone(),
                };
                temp_stack.push_front((new_call, false));
            }
        }
    }

    fn handle_unwrap(
        expr: &Expr,
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
    ) {
        let expr = temp_stack.pop_front().map(|x| x.0).unwrap_or(expr.clone());
        let new_unwrap = expr
            .unwrap()
            .with_inferred_type(current_inferred_type.clone())
            .with_source_span(source_span.clone());
        temp_stack.push_front((new_unwrap, false));
    }

    fn handle_get_tag(
        expr: &Expr,
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
    ) {
        let expr = temp_stack.pop_front().map(|x| x.0).unwrap_or(expr.clone());
        let new_get_tag = Expr::get_tag(expr.clone())
            .with_inferred_type(current_inferred_type.clone())
            .with_source_span(source_span.clone());
        temp_stack.push_front((new_get_tag, false));
    }

    fn handle_let(
        original_variable_id: &VariableId,
        original_expr: &Expr,
        optional_type: &Option<TypeName>,
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
    ) {
        let expr = temp_stack
            .pop_front()
            .map(|x| x.0)
            .unwrap_or(original_expr.clone());
        let new_let = Expr::Let {
            variable_id: original_variable_id.clone(),
            type_annotation: optional_type.clone(),
            expr: Box::new(expr),
            inferred_type: current_inferred_type.clone(),
            source_span: source_span.clone(),
        };
        temp_stack.push_front((new_let, false));
    }

    fn handle_sequence(
        current_expr_list: &[Expr],
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        type_name: &Option<TypeName>,
        source_span: &SourceSpan,
    ) {
        let mut new_exprs = vec![];

        for expr in current_expr_list.iter().rev() {
            let expr = temp_stack.pop_front().map(|x| x.0).unwrap_or(expr.clone());
            new_exprs.push(expr);
        }

        new_exprs.reverse();

        let expr = Expr::sequence(new_exprs, type_name.clone())
            .with_inferred_type(current_inferred_type.clone())
            .with_source_span(source_span.clone());

        temp_stack.push_front((expr, false));
    }

    fn handle_range(
        range: &Range,
        source_span: &SourceSpan,
        inferred_type: InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        type_annotation: &Option<TypeName>,
    ) {
        match range {
            Range::Range { from, to } => {
                let right = temp_stack
                    .pop_front()
                    .map(|x| x.0)
                    .unwrap_or(to.deref().clone());
                let left = temp_stack
                    .pop_front()
                    .map(|x| x.0)
                    .unwrap_or(from.deref().clone());
                let new_range = Expr::range(left, right)
                    .with_inferred_type(inferred_type)
                    .with_source_span(source_span.clone())
                    .with_type_annotation_opt(type_annotation.clone());

                temp_stack.push_front((new_range, false));
            }
            Range::RangeInclusive { from, to } => {
                let right = temp_stack
                    .pop_front()
                    .map(|x| x.0)
                    .unwrap_or(to.deref().clone());
                let left = temp_stack
                    .pop_front()
                    .map(|x| x.0)
                    .unwrap_or(from.deref().clone());
                let new_range = Expr::range_inclusive(left, right)
                    .with_inferred_type(inferred_type)
                    .with_source_span(source_span.clone())
                    .with_type_annotation_opt(type_annotation.clone());

                temp_stack.push_front((new_range, false));
            }
            Range::RangeFrom { from } => {
                let left = temp_stack
                    .pop_front()
                    .map(|x| x.0)
                    .unwrap_or(from.deref().clone());
                let new_range = Expr::range_from(left)
                    .with_inferred_type(inferred_type)
                    .with_source_span(source_span.clone())
                    .with_type_annotation_opt(type_annotation.clone());

                temp_stack.push_front((new_range, false));
            }
        }
    }

    fn handle_record(
        current_expr_list: &[(String, Box<Expr>)],
        current_inferred_type: &InferredType,
        temp_stack: &mut VecDeque<(Expr, bool)>,
        source_span: &SourceSpan,
        type_annotation: &Option<TypeName>,
    ) {
        let mut new_exprs = vec![];

        for (field, expr) in current_expr_list.iter().rev() {
            let expr: Expr = temp_stack
                .pop_front()
                .map(|x| x.0)
                .unwrap_or(expr.deref().clone());
            new_exprs.push((field.clone(), expr.clone()));
        }

        new_exprs.reverse();

        let new_record = Expr::record(new_exprs.to_vec())
            .with_inferred_type(current_inferred_type.clone())
            .with_source_span(source_span.clone())
            .with_type_annotation_opt(type_annotation.clone());
        temp_stack.push_front((new_record, false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rib_source_span::SourceSpan;
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

        let result = expr.bind_global_variable_types(&vec![type_spec]).unwrap();

        let expected = Expr::identifier_global("foo", None).with_inferred_type(InferredType::Str);

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

        let result = expr.bind_global_variable_types(&vec![type_spec]).unwrap();

        let expected = Expr::select_field(
            Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
            "baz",
            None,
        )
        .with_inferred_type(InferredType::Str);

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

        let result = expr.bind_global_variable_types(&vec![type_spec]).unwrap();

        let expected = Expr::select_field(
            Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
            "baz",
            None,
        )
        .with_inferred_type(InferredType::Str);

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

        let result = expr.bind_global_variable_types(&vec![type_spec]).unwrap();

        let expected = Expr::select_field(
            Expr::select_field(Expr::identifier_global("foo", None), "bar", None),
            "baz",
            None,
        )
        .with_inferred_type(InferredType::Str);

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

        let expected = Expr::expr_block(vec![
            Expr::Let {
                variable_id: VariableId::Local("res".to_string(), Some(Id(0))),
                type_annotation: None,
                expr: Box::new(
                    Expr::select_field(
                        Expr::select_field(
                            Expr::identifier_global("foo", None).with_inferred_type(
                                InferredType::Record(vec![(
                                    "bar".to_string(),
                                    InferredType::Record(vec![
                                        ("number".to_string(), InferredType::U64),
                                        ("user-id".to_string(), InferredType::Str),
                                    ]),
                                )]),
                            ),
                            "bar",
                            None,
                        )
                        .with_inferred_type(InferredType::Record(vec![
                            ("number".to_string(), InferredType::U64),
                            ("user-id".to_string(), InferredType::Str),
                        ])),
                        "user-id",
                        None,
                    )
                    .with_inferred_type(InferredType::Str),
                ),
                inferred_type: InferredType::Unknown,
                source_span: SourceSpan::default(),
            },
            Expr::Let {
                variable_id: VariableId::Local("hello".to_string(), Some(Id(0))),
                type_annotation: Some(TypeName::U64),
                expr: Box::new(
                    Expr::select_field(
                        Expr::select_field(
                            Expr::identifier_global("foo", None).with_inferred_type(
                                InferredType::Record(vec![(
                                    "bar".to_string(),
                                    InferredType::Record(vec![
                                        ("number".to_string(), InferredType::U64),
                                        ("user-id".to_string(), InferredType::Str),
                                    ]),
                                )]),
                            ),
                            "bar",
                            None,
                        )
                        .with_inferred_type(InferredType::Record(vec![
                            ("number".to_string(), InferredType::U64),
                            ("user-id".to_string(), InferredType::Str),
                        ])),
                        "number",
                        None,
                    )
                    .with_inferred_type(InferredType::U64),
                ),
                inferred_type: InferredType::Unknown,
                source_span: SourceSpan::default(),
            },
            Expr::Identifier {
                variable_id: VariableId::Local("hello".to_string(), Some(Id(0))),
                type_annotation: None,
                inferred_type: InferredType::U64,
                source_span: SourceSpan::default(),
            },
        ])
        .with_inferred_type(InferredType::U64);

        assert_eq!(expr, expected);
    }
}
