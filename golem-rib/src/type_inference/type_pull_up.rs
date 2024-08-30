use crate::{Expr, InferredType};

// TODO; This is recursion because we bumped into Rust borrowing issues with the following logic,
// which may require changing Expr data structure with RefCells.
// Logic that we need:
//   * Fill up a queue with the root node being first
//  [select_field(select_field(a, b), c), select_field(a, b), identifier(a)]
//  Pop from back and push to the front of a stack of the current expression's inferred type, and keep assigning in between
// Example:
//  * Pop back to get identifier(a)
//  * Try to pop_front inferred_type_stack, and its None. Push front the identifier(a)'s inferred_type:  Record(b -> Record(c -> u64))
//  * Pop back from stack to get select_field(a, b)
//  * Try to pop_front inferred_type_stack, and its Record(b -> Record(c -> u64)). Get the type of b and assign itself and push_front to stack.
//  * Pop back from stack to get select_field(select_field(a, b), c)
//  * Try to pop_front inferred_type_stack, and its  Record(c -> u64). Get the type of c and assign itself and push to stack.
pub fn pull_types_up(expr: &mut Expr) -> Result<(), String> {
    match expr {
        Expr::Tuple(exprs, inferred_type) => {
            let mut types = vec![];
            for expr in exprs {
                expr.pull_types_up()?;
                types.push(expr.inferred_type());
            }
            let tuple_type = InferredType::Tuple(types);
            inferred_type.update(tuple_type)
        }
        Expr::Sequence(exprs, inferred_type) => {
            let mut types = vec![];
            for expr in exprs {
                expr.pull_types_up()?;
                types.push(expr.inferred_type());
            }
            if let Some(new_inferred_type) = types.first() {
                let sequence_type = InferredType::List(Box::new(new_inferred_type.clone()));
                inferred_type.update(sequence_type)
            }
        }
        Expr::Record(exprs, inferred_type) => {
            let mut types = vec![];
            for (field_name, expr) in exprs {
                expr.pull_types_up()?;
                types.push((field_name.clone(), expr.inferred_type()));
            }
            let record_type = InferredType::Record(types);
            inferred_type.update(record_type)
        }
        Expr::Option(Some(expr), inferred_type) => {
            expr.pull_types_up()?;
            let option_type = InferredType::Option(Box::new(expr.inferred_type()));
            inferred_type.update(option_type)
        }
        Expr::Result(Ok(expr), inferred_type) => {
            expr.pull_types_up()?;
            let result_type = InferredType::Result {
                ok: Some(Box::new(expr.inferred_type())),
                error: None,
            };
            inferred_type.update(result_type)
        }
        Expr::Result(Err(expr), inferred_type) => {
            expr.pull_types_up()?;
            let result_type = InferredType::Result {
                ok: None,
                error: Some(Box::new(expr.inferred_type())),
            };
            inferred_type.update(result_type)
        }

        Expr::Cond(_, then_, else_, inferred_type) => {
            then_.pull_types_up()?;
            else_.pull_types_up()?;
            let then_type = then_.inferred_type();
            let else_type = else_.inferred_type();

            if then_type == else_type {
                inferred_type.update(then_type);
            } else if let Some(cond_then_else_type) =
                InferredType::all_of(vec![then_type, else_type])
            {
                inferred_type.update(cond_then_else_type);
            }
        }

        // When it comes to pattern match, the only way to resolve the type of the pattern match
        // from children (pulling types up) is from the match_arms
        Expr::PatternMatch(predicate, match_arms, inferred_type) => {
            predicate.pull_types_up()?;
            let mut possible_inference_types = vec![];

            for match_arm in match_arms {
                internal::pull_up_types_of_arm_pattern(&mut match_arm.arm_pattern)?;

                match_arm.arm_resolution_expr.pull_types_up()?;
                possible_inference_types.push(match_arm.arm_resolution_expr.inferred_type())
            }

            if !possible_inference_types.is_empty() {
                let first_type = possible_inference_types[0].clone();
                if possible_inference_types.iter().all(|t| t == &first_type) {
                    inferred_type.update(first_type);
                } else if let Some(all_of) = InferredType::all_of(possible_inference_types) {
                    inferred_type.update(all_of);
                }
            }
        }
        Expr::Let(_, _, expr, _) => expr.pull_types_up()?,
        Expr::SelectField(expr, field, inferred_type) => {
            expr.pull_types_up()?;
            let expr_type = expr.inferred_type();
            let field_type = internal::get_inferred_type_of_selected_field(field, &expr_type)?;
            inferred_type.update(field_type);
        }

        Expr::SelectIndex(expr, index, inferred_type) => {
            expr.pull_types_up()?;
            let expr_type = expr.inferred_type();
            let list_type = internal::get_inferred_type_of_selected_index(*index, &expr_type)?;
            inferred_type.update(list_type);
        }
        Expr::Literal(_, _) => {}
        Expr::Number(_, _) => {}
        Expr::Flags(_, _) => {}
        Expr::Identifier(_, _) => {}
        Expr::Boolean(_, _) => {}
        Expr::Concat(exprs, _) => {
            for expr in exprs {
                expr.pull_types_up()?
            }
        }
        Expr::Multiple(exprs, inferred_type) => {
            let length = &exprs.len();
            for (index, expr) in exprs.iter_mut().enumerate() {
                expr.pull_types_up()?;

                if index == length - 1 {
                    inferred_type.update(expr.inferred_type());
                }
            }
        }
        Expr::Not(expr, _) => expr.pull_types_up()?,
        Expr::GreaterThan(left, right, _) => {
            left.pull_types_up()?;
            right.pull_types_up()?;
        }
        Expr::GreaterThanOrEqualTo(left, right, _) => {
            left.pull_types_up()?;
            right.pull_types_up()?;
        }
        Expr::LessThanOrEqualTo(left, right, _) => {
            left.pull_types_up()?;
            right.pull_types_up()?;
        }
        Expr::EqualTo(left, right, _) => {
            left.pull_types_up()?;
            right.pull_types_up()?;
        }
        Expr::LessThan(left, right, _) => {
            left.pull_types_up()?;
            right.pull_types_up()?;
        }
        Expr::Call(_, exprs, _) => {
            for expr in exprs {
                expr.pull_types_up()?
            }
        }
        Expr::Unwrap(expr, _) => expr.pull_types_up()?,
        Expr::Throw(_, _) => {}
        Expr::Tag(expr, _) => expr.pull_types_up()?,
        Expr::Option(None, _) => {}
    }

    Ok(())
}

