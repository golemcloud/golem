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

use crate::Expr;
use std::collections::VecDeque;

// This function will assign ids to variables declared with `let` expressions,
// and propagate these ids to the usage sites (`Expr::Identifier` nodes).
pub fn bind_variables_of_let_assignment(expr: &mut Expr) {
    let mut identifier_id_state = internal::IdentifierVariableIdState::new();
    let mut queue = VecDeque::new();
    queue.push_front(expr);

    // Start from the end
    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::Let {
                variable_id, expr, ..
            } => {
                let field_name = variable_id.name();
                identifier_id_state.update_variable_id(&field_name); // Increment the variable_id
                if let Some(latest_variable_id) = identifier_id_state.lookup(&field_name) {
                    *variable_id = latest_variable_id.clone();
                }
                queue.push_front(expr);
            }

            Expr::Identifier { variable_id, .. } if !variable_id.is_match_binding() => {
                let field_name = variable_id.name();
                if let Some(latest_variable_id) = identifier_id_state.lookup(&field_name) {
                    *variable_id = latest_variable_id.clone();
                }
            }

            _ => {
                expr.visit_children_mut_top_down(&mut queue);
            }
        }
    }
}

mod internal {
    use crate::VariableId;
    use std::collections::HashMap;

    pub(crate) struct IdentifierVariableIdState(HashMap<String, VariableId>);

    impl IdentifierVariableIdState {
        pub(crate) fn new() -> Self {
            IdentifierVariableIdState(HashMap::new())
        }

        pub(crate) fn update_variable_id(&mut self, identifier: &str) {
            self.0
                .entry(identifier.to_string())
                .and_modify(|x| {
                    *x = x.increment_local_variable_id();
                })
                .or_insert(VariableId::local(identifier, 0));
        }

        pub(crate) fn lookup(&self, identifier: &str) -> Option<VariableId> {
            self.0.get(identifier).cloned()
        }
    }
}

#[cfg(test)]
mod name_binding_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::call_type::CallType;
    use crate::function_name::{DynamicParsedFunctionName, DynamicParsedFunctionReference};
    use crate::{Expr, ParsedFunctionSite, VariableId};

    #[test]
    fn test_name_binding_simple() {
        let rib_expr = r#"
          let x = 1;
          foo(x)
        "#;

        let mut expr = Expr::from_text(rib_expr).unwrap();

        // Bind x in let with the x in foo
        expr.bind_variables_of_let_assignment();

        let let_binding = Expr::let_binding_with_variable_id(
            VariableId::local("x", 0),
            Expr::number(BigDecimal::from(1)),
            None,
        );

        let call_expr = Expr::call(
            CallType::function_without_worker(DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            None,
            vec![Expr::identifier_local("x", 0, None)],
        );

        let expected = Expr::expr_block(vec![let_binding, call_expr]);

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_name_binding_multiple() {
        let rib_expr = r#"
          let x = 1;
          let y = 2;
          foo(x);
          foo(y)
        "#;

        let mut expr = Expr::from_text(rib_expr).unwrap();

        // Bind x in let with the x in foo
        expr.bind_variables_of_let_assignment();

        let let_binding1 = Expr::let_binding_with_variable_id(
            VariableId::local("x", 0),
            Expr::number(BigDecimal::from(1)),
            None,
        );

        let let_binding2 = Expr::let_binding_with_variable_id(
            VariableId::local("y", 0),
            Expr::number(BigDecimal::from(2)),
            None,
        );

        let call_expr1 = Expr::call(
            CallType::function_without_worker(DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            None,
            vec![Expr::identifier_local("x", 0, None)],
        );

        let call_expr2 = Expr::call(
            CallType::function_without_worker(DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            None,
            vec![Expr::identifier_local("y", 0, None)],
        );

        let expected = Expr::expr_block(vec![let_binding1, let_binding2, call_expr1, call_expr2]);

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_name_binding_shadowing() {
        let rib_expr = r#"
          let x = 1;
          foo(x);
          let x = 2;
          foo(x)
        "#;

        let mut expr = Expr::from_text(rib_expr).unwrap();

        // Bind x in let with the x in foo
        expr.bind_variables_of_let_assignment();

        let let_binding1 = Expr::let_binding_with_variable_id(
            VariableId::local("x", 0),
            Expr::number(BigDecimal::from(1)),
            None,
        );

        let let_binding2 = Expr::let_binding_with_variable_id(
            VariableId::local("x", 1),
            Expr::number(BigDecimal::from(2)),
            None,
        );

        let call_expr1 = Expr::call(
            CallType::function_without_worker(DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            None,
            vec![Expr::identifier_local("x", 0, None)],
        );

        let call_expr2 = Expr::call(
            CallType::function_without_worker(DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            None,
            vec![Expr::identifier_local("x", 1, None)],
        );

        let expected = Expr::expr_block(vec![let_binding1, call_expr1, let_binding2, call_expr2]);

        assert_eq!(expr, expected);
    }
}
