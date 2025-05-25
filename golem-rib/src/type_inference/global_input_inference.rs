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

use crate::{Expr, ExprVisitor, InferredType};
use std::collections::HashMap;

// request.path.user is used as a string in one place
// request.path.id is used an integer in some other
// request -> AllOf(path -> user, path -> id)
pub fn infer_global_inputs(expr: &mut Expr) {
    let global_variables_dictionary = collect_all_global_variables_type(expr);
    // Updating the collected types in all positions of input
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_back() {
        if let Expr::Identifier {
            variable_id,
            inferred_type,
            ..
        } = expr
        {
            // We are only interested in global variables
            if variable_id.is_global() {
                if let Some(types) = global_variables_dictionary.get(&variable_id.name()) {
                    *inferred_type = InferredType::all_of(types.clone())
                }
            }
        }
    }
}

fn collect_all_global_variables_type(expr: &mut Expr) -> HashMap<String, Vec<InferredType>> {
    let mut visitor = ExprVisitor::bottom_up(expr);

    let mut all_types_of_global_variables = HashMap::new();
    while let Some(expr) = visitor.pop_back() {
        if let Expr::Identifier {
            variable_id,
            inferred_type,
            ..
        } = expr
        {
            if variable_id.is_global() {
                match all_types_of_global_variables.get_mut(&variable_id.name().clone()) {
                    None => {
                        all_types_of_global_variables
                            .insert(variable_id.name(), vec![inferred_type.clone()]);
                    }

                    Some(v) => {
                        if !v.contains(inferred_type) {
                            v.push(inferred_type.clone())
                        }
                    }
                }
            }
        }
    }

    all_types_of_global_variables
}
