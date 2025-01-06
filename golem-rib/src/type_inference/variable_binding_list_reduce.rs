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

use crate::{Expr, VariableId};
use std::collections::VecDeque;
pub fn bind_variables_of_list_reduce(expr: &mut Expr) {
    let mut queue = VecDeque::new();
    queue.push_front(expr);

    // Start from the end
    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::ListReduce {
                reduce_variable,
                iterated_variable,
                iterable_expr,
                init_value_expr,
                yield_expr,
                ..
            } => {
                queue.push_front(iterable_expr);
                queue.push_front(init_value_expr);

                // While parser may update this directly, type inference phase
                // still ensures that these variables are tagged to its appropriately
                *iterated_variable =
                    VariableId::list_comprehension_identifier(iterated_variable.name());

                *reduce_variable = VariableId::list_reduce_identifier(reduce_variable.name());

                internal::process_yield_expr(reduce_variable, iterated_variable, yield_expr)
            }
            _ => {
                expr.visit_children_mut_top_down(&mut queue);
            }
        }
    }
}

mod internal {
    use crate::{Expr, VariableId};
    use std::collections::VecDeque;

    pub(crate) fn process_yield_expr(
        reduce_variable: &mut VariableId,
        iterated_variable_id: &mut VariableId,
        yield_expr: &mut Expr,
    ) {
        let mut queue = VecDeque::new();

        queue.push_front(yield_expr);

        while let Some(expr) = queue.pop_front() {
            match expr {
                Expr::Identifier(variable_in_yield, _) => {
                    if iterated_variable_id.name() == variable_in_yield.name() {
                        *variable_in_yield = iterated_variable_id.clone();
                    } else if reduce_variable.name() == variable_in_yield.name() {
                        *variable_in_yield = reduce_variable.clone()
                    }
                }
                _ => expr.visit_children_mut_top_down(&mut queue),
            }
        }
    }
}
