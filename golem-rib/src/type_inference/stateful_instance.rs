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

use crate::{CallType, Expr, ExprVisitor, InstanceCreationType, TypeInternal};
use uuid::Uuid;

//
pub fn ensure_stateful_instance(expr: &mut Expr) {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_back() {
        match expr {
            Expr::Call {
                call_type:
                    CallType::InstanceCreation(InstanceCreationType::Worker { worker_name, .. }),
                inferred_type,
                ..
            } => {
                let generated = Uuid::new_v4().to_string();
                let new_worker_name = Expr::literal(generated);

                if worker_name.is_none() {
                    *worker_name = Some(Box::new(new_worker_name.clone()));
                }

                let type_internal = &mut *inferred_type.inner;

                match type_internal {
                    TypeInternal::Instance {instance_type} => {
                        instance_type.set_worker_name(new_worker_name)
                    }
                    _ => {}
                }
            }

            _ => {}
        }
    }
}
