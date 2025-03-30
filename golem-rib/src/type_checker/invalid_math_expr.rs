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

use crate::{Expr, ExprVisitor};
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
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_back() {
        if let Expr::Plus { lhs, rhs, .. }
        | Expr::Minus { lhs, rhs, .. }
        | Expr::Multiply { lhs, rhs, .. }
        | Expr::Divide { lhs, rhs, .. } = &expr
        {
            check_math_expression_types(expr, lhs, rhs)?;
        }
    }

    Ok(())
}

fn check_math_expression_types(
    original_expr: &Expr,
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
