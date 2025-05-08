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

use crate::{Expr, ExprVisitor};
use crate::inferred_type::TypeOrigin;

pub fn set_pattern_match_origins(expr: &mut Expr) {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_front() {
        match expr {
            // Expr::PatternMatch {
            //     match_arms,
            //     inferred_type,
            //     source_span,
            //     ..
            // } => {
            //     for arm in match_arms.iter_mut() {
            //         let resolution = arm.arm_resolution_expr.as_mut();
            //         let inferred_type = resolution.inferred_type();
            //         let with_origin = inferred_type.add_origin(TypeOrigin::PatternMatch(resolution.source_span()));
            //         *resolution = resolution.with_inferred_type(with_origin);
            //     }
            //     *inferred_type = inferred_type.add_origin(TypeOrigin::OriginatedAt(source_span.clone()))
            // }
            expr => expr.propagate_origin(),
        }
    }
}
