use crate::{Expr, TypeName};
use std::collections::VecDeque;

pub fn check_number_types(expr: &Expr) -> Result<(), String> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Number(_, _, inferred_type) => match inferred_type.as_number() {
                Ok(_) => {}
                Err(msg) => {
                    let type_name = TypeName::try_from(inferred_type.clone())?;
                    return Err(format!("{} has invalid type {}. {}", expr, type_name, msg));
                }
            },
            _ => expr.visit_children_bottom_up(&mut queue),
        }
    }

    Ok(())
}
