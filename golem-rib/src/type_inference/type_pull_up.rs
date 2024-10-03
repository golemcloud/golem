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

use crate::{Expr, InferredType};
use std::collections::VecDeque;

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

fn type_pull_up_non_recursive<'a>(expr: &'a Expr) {
    let mut expr_queue = VecDeque::new();
    make_expr_queue(expr, &mut expr_queue);

    // select_field(expr, b)
    let mut inferred_type_stack = VecDeque::new();

    // First one will be identifier(a) in Expr::Tuple((identfier(a), identifer(b)))
    while let Some(expr) = expr_queue.pop_back() {
        match expr {
            Expr::Tuple(exprs, current_inferred_type) => {
                let mut ordered_types = vec![];
                let mut new_exprs = vec![];

                for _ in 0..exprs.len() {
                    let expr: Expr = inferred_type_stack.pop_front().unwrap();
                    new_exprs.push(expr.clone());
                    let inferred_type: InferredType = expr.inferred_type();
                    ordered_types.push(inferred_type);
                }

                let new_tuple_type = InferredType::Tuple(ordered_types);

                let merged_tuple_type = current_inferred_type.merge(new_tuple_type);
                let new_tuple = Expr::Tuple(new_exprs.iter().cloned().collect(), merged_tuple_type);
                inferred_type_stack.push_front(new_tuple);
            }

            Expr::Identifier(variable_id, current_inferred_type) => {
                inferred_type_stack
                    .push_front(Expr::Identifier(variable_id.clone(), current_inferred_type.clone()));
            }

            Expr::Flags(flags, current_inferred_type) => {
                inferred_type_stack.push_front(Expr::Flags(flags.clone(), current_inferred_type.clone()));
            }

            Expr::SelectField(_, field, current_inferred_type) => {
                let expr = inferred_type_stack.pop_front().unwrap();
                let inferred_type_of_selection_expr = expr.inferred_type();
                let field_type = internal::get_inferred_type_of_selected_field(field, &inferred_type_of_selection_expr).unwrap();
                let new_select_field = Expr::SelectField(Box::new(expr.clone()), field.clone(), current_inferred_type.merge(field_type));
                inferred_type_stack.push_front(new_select_field);
            }

            Expr::SelectIndex(expr, index, current_inferred_type) => {
                let expr = inferred_type_stack.pop_front().unwrap();
                let inferred_type_of_selection_expr = expr.inferred_type();
                let list_type = internal::get_inferred_type_of_selected_index(*index, &inferred_type_of_selection_expr).unwrap();
                let new_select_index = Expr::SelectIndex(Box::new(expr.clone()), *index, current_inferred_type.merge(list_type));
                inferred_type_stack.push_front(new_select_index);
            }

            Expr::Result(Ok(expr), current_inferred_type) => {
                let expr = inferred_type_stack.pop_front().unwrap();
                let inferred_type_of_ok_expr = expr.inferred_type();
                let result_type = InferredType::Result {
                    ok: Some(Box::new(inferred_type_of_ok_expr)),
                    error: None,
                };
                let new_result = Expr::Result(Ok(Box::new(expr.clone())), current_inferred_type.merge(result_type));
                inferred_type_stack.push_front(new_result);
            }

            Expr::Result(Err(expr), current_inferred_type) => {
                let expr = inferred_type_stack.pop_front().unwrap();
                let inferred_type_of_error_expr = expr.inferred_type();
                let result_type = InferredType::Result {
                    ok: None,
                    error: Some(Box::new(inferred_type_of_error_expr)),
                };
                let new_result = Expr::Result(Err(Box::new(expr.clone())), current_inferred_type.merge(result_type));
                inferred_type_stack.push_front(new_result);
            }

            Expr::Option(Some(expr), current_inferred_type) => {
                let expr = inferred_type_stack.pop_front().unwrap();
                let inferred_type_of_some_expr = expr.inferred_type();
                let option_type = InferredType::Option(Box::new(inferred_type_of_some_expr));
                let new_option = Expr::Option(Some(Box::new(expr.clone())), current_inferred_type.merge(option_type));
                inferred_type_stack.push_front(new_option);
            }

            Expr::Option(None, current_inferred_type) => {
                inferred_type_stack.push_front(Expr::Option(None, current_inferred_type.clone()));
            }

            Expr::Cond(cond, then_, else_, current_inferred_type) => {
                let else_expr = inferred_type_stack.pop_front().unwrap();
                let then_expr = inferred_type_stack.pop_front().unwrap();
                let cond_expr = inferred_type_stack.pop_front().unwrap();
                let inferred_type_of_then_expr = then_expr.inferred_type();
                let inferred_type_of_else_expr = else_expr.inferred_type();

                let new_type = if inferred_type_of_then_expr == inferred_type_of_then_expr {
                    current_inferred_type.merge(inferred_type_of_then_expr)
                } else if let Some(cond_then_else_type) =
                    InferredType::all_of(vec![inferred_type_of_then_expr, inferred_type_of_else_expr])
                {
                    current_inferred_type.merge(cond_then_else_type)
                } else { current_inferred_type.clone() };

                let new_expr =
                    Expr::Cond(Box::new(cond_expr), Box::new(then_expr.clone()), Box::new(else_expr.clone()), new_type);

                inferred_type_stack.push_front(new_expr);

            }

            Expr::PatternMatch(predicate, match_arms, current_inferred_type) => {
                let length = match_arms.len();
                let mut new_resolutions = vec![];
                for _ in 0..length {
                    let arm_resolution = inferred_type_stack.pop_front().unwrap();
                    new_resolutions.push(arm_resolution);
                }

                let inferred_types = new_resolutions.iter().map(|x| x.inferred_type()).collect::<Vec<_>>();
                let new_inferred_type = InferredType::all_of(inferred_types).unwrap();

                let mut total_exprs = 0;
                for i in match_arms {
                    let size = i.arm_pattern.get_expr_literals().len();
                    total_exprs += size;
                }

                let mut new_arm_pattern_exprs = vec![];

                for _ in 0..total_exprs {
                    let expr = inferred_type_stack.pop_front().unwrap();
                    new_arm_pattern_exprs.push(expr);
                }

                let mut new_arm_patterns = vec![];

                for match_arm in match_arms {
                    let mut arm_pattern = match_arm.arm_pattern.clone();
                    let mut arm_pattern_exprs = arm_pattern.get_expr_literals_mut();
                    arm_pattern_exprs.iter_mut().zip(new_arm_pattern_exprs.iter()).for_each(|(arm_expr, new_expr)| {
                        *arm_expr = &mut Box::new(new_expr.clone());
                    });

                    new_arm_patterns.push(arm_pattern);
                }


                let new_match_arms = new_arm_patterns.iter().zip(new_resolutions.iter()).map(|(arm_pattern, arm_resolution)| {
                    crate::MatchArm {
                        arm_pattern: arm_pattern.clone(),
                        arm_resolution_expr: Box::new(arm_resolution.clone()),
                    }
                }).collect::<Vec<_>>();


                let new_expr = Expr::PatternMatch(predicate.clone(), new_match_arms, current_inferred_type.merge(new_inferred_type));

                inferred_type_stack.push_front(new_expr);
            }

            Expr::Concat(exprs, current_inferred_type) => {
                let mut new_exprs = vec![];
                for _ in 0..exprs.len() {
                    let expr = inferred_type_stack.pop_front().unwrap();
                    new_exprs.push(expr);
                }

                let new_concat = Expr::Concat(new_exprs, InferredType::Str);
                inferred_type_stack.push_front(new_concat);
            }

            Expr::Multiple(exprs, current_inferred_type) => {
                let length = exprs.len();
                let mut new_exprs = vec![];
                for _ in 0..length {
                    let expr = inferred_type_stack.pop_front().unwrap();
                    new_exprs.push(expr);
                }

                let new_inferred_type = new_exprs.last().unwrap().inferred_type();

                let new_multiple = Expr::Multiple(new_exprs, current_inferred_type.merge(new_inferred_type));
                inferred_type_stack.push_front(new_multiple);
            }

            Expr::Not(_, current_inferred_type) => {
                let expr = inferred_type_stack.pop_front().unwrap();
                let new_not = Expr::Not(Box::new(expr), current_inferred_type.clone());
                inferred_type_stack.push_front(new_not);
            }

            Expr::GreaterThan(_, _, current_inferred_type) => {
                let right_expr = inferred_type_stack.pop_front().unwrap();
                let left_expr = inferred_type_stack.pop_front().unwrap();
                let new_greater_than = Expr::GreaterThan(Box::new(left_expr), Box::new(right_expr), current_inferred_type.clone());
                inferred_type_stack.push_front(new_greater_than);
            }

            Expr::GreaterThanOrEqualTo(_, _, current_inferred_type) => {
                let right_expr = inferred_type_stack.pop_front().unwrap();
                let left_expr = inferred_type_stack.pop_front().unwrap();
                let new_greater_than_or_equal_to = Expr::GreaterThanOrEqualTo(Box::new(left_expr), Box::new(right_expr), current_inferred_type.clone());
                inferred_type_stack.push_front(new_greater_than_or_equal_to);
            }

            Expr::LessThanOrEqualTo(_, _, current_inferred_type) => {
                let right_expr = inferred_type_stack.pop_front().unwrap();
                let left_expr = inferred_type_stack.pop_front().unwrap();
                let new_less_than_or_equal_to = Expr::LessThanOrEqualTo(Box::new(left_expr), Box::new(right_expr), current_inferred_type.clone());
                inferred_type_stack.push_front(new_less_than_or_equal_to);
            }

            Expr::EqualTo(_, _, current_inferred_type) => {
                let right_expr = inferred_type_stack.pop_front().unwrap();
                let left_expr = inferred_type_stack.pop_front().unwrap();
                let new_equal_to = Expr::EqualTo(Box::new(left_expr), Box::new(right_expr), current_inferred_type.clone());
                inferred_type_stack.push_front(new_equal_to);
            }

            Expr::LessThan(_, _, current_inferred_type) => {
                let right_expr = inferred_type_stack.pop_front().unwrap();
                let left_expr = inferred_type_stack.pop_front().unwrap();
                let new_less_than = Expr::LessThan(Box::new(left_expr), Box::new(right_expr), current_inferred_type.clone());
                inferred_type_stack.push_front(new_less_than);
            }

            _ => {
                inferred_type_stack.push_front(expr.clone());
            }
        }
    }
}

