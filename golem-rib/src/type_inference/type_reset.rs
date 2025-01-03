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

pub fn reset_type_info(expr: &mut Expr) {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    // Start from the end
    while let Some(expr) = queue.pop_back() {
        expr.override_type_type_mut(InferredType::Unknown);
        expr.visit_children_mut_bottom_up(&mut queue);
    }
}
