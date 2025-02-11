use crate::Expr;
use std::collections::VecDeque;
use std::fmt::{Display, Formatter};

pub struct BinaryMathExprError {
    error_type: BinaryMathExprErrorType,
    op_type: BinaryOpType,
}

impl Display for BinaryMathExprError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.error_type {
            BinaryMathExprErrorType::Left {
                math_expr,
                left_expr,
                left_error,
            } => write!(
                f,
                "`{}` is invalid. `{}` cannot be part of {}. {}",
                math_expr, left_expr, self.op_type, left_error
            ),
            BinaryMathExprErrorType::Both {
                math_expr,
                left_expr,
                left_error,
                right_expr,
                right_error,
            } => {
                write!(
                    f,
                    "`{}` is invalid. `{}` cannot be part of {}. {}. {} cannot be part of {}. {}",
                    math_expr,
                    left_expr,
                    self.op_type,
                    left_error,
                    right_expr,
                    self.op_type,
                    right_error
                )
            }

            BinaryMathExprErrorType::Right {
                math_expr,
                right_expr,
                right_error,
            } => write!(
                f,
                "`{}` is invalid. `{}` cannot be part of {}. {}",
                math_expr, right_expr, self.op_type, right_error
            ),
        }
    }
}

pub enum BinaryMathExprErrorType {
    Both {
        math_expr: String,
        left_expr: Expr,
        left_error: String,
        right_expr: Expr,
        right_error: String,
    },
    Left {
        math_expr: String,
        left_expr: Expr,
        left_error: String,
    },

    Right {
        math_expr: String,
        right_expr: Expr,
        right_error: String,
    },
}

enum BinaryOpType {
    Addition,
    Multiplication,
    Subtraction,
    Division,
}

impl Display for BinaryOpType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryOpType::Addition => write!(f, "addition"),
            BinaryOpType::Multiplication => write!(f, "multiplication"),
            BinaryOpType::Subtraction => write!(f, "subtraction"),
            BinaryOpType::Division => write!(f, "division"),
        }
    }
}

pub fn check_types_in_math_expr(expr: &mut Expr) -> Result<(), BinaryMathExprError> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        let expr_str = expr.to_string();
        match expr {
            Expr::Plus(left_expr, right_expr, _) => {
                if let Err(error_type) =
                    internal::check_math_expression_types(expr_str, left_expr, right_expr)
                {
                    return Err(BinaryMathExprError {
                        error_type,
                        op_type: BinaryOpType::Addition,
                    });
                }
            }
            Expr::Minus(left_expr, right_expr, _) => {
                if let Err(error_type) =
                    internal::check_math_expression_types(expr_str, left_expr, right_expr)
                {
                    return Err(BinaryMathExprError {
                        error_type,
                        op_type: BinaryOpType::Subtraction,
                    });
                }
            }
            Expr::Multiply(left_expr, right_expr, _) => {
                if let Err(error_type) =
                    internal::check_math_expression_types(expr_str, left_expr, right_expr)
                {
                    return Err(BinaryMathExprError {
                        error_type,
                        op_type: BinaryOpType::Multiplication,
                    });
                }
            }
            Expr::Divide(left_expr, right_expr, _) => {
                if let Err(error_type) =
                    internal::check_math_expression_types(expr_str, left_expr, right_expr)
                {
                    return Err(BinaryMathExprError {
                        error_type,
                        op_type: BinaryOpType::Division,
                    });
                }
            }

            expr => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
}

mod internal {
    use crate::type_checker::math::BinaryMathExprErrorType;
    use crate::Expr;

    pub(crate) fn check_math_expression_types(
        original_expr: String,
        left_expr: &Expr,
        right_expr: &Expr,
    ) -> Result<(), BinaryMathExprErrorType> {
        let left_inferred_type = left_expr.inferred_type().as_number();
        let right_inferred_type = right_expr.inferred_type().as_number();

        match (left_inferred_type, right_inferred_type) {
            (Err(left_error), Err(right_error)) => Err(BinaryMathExprErrorType::Both {
                math_expr: original_expr.clone(),
                left_expr: left_expr.clone(),
                left_error,
                right_expr: right_expr.clone(),
                right_error,
            }),
            (Err(left_error), _) => Err(BinaryMathExprErrorType::Left {
                math_expr: original_expr.clone(),
                left_expr: left_expr.clone(),
                left_error,
            }),
            (_, Err(right_error)) => Err(BinaryMathExprErrorType::Right {
                math_expr: original_expr.clone(),
                right_expr: right_expr.clone(),
                right_error,
            }),
            (_, _) => Ok(()),
        }
    }
}