fn make_expr_queue<'a>(expr: &'a Expr, expr_queue: &mut VecDeque<&'a Expr>) {
    let mut stack = VecDeque::new();

    stack.push_back(expr);

    while let Some(current_expr) = stack.pop_back() {
        expr_queue.push_back(current_expr);

        expr.visit_children_bottom_up(&mut stack)
    }
}

pub fn pull_types_up(expr: &mut Expr) -> Result<(), String> {
    match expr {
        Expr::Tuple(exprs, inferred_type) => {
            let mut types = vec![];
            for expr in exprs {
                expr.pull_types_up()?;
                types.push(expr.inferred_type());
            }
            let tuple_type = InferredType::Tuple(types);
            *inferred_type = inferred_type.merge(tuple_type)
        }
        Expr::Sequence(exprs, inferred_type) => {
            let mut types = vec![];
            for expr in exprs {
                expr.pull_types_up()?;
                types.push(expr.inferred_type());
            }
            if let Some(new_inferred_type) = types.first() {
                let sequence_type = InferredType::List(Box::new(new_inferred_type.clone()));
                *inferred_type = inferred_type.merge(sequence_type)
            }
        }
        Expr::Record(exprs, inferred_type) => {
            let mut types = vec![];
            for (field_name, expr) in exprs {
                expr.pull_types_up()?;
                types.push((field_name.clone(), expr.inferred_type()));
            }
            let record_type = InferredType::Record(types);
            *inferred_type = inferred_type.merge(record_type);
        }
        Expr::Option(Some(expr), inferred_type) => {
            expr.pull_types_up()?;
            let option_type = InferredType::Option(Box::new(expr.inferred_type()));
            *inferred_type = inferred_type.merge(option_type)
        }
        Expr::Result(Ok(expr), inferred_type) => {
            expr.pull_types_up()?;
            let result_type = InferredType::Result {
                ok: Some(Box::new(expr.inferred_type())),
                error: None,
            };
            *inferred_type = inferred_type.merge(result_type)
        }
        Expr::Result(Err(expr), inferred_type) => {
            expr.pull_types_up()?;
            let result_type = InferredType::Result {
                ok: None,
                error: Some(Box::new(expr.inferred_type())),
            };
            *inferred_type = inferred_type.merge(result_type)
        }

        Expr::Cond(_, then_, else_, inferred_type) => {
            then_.pull_types_up()?;
            else_.pull_types_up()?;
            let then_type = then_.inferred_type();
            let else_type = else_.inferred_type();

            if then_type == else_type {
                *inferred_type = inferred_type.merge(then_type);
            } else if let Some(cond_then_else_type) =
                InferredType::all_of(vec![then_type, else_type])
            {
                *inferred_type = inferred_type.merge(cond_then_else_type);
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
                    *inferred_type = inferred_type.merge(first_type);
                } else if let Some(all_of) = InferredType::all_of(possible_inference_types) {
                    *inferred_type = inferred_type.merge(all_of);
                }
            }
        }
        Expr::Let(_, _, expr, _) => expr.pull_types_up()?,
        Expr::SelectField(expr, field, inferred_type) => {
            expr.pull_types_up()?;
            let expr_type = expr.inferred_type();
            let field_type = internal::get_inferred_type_of_selected_field(field, &expr_type)?;
            *inferred_type = inferred_type.merge(field_type);
        }

        Expr::SelectIndex(expr, index, inferred_type) => {
            expr.pull_types_up()?;
            let expr_type = expr.inferred_type();
            let list_type = internal::get_inferred_type_of_selected_index(*index, &expr_type)?;
            *inferred_type = inferred_type.merge(list_type);
        }
        Expr::Literal(_, _) => {}
        Expr::Number(_, _, _) => {}
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
                    *inferred_type = inferred_type.merge(expr.inferred_type());
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
        Expr::And(left, right, _) => {
            left.pull_types_up()?;
            right.pull_types_up()?;
        }
        Expr::Throw(_, _) => {}
        Expr::GetTag(expr, _) => expr.pull_types_up()?,
        Expr::Option(None, _) => {}
    }

    Ok(())
}

