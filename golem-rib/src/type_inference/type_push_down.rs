use crate::{Expr, InferredType, MatchArm};
use std::collections::VecDeque;

pub fn push_types_down(expr: &mut Expr) {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::SelectField(expr, field, inferred_type) => {
                let field_type = inferred_type.clone();
                let record_type = vec![(field.to_string(), field_type)];
                let inferred_record_type = InferredType::Record(record_type);

                // the type of the expr is a record type having the specific field
                expr.add_infer_type_mut(inferred_record_type);
                queue.push_back(expr);
            }

            Expr::SelectIndex(expr, _, inferred_type) => {
                // If the field is not known, we update the inferred type with the field type

                let field_type = inferred_type.clone();
                let inferred_record_type = InferredType::List(Box::new(field_type));

                // the type of the expr is a record type having the specific field
                expr.add_infer_type_mut(inferred_record_type);
                queue.push_back(expr);
            }
            Expr::Cond(cond, then, else_, inferred_type) => {
                // If an entire if condition is inferred to be a specific type, then both branches should be of the same type
                // If the field is not known, we update the inferred type with the field type
                then.add_infer_type_mut(inferred_type.clone());
                else_.add_infer_type_mut(inferred_type.clone());

                // A condition expression is always a boolean type and can be tagged as a boolean
                cond.add_infer_type_mut(InferredType::Bool);
                queue.push_back(cond);
                queue.push_back(then);
                queue.push_back(else_);
            }
            Expr::Not(expr, inferred_type) => {
                // The inferred_type should be ideally boolean type and should be pushed down as a boolean type
                // however, at this phase, we are unsure and we propogate the inferred_type as is
                expr.add_infer_type_mut(inferred_type.clone());
                queue.push_back(expr);
            }
            Expr::Option(Some(expr), inferred_type) => {
                // The inferred_type should be ideally optional type, i.e, either Unknown type. or all of multiple optional types, or one of all optional types,
                // and otherwise we give up inferring the internal type at this phase
                match inferred_type {
                    InferredType::Option(ref t) => {
                        expr.add_infer_type_mut(*t.clone());
                    }
                    InferredType::AllOf(types) => {
                        let mut all_types = vec![];
                        for typ in types {
                            if let InferredType::Option(ref t) = typ {
                                all_types.push(*t.clone());
                            }
                        }
                        expr.add_infer_type_mut(InferredType::AllOf(all_types));
                    }
                    InferredType::OneOf(types) => {
                        let mut one_of_types = vec![];
                        for typ in types {
                            if let InferredType::Option(ref t) = typ {
                                one_of_types.push(*t.clone());
                            }
                        }
                        expr.add_infer_type_mut(InferredType::OneOf(one_of_types));
                    }
                    // we can't push down the types otherwise
                    _ => {}
                }
            }

            Expr::Result(Ok(expr), inferred_type) => {
                // The inferred_type should be ideally result type, i.e, either Unknown type. or all of multiple result types, or one of all result types,
                // and otherwise we give up inferring the internal type at this phase
                match inferred_type {
                    InferredType::Result { ok: Some(ok), .. } => {
                        expr.add_infer_type_mut(*ok.clone());
                        queue.push_back(expr);
                    }
                    InferredType::AllOf(types) => {
                        let mut all_types = vec![];
                        for typ in types {
                            if let InferredType::Result { ok: Some(ok), .. } = typ {
                                all_types.push(*ok.clone());
                            }
                        }
                        expr.add_infer_type_mut(InferredType::AllOf(all_types));
                        queue.push_back(expr);
                    }
                    InferredType::OneOf(types) => {
                        let mut one_of_types = vec![];
                        for typ in types {
                            if let InferredType::Result { ok: Some(ok), .. } = typ {
                                one_of_types.push(*ok.clone());
                            }
                        }
                        expr.add_infer_type_mut(InferredType::OneOf(one_of_types));
                        queue.push_back(expr);
                    }
                    // we can't push down the types otherwise
                    _ => {}
                }
            }

            Expr::Result(Err(expr), inferred_type) => {
                // The inferred_type should be ideally result type, i.e, either Unknown type. or all of multiple result types, or one of all result types,
                // and otherwise we give up inferring the internal type at this phase
                match inferred_type {
                    InferredType::Result {
                        error: Some(error), ..
                    } => {
                        expr.add_infer_type_mut(*error.clone());
                        queue.push_back(expr);
                    }
                    InferredType::AllOf(types) => {
                        let mut all_types = vec![];
                        for typ in types {
                            if let InferredType::Result {
                                error: Some(error), ..
                            } = typ
                            {
                                all_types.push(*error.clone());
                            }
                        }
                        expr.add_infer_type_mut(InferredType::AllOf(all_types));
                        queue.push_back(expr);
                    }
                    InferredType::OneOf(types) => {
                        let mut one_of_types = vec![];
                        for typ in types {
                            if let InferredType::Result {
                                error: Some(error), ..
                            } = typ
                            {
                                one_of_types.push(*error.clone());
                            }
                        }
                        expr.add_infer_type_mut(InferredType::OneOf(one_of_types));
                        queue.push_back(expr);
                    }
                    // we can't push down the types otherwise
                    _ => {}
                }
            }

            // In a pattern the type of the whole pattern match is pushed to the arm resolution expressions
            // And the type of predicate is pushed down to all those arm patterns that are expressions
            // It is currently impossible to transfer type info embedded in arm patterns to the arm resolution expressions
            // Example:  match result { a @ err(_) => a }.  `a` is not an Expr::identifier but rather a name in `ArmPattern::As(name, ..)
            // Since a field "a"  doesn't have a type, we can't push down / translate that type info to the arm resolution expression, even if we know a is err.
            // This can be solved though.
            Expr::PatternMatch(pred, match_arms, inferred_type) => {
                for MatchArm {
                    arm_resolution_expr,
                    arm_pattern,
                } in match_arms
                {
                    let predicate_type = pred.inferred_type();
                    internal::update_arm_pattern_type(arm_pattern, &predicate_type); // recursively push down the types as much as we can
                    arm_resolution_expr.add_infer_type_mut(inferred_type.clone());
                    queue.push_back(arm_resolution_expr);
                }
            }

            Expr::Tuple(exprs, inferred_type) => {
                // The inferred_type should be ideally tuple type, i.e, either Unknown type. or all of multiple tuple types, or one of all tuple types,
                // and otherwise we give up inferring the internal type at this phase
                match inferred_type {
                    InferredType::Tuple(types) => {
                        for (expr, typ) in exprs.iter_mut().zip(types) {
                            expr.add_infer_type_mut(typ.clone());
                            queue.push_back(expr);
                        }
                    }
                    InferredType::AllOf(types) => {
                        let mut all_types = vec![];
                        for typ in types {
                            if let InferredType::Tuple(types) = typ {
                                all_types.extend(types);
                            }
                        }
                        for (expr, typ) in exprs.iter_mut().zip(all_types) {
                            expr.add_infer_type_mut(typ.clone());
                            queue.push_back(expr);
                        }
                    }
                    InferredType::OneOf(types) => {
                        let mut one_of_types = vec![];
                        for typ in types {
                            if let InferredType::Tuple(types) = typ {
                                one_of_types.extend(types);
                            }
                        }
                        for (expr, typ) in exprs.iter_mut().zip(one_of_types) {
                            expr.add_infer_type_mut(typ.clone());
                            queue.push_back(expr);
                        }
                    }
                    // we can't push down the types otherwise
                    _ => {}
                }
            }
            Expr::Sequence(expressions, inferred_type) => {
                // The inferred_type should be ideally sequence type, i.e, either Unknown type. or all of multiple sequence types, or one of all sequence types,
                // and otherwise we give up inferring the internal type at this phase
                match inferred_type {
                    InferredType::Sequence(types) => {
                        for (expr, typ) in expressions.iter_mut().zip(types) {
                            expr.add_infer_type_mut(typ.clone());
                            queue.push_back(expr);
                        }
                    }
                    InferredType::AllOf(types) => {
                        let mut all_types = vec![];
                        for typ in types {
                            if let InferredType::Sequence(types) = typ {
                                all_types.extend(types);
                            }
                        }
                        for (expr, typ) in expressions.iter_mut().zip(all_types) {
                            expr.add_infer_type_mut(typ.clone());
                            queue.push_back(expr);
                        }
                    }
                    InferredType::OneOf(types) => {
                        let mut one_of_types = vec![];
                        for typ in types {
                            if let InferredType::Sequence(types) = typ {
                                one_of_types.extend(types);
                            }
                        }
                        for (expr, typ) in expressions.iter_mut().zip(one_of_types) {
                            expr.add_infer_type_mut(typ.clone());
                            queue.push_back(expr);
                        }
                    }
                    // we can't push down the types otherwise
                    _ => {}
                }
            }

            // {bohy
            Expr::Record(expressions, inferred_type) => {
                match inferred_type {
                    InferredType::Record(types) => {
                        for (field_name, expr) in expressions.iter_mut() {
                            if let Some((_, typ)) =
                                types.iter().find(|(name, _)| name == field_name)
                            {
                                expr.add_infer_type_mut(typ.clone());
                            }
                            queue.push_back(expr);
                        }
                    }

                    InferredType::AllOf(types) => {
                        internal::handle_all_of_push_down_for_record(
                            types,
                            expressions,
                            &mut queue,
                        );
                    }

                    InferredType::OneOf(types) => {
                        internal::handle_one_of_push_down_for_record(
                            types,
                            expressions,
                            &mut queue,
                        );
                    }
                    // we can't push down the types otherwise
                    _ => {}
                }
            }

            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }
}

