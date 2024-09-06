// Copyright 2024 Golem Cloud
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

pub fn name_binding_local_variables(expr: &mut Expr) {
    let mut identifier_id_state = internal::IdentifierVariableIdState::new();
    let mut queue = VecDeque::new();
    queue.push_front(expr);

    // Start from the end
    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::Let(variable_id, _, expr, _) => {
                let field_name = variable_id.name();
                identifier_id_state.update_variable_id(&field_name); // Increment the variable_id
                *variable_id = identifier_id_state.lookup(&field_name).unwrap();
                queue.push_front(expr);
            }

            Expr::Identifier(variable_id, _) if !variable_id.is_match_binding() => {
                let field_name = variable_id.name();
                if let Some(latest_variable_id) = identifier_id_state.lookup(&field_name) {
                    // If there existed a let statement, this ensures global is changed to local
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
    use crate::call_type::CallType;
    use crate::{
        Expr, InferredType, ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite,
        VariableId,
    };

    #[test]
    fn test_name_binding_simple() {
        let rib_expr = r#"
          let x = 1;
          foo(x)
        "#;

        let mut expr = Expr::from_text(rib_expr).unwrap();

        // Bind x in let with the x in foo
        expr.name_binding_local_variables();

        let let_binding = Expr::Let(
            VariableId::local("x", 0),
            None,
            Box::new(Expr::number(1f64)),
            InferredType::Unknown,
        );

        let call_expr = Expr::Call(
            CallType::Function(ParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: ParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            vec![Expr::Identifier(
                VariableId::local("x", 0),
                InferredType::Unknown,
            )],
            InferredType::Unknown,
        );

        let expected = Expr::multiple(vec![let_binding, call_expr]);

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
        expr.name_binding_local_variables();

        let let_binding1 = Expr::Let(
            VariableId::local("x", 0),
            None,
            Box::new(Expr::number(1f64)),
            InferredType::Unknown,
        );

        let let_binding2 = Expr::Let(
            VariableId::local("y", 0),
            None,
            Box::new(Expr::number(2f64)),
            InferredType::Unknown,
        );

        let call_expr1 = Expr::Call(
            CallType::Function(ParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: ParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            vec![Expr::Identifier(
                VariableId::local("x", 0),
                InferredType::Unknown,
            )],
            InferredType::Unknown,
        );

        let call_expr2 = Expr::Call(
            CallType::Function(ParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: ParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            vec![Expr::Identifier(
                VariableId::local("y", 0),
                InferredType::Unknown,
            )],
            InferredType::Unknown,
        );

        let expected = Expr::multiple(vec![let_binding1, let_binding2, call_expr1, call_expr2]);

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
        expr.name_binding_local_variables();

        let let_binding1 = Expr::Let(
            VariableId::local("x", 0),
            None,
            Box::new(Expr::number(1f64)),
            InferredType::Unknown,
        );

        let let_binding2 = Expr::Let(
            VariableId::local("x", 1),
            None,
            Box::new(Expr::number(2f64)),
            InferredType::Unknown,
        );

        let call_expr1 = Expr::Call(
            CallType::Function(ParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: ParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            vec![Expr::Identifier(
                VariableId::local("x", 0),
                InferredType::Unknown,
            )],
            InferredType::Unknown,
        );

        let call_expr2 = Expr::Call(
            CallType::Function(ParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: ParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            vec![Expr::Identifier(
                VariableId::local("x", 1),
                InferredType::Unknown,
            )],
            InferredType::Unknown,
        );

        let expected = Expr::multiple(vec![let_binding1, call_expr1, let_binding2, call_expr2]);

        assert_eq!(expr, expected);
    }
}
