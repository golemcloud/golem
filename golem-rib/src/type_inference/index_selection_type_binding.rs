use crate::{Expr, InferredType};
use std::collections::VecDeque;

// All select indices with literal numbers don't need to explicit
// type annotation, and will be automatically inferred as u64
pub fn bind_default_types_to_index_expressions(expr: &mut Expr) {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::SelectDynamic { expr, index, .. } => {
                let existing = index.inferred_type();
                if existing.is_unknown() || existing.is_one_of() {
                    if let Expr::Number { inferred_type, .. } = &mut **index {
                        *inferred_type = InferredType::U64
                    }
                }
                queue.push_back(expr);
            }

            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }
}
