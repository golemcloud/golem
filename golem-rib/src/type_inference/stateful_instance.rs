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

pub fn ensure_stateful_instance(expr: &mut Expr) {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_back() {
        if let Expr::Call {
            call_type:
                CallType::InstanceCreation(InstanceCreationType::WitWorker { worker_name, .. }),
            inferred_type,
            args,
            ..
        } = expr
        {
            if worker_name.is_none() {
                *worker_name = Some(Box::new(Expr::generate_worker_name(None)));

                let type_internal = &mut *inferred_type.inner;

                *args = vec![Expr::generate_worker_name(None)];

                if let TypeInternal::Instance { instance_type } = type_internal {
                    instance_type.set_worker_name(Expr::generate_worker_name(None))
                }
            }
        }
    }
}
