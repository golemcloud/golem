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

use crate::{CallType, Expr, ExprVisitor, InstanceCreationType};
use uuid::Uuid;

pub fn ensure_stateful_instance(expr: &mut Expr) {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(Expr::Call {
                       call_type: CallType::InstanceCreation(InstanceCreationType::Worker { worker_name, .. }),
                       ..
                   }) = visitor.pop_back()
    {
        if worker_name.is_none() {
            *worker_name = Some(Box::new(Expr::literal(Uuid::new_v4().to_string())));
        }
       
    }
}
