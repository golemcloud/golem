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
use std::ops::DerefMut;

// All select indices with literal numbers don't need to explicit
// type annotation to get better developer experience,
// and all literal numbers will be automatically inferred as u64
pub fn bind_default_types_to_index_expressions(expr: &mut Expr) {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_back() {
        match expr {
            Expr::SelectIndex { index, .. } => {
                if let Expr::Number { inferred_type, .. } = index.deref_mut() {
                    *inferred_type = InferredType::u64()
                }

                if let Expr::Range { range, .. } = index.deref_mut() {
                    let exprs = range.get_exprs_mut();

                    for expr in exprs {
                        if let Expr::Number { inferred_type, .. } = expr.deref_mut() {
                            *inferred_type = InferredType::u64()
                        }
                    }
                }
            }

            Expr::Range { range, .. } => {
                let exprs = range.get_exprs_mut();

                for expr in exprs {
                    if let Expr::Number { inferred_type, .. } = expr.deref_mut() {
                        *inferred_type = InferredType::u64()
                    }
                }
            }

            _ => {}
        }
    }
}
