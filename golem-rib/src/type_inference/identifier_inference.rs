use crate::{Expr, InferredType};
use std::collections::VecDeque;

// Propagates the type information of the known identifiers
// to all sites of its usage where types are unknown, based on a variable-id.
// This will also update the let binding whose variable name is considered to be the origin of identifier
pub fn infer_all_identifiers_bottom_up(expr: &mut Expr) {
    let mut identifier_lookup = internal::IdentifierTypeState::new();
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    // We start from the end and pick the identifiers type
    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Identifier(variable_id, inferred_type) => {
                if inferred_type == &InferredType::Unknown {
                    if let Some(inferred_type) = identifier_lookup.lookup(&variable_id) {
                        expr.add_infer_type_mut(inferred_type);
                    }
                } else {
                    identifier_lookup.update(variable_id.clone(), inferred_type.clone());
                }
            }
            Expr::Let(variable_id, expr, _) => {
                if let Some(inferred_type) = identifier_lookup.lookup(variable_id) {
                    if inferred_type == InferredType::Unknown {
                        identifier_lookup.update(variable_id.clone(), expr.inferred_type())
                    } else {
                        expr.add_infer_type_mut(inferred_type);
                        expr.push_types_down();
                    }
                } else {
                    identifier_lookup.update(variable_id.clone(), expr.inferred_type())
                }

                queue.push_back(expr)
            }
            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }
}

pub fn infer_all_identifiers_top_down(expr: &mut Expr) {
    let mut identifier_lookup = internal::IdentifierTypeState::new();
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    // We start from the end and pick the identifiers type
    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::Identifier(variable_id, inferred_type) => {
                if inferred_type == &InferredType::Unknown {
                    if let Some(inferred_type) = identifier_lookup.lookup(&variable_id) {
                        expr.add_infer_type_mut(inferred_type);
                    }
                } else {
                    identifier_lookup.update(variable_id.clone(), inferred_type.clone());
                }
            }
            Expr::Let(variable_id, expr, _) => {
                if let Some(inferred_type) = identifier_lookup.lookup(variable_id) {
                    if inferred_type == InferredType::Unknown {
                        identifier_lookup.update(variable_id.clone(), expr.inferred_type())
                    } else {
                        expr.add_infer_type_mut(inferred_type);
                        expr.push_types_down();
                    }
                } else {
                    identifier_lookup.update(variable_id.clone(), expr.inferred_type())
                }

                queue.push_front(expr)
            }
            _ => expr.visit_children_mut_top_down(&mut queue),
        }
    }
}

mod internal {
    use crate::{Expr, InferredType, VariableId};
    use std::collections::HashMap;

    // A state that maps from the identifers to the types inferred
    #[derive(Debug, Clone)]
    pub struct IdentifierTypeState(HashMap<VariableId, InferredType>);

    impl IdentifierTypeState {
        pub fn new() -> Self {
            IdentifierTypeState(HashMap::new())
        }

        pub fn update(&mut self, id: VariableId, ty: InferredType) {
            self.0
                .entry(id)
                .and_modify(|e| e.update(ty.clone()))
                .or_insert(ty);
        }

        pub fn lookup(&self, id: &VariableId) -> Option<InferredType> {
            self.0.get(id).cloned()
        }
    }
}