mod internal {
    use crate::{ArmPattern, Expr, InferredType};
    use std::collections::{HashMap, VecDeque};

    pub(crate) fn handle_all_of_push_down_for_record<'a>(
        outer_inferred_types: &'a mut Vec<InferredType>,
        inner_expressions: &'a mut [(String, Box<Expr>)],
        push_down_queue: &mut VecDeque<&'a mut Expr>,
    ) {
        handle_push_down_for_record(
            outer_inferred_types,
            inner_expressions,
            push_down_queue,
            InferredType::all_of,
        );
    }

    pub(crate) fn handle_one_of_push_down_for_record<'a>(
        outer_inferred_types: &'a mut Vec<InferredType>,
        inner_expressions: &'a mut [(String, Box<Expr>)],
        push_down_queue: &mut VecDeque<&'a mut Expr>,
    ) {
        handle_push_down_for_record(
            outer_inferred_types,
            inner_expressions,
            push_down_queue,
            InferredType::one_of,
        );
    }

    fn handle_push_down_for_record<'a, F>(
        outer_inferred_types: &'a mut Vec<InferredType>,
        inner_expressions: &'a mut [(String, Box<Expr>)],
        push_down_queue: &mut VecDeque<&'a mut Expr>,
        process_inferred_type: F,
    ) where
        F: Fn(Vec<InferredType>) -> InferredType,
    {
        let mut all_of_types = vec![];

        for typ in outer_inferred_types {
            if let InferredType::Record(types) = typ {
                all_of_types.push(types);
            }
        }

        let mut map = HashMap::new();

        for vec in all_of_types {
            for (key, value) in vec {
                if !value.is_unknown() {
                    map.entry(key).or_insert_with(Vec::new).push(value);
                }
            }
        }
        for (field_name, expr) in inner_expressions.iter_mut() {
            if let Some(types) = map.get(field_name) {
                let new_types = types.iter().map(|x| (**x).clone()).collect::<Vec<_>>();
                expr.add_infer_type_mut(process_inferred_type(new_types));
            }
            push_down_queue.push_back(expr);
        }
    }

    pub(crate) fn update_arm_pattern_type(
        arm_pattern: &mut ArmPattern,
        inferred_type: &InferredType,
    ) {
        match arm_pattern {
            ArmPattern::Literal(expr) => {
                expr.add_infer_type_mut(inferred_type.clone());
                expr.push_types_down()
            }
            ArmPattern::As(_, pattern) => {
                update_arm_pattern_type(pattern, inferred_type);
            }
            ArmPattern::Constructor(constructor_name, patterns) => match inferred_type {
                InferredType::Option(inner_type) => {
                    if constructor_name == "some" || constructor_name == "none" {
                        for pattern in &mut *patterns {
                            update_arm_pattern_type(pattern, inner_type);
                        }
                    }
                }
                InferredType::Result { ok, error } => {
                    if constructor_name == "ok" {
                        if let Some(ok_type) = ok {
                            for pattern in &mut *patterns {
                                update_arm_pattern_type(pattern, ok_type);
                            }
                        }
                    };
                    if constructor_name == "err" {
                        if let Some(err_type) = error {
                            for pattern in &mut *patterns {
                                update_arm_pattern_type(pattern, err_type);
                            }
                        }
                    };
                }
                InferredType::Variant(variant) => {
                    variant
                        .iter()
                        .find(|(name, _optional_type)| name == constructor_name)
                        .iter()
                        .for_each(|(_, optional_type)| {
                            if let Some(inner_type) = optional_type {
                                for pattern in &mut *patterns {
                                    update_arm_pattern_type(pattern, inner_type);
                                }
                            }
                        });
                }
                _ => {}
            },
            ArmPattern::WildCard => {}
        }
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

        expr.push_types_down();
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
}
