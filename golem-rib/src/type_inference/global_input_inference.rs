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

pub fn infer_global_inputs(expr: &mut Expr) {
    internal::infer_global_inputs(expr);
}

mod internal {
    use crate::{Expr, InferredType};
    use std::collections::{HashMap, VecDeque};

    // Unlike inferring all identifiers, inputs don't have an associated let binding,
    // and yet we need to propagate this type info all over
    pub(crate) fn infer_global_inputs(expr: &mut Expr) {
        let global_variables_dictionary = collect_all_global_variables_type(expr);
        // Updating the collected types in all positions of input
        let mut queue = VecDeque::new();
        queue.push_back(expr);
        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_id, inferred_type) => {
                    // We are only interested in global variables
                    if variable_id.is_global() {
                        if let Some(types) = global_variables_dictionary.get(&variable_id.name()) {
                            if let Some(all_of) = InferredType::all_of(types.clone()) {
                                *inferred_type = inferred_type.merge(all_of)
                            }
                        }
                    }
                }
                _ => expr.visit_children_mut_bottom_up(&mut queue),
            }
        }
    }

    fn collect_all_global_variables_type(expr: &mut Expr) -> HashMap<String, Vec<InferredType>> {
        let mut queue = VecDeque::new();
        queue.push_back(expr);

        let mut all_types_of_global_variables = HashMap::new();
        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_id, inferred_type) => {
                    // We are only interested in global variables
                    if variable_id.is_global() {
                        all_types_of_global_variables
                            .entry(variable_id.name().clone())
                            .or_insert(Vec::new())
                            .push(inferred_type.clone());
                    }
                }
                _ => expr.visit_children_mut_bottom_up(&mut queue),
            }
        }

        all_types_of_global_variables
    }
}
