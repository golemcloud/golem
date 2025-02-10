use std::collections::VecDeque;
use crate::{Expr};

pub fn check_math_op_types(
    expr: &mut Expr,
) -> Result<(), String> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            expr @ Expr::Plus(left, right, _) => {
                let left_inferred_type = left.inferred_type().as_number();
                let right_inferred_type = right.inferred_type().as_number();

                match (left_inferred_type, right_inferred_type) {
                    (Err(left), Err(right)) => return Err(format!("Addition with invalid type of values. {} {}. {}", expr.to_string(), left, right)),
                    (Err(left), _) => return Err(format!("Addition with invalid type of values. {}. {}", expr.to_string(), left)),
                    (_,  Err(right)) => return Err(format!("Addition with invalid type of values. {}. {}", expr.to_string(), right)),
                    (_, _) => {}
                }
            },
            expr @ Expr::Minus(left, right, _) => {
                let left_inferred_type = left.inferred_type().as_number();
                let right_inferred_type = right.inferred_type().as_number();

                match (left_inferred_type, right_inferred_type) {
                    (Err(left), Err(right)) => return Err(format!("Subtraction with invalid type of values. {} {}. {}", expr.to_string(), left, right)),
                    (Err(left), _) => return Err(format!("Subtraction with invalid type of values. {}. {}", expr.to_string(), left)),
                    (_,  Err(right)) => return Err(format!("Subtraction with invalid type of values. {}. {}", expr.to_string(), right)),
                    (_, _) => {}
                }
            },
            expr @ Expr::Multiply(left, right, _) => {
                let left_inferred_type = left.inferred_type().as_number();
                let right_inferred_type = right.inferred_type().as_number();

                match (left_inferred_type, right_inferred_type) {
                    (Err(left), Err(right)) => return Err(format!("Multiply with invalid type of values. {} {}. {}", expr.to_string(), left, right)),
                    (Err(left), _) => return Err(format!("Multiply with invalid type of values. {}. {}", expr.to_string(), left)),
                    (_,  Err(right)) => return Err(format!("Multiply with invalid type of values. {}. {}", expr.to_string(), right)),
                    (_, _) => {}
                }
            },
            expr @ Expr::Divide(left, right, _) => {
                let left_inferred_type = left.inferred_type().as_number();
                let right_inferred_type = right.inferred_type().as_number();

                match (left_inferred_type, right_inferred_type) {
                    (Err(left), Err(right)) => return Err(format!("Divide with invalid type of values. {} {}. {}", expr.to_string(), left, right)),
                    (Err(left), _) => return Err(format!("Divide with invalid type of values. {}. {}", expr.to_string(), left)),
                    (_,  Err(right)) => return Err(format!("Divide with invalid type of values. {}. {}", expr.to_string(), right)),
                    (_, _) => {}
                }
            }

            expr => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
}