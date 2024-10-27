use crate::Expr;

// We assign unique variable identifiers to the identifiers present in the match arm literals,
// and ensuring they get propagated to the usage sites within resolution expressions. Here
// we make sure to replace global variable or local variable identifiers with match-arm identifiers (VariableId enum)
// to prevent conflicts with other local let bindings
// or global variables, thereby maintaining clear variable scoping and avoiding unintended clashes.
pub fn name_binding_list_comprehension(expr: &mut Expr) {
    internal::list_comprehension_name_binding(expr);
}

mod internal {
    use crate::{Expr, VariableId};
    use std::collections::VecDeque;

    pub(crate) fn list_comprehension_name_binding(expr: &mut Expr) {
        let mut queue = VecDeque::new();
        queue.push_front(expr);

        // Start from the end
        while let Some(expr) = queue.pop_front() {
            match expr {
                Expr::ListComprehension(variable_id, iterable_expr, yield_expr, _) => {
                    queue.push_front(iterable_expr);
                    *variable_id = VariableId::list_comprehension_identifier(variable_id.name());

                    process_yield_expr(variable_id, yield_expr)
                }
                _ => {
                    expr.visit_children_mut_top_down(&mut queue);
                }
            }
        }
    }

    fn process_yield_expr(variable_id: &mut VariableId, yield_expr: &mut Expr) {
        let mut queue = VecDeque::new();

        queue.push_front(yield_expr);

        while let Some(expr) = queue.pop_front() {
            match expr {
                Expr::Identifier(variable_in_yield, _) => {
                    if variable_id.name() == variable_in_yield.name() {
                        *variable_in_yield = variable_id.clone();
                    }
                }
                _ => expr.visit_children_mut_top_down(&mut queue),
            }
        }
    }
}
