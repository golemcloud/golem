// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{ArmPattern, Expr, ExprVisitor, InferredType, MatchArm, VariableId};
use std::collections::HashMap;

pub fn infer_all_identifiers(expr: &mut Expr) {
    // We scan top-down and bottom-up to inform the type between the identifiers
    // It doesn't matter which order we do it in (i.e, which identifier expression has the right type isn't a problem),
    // as we accumulate all the types in both directions
    infer_all_identifiers_bottom_up(expr);
    infer_all_identifiers_top_down(expr);
    infer_match_binding_variables(expr);
}

fn infer_all_identifiers_bottom_up(expr: &mut Expr) {
    let mut identifier_lookup = IdentifierTypeState::new();

    // Given
    //   `Expr::Block(Expr::Let(x, Expr::Num(1)), Expr::Call(func, x))`
    // Expr::Num(1)
    // Expr::Let(Variable(x), Expr::Num(1))
    // Expr::Identifier(x)
    // Expr::Call(func, Expr::Identifier(x))
    // Expr::Block(Expr::Let(x, Expr::Num(1)), Expr::Call(func, x))
    let mut visitor = ExprVisitor::bottom_up(expr);

    // Popping it from the back results in `Expr::Identifier(x)` to be processed first
    // in the above example.
    while let Some(expr) = visitor.pop_back() {
        match expr {
            // If identifier is inferred (probably because it was part of a function call befre),
            // make sure to update the identifier inference lookup table.
            // If lookup table is already updated, merge the inferred type
            Expr::Identifier {
                variable_id,
                inferred_type,
                ..
            } => {
                if let Some(new_inferred_type) = identifier_lookup.lookup(variable_id) {
                    *inferred_type = inferred_type.merge(new_inferred_type)
                }

                identifier_lookup.update(variable_id.clone(), inferred_type.clone());
            }

            // In the above example `let x = 1`,
            // since `x` is already inferred before, we propagate the type to the expression to `1`.
            // Also if `1` is already inferred we update the identifier lookup table with x's type as 1's type
            Expr::Let {
                variable_id, expr, ..
            } => {
                if let Some(inferred_type) = identifier_lookup.lookup(variable_id) {
                    expr.add_infer_type_mut(inferred_type);
                }
                identifier_lookup.update(variable_id.clone(), expr.inferred_type());
            }

            _ => {}
        }
    }
}

// This is more of an optional stage, as bottom-up type propagation would be enough
// but helps with reaching early fix point later down the line of compilation phases
fn infer_all_identifiers_top_down(expr: &mut Expr) {
    let mut identifier_lookup = IdentifierTypeState::new();
    let mut visitor = ExprVisitor::top_down(expr);
    while let Some(expr) = visitor.pop_front() {
        match expr {
            Expr::Let {
                variable_id, expr, ..
            } => {
                if let Some(inferred_type) = identifier_lookup.lookup(variable_id) {
                    expr.add_infer_type_mut(inferred_type);
                }

                identifier_lookup.update(variable_id.clone(), expr.inferred_type());
            }
            Expr::Identifier {
                variable_id,
                inferred_type,
                ..
            } => {
                if let Some(new_inferred_type) = identifier_lookup.lookup(variable_id) {
                    *inferred_type = inferred_type.merge(new_inferred_type)
                }

                identifier_lookup.update(variable_id.clone(), inferred_type.clone());
            }

            _ => {}
        }
    }
}

fn infer_match_binding_variables(expr: &mut Expr) {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_back() {
        if let Expr::PatternMatch { match_arms, .. } = expr {
            for arm in match_arms {
                process_arm(arm)
            }
        }
    }
}

// A state that maps from the identifiers to the types inferred
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
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_back() {
        if let Expr::Identifier {
            variable_id,
            inferred_type,
            ..
        } = expr
        {
            if !inferred_type.is_unknown() {
                state.update(variable_id.clone(), inferred_type.clone())
            }
        }
    }
}

fn update_arm_resolution_expr_with_identifiers(
    arm_resolution: &mut Expr,
    state: &IdentifierTypeState,
) {
    let mut visitor = ExprVisitor::bottom_up(arm_resolution);

    while let Some(expr) = visitor.pop_back() {
        match expr {
            Expr::Identifier {
                variable_id,
                inferred_type,
                ..
            } if variable_id.is_match_binding() => {
                if let Some(new_inferred_type) = state.lookup(variable_id) {
                    *inferred_type = inferred_type.merge(new_inferred_type)
                }
            }
            _ => {}
        }
    }
}
