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

use crate::{ArmPattern, Expr, ExprVisitor, MatchArm, MatchIdentifier, VariableId};
use std::collections::HashMap;

// This function will assign ids to variables declared with `let` expressions,
// and propagate these ids to the usage sites (`Expr::Identifier` nodes).
pub fn bind_variables_of_let_assignment(expr: &mut Expr) {
    let mut identifier_id_state = IdentifierVariableIdState::new();
    let mut visitor = ExprVisitor::bottom_up(expr);

    // Start from the end
    while let Some(expr) = visitor.pop_front() {
        match expr {
            Expr::Let { variable_id, .. } => {
                let field_name = variable_id.name();
                identifier_id_state.update_variable_id(&field_name); // Increment the variable_id
                if let Some(latest_variable_id) = identifier_id_state.lookup(&field_name) {
                    *variable_id = latest_variable_id.clone();
                }
            }

            Expr::Identifier { variable_id, .. } if !variable_id.is_match_binding() => {
                let field_name = variable_id.name();
                if let Some(latest_variable_id) = identifier_id_state.lookup(&field_name) {
                    *variable_id = latest_variable_id.clone();
                }
            }
            _ => {}
        }
    }
}

pub fn bind_variables_of_list_comprehension(expr: &mut Expr) {
    let mut visitor = ExprVisitor::top_down(expr);

    while let Some(expr) = visitor.pop_front() {
        if let Expr::ListComprehension {
            iterated_variable,
            yield_expr,
            ..
        } = expr
        {
            *iterated_variable =
                VariableId::list_comprehension_identifier(iterated_variable.name());

            process_yield_expr_in_comprehension(iterated_variable, yield_expr)
        }
    }
}

pub fn bind_variables_of_list_reduce(expr: &mut Expr) {
    let mut visitor = ExprVisitor::top_down(expr);

    // Start from the end
    while let Some(expr) = visitor.pop_front() {
        if let Expr::ListReduce {
            reduce_variable,
            iterated_variable,
            yield_expr,
            ..
        } = expr
        {
            // While parser may update this directly, type inference phase
            // still ensures that these variables are tagged to its appropriately
            *iterated_variable =
                VariableId::list_comprehension_identifier(iterated_variable.name());

            *reduce_variable = VariableId::list_reduce_identifier(reduce_variable.name());

            process_yield_expr_in_reduce(reduce_variable, iterated_variable, yield_expr)
        }
    }
}

pub fn bind_variables_of_pattern_match(expr: &mut Expr) {
    bind_variables_in_pattern_match_internal(expr, 0, &mut []);
}

fn bind_variables_in_pattern_match_internal(
    expr: &mut Expr,
    previous_index: usize,
    match_identifiers: &mut [MatchIdentifier],
) -> usize {
    let mut index = previous_index;
    let mut queue = ExprVisitor::top_down(expr);
    let mut shadowed_let_binding = vec![];

    // Start from the end
    while let Some(expr) = queue.pop_front() {
        match expr {
            Expr::PatternMatch { match_arms, .. } => {
                for arm in match_arms {
                    // We increment the index for each arm regardless of whether there is an identifier exist or not
                    index += 1;
                    let latest = process_arm(arm, index);
                    // An arm can increment the index if there are nested pattern match arms, and therefore
                    // set it to the latest max.
                    index = latest
                }
            }
            Expr::Let { variable_id, .. } => {
                shadowed_let_binding.push(variable_id.name());
            }
            Expr::Identifier { variable_id, .. } => {
                let identifier_name = variable_id.name();
                if let Some(x) = match_identifiers.iter().find(|x| x.name == identifier_name) {
                    if !shadowed_let_binding.contains(&identifier_name) {
                        *variable_id = VariableId::MatchIdentifier(x.clone());
                    }
                }
            }

            _ => {}
        }
    }

    index
}

fn process_arm(match_arm: &mut MatchArm, global_arm_index: usize) -> usize {
    let match_arm_pattern = &mut match_arm.arm_pattern;

    pub fn go(
        arm_pattern: &mut ArmPattern,
        global_arm_index: usize,
        match_identifiers: &mut Vec<MatchIdentifier>,
    ) {
        match arm_pattern {
            ArmPattern::Literal(expr) => {
                let new_match_identifiers =
                    update_all_identifier_in_lhs_expr(expr, global_arm_index);
                match_identifiers.extend(new_match_identifiers);
            }

            ArmPattern::WildCard => {}
            ArmPattern::As(name, arm_pattern) => {
                let match_identifier = MatchIdentifier::new(name.clone(), global_arm_index);
                match_identifiers.push(match_identifier);

                go(arm_pattern, global_arm_index, match_identifiers);
            }

            ArmPattern::Constructor(_, arm_patterns) => {
                for arm_pattern in arm_patterns {
                    go(arm_pattern, global_arm_index, match_identifiers);
                }
            }

            ArmPattern::TupleConstructor(arm_patterns) => {
                for arm_pattern in arm_patterns {
                    go(arm_pattern, global_arm_index, match_identifiers);
                }
            }

            ArmPattern::ListConstructor(arm_patterns) => {
                for arm_pattern in arm_patterns {
                    go(arm_pattern, global_arm_index, match_identifiers);
                }
            }

            ArmPattern::RecordConstructor(fields) => {
                for (_, arm_pattern) in fields {
                    go(arm_pattern, global_arm_index, match_identifiers);
                }
            }
        }
    }

    let mut match_identifiers = vec![];

    // Recursively identify the arm within an arm literal
    go(match_arm_pattern, global_arm_index, &mut match_identifiers);

    let resolution_expression = &mut *match_arm.arm_resolution_expr;

    // Continue with original pattern_match_name_binding for resolution expressions
    // to target nested pattern matching.
    bind_variables_in_pattern_match_internal(
        resolution_expression,
        global_arm_index,
        &mut match_identifiers,
    )
}

