use crate::Expr;
use std::collections::VecDeque;
use std::fmt::Display;

pub enum InvalidMathExprError {
    Both {
        math_expr: Expr,
        left_error: String,
        right_error: String,
    },
    Left {
        math_expr: Expr,
        left_error: String,
    },

    Right {
        math_expr: Expr,
        right_error: String,
    },
}

pub fn check_invalid_math_expr(expr: &mut Expr) -> Result<(), InvalidMathExprError> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        let copied = expr.clone();
        if let Expr::Plus { lhs, rhs, .. }
        | Expr::Minus { lhs, rhs, .. }
        | Expr::Multiply { lhs, rhs, .. }
        | Expr::Divide { lhs, rhs, .. } = expr
        {
            check_math_expression_types(copied, lhs, rhs)?;
        }

        expr.visit_children_mut_bottom_up(&mut queue);
    }

    Ok(())
}

fn check_math_expression_types(
    original_expr: Expr,
    left_expr: &Expr,
    right_expr: &Expr,
) -> Result<(), InvalidMathExprError> {
    let left_inferred_type = left_expr.inferred_type().as_number();
    let right_inferred_type = right_expr.inferred_type().as_number();

    match (left_inferred_type, right_inferred_type) {
        (Err(left_error), Err(right_error)) => Err(InvalidMathExprError::Both {
            math_expr: original_expr.clone(),
            left_error,
            right_error,
        }),
        (Err(left_error), _) => Err(InvalidMathExprError::Left {
            math_expr: original_expr.clone(),
            left_error,
        }),
        (_, Err(right_error)) => Err(InvalidMathExprError::Right {
            math_expr: original_expr.clone(),
            right_error,
        }),
        (_, _) => Ok(()),
    }
}
