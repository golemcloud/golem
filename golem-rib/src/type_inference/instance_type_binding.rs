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

use crate::{Expr, ExprVisitor, InferredType, TypeInternal, TypeOrigin};
use std::collections::HashMap;

// This is about binding the `InstanceType` to the corresponding identifiers.
//
// Example:
//  let foo = instance("worker-name");
//  foo.bar("baz")
//  With this phase `foo` in `foo.bar("baz")` will have inferred type of `InstanceType`
//
// Note that this compilation phase should be after variable binding phases
// (where we assign identities to variables that ensuring scoping).
//
// Example:
//  let foo = instance("worker-name");
//  let foo = "bar";
//  foo
//
// In this case `foo` in `foo` should have inferred type of `String` and not `InstanceType`
pub fn bind_instance_types(expr: &mut Expr) {
    let mut queue = ExprVisitor::top_down(expr);

    let mut instance_variables = HashMap::new();

    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::Let {
                variable_id, expr, ..
            } => {
                if let TypeInternal::Instance { instance_type } =
                    expr.inferred_type().internal_type()
                {
                    instance_variables.insert(variable_id.clone(), instance_type.clone());
                }
            }
            Expr::Identifier {
                variable_id,
                inferred_type,
                ..
            } => {
                if let Some(new_inferred_type) = instance_variables.get(variable_id) {
                    *inferred_type = InferredType::new(
                        TypeInternal::Instance {
                            instance_type: new_inferred_type.clone(),
                        },
                        TypeOrigin::NoOrigin,
                    );
                }
            }

            _ => {}
        }
    }
}
