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

pub fn reset_type_info(expr: &mut Expr) {
    let mut visitor = ExprVisitor::bottom_up(expr);

    // Start from the end
    while let Some(expr) = visitor.pop_back() {
        expr.with_inferred_type_mut(InferredType::unknown());
    }
}
