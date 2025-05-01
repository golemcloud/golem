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
use crate::type_inference::type_push_down::internal::{
    handle_list_comprehension, handle_list_reduce,
};
use crate::{Expr, ExprVisitor, InferredType, MatchArm, TypeInternal};
use std::ops::Deref;

pub fn push_types_down(expr: &mut Expr) -> Result<(), RibTypeError> {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(outer_expr) = visitor.pop_back() {
        let copied = outer_expr.clone();

        match outer_expr {
            Expr::SelectField {
                expr,
                field,
                inferred_type,
                ..
            } => {
                let field_type = inferred_type.clone();
                let record_type = vec![(field.to_string(), field_type)];
                let inferred_record_type = InferredType::record(record_type);

                expr.add_infer_type_mut(inferred_record_type);
            }

            Expr::SelectIndex {
                expr,          // LHS
                index,         // RHS
                inferred_type, // This is the type of the total expression
                ..
            } => {
                let field_type = inferred_type.clone();

                // How to push down here depends on the type of index
                // If the index is not a range type then the left hand side expression's type becomes list(field_type) similar to
                // select-index, If the index is range,
                // since the field type is infact the same as LHS
                let index_expr_type = index.inferred_type();

                match index_expr_type.inner.deref() {
                    TypeInternal::Range { .. } => {
                        expr.add_infer_type_mut(inferred_type.clone());
                    }
                    _ => {
                        // Similar to selectIndex
                        let new_inferred_type = InferredType::list(field_type);
                        expr.add_infer_type_mut(new_inferred_type);
                    }
                }
            }

            Expr::Cond {
                cond,
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                lhs.add_infer_type_mut(inferred_type.clone());
                rhs.add_infer_type_mut(inferred_type.clone());

                cond.add_infer_type_mut(InferredType::bool());
            }
            Expr::Not {
                expr,
                inferred_type,
                ..
            } => {
                expr.add_infer_type_mut(inferred_type.clone());
            }
            Expr::Option {
                expr: Some(inner_expr),
                inferred_type,
                ..
            } => {
                internal::handle_option(inner_expr, copied, inferred_type)?;
            }

            Expr::Result {
                expr: Ok(expr),
                inferred_type,
                ..
            } => {
                internal::handle_ok(expr, copied, inferred_type)?;
            }

            Expr::Result {
                expr: Err(expr),
                inferred_type,
                ..
            } => {
                internal::handle_err(expr, copied, inferred_type)?;
            }

            Expr::PatternMatch {
                predicate,
                match_arms,
                inferred_type,
                ..
            } => {
                for MatchArm {
                    arm_resolution_expr,
                    arm_pattern,
                } in match_arms
                {
                    let predicate_type = predicate.inferred_type();
                    internal::update_arm_pattern_type(
                        &copied,
                        arm_pattern,
                        &predicate_type,
                        predicate,
                    )?;
                    arm_resolution_expr.add_infer_type_mut(inferred_type.clone());
                }
            }

            Expr::Tuple {
                exprs,
                inferred_type,
                ..
            } => {
                internal::handle_tuple(exprs, copied, inferred_type)?;
            }
            Expr::Sequence {
                exprs,
                inferred_type,
                ..
            } => {
                internal::handle_sequence(exprs, copied, inferred_type)?;
            }

            Expr::Record {
                exprs,
                inferred_type,
                ..
            } => {
                internal::handle_record(exprs, copied, inferred_type)?;
            }

            Expr::Call {
                call_type,
                args,
                inferred_type,
                ..
            } => {
                internal::handle_call(call_type, args, inferred_type);
            }

            Expr::ListComprehension {
                iterated_variable,
                iterable_expr,
                yield_expr,
                inferred_type,
                ..
            } => {
                handle_list_comprehension(
                    iterated_variable,
                    iterable_expr,
                    yield_expr,
                    inferred_type,
                )?;
            }

            Expr::ListReduce {
                reduce_variable,
                iterated_variable,
                iterable_expr,
                init_value_expr,
                yield_expr,
                inferred_type,
                ..
            } => {
                handle_list_reduce(
                    reduce_variable,
                    iterated_variable,
                    iterable_expr,
                    init_value_expr,
                    yield_expr,
                    inferred_type,
                )?;
            }

            Expr::Plus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                lhs.add_infer_type_mut(rhs.inferred_type());
                rhs.add_infer_type_mut(lhs.inferred_type());
                lhs.add_infer_type_mut(inferred_type.clone());
                rhs.add_infer_type_mut(inferred_type.clone());
            }

            Expr::Divide {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                lhs.add_infer_type_mut(rhs.inferred_type());
                rhs.add_infer_type_mut(lhs.inferred_type());
                lhs.add_infer_type_mut(inferred_type.clone());
                rhs.add_infer_type_mut(inferred_type.clone());
            }

            Expr::Minus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                lhs.add_infer_type_mut(rhs.inferred_type());
                rhs.add_infer_type_mut(lhs.inferred_type());
                lhs.add_infer_type_mut(inferred_type.clone());
                rhs.add_infer_type_mut(inferred_type.clone());
            }

            Expr::Multiply {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                lhs.add_infer_type_mut(rhs.inferred_type());
                rhs.add_infer_type_mut(lhs.inferred_type());
                lhs.add_infer_type_mut(inferred_type.clone());
                rhs.add_infer_type_mut(inferred_type.clone());
            }

            _ => {}
        }
    }

    Ok(())
}

