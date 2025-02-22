use crate::type_inference::kind::TypeKind;
use crate::{Expr, InferredType};
use std::collections::VecDeque;

// Check all exprs that cannot be the type it is tagged against
pub fn check_invalid_expr(expr: &Expr) -> Result<(), InvalidExpr> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Number { inferred_type, .. } => match inferred_type.as_number() {
                Ok(_) => {}
                Err(msg) => {
                    return Err(InvalidExpr {
                        expr: expr.clone(),
                        expected_type: TypeKind::Number,
                        found: inferred_type.clone(),
                        message: msg,
                    });
                }
            },
            _ => expr.visit_children_bottom_up(&mut queue),
        }
    }

    Ok(())
}

pub struct InvalidExpr {
    pub expr: Expr,
    pub expected_type: TypeKind,
    pub found: InferredType,
    pub message: String,
}
