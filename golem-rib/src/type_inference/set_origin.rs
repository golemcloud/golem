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

use crate::inferred_type::TypeOrigin;
use crate::{Expr, ExprVisitor};

pub fn set_origin(expr: &mut Expr) {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_front() {
        match expr {
            expr => {
                let source_location = expr.source_span();
                let origin = TypeOrigin::OriginatedAt(source_location.clone());
                let inferred_type = expr.inferred_type();
                let origin = inferred_type.add_origin(origin);
                expr.with_inferred_type_mut(origin);
            }
        }
    }
}
