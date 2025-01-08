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

use crate::Expr;

pub fn infer_all_identifiers(expr: &mut Expr) -> Result<(), String> {
    // We scan top-down and bottom-up to inform the type info between the identifiers
    // It doesn't matter which order we do it in (i.e, which identifier expression has the right type isn't a problem),
    // as we accumulate all the types in both directions
    internal::infer_all_identifiers_bottom_up(expr)?;
    internal::infer_all_identifiers_top_down(expr)?;
    internal::infer_match_binding_variables(expr);

    Ok(())
}

mod internal {
    use crate::type_inference::identifier_inference::internal;
    use crate::{ArmPattern, Expr, InferredType, MatchArm, VariableId};
    use std::collections::{HashMap, VecDeque};

    pub(crate) fn infer_all_identifiers_bottom_up(expr: &mut Expr) -> Result<(), String> {
        let mut identifier_lookup = internal::IdentifierTypeState::new();
        let mut queue = VecDeque::new();
        queue.push_back(expr);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_id, existing_type) => {
                    if let Some(new_inferred_type) = identifier_lookup.lookup(variable_id) {
                        *existing_type = existing_type.merge(new_inferred_type)
                    }

                    identifier_lookup.update(variable_id.clone(), existing_type.clone());
                }
                Expr::Let(variable_id, _, expr, _) => {
                    if let Some(inferred_type) = identifier_lookup.lookup(variable_id) {
                        expr.add_infer_type_mut(inferred_type);
                    }
                    identifier_lookup.update(variable_id.clone(), expr.inferred_type());
                    queue.push_back(expr)
                }
                _ => expr.visit_children_mut_bottom_up(&mut queue),
            }
        }

        Ok(())
    }

    pub(crate) fn infer_all_identifiers_top_down(expr: &mut Expr) -> Result<(), String> {
        let mut identifier_lookup = internal::IdentifierTypeState::new();
        let mut queue = VecDeque::new();
        queue.push_back(expr);

        while let Some(expr) = queue.pop_front() {
            match expr {
                Expr::Let(variable_id, _, expr, _) => {
                    if let Some(inferred_type) = identifier_lookup.lookup(variable_id) {
                        expr.add_infer_type_mut(inferred_type);
                    }

                    identifier_lookup.update(variable_id.clone(), expr.inferred_type());
                    queue.push_front(expr)
                }
                Expr::Identifier(variable_id, existing_type) => {
                    if let Some(new_inferred_type) = identifier_lookup.lookup(variable_id) {
                        *existing_type = existing_type.merge(new_inferred_type)
                    }

                    identifier_lookup.update(variable_id.clone(), existing_type.clone());
                }

                _ => expr.visit_children_mut_top_down(&mut queue),
            }
        }

        Ok(())
    }

    pub(crate) fn infer_match_binding_variables(expr: &mut Expr) {
        let mut queue = VecDeque::new();
        queue.push_back(expr);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::PatternMatch(_, arms, _) => {
                    for arm in arms {
                        process_arm(arm)
                    }
                }
                _ => expr.visit_children_mut_top_down(&mut queue),
            }
        }
    }

    // A state that maps from the identifers to the types inferred
    #[derive(Debug, Clone)]
    struct IdentifierTypeState(HashMap<VariableId, InferredType>);

    impl IdentifierTypeState {
        fn new() -> Self {
            IdentifierTypeState(HashMap::new())
        }

        fn update(&mut self, id: VariableId, ty: InferredType) {
            self.0
                .entry(id)
                .and_modify(|e| *e = e.merge(ty.clone()))
                .or_insert(ty);
        }

        pub fn lookup(&self, id: &VariableId) -> Option<InferredType> {
            self.0.get(id).cloned()
        }
    }

    fn process_arm(arm: &mut MatchArm) {
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
            ArmPattern::TupleConstructor(patterns) => {
                for pattern in patterns {
                    collect_all_identifiers(pattern, state)
                }
            }
            ArmPattern::ListConstructor(patterns) => {
                for pattern in patterns {
                    collect_all_identifiers(pattern, state)
                }
            }
            ArmPattern::RecordConstructor(fields) => {
                for (_, pattern) in fields {
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
                        *inferred_type = inferred_type.merge(new_inferred_type)
                    }
                }
                _ => expr.visit_children_mut_bottom_up(&mut queue),
            }
        }
    }
}
