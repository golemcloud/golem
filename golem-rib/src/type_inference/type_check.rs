use crate::Expr;
use std::collections::VecDeque;

pub fn type_check(expr: &Expr) -> Result<(), Vec<String>> {
    let mut queue = VecDeque::new();

    let mut errors = vec![];

    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Record(vec, inferred_type) => {
                queue.extend(vec.iter().map(|(_, expr)| expr.as_ref()));
                internal::accumulate_errors(expr, inferred_type.type_check(), &mut errors);
            }
            Expr::Tuple(vec, inferred_type) => {
                queue.extend(vec.iter());
                internal::accumulate_errors(expr, inferred_type.type_check(), &mut errors);
            }
            Expr::Sequence(vec, inferred_type) => {
                queue.extend(vec.iter());
                internal::accumulate_errors(expr, inferred_type.type_check(), &mut errors);
            }
            Expr::Option(Some(inner), inferred_type) => {
                queue.push_back(inner);
                internal::accumulate_errors(expr, inferred_type.type_check(), &mut errors);
            }
            Expr::Result(Ok(inner), inferred_type) => {
                queue.push_back(inner);
                internal::accumulate_errors(expr, inferred_type.type_check(), &mut errors);
            }
            Expr::Result(Err(inner), inferred_type) => {
                queue.push_back(inner);
                internal::accumulate_errors(expr, inferred_type.type_check(), &mut errors);
            }
            Expr::Cond(cond, then, else_, inferred_type) => {
                queue.push_back(cond);
                queue.push_back(then);
                queue.push_back(else_);
                internal::accumulate_errors(expr, inferred_type.type_check(), &mut errors);
            }
            Expr::PatternMatch(expr, arms, inferred_type) => {
                queue.push_back(expr);
                for _arm in arms {
                    // TODO
                    //let mut arm_expr = arm.arm_expr();
                    //queue.push_back(&mut arm_expr);
                }
                internal::accumulate_errors(expr, inferred_type.type_check(), &mut errors);
            }
            Expr::Call(_, vec, inferred_type) => {
                queue.extend(vec.iter());
                internal::accumulate_errors(expr, inferred_type.type_check(), &mut errors);
            }
            Expr::SelectField(inner, _, inferred_type) => {
                queue.push_back(inner);
                internal::accumulate_errors(expr, inferred_type.type_check(), &mut errors);
            }
            Expr::SelectIndex(inner, _, inferred_type) => {
                queue.push_back(inner);
                internal::accumulate_errors(expr, inferred_type.type_check(), &mut errors);
            }
            _ => expr.visit_children_bottom_up(&mut queue),
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.into_iter().map(|e| e.0).collect())
    }
}

mod internal {
    use crate::{Expr, TypeErrorMessage};

    pub(crate) fn accumulate_errors<A>(
        expr: &Expr,
        result: Result<A, Vec<TypeErrorMessage>>,
        errors: &mut Vec<TypeErrorMessage>,
    ) {
        match result {
            Ok(_) => {}
            Err(errs) => {
                let error_message = format!("Type error: {}", expr);
                errors.push(TypeErrorMessage(error_message));
                errors.extend(errs)
            }
        }
    }
}
