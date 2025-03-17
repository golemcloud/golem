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

use crate::{Expr, InferredType};
use std::collections::VecDeque;
use std::ops::DerefMut;

// All select indices with literal numbers don't need to explicit
// type annotation to get better developer experience,
// and all literal numbers will be automatically inferred as u64
pub fn bind_default_types_to_index_expressions(expr: &mut Expr) {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::SelectIndex { expr, index, .. } => {
                if let Expr::Number { inferred_type, .. } = index.deref_mut() {
                    if inferred_type.is_unknown() || inferred_type.is_one_of() {
                        *inferred_type = InferredType::U64
                    }
                }

                if let Expr::Range { range, .. } = index.deref_mut() {
                    let exprs = range.get_exprs_mut();

                    for expr in exprs {
                        if let Expr::Number { inferred_type, .. } = expr.deref_mut() {
                            if inferred_type.is_unknown() || inferred_type.is_one_of() {
                                *inferred_type = InferredType::U64
                            }
                        }
                    }
                }

                queue.push_back(expr);
            }

            Expr::Range { range, .. } => {
                let exprs = range.get_exprs_mut();

                for expr in exprs {
                    if let Expr::Number { inferred_type, .. } = expr.deref_mut() {
                        if inferred_type.is_unknown() || inferred_type.is_one_of() {
                            *inferred_type = InferredType::U64
                        }
                    }
                }
            }

            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }
}
