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
pub fn pull_types_up(expr: &mut Expr) {
    match expr {
        Expr::Tuple(exprs, inferred_type) => {
            let mut types = vec![];
            for expr in exprs {
                types.push(expr.inferred_type());
            }
            let tuple_type = InferredType::Tuple(types);
            inferred_type.update(tuple_type)
        }
        Expr::Sequence(exprs, inferred_type) => {
            let mut types = vec![];
            for expr in exprs {
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
                types.push((field_name.clone(), expr.inferred_type()));
            }
            let record_type = InferredType::Record(types);
            inferred_type.update(record_type)
        }
        Expr::Option(Some(expr), inferred_type) => {
            let option_type = InferredType::Option(Box::new(expr.inferred_type()));
            inferred_type.update(option_type)
        }
        Expr::Result(Ok(expr), inferred_type) => {
            let result_type = InferredType::Result {
                ok: Some(Box::new(expr.inferred_type())),
                error: None,
            };
            inferred_type.update(result_type)
        }
        Expr::Result(Err(expr), inferred_type) => {
            let result_type = InferredType::Result {
                ok: None,
                error: Some(Box::new(expr.inferred_type())),
            };
            inferred_type.update(result_type)
        }

        Expr::Cond(_, then_, else_, inferred_type) => {
            then_.pull_types_up();
            else_.pull_types_up();
            let then_type = then_.inferred_type();
            let else_type = else_.inferred_type();

            if then_type == else_type {
                inferred_type.update(then_type);
            } else {
                let cond_then_else_type = InferredType::AllOf(vec![then_type, else_type]);
                inferred_type.update(cond_then_else_type)
            }
        }

        // When it comes to pattern match, the only way to resolve the type of the pattern match
        // from children (pulling types up) is from the match_arms
        Expr::PatternMatch(_, match_arms, inferred_type) => {
            let mut possible_inference_types = vec![];
            for match_arm in match_arms {
                // match_arm.arm_resolution_expr.pull_types_up();
                possible_inference_types.push(match_arm.arm_resolution_expr.inferred_type())
            }

            if !possible_inference_types.is_empty() {
                let first_type = possible_inference_types[0].clone();
                if possible_inference_types.iter().all(|t| t == &first_type) {
                    inferred_type.update(first_type);
                } else {
                    inferred_type.update(InferredType::AllOf(possible_inference_types));
                }
            }
        }
        Expr::Let(_, expr, _) => expr.pull_types_up(),
        Expr::SelectField(expr, field, inferred_type) => {
            expr.pull_types_up();
            let expr_type = expr.inferred_type();
            if let InferredType::Record(fields) = expr_type {
                for (field_name, field_type) in fields {
                    if field_name == *field {
                        inferred_type.update(field_type);
                        break;
                    }
                }
            }
        }
        Expr::SelectIndex(expr, _, inferred_type) => {
            expr.pull_types_up();
            let expr_type = expr.inferred_type();
            if let InferredType::List(inner_type) = expr_type {
                inferred_type.update(*inner_type);
            }
        }
        Expr::Literal(_, _) => {}
        Expr::Number(_, _) => {}
        Expr::Flags(_, _) => {}
        Expr::Identifier(_, _) => {}
        Expr::Boolean(_, _) => {}
        Expr::Concat(exprs, _) => {
            for expr in exprs {
                expr.pull_types_up()
            }
        }
        Expr::Multiple(exprs, _) => {
            for expr in exprs {
                expr.pull_types_up()
            }
        }
        Expr::Not(expr, _) => expr.pull_types_up(),
        Expr::GreaterThan(left, right, _) => {
            left.pull_types_up();
            right.pull_types_up();
        }
        Expr::GreaterThanOrEqualTo(left, right, _) => {
            left.pull_types_up();
            right.pull_types_up();
        }
        Expr::LessThanOrEqualTo(left, right, _) => {
            left.pull_types_up();
            right.pull_types_up();
        }
        Expr::EqualTo(left, right, _) => {
            left.pull_types_up();
            right.pull_types_up();
        }
        Expr::LessThan(left, right, _) => {
            left.pull_types_up();
            right.pull_types_up();
        }
        Expr::Call(_, exprs, _) => {
            for expr in exprs {
                expr.pull_types_up()
            }
        }
        Expr::Unwrap(expr, _) => expr.pull_types_up(),
        Expr::Throw(_, _) => {}
        Expr::Tag(expr, _) => expr.pull_types_up(),
        Expr::Option(None, _) => {}
    }
}
