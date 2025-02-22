use crate::{Expr, InferredType, TypeName};
use std::collections::VecDeque;
use crate::type_inference::kind::TypeKind;

pub fn check_invalid_type_cast(expr: &Expr) -> Result<(), InvalidTypeCast> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Number { inferred_type, .. } => match inferred_type.as_number() {
                Ok(_) => {}
                Err(msg) => {
                    return Err(InvalidTypeCast {
                        expr: expr.clone(),
                        expected: TypeKind::Number,
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

pub struct InvalidTypeCast {
    pub expr: Expr,
    pub expected: TypeKind,
    pub found: InferredType,
    pub message: String
}
