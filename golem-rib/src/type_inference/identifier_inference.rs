use crate::Expr;
use std::collections::VecDeque;

// Propagates the type information of the known identifiers
// to all sites of its usage where types are unknown, based on a variable-id.
// This will also update the let binding whose variable name is considered to be the origin of identifier
pub fn infer_all_identifiers_bottom_up(expr: &mut Expr) -> Result<(), String> {
    let mut identifier_lookup = internal::IdentifierTypeState::new();
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    // We start from the end and pick the identifiers type
    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Identifier(variable_id, inferred_type) => {
                if inferred_type.is_unknown() {
                    if let Some(inferred_type) = identifier_lookup.lookup(variable_id) {
                        expr.add_infer_type_mut(inferred_type);
                    }
                } else {
                    identifier_lookup.update(variable_id.clone(), inferred_type.clone());
                }
            }
            Expr::Let(variable_id, expr, _) => {
                if let Some(inferred_type) = identifier_lookup.lookup(variable_id) {
                    if inferred_type.is_unknown() {
                        identifier_lookup.update(variable_id.clone(), expr.inferred_type())
                    } else {
                        expr.add_infer_type_mut(inferred_type);
                        expr.push_types_down()?;
                    }
                } else {
                    identifier_lookup.update(variable_id.clone(), expr.inferred_type())
                }

                queue.push_back(expr)
            }
            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
}

// TODO; Clean up
pub fn infer_all_identifiers_top_down(expr: &mut Expr) -> Result<(), String> {
    let mut identifier_lookup = internal::IdentifierTypeState::new();
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    // We start from the end and pick the identifiers type
    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::Let(variable_id, expr, _) => {
                if let Some(inferred_type) = identifier_lookup.lookup(variable_id) {
                    if inferred_type.is_unknown() {
                        identifier_lookup.update(variable_id.clone(), expr.inferred_type())
                    } else {
                        expr.add_infer_type_mut(inferred_type);
                        expr.push_types_down()?;
                    }
                } else {
                    identifier_lookup.update(variable_id.clone(), expr.inferred_type())
                }

                queue.push_front(expr)
            }
            Expr::Identifier(variable_id, inferred_type) => {
                if let Some(inferred_type) = identifier_lookup.lookup(variable_id) {
                    expr.add_infer_type_mut(inferred_type);
                } else {
                    identifier_lookup.update(variable_id.clone(), inferred_type.clone());
                }
            }

            _ => expr.visit_children_mut_top_down(&mut queue),
        }
    }

    Ok(())
}

pub fn infer_match_binding_variables(expr: &mut Expr) {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::PatternMatch(_, arms, _) => {
                for arm in arms {
                    internal::process_arm(arm)
                }
            }
            _ => expr.visit_children_mut_top_down(&mut queue),
        }
    }
}

mod internal {
    use crate::{ArmPattern, Expr, InferredType, MatchArm, VariableId};
    use std::collections::{HashMap, VecDeque};

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

    pub(crate) fn process_arm(arm: &mut MatchArm) {
        let arm_pattern = &mut arm.arm_pattern;
        let mut initial_set = IdentifierTypeState::new();
        collect_all_identifiers(arm_pattern, &mut initial_set);
        let arm_resolution = &mut arm.arm_resolution_expr;

        update_arm_resolution_expr_with_identifiers(arm_resolution, &initial_set);
    }

    fn collect_all_identifiers(pattern: &mut ArmPattern, state: &mut IdentifierTypeState) {
        match pattern {
            ArmPattern::WildCard => {}
            ArmPattern::As(_, arm_pattern) => collect_all_identifiers(arm_pattern, state),
            ArmPattern::Constructor(_, patterns) => {
                for pattern in patterns {
                    collect_all_identifiers(pattern, state)
                }
            }
            ArmPattern::Literal(expr) => accumulate_types_of_identifiers(&mut *expr, state),
        }
    }

    fn accumulate_types_of_identifiers(expr: &mut Expr, state: &mut IdentifierTypeState) {
        let mut queue = VecDeque::new();

        queue.push_back(expr);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_id, inferred_type) => {
                    if !inferred_type.is_unknown() {
                        state.update(variable_id.clone(), inferred_type.clone())
                    }
                }

                _ => expr.visit_children_mut_bottom_up(&mut queue),
            }
        }
    }

    fn update_arm_resolution_expr_with_identifiers(
        arm_resolution: &mut Expr,
        state: &IdentifierTypeState,
    ) {
        let mut queue = VecDeque::new();
        queue.push_back(arm_resolution);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_id, inferred_type) if variable_id.is_match_binding() => {
                    if let Some(new_inferred_type) = state.lookup(variable_id) {
                        inferred_type.update(new_inferred_type)
                    }
                }
                _ => expr.visit_children_mut_bottom_up(&mut queue),
            }
        }
    }
}
