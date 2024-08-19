use crate::{Expr, InferredType};
use std::collections::VecDeque;

pub fn reset_type_info(expr: &mut Expr) {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    // Start from the end
    while let Some(expr) = queue.pop_back() {
        expr.override_type_type_mut(InferredType::Unknown);
        expr.visit_children_mut_bottom_up(&mut queue);
    }
}