fn update_all_identifier_in_lhs_expr(
    expr: &mut Expr,
    global_arm_index: usize,
) -> Vec<MatchIdentifier> {
    let mut identifier_names = vec![];
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_front() {
        if let Expr::Identifier { variable_id, .. } = expr {
            let match_identifier = MatchIdentifier::new(variable_id.name(), global_arm_index);
            identifier_names.push(match_identifier);
            let new_variable_id =
                VariableId::match_identifier(variable_id.name(), global_arm_index);
            *variable_id = new_variable_id;
        }
    }

    identifier_names
}

fn process_yield_expr_in_comprehension(variable: &mut VariableId, yield_expr: &mut Expr) {
    let mut visitor = ExprVisitor::top_down(yield_expr);

    while let Some(expr) = visitor.pop_front() {
        if let Expr::Identifier { variable_id, .. } = expr {
            if variable.name() == variable_id.name() {
                *variable_id = variable.clone();
            }
        }
    }
}

fn process_yield_expr_in_reduce(
    reduce_variable: &mut VariableId,
    iterated_variable_id: &mut VariableId,
    yield_expr: &mut Expr,
) {
    let mut visitor = ExprVisitor::top_down(yield_expr);

    while let Some(expr) = visitor.pop_front() {
        if let Expr::Identifier { variable_id, .. } = expr {
            if iterated_variable_id.name() == variable_id.name() {
                *variable_id = iterated_variable_id.clone();
            } else if reduce_variable.name() == variable_id.name() {
                *variable_id = reduce_variable.clone()
            }
        }
    }
}

struct IdentifierVariableIdState(HashMap<String, VariableId>);

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