mod internal {
    use crate::type_inference::precise_types::{ListType, RecordType};
    use crate::{ArmPattern, InferredType, TypeRefinement};

    pub(crate) fn get_inferred_type_of_selected_field(
        select_field: &str,
        select_from_type: &InferredType,
    ) -> Result<InferredType, String> {
        let refined_record = RecordType::refine(select_from_type).ok_or(format!(
            "Cannot select {} since it is not a record type. Found: {:?}",
            select_field, select_from_type
        ))?;

        Ok(refined_record.inner_type_by_field(select_field))
    }

    pub(crate) fn get_inferred_type_of_selected_index(
        selected_index: usize,
        select_from_type: &InferredType,
    ) -> Result<InferredType, String> {
        let refined_list = ListType::refine(select_from_type).ok_or(format!(
            "Cannot get index {} since it is not a list type. Found: {:?}",
            selected_index, select_from_type
        ))?;

        Ok(refined_list.inner_type())
    }

    pub(crate) fn pull_up_types_of_arm_pattern(arm_pattern: &mut ArmPattern) -> Result<(), String> {
        match arm_pattern {
            ArmPattern::WildCard => {}
            ArmPattern::As(_, arms_patterns) => {
                pull_up_types_of_arm_pattern(arms_patterns)?;
            }
            ArmPattern::Constructor(_, arm_patterns) => {
                for arm_pattern in arm_patterns {
                    pull_up_types_of_arm_pattern(arm_pattern)?;
                }
            }
            ArmPattern::Literal(expr) => {
                expr.pull_types_up()?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod type_pull_up_tests {
    use crate::{ArmPattern, Expr, InferredType, Number, ParsedFunctionName};

    #[test]
    pub fn test_pull_up_identifier() {
        let expr = "foo";
        let mut expr = Expr::from_text(expr).unwrap();
        expr.add_infer_type_mut(InferredType::Str);
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::Str);
    }

    #[test]
    pub fn test_pull_up_for_select_field() {
        let record_identifier =
            Expr::identifier("foo").add_infer_type(InferredType::Record(vec![(
                "foo".to_string(),
                InferredType::Record(vec![("bar".to_string(), InferredType::U64)]),
            )]));
        let select_expr = Expr::select_field(record_identifier, "foo");
        let mut expr = Expr::select_field(select_expr, "bar");
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::U64);
    }

    #[test]
    pub fn test_pull_up_for_select_index() {
        let expr =
            Expr::identifier("foo").add_infer_type(InferredType::List(Box::new(InferredType::U64)));
        let mut expr = Expr::select_index(expr, 0);
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::U64);
    }

    #[test]
    pub fn test_pull_up_for_sequence() {
        let mut expr = Expr::Sequence(
            vec![
                Expr::Number(Number { value: 1f64 }, InferredType::U64),
                Expr::Number(Number { value: 1f64 }, InferredType::U32),
            ],
            InferredType::Unknown,
        );
        expr.pull_types_up().unwrap();
        assert_eq!(
            expr.inferred_type(),
            InferredType::List(Box::new(InferredType::U64))
        );
    }

    #[test]
    pub fn test_pull_up_for_tuple() {
        let mut expr = Expr::tuple(vec![
            Expr::literal("foo"),
            Expr::Number(Number { value: 1f64 }, InferredType::U64),
        ]);
        expr.pull_types_up().unwrap();
        assert_eq!(
            expr.inferred_type(),
            InferredType::Tuple(vec![InferredType::Str, InferredType::U64])
        );
    }

    #[test]
    pub fn test_pull_up_for_record() {
        let mut expr = Expr::Record(
            vec![
                (
                    "foo".to_string(),
                    Box::new(Expr::Number(Number { value: 1f64 }, InferredType::U64)),
                ),
                (
                    "bar".to_string(),
                    Box::new(Expr::Number(Number { value: 1f64 }, InferredType::U64)),
                ),
            ],
            InferredType::Record(vec![
                ("foo".to_string(), InferredType::Unknown),
                ("bar".to_string(), InferredType::Unknown),
            ]),
        );
        expr.pull_types_up().unwrap();

        assert_eq!(
            expr.inferred_type(),
            InferredType::AllOf(vec![
                InferredType::Record(vec![
                    ("foo".to_string(), InferredType::Unknown),
                    ("bar".to_string(), InferredType::Unknown)
                ]),
                InferredType::Record(vec![
                    ("foo".to_string(), InferredType::U64),
                    ("bar".to_string(), InferredType::U64)
                ])
            ])
        );
    }

    #[test]
    pub fn test_pull_up_for_concat() {
        let mut expr = Expr::concat(vec![Expr::number(1f64), Expr::number(2f64)]);
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::Str);
    }

