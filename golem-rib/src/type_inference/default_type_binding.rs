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

use crate::{Expr, ExprVisitor, InferredType};

pub fn bind_default_types_to_numbers(expr: &mut Expr) {
    let mut visitor = ExprVisitor::top_down(expr);

    while let Some(expr) = visitor.pop_front() {
        if let Expr::Number {
            number,
            inferred_type,
            ..
        } = expr
        {
            if inferred_type.un_resolved() {
                *inferred_type = InferredType::from(&number.value)
            }
        }
    }
}