#[cfg(test)]
mod name_binding_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::call_type::CallType;
    use crate::function_name::{DynamicParsedFunctionName, DynamicParsedFunctionReference};
    use crate::{Expr, InferredType, ParsedFunctionSite, VariableId};

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

    #[test]
    fn test_simple_pattern_match_name_binding() {
        // The first x is global and the second x is a match binding
        let expr_string = r#"
          match some(x) {
            some(x) => x,
            none => 0
          }
        "#;

        let mut expr = Expr::from_text(expr_string).unwrap();

        expr.bind_variables_of_pattern_match();

        assert_eq!(expr, expectations::expected_match(1));
    }

    #[test]
    fn test_simple_pattern_match_name_binding_with_shadow() {
        // The first x is global and the second x is a match binding
        let expr_string = r#"
          match some(x) {
            some(x) => {
              let x = 1;
              x
            },
            none => 0
          }
        "#;

        let mut expr = Expr::from_text(expr_string).unwrap();

        expr.bind_variables_of_pattern_match();

        assert_eq!(expr, expectations::expected_match_with_let_binding(1));
    }

    #[test]
    fn test_simple_pattern_match_name_binding_block() {
        // The first x is global and the second x is a match binding
        let expr_string = r#"
          match some(x) {
            some(x) => x,
            none => 0
          };

          match some(x) {
            some(x) => x,
            none => 0
          }
        "#;

        let mut expr = Expr::from_text(expr_string).unwrap();

        expr.bind_variables_of_pattern_match();

        let first_expr = expectations::expected_match(1);
        let second_expr = expectations::expected_match(3); // 3 because first block has 2 arms

        let block = Expr::expr_block(vec![first_expr, second_expr])
            .with_inferred_type(InferredType::unknown());

        assert_eq!(expr, block);
    }

    #[test]
    fn test_nested_simple_pattern_match_binding() {
        let expr_string = r#"
          match ok(some(x)) {
            ok(x) => match x {
              some(x) => x,
              none => 0
            },
            err(x) => 0
          }
        "#;

        let mut expr = Expr::from_text(expr_string).unwrap();

        expr.bind_variables_of_pattern_match();

        assert_eq!(expr, expectations::expected_nested_match());
    }

    mod expectations {
        use crate::{ArmPattern, Expr, InferredType, MatchArm, MatchIdentifier, VariableId};
        use bigdecimal::BigDecimal;

        pub(crate) fn expected_match(index: usize) -> Expr {
            Expr::pattern_match(
                Expr::option(Some(Expr::identifier_global("x", None)))
                    .with_inferred_type(InferredType::option(InferredType::unknown())),
                vec![
                    MatchArm {
                        arm_pattern: ArmPattern::constructor(
                            "some",
                            vec![ArmPattern::literal(Expr::identifier_with_variable_id(
                                VariableId::MatchIdentifier(MatchIdentifier::new(
                                    "x".to_string(),
                                    index,
                                )),
                                None,
                            ))],
                        ),
                        arm_resolution_expr: Box::new(Expr::identifier_with_variable_id(
                            VariableId::MatchIdentifier(MatchIdentifier::new(
                                "x".to_string(),
                                index,
                            )),
                            None,
                        )),
                    },
                    MatchArm {
                        arm_pattern: ArmPattern::constructor("none", vec![]),
                        arm_resolution_expr: Box::new(Expr::number(BigDecimal::from(0))),
                    },
                ],
            )
        }

        pub(crate) fn expected_match_with_let_binding(index: usize) -> Expr {
            let let_binding = Expr::let_binding("x", Expr::number(BigDecimal::from(1)), None);
            let identifier_expr =
                Expr::identifier_with_variable_id(VariableId::Global("x".to_string()), None);
            let block = Expr::expr_block(vec![let_binding, identifier_expr]);

            Expr::pattern_match(
                Expr::option(Some(Expr::identifier_global("x", None))),
                vec![
                    MatchArm {
                        arm_pattern: ArmPattern::constructor(
                            "some",
                            vec![ArmPattern::literal(Expr::identifier_with_variable_id(
                                VariableId::MatchIdentifier(MatchIdentifier::new(
                                    "x".to_string(),
                                    index,
                                )),
                                None,
                            ))],
                        ),
                        arm_resolution_expr: Box::new(block),
                    },
                    MatchArm {
                        arm_pattern: ArmPattern::constructor("none", vec![]),
                        arm_resolution_expr: Box::new(Expr::number(BigDecimal::from(0))),
                    },
                ],
            )
        }

        pub(crate) fn expected_nested_match() -> Expr {
            Expr::pattern_match(
                Expr::ok(
                    Expr::option(Some(Expr::identifier_with_variable_id(
                        VariableId::Global("x".to_string()),
                        None,
                    )))
                    .with_inferred_type(InferredType::option(InferredType::unknown())),
                    None,
                )
                .with_inferred_type(InferredType::result(
                    Some(InferredType::option(InferredType::unknown())),
                    Some(InferredType::unknown()),
                )),
                vec![
                    MatchArm {
                        arm_pattern: ArmPattern::constructor(
                            "ok",
                            vec![ArmPattern::literal(Expr::identifier_with_variable_id(
                                VariableId::MatchIdentifier(MatchIdentifier::new(
                                    "x".to_string(),
                                    1,
                                )),
                                None,
                            ))],
                        ),
                        arm_resolution_expr: Box::new(Expr::pattern_match(
                            Expr::identifier_with_variable_id(
                                VariableId::MatchIdentifier(MatchIdentifier::new(
                                    "x".to_string(),
                                    1,
                                )),
                                None,
                            ),
                            vec![
                                MatchArm {
                                    arm_pattern: ArmPattern::constructor(
                                        "some",
                                        vec![ArmPattern::literal(
                                            Expr::identifier_with_variable_id(
                                                VariableId::MatchIdentifier(MatchIdentifier::new(
                                                    "x".to_string(),
                                                    5,
                                                )),
                                                None,
                                            ),
                                        )],
                                    ),
                                    arm_resolution_expr: Box::new(
                                        Expr::identifier_with_variable_id(
                                            VariableId::MatchIdentifier(MatchIdentifier::new(
                                                "x".to_string(),
                                                5,
                                            )),
                                            None,
                                        ),
                                    ),
                                },
                                MatchArm {
                                    arm_pattern: ArmPattern::constructor("none", vec![]),
                                    arm_resolution_expr: Box::new(Expr::number(BigDecimal::from(
                                        0,
                                    ))),
                                },
                            ],
                        )),
                    },
                    MatchArm {
                        arm_pattern: ArmPattern::constructor(
                            "err",
                            vec![ArmPattern::literal(Expr::identifier_with_variable_id(
                                VariableId::MatchIdentifier(MatchIdentifier::new(
                                    "x".to_string(),
                                    4,
                                )),
                                None,
                            ))],
                        ),
                        arm_resolution_expr: Box::new(Expr::number(BigDecimal::from(0))),
                    },
                ],
            )
        }
    }
}
