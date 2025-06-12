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

use crate::compiler::WorkerNameGen;
use crate::{CallType, Expr, ExprVisitor, InstanceCreationType, TypeInternal};
use std::sync::Arc;

pub fn ensure_stateful_instance(
    expr: &mut Expr,
    worker_name_gen: &Arc<dyn WorkerNameGen + Send + Sync + 'static>,
) {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_back() {
        if let Expr::Call {
            call_type: CallType::InstanceCreation(InstanceCreationType::Worker { worker_name, .. }),
            inferred_type,
            ..
        } = expr
        {
            if worker_name.is_none() {
                let generated = worker_name_gen.generate_worker_name();
                let new_worker_name = Expr::literal(generated);

                *worker_name = Some(Box::new(new_worker_name.clone()));

                let type_internal = &mut *inferred_type.inner;

                if let TypeInternal::Instance { instance_type } = type_internal {
                    instance_type.set_worker_name(new_worker_name)
                }
            }
        }
    }
}