    #[test]
    pub fn test_pull_up_for_not() {
        let mut expr = Expr::not(Expr::boolean(true));
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::Bool);
    }

    #[test]
    pub fn test_pull_up_for_greater_than() {
        let mut expr = Expr::greater_than(Expr::number(1f64), Expr::number(2f64));
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::Bool);
    }

    #[test]
    pub fn test_pull_up_for_greater_than_or_equal_to() {
        let mut expr = Expr::greater_than_or_equal_to(Expr::number(1f64), Expr::number(2f64));
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::Bool);
    }

    #[test]
    pub fn test_pull_up_for_less_than_or_equal_to() {
        let mut expr = Expr::less_than_or_equal_to(Expr::number(1f64), Expr::number(2f64));
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::Bool);
    }

    #[test]
    pub fn test_pull_up_for_equal_to() {
        let mut expr = Expr::equal_to(Expr::number(1f64), Expr::number(2f64));
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::Bool);
    }

    #[test]
    pub fn test_pull_up_for_less_than() {
        let mut expr = Expr::less_than(Expr::number(1f64), Expr::number(2f64));
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::Bool);
    }

    #[test]
    pub fn test_pull_up_for_call() {
        let mut expr = Expr::call(
            ParsedFunctionName::parse("global_fn").unwrap(),
            vec![Expr::number(1f64)],
        );
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::Unknown);
    }

    #[test]
    pub fn test_pull_up_for_unwrap() {
        let mut expr = Expr::option(Some(Expr::number(1f64))).unwrap();
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::Unknown);
    }

    #[test]
    pub fn test_pull_up_for_tag() {
        let mut expr = Expr::tag(Expr::number(1f64));
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::Unknown);
    }

    #[test]
    pub fn test_pull_up_for_pattern_match() {
        let mut expr = Expr::pattern_match(
            Expr::number(1f64),
            vec![
                crate::MatchArm {
                    arm_pattern: ArmPattern::Literal(Box::new(Expr::Number(
                        Number { value: 1f64 },
                        InferredType::U64,
                    ))),
                    arm_resolution_expr: Box::new(Expr::Number(
                        Number { value: 1f64 },
                        InferredType::U64,
                    )),
                },
                crate::MatchArm {
                    arm_pattern: ArmPattern::Literal(Box::new(Expr::Number(
                        Number { value: 2f64 },
                        InferredType::U64,
                    ))),
                    arm_resolution_expr: Box::new(Expr::Number(
                        Number { value: 2f64 },
                        InferredType::U64,
                    )),
                },
            ],
        );
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::U64);
    }
}