mod internal {
    use crate::call_type::CallType;
    use crate::rib_type_error::RibTypeError;
    use crate::type_inference::type_hint::{GetTypeHint, TypeHint};
    use crate::type_refinement::precise_types::*;
    use crate::type_refinement::TypeRefinement;
    use crate::{
        ActualType, AmbiguousTypeError, ArmPattern, ExpectedType, Expr, InferredType,
        InvalidPatternMatchError, TypeInternal, TypeMismatchError, VariableId,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use std::collections::VecDeque;
    use std::ops::Deref;

    pub(crate) fn handle_list_comprehension(
        variable_id: &mut VariableId,
        iterable_expr: &mut Expr,
        yield_expr: &mut Expr,
        comprehension_result_type: &InferredType,
    ) -> Result<(), RibTypeError> {
        update_yield_expr_in_list_comprehension(variable_id, iterable_expr, yield_expr)?;

        let refined_list_type = ListType::refine(comprehension_result_type).ok_or_else(|| {
            get_compilation_error_for_ambiguity(
                comprehension_result_type,
                yield_expr,
                &TypeHint::List(None),
            )
            .with_additional_error_detail("the result of a comprehension should be of type list")
        })?;

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
    ) -> Result<(), RibTypeError> {
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
        variable: &mut VariableId,
        iterable_expr: &Expr,
        yield_expr: &mut Expr,
    ) -> Result<(), RibTypeError> {
        let iterable_type: InferredType = iterable_expr.inferred_type();

        if !iterable_type.is_unknown() {
            let refined_iterable = ListType::refine(&iterable_type);

            let iterable_variable_type = match refined_iterable {
                Some(refined_iterable) => refined_iterable.inner_type(),
                None => {
                    let refined_range = RangeType::refine(&iterable_type).ok_or_else(||
                        get_compilation_error_for_ambiguity(&iterable_type, iterable_expr, &TypeHint::List(None))
                            .with_additional_error_detail(
                                "the iterable expression in list comprehension should be of type list or a range",
                            ),
                    )?;

                    refined_range.inner_type()
                }
            };

            let mut queue = VecDeque::new();
            queue.push_back(yield_expr);

            while let Some(expr) = queue.pop_back() {
                match expr {
                    Expr::Identifier {
                        variable_id,
                        inferred_type,
                        ..
                    } => {
                        if let VariableId::ListComprehension(l) = variable_id {
                            if l.name == variable.name() {
                                *inferred_type = inferred_type.merge(iterable_variable_type.clone())
                            }
                        }
                    }
                    _ => expr.visit_expr_nodes_lazy(&mut queue),
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
    ) -> Result<(), RibTypeError> {
        let iterable_type = iterable_expr.inferred_type();

        if !iterable_expr.inferred_type().is_unknown() {
            let refined_iterable = ListType::refine(&iterable_type);

            let iterable_variable_type = match refined_iterable {
                Some(refined_iterable) => refined_iterable.inner_type(),
                None => {
                    let refined_range = RangeType::refine(&iterable_type).ok_or_else(||
                        get_compilation_error_for_ambiguity(&iterable_type, iterable_expr, &TypeHint::List(None))
                            .with_additional_error_detail(
                                "the iterable expression in list comprehension should be of type list or a range",
                            ),
                    )?;

                    refined_range.inner_type()
                }
            };

            let init_value_expr_type = init_value_expr.inferred_type();
            let mut queue = VecDeque::new();
            queue.push_back(yield_expr);

            while let Some(expr) = queue.pop_back() {
                match expr {
                    Expr::Identifier {
                        variable_id,
                        inferred_type,
                        ..
                    } => {
                        if let VariableId::ListComprehension(l) = variable_id {
                            if l.name == iterated_variable.name() {
                                *inferred_type = inferred_type.merge(iterable_variable_type.clone())
                            }
                        } else if let VariableId::ListReduce(l) = variable_id {
                            if l.name == reduce_variable.name() {
                                *inferred_type = inferred_type.merge(init_value_expr_type.clone())
                            }
                        }
                    }

                    _ => expr.visit_expr_nodes_lazy(&mut queue),
                }
            }
        }
        Ok(())
    }

    pub(crate) fn handle_option(
        inner_expr: &mut Expr,
        outer_expr: Expr,
        outer_inferred_type: &InferredType,
    ) -> Result<(), RibTypeError> {
        let refined_optional_type = OptionalType::refine(outer_inferred_type).ok_or_else(|| {
            get_compilation_error_for_ambiguity(
                outer_inferred_type,
                &outer_expr,
                &TypeHint::Option(None),
            )
        })?;

        let inner_type = refined_optional_type.inner_type();

        inner_expr.add_infer_type_mut(inner_type.clone());
        Ok(())
    }

    pub(crate) fn handle_ok(
        inner_expr: &mut Expr,
        outer_expr: Expr,
        outer_inferred_type: &InferredType,
    ) -> Result<(), RibTypeError> {
        let refined_ok_type = OkType::refine(outer_inferred_type).ok_or_else(|| {
            get_compilation_error_for_ambiguity(
                outer_inferred_type,
                &outer_expr,
                &TypeHint::Result {
                    ok: None,
                    err: None,
                },
            )
        })?;

        let inner_type = refined_ok_type.inner_type();

        inner_expr.add_infer_type_mut(inner_type.clone());

        Ok(())
    }

    pub(crate) fn handle_err(
        inner_expr: &mut Expr,
        outer_expr: Expr,
        outer_inferred_type: &InferredType,
    ) -> Result<(), RibTypeError> {
        let refined_err_type = ErrType::refine(outer_inferred_type).ok_or_else(|| {
            get_compilation_error_for_ambiguity(
                outer_inferred_type,
                &outer_expr,
                &TypeHint::Result {
                    ok: None,
                    err: None,
                },
            )
        })?;

        let inner_type = refined_err_type.inner_type();

        inner_expr.add_infer_type_mut(inner_type.clone());

        Ok(())
    }

    pub(crate) fn handle_sequence(
        inner_expressions: &mut [Expr],
        outer_expr: Expr,
        outer_inferred_type: &InferredType,
    ) -> Result<(), RibTypeError> {
        let refined_list_type = ListType::refine(outer_inferred_type).ok_or_else(|| {
            get_compilation_error_for_ambiguity(
                outer_inferred_type,
                &outer_expr,
                &TypeHint::List(None),
            )
        })?;
        let inner_type = refined_list_type.inner_type();

        for expr in inner_expressions.iter_mut() {
            expr.add_infer_type_mut(inner_type.clone());
        }

        Ok(())
    }

    pub(crate) fn handle_tuple(
        inner_expressions: &mut [Expr],
        outer_expr: Expr,
        outer_inferred_type: &InferredType,
    ) -> Result<(), RibTypeError> {
        let refined_tuple_type = TupleType::refine(outer_inferred_type).ok_or_else(|| {
            get_compilation_error_for_ambiguity(
                outer_inferred_type,
                &outer_expr,
                &TypeHint::Tuple(None),
            )
        })?;
        let inner_types = refined_tuple_type.inner_types();

        for (expr, typ) in inner_expressions.iter_mut().zip(inner_types) {
            expr.add_infer_type_mut(typ.clone());
        }

        Ok(())
    }

    pub(crate) fn handle_record(
        inner_expressions: &mut [(String, Box<Expr>)],
        outer_expr: Expr,
        outer_inferred_type: &InferredType,
    ) -> Result<(), RibTypeError> {
        let refined_record_type = RecordType::refine(outer_inferred_type).ok_or_else(|| {
            get_compilation_error_for_ambiguity(
                outer_inferred_type,
                &outer_expr,
                &TypeHint::Record(None),
            )
        })?;

        for (field, expr) in inner_expressions {
            let inner_type = refined_record_type.inner_type_by_name(field);
            expr.add_infer_type_mut(inner_type.clone());
        }

        Ok(())
    }

    pub(crate) fn handle_call<'a>(
        call_type: &'a mut CallType,
        expressions: &'a mut Vec<Expr>,
        inferred_type: &'a mut InferredType,
    ) {
        match call_type {
            // For CallType::Enum, there are no argument expressions
            // For CallType::Function, there is no type available to push down to arguments, as it is invalid
            // to push down the return type of function to its arguments.
            // For variant constructor, the type of the arguments are present in the return type of the call
            // and should be pushed down to arguments
            CallType::VariantConstructor(name) => {
                if let TypeInternal::Variant(variant) = inferred_type.inner.deref() {
                    let identified_variant = variant
                        .iter()
                        .find(|(variant_name, _)| variant_name == name);
                    if let Some((_name, Some(inner_type))) = identified_variant {
                        for expr in expressions {
                            expr.add_infer_type_mut(inner_type.clone());
                        }
                    }
                }
            }

            _ => {}
        }
    }

    pub(crate) fn update_arm_pattern_type(
        pattern_match_expr: &Expr,
        arm_pattern: &mut ArmPattern,
        predicate_type: &InferredType,
        original_predicate: &Expr,
    ) -> Result<(), RibTypeError> {
        match arm_pattern {
            ArmPattern::Literal(expr) => {
                expr.add_infer_type_mut(predicate_type.clone());
                //expr.push_types_down()?;
            }
            ArmPattern::As(_, pattern) => {
                update_arm_pattern_type(
                    pattern_match_expr,
                    pattern,
                    predicate_type,
                    original_predicate,
                )?;
            }

            ArmPattern::Constructor(constructor_name, patterns) => {
                if constructor_name == "some" || constructor_name == "none" {
                    let resolved = OptionalType::refine(predicate_type).ok_or_else(|| {
                        InvalidPatternMatchError::constructor_type_mismatch(
                            original_predicate,
                            pattern_match_expr,
                            constructor_name,
                        )
                    })?;

                    let inner = resolved.inner_type();

                    for pattern in patterns {
                        update_arm_pattern_type(
                            pattern_match_expr,
                            pattern,
                            &inner,
                            original_predicate,
                        )?;
                    }
                } else if constructor_name == "ok" {
                    let resolved = OkType::refine(predicate_type);

                    match resolved {
                        Some(resolved) => {
                            let inner = resolved.inner_type();

                            for pattern in patterns {
                                update_arm_pattern_type(
                                    pattern_match_expr,
                                    pattern,
                                    &inner,
                                    original_predicate,
                                )?;
                            }
                        }

                        None => {
                            ErrType::refine(predicate_type).ok_or_else(|| {
                                InvalidPatternMatchError::constructor_type_mismatch(
                                    original_predicate,
                                    pattern_match_expr,
                                    "ok",
                                )
                            })?;
                        }
                    }
                } else if constructor_name == "err" {
                    let resolved = ErrType::refine(predicate_type);

                    match resolved {
                        Some(resolved) => {
                            let inner = resolved.inner_type();

                            for pattern in patterns {
                                update_arm_pattern_type(
                                    pattern_match_expr,
                                    pattern,
                                    &inner,
                                    original_predicate,
                                )?;
                            }
                        }

                        None => {
                            OkType::refine(predicate_type).ok_or_else(|| {
                                InvalidPatternMatchError::constructor_type_mismatch(
                                    original_predicate,
                                    pattern_match_expr,
                                    "err",
                                )
                            })?;
                        }
                    }
                } else if let Some(variant_type) = VariantType::refine(predicate_type) {
                    let variant_arg_type = variant_type.inner_type_by_name(constructor_name);
                    for pattern in patterns {
                        update_arm_pattern_type(
                            pattern_match_expr,
                            pattern,
                            &variant_arg_type,
                            original_predicate,
                        )?;
                    }
                }
            }

            ArmPattern::TupleConstructor(patterns) => {
                let tuple_type = TupleType::refine(predicate_type).ok_or_else(|| {
                    InvalidPatternMatchError::constructor_type_mismatch(
                        original_predicate,
                        pattern_match_expr,
                        "tuple",
                    )
                })?;

                let inner_types = tuple_type.inner_types();

                if patterns.len() == inner_types.len() {
                    for (pattern, inner_type) in patterns.iter_mut().zip(inner_types) {
                        update_arm_pattern_type(
                            pattern_match_expr,
                            pattern,
                            &inner_type,
                            original_predicate,
                        )?;
                    }
                } else {
                    return Err(InvalidPatternMatchError::arg_size_mismatch(
                        original_predicate,
                        pattern_match_expr,
                        "tuple",
                        inner_types.len(),
                        patterns.len(),
                    )
                    .into());
                }
            }

            ArmPattern::ListConstructor(patterns) => {
                let list_type = ListType::refine(predicate_type).ok_or_else(|| {
                    InvalidPatternMatchError::constructor_type_mismatch(
                        original_predicate,
                        pattern_match_expr,
                        "list",
                    )
                })?;

                let list_elem_type = list_type.inner_type();

                for pattern in &mut *patterns {
                    update_arm_pattern_type(
                        pattern_match_expr,
                        pattern,
                        &list_elem_type,
                        original_predicate,
                    )?;
                }
            }

            ArmPattern::RecordConstructor(fields) => {
                let record_type = RecordType::refine(predicate_type).ok_or_else(|| {
                    InvalidPatternMatchError::constructor_type_mismatch(
                        original_predicate,
                        pattern_match_expr,
                        "record",
                    )
                })?;

                for (field, pattern) in fields {
                    let type_of_field = record_type.inner_type_by_name(field);
                    update_arm_pattern_type(
                        pattern_match_expr,
                        pattern,
                        &type_of_field,
                        original_predicate,
                    )?;
                }
            }

            ArmPattern::WildCard => {}
        }

        Ok(())
    }

    // actual_inferred_type: InferredType found in the outer structure
    // expr: The expr corresponding to the outer inferred type. Example: yield expr in a list comprehension
    // push_down_kind: The expected kind of the outer expression before pushing down
    pub fn get_compilation_error_for_ambiguity(
        actual_inferred_type: &InferredType,
        expr: &Expr,
        push_down_kind: &TypeHint,
    ) -> RibTypeError {
        // First check if the inferred type is a fully valid WIT type
        // If so, we trust this as this may handle majority of the cases
        // in compiler's best effort to create precise error message
        match AnalysedType::try_from(actual_inferred_type) {
            Ok(wit_tpe) => {
                TypeMismatchError::with_actual_type_kind(expr, None, wit_tpe, push_down_kind).into()
            }

            Err(_) => {
                // InferredType is not a fully valid WIT type yet
                // however it has enough information for compiler to trust it over the expected `type_kind`
                let actual_kind = actual_inferred_type.get_type_hint();
                match actual_kind {
                    TypeHint::Number | TypeHint::Str | TypeHint::Boolean | TypeHint::Char => {
                        TypeMismatchError {
                            expr_with_wrong_type: expr.clone(),
                            parent_expr: None,
                            expected_type: ExpectedType::Hint(actual_kind.clone()),
                            actual_type: ActualType::Hint(push_down_kind.clone()),
                            field_path: Default::default(),
                            additional_error_detail: vec![],
                        }
                        .into()
                    }

                    _ => AmbiguousTypeError::new(actual_inferred_type, expr, push_down_kind).into(),
                }
            }
        }
    }
}

#[cfg(test)]
mod type_push_down_tests {
    use test_r::test;

    use crate::type_inference::type_push_down::type_push_down_tests::internal::strip_spaces;
    use crate::{compile, Expr, InferredType};

    #[test]
    fn test_push_down_for_record() {
        let mut expr = Expr::record(vec![(
            "titles".to_string(),
            Expr::identifier_global("x", None),
        )])
        .with_inferred_type(
            InferredType::all_of(vec![
                InferredType::record(vec![("titles".to_string(), InferredType::unknown())]),
                InferredType::record(vec![("titles".to_string(), InferredType::u64())]),
            ])
            .unwrap(),
        );

        expr.push_types_down().unwrap();
        let expected = Expr::record(vec![(
            "titles".to_string(),
            Expr::identifier_global("x", None).with_inferred_type(InferredType::u64()),
        )])
        .with_inferred_type(
            InferredType::all_of(vec![
                InferredType::record(vec![("titles".to_string(), InferredType::unknown())]),
                InferredType::record(vec![("titles".to_string(), InferredType::u64())]),
            ])
            .unwrap(),
        );
        assert_eq!(expr, expected);
    }

    #[test]
    fn test_push_down_for_sequence() {
        let mut expr = Expr::sequence(
            vec![
                Expr::identifier_global("x", None),
                Expr::identifier_global("y", None),
            ],
            None,
        )
        .with_inferred_type(
            InferredType::all_of(vec![
                InferredType::list(InferredType::u32()),
                InferredType::list(InferredType::u64()),
            ])
            .unwrap(),
        );

        expr.push_types_down().unwrap();
        let expected = Expr::sequence(
            vec![
                Expr::identifier_global("x", None).with_inferred_type(
                    InferredType::all_of(vec![InferredType::u32(), InferredType::u64()]).unwrap(),
                ),
                Expr::identifier_global("y", None).with_inferred_type(
                    InferredType::all_of(vec![InferredType::u32(), InferredType::u64()]).unwrap(),
                ),
            ],
            None,
        )
        .with_inferred_type(
            InferredType::all_of(vec![
                InferredType::list(InferredType::u32()),
                InferredType::list(InferredType::u64()),
            ])
            .unwrap(),
        );
        assert_eq!(expr, expected);
    }

    #[test]
    fn invalid_push_down() {
        let expr = r#"
          let x: tuple<u32, u16> = [1, 2];
          x
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let error_message = compile(expr, &vec![]).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 2, column 36
        `[1, 2]`
        cause: type mismatch. expected tuple<u32, u16>, found list
        "#;

        assert_eq!(error_message, strip_spaces(expected));
    }

    mod internal {
        pub(crate) fn strip_spaces(input: &str) -> String {
            let lines = input.lines();

            let first_line = lines
                .clone()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("");
            let margin_width = first_line.chars().take_while(|c| c.is_whitespace()).count();

            let result = lines
                .map(|line| {
                    if line.trim().is_empty() {
                        String::new()
                    } else {
                        line[margin_width..].to_string()
                    }
                })
                .collect::<Vec<String>>()
                .join("\n");

            result.strip_prefix("\n").unwrap_or(&result).to_string()
        }
    }
}