mod internal {
    use crate::type_refinement::precise_types::{ListType, RecordType};
    use crate::type_refinement::TypeRefinement;
    use crate::{ArmPattern, InferredType};

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
            ArmPattern::TupleConstructor(arm_patterns) => {
                for arm_pattern in arm_patterns {
                    pull_up_types_of_arm_pattern(arm_pattern)?;
                }
            }

            ArmPattern::ListConstructor(arm_patterns) => {
                for arm_pattern in arm_patterns {
                    pull_up_types_of_arm_pattern(arm_pattern)?;
                }
            }

            ArmPattern::RecordConstructor(fields) => {
                for (_, arm_pattern) in fields {
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
    use crate::function_name::DynamicParsedFunctionName;
    use crate::{ArmPattern, Expr, InferredType, Number};

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
                Expr::Number(Number { value: 1f64 }, None, InferredType::U64),
                Expr::Number(Number { value: 1f64 }, None, InferredType::U32),
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
            Expr::Number(Number { value: 1f64 }, None, InferredType::U64),
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
                    Box::new(Expr::Number(
                        Number { value: 1f64 },
                        None,
                        InferredType::U64,
                    )),
                ),
                (
                    "bar".to_string(),
                    Box::new(Expr::Number(
                        Number { value: 1f64 },
                        None,
                        InferredType::U64,
                    )),
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
                    ("foo".to_string(), InferredType::U64),
                    ("bar".to_string(), InferredType::U64)
                ]),
                InferredType::Record(vec![
                    ("foo".to_string(), InferredType::Unknown),
                    ("bar".to_string(), InferredType::Unknown)
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
            DynamicParsedFunctionName::parse("global_fn").unwrap(),
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
                        None,
                        InferredType::U64,
                    ))),
                    arm_resolution_expr: Box::new(Expr::Number(
                        Number { value: 1f64 },
                        None,
                        InferredType::U64,
                    )),
                },
                crate::MatchArm {
                    arm_pattern: ArmPattern::Literal(Box::new(Expr::Number(
                        Number { value: 2f64 },
                        None,
                        InferredType::U64,
                    ))),
                    arm_resolution_expr: Box::new(Expr::Number(
                        Number { value: 2f64 },
                        None,
                        InferredType::U64,
                    )),
                },
            ],
        );
        expr.pull_types_up().unwrap();
        assert_eq!(expr.inferred_type(), InferredType::U64);
    }
}
