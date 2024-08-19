use std::collections::VecDeque;
use crate::{Expr, InferredType, MatchArm, VariableId};

pub fn desugar_pattern_match(
    pred: &Expr,
    match_arms: &Vec<MatchArm>,
    expr_type: InferredType,
) -> Option<Expr> {
    let mut if_else_branches = vec![];

    for match_arm in match_arms.iter() {
        let if_else_branch = internal::IfElseBranch::from_pred_and_match_arm(match_arm, pred);
        if let Some(condition) = if_else_branch {
            if_else_branches.push(condition);
        }
    }

    internal::build_expr_from(if_else_branches).map(|expr| expr.add_infer_type(expr_type))
}

mod internal {
    use std::collections::VecDeque;
    use crate::{ArmPattern, Expr, InferredType, MatchArm, VariableId};

    pub(crate) fn build_expr_from(if_branches: Vec<IfElseBranch>) -> Option<Expr> {
        if let Some(branch) = if_branches.first() {
            let mut expr = Expr::cond(
                branch.condition.clone(),
                branch.body.clone(),
                Expr::Throw("No match found".to_string(), InferredType::Unknown),
            );

            for branch in if_branches.iter().skip(1).rev() {
                if let Expr::Cond(_, _, else_, _) = &mut expr {
                    let else_copy = *else_.clone();
                    *else_ = Box::new(
                        Expr::cond(branch.condition.clone(), branch.body.clone(), else_copy)
                            .add_infer_type(branch.body.inferred_type()),
                    );
                }
            }

            Some(expr)
        } else {
            None
        }
    }


    #[derive(Debug, Clone)]
    pub(crate) struct IfElseBranch {
        pub(crate) condition: Expr,
        pub(crate) body: Expr,
    }

    impl IfElseBranch {
        pub(crate) fn from_pred_and_match_arm(match_arm: &MatchArm, pred: &Expr) -> Option<IfElseBranch> {
            get_conditions(match_arm, pred, None, pred.inferred_type())
        }
    }

    // Match arms are converted to if-else conditions, with the help of instruction called `GetTag`.
    // We rely on the actual constructor names, and for that reason we pattern ok, err, option, none
    // having a precise control. For example, if it's none, there is no need to check the tag of predicate and
    // and arm-literal, as it is a direct boolean expression.
    fn get_conditions(match_arm: &MatchArm, pred_expr: &Expr, tag: Option<Expr>, inferred_type_of_pred: InferredType) -> Option<IfElseBranch> {
        let arm_pattern = &match_arm.arm_pattern;
        let resolution = &match_arm.arm_resolution_expr;

        match arm_pattern {
            ArmPattern::Literal(arm_pattern_expr) => {
                handle_literal(arm_pattern_expr, pred_expr, resolution, tag, inferred_type_of_pred)
            }

            ArmPattern::Constructor(constructor_name, expressions) => {
               hande_constructor(pred_expr, constructor_name, expressions, resolution, inferred_type_of_pred)
            }

            ArmPattern::As(name, inner_pattern) => {
                handle_as_pattern(name, inner_pattern, pred_expr, resolution, tag, inferred_type_of_pred)
            }

            ArmPattern::WildCard => {
                let branch = IfElseBranch {
                    condition: tag.unwrap_or(Expr::boolean(true)),
                    body: resolution.as_ref().clone(),
                };
                Some(branch)
            }
        }
    }

    fn handle_literal(
        arm_pattern_expr: &Expr,
        pred_expr: &Expr,
        resolution: &Expr,
        tag: Option<Expr>,
        pred_expr_inferred_type: InferredType,
    ) -> Option<IfElseBranch> {
        match arm_pattern_expr {
            Expr::Option(Some(inner_pattern), _) => {
                let unwrapped_inferred_type =
                    match pred_expr_inferred_type {
                        InferredType::Option(inner) => *inner,
                        _ => InferredType::Unknown
                    };

                get_conditions(
                    &MatchArm::match_arm(
                        ArmPattern::Literal(inner_pattern.clone()),
                        resolution.clone(),
                    ),
                    &pred_expr.unwrap(),
                    Some(Expr::equal_to(Expr::tag(pred_expr.clone()), Expr::literal("some"))),
                    unwrapped_inferred_type
                )
            }

            Expr::Option(None, _) => {
                let branch = IfElseBranch {
                    condition: Expr::equal_to(
                        Expr::tag(pred_expr.clone()),
                        Expr::literal("none"),
                    ),
                    body: resolution.clone(),
                };
                Some(branch)
            }

            Expr::Result(Ok(inner_pattern), _) => {
                let unwrapped_inferred_type =
                    match pred_expr_inferred_type {
                        InferredType::Result {
                            ok,..
                        } => ok.unwrap_or(Box::new(InferredType::Unknown)),
                        _ => Box::new(InferredType::Unknown)
                    };

                get_conditions(
                    &MatchArm::match_arm(
                        ArmPattern::Literal(inner_pattern.clone()),
                        resolution.clone(),
                    ),
                    &pred_expr.unwrap(),
                    Some(Expr::equal_to(Expr::tag(pred_expr.clone()), Expr::literal("ok"))),
                    *unwrapped_inferred_type
                )
            }
            Expr::Result(Err(inner_pattern), _) => {
                let unwrapped_inferred_type =
                    match pred_expr_inferred_type {
                        InferredType::Result {
                            error,..
                        } => error.unwrap_or(Box::new(InferredType::Unknown)),
                        _ => Box::new(InferredType::Unknown)
                    };

                get_conditions(
                    &MatchArm::match_arm(
                        ArmPattern::Literal(inner_pattern.clone()),
                        resolution.clone(),
                    ),
                    &pred_expr.unwrap(),
                    Some(Expr::equal_to(Expr::tag(pred_expr.clone()), Expr::literal("err"))),
                   *unwrapped_inferred_type
                )
            }

            Expr::Identifier(identifier, inferred_type) => {
                let assign_var = Expr::Let(
                    VariableId::global(identifier.name()),
                    Box::new(pred_expr.clone()),
                    inferred_type.clone(),
                );

                let block = Expr::multiple(vec![assign_var, resolution.clone()]);
                let branch = IfElseBranch {
                    condition: tag.unwrap_or(Expr::boolean(true)),
                    body: block,
                };
                Some(branch)
            }

            _ => {
                // use tag lookup
                let branch = IfElseBranch {
                    condition: Expr::equal_to(pred_expr.clone(), arm_pattern_expr.clone()),
                    body: resolution.clone(),
                };
                Some(branch)
            }
        }
    }

    fn hande_constructor(pred_expr: &Expr, constructor_name: &str, bind_patterns: &Vec<ArmPattern>, resolution: &Expr, pred_expr_inferred_type: InferredType) -> Option<IfElseBranch> {
        match pred_expr_inferred_type {
            InferredType::Variant(variant) => {
                let arg_pattern_opt = bind_patterns.first();

                let inner_type = &variant
                    .iter()
                    .find(|(case_name, _)| case_name == constructor_name);
                let inner_variant_arg_type = inner_type.and_then(|(_, typ)| typ.clone());

                match (arg_pattern_opt, inner_variant_arg_type) {
                    (None, None) => None,
                    (Some(pattern), Some(inferred_type)) => get_conditions(
                        &MatchArm::match_arm(pattern.clone(), resolution.clone()),
                        &pred_expr.unwrap(),
                        Some(Expr::equal_to(Expr::tag(pred_expr.clone()), Expr::literal(constructor_name))),
                        inferred_type
                    ),
                    _ => None, // Probably fail here
                }
            }
            InferredType::Option(inner) => {
                match bind_patterns.first() {
                    Some(pattern) => get_conditions(
                        &MatchArm::match_arm(pattern.clone(), resolution.clone()),
                        &pred_expr.unwrap(),
                        Some(Expr::equal_to(Expr::tag(pred_expr.clone()), Expr::literal(constructor_name))),
                        *inner
                    ),
                    _ => Some(IfElseBranch {
                        condition: Expr::equal_to(Expr::tag(pred_expr.clone()), Expr::literal(constructor_name)),
                        body: resolution.clone(),
                    })
                }
            }
            InferredType::Result { ok, error } => {
                let inner_variant_arg_type = if constructor_name == "ok" {
                    ok.as_deref()
                } else {
                    error.as_deref()
                };

                match bind_patterns.first() {
                    Some(pattern) => {
                        get_conditions(
                            &MatchArm::match_arm(pattern.clone(), resolution.clone()),
                            &pred_expr.unwrap(),
                            Some(Expr::equal_to(Expr::tag(pred_expr.clone()), Expr::literal(constructor_name))),
                            inner_variant_arg_type.unwrap_or(&InferredType::Unknown).clone()
                        )
                    },

                    _ => None, // Probably fail here to get fine grained error message
                }
            }
            InferredType::Unknown => Some(IfElseBranch {
                condition: Expr::boolean(false),
                body: resolution.clone(),
            }),
            _ => None // probably fail here to get fine grained error message

        }
    }

    fn handle_as_pattern(name: &str, inner_pattern: &ArmPattern, pred_expr: &Expr, resolution: &Expr, tag: Option<Expr>, pred_expr_inferred_type: InferredType) -> Option<IfElseBranch> {
        let binding = Expr::Let(
            VariableId::global(name.to_string()),
            Box::new(pred_expr.clone()),
            pred_expr.inferred_type(),
        );

        let block = Expr::multiple(vec![binding, resolution.clone()]);
        get_conditions(
            &MatchArm::match_arm(
                inner_pattern.clone(),
                block
            ),
            pred_expr,
            tag,
            pred_expr_inferred_type
        )
    }
}




#[cfg(test)]
mod desugar_tests {
    use crate::type_registry::FunctionTypeRegistry;
    use crate::{Expr, InferredType, Number, VariableId};
    use golem_wasm_ast::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedType, TypeU32, TypeU64,
    };
    use std::ops::Deref;

    use super::*;

    fn get_function_type_registry() -> FunctionTypeRegistry {
        let metadata = vec![
            AnalysedExport::Function(AnalysedFunction {
                name: "foo".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "my_parameter".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                }],
                results: vec![],
            }),
            AnalysedExport::Function(AnalysedFunction {
                name: "baz".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "my_parameter".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                }],
                results: vec![],
            }),
        ];
        FunctionTypeRegistry::from_export_metadata(&metadata)
    }

    #[test]
    fn test_desugar_pattern_match() {
        let rib_expr = r#"
          match 1 {
            1 => true,
            2 => false
          }
        "#;

        let function_type_registry = get_function_type_registry();

        let mut expr = Expr::from_text(rib_expr).unwrap();
        let _ = expr.infer_types(&function_type_registry).unwrap();

        let desugared_expr = match expr.clone() {
            Expr::PatternMatch(predicate, match_arms, _) => {
                desugar_pattern_match(predicate.deref(), &match_arms, expr.inferred_type()).unwrap()
            }
            _ => panic!("Expected a match expression"),
        };

        let expected = Expr::cond(
            Expr::equal_to(
                Expr::Number(Number { value: 1f64 }, InferredType::U64),
                Expr::Number(Number { value: 1f64 }, InferredType::U64),
            ),
            Expr::boolean(true),
            Expr::cond(
                Expr::equal_to(
                    Expr::Number(Number { value: 1f64 }, InferredType::U64),
                    Expr::Number(Number { value: 2f64 }, InferredType::U64),
                ),
                Expr::boolean(false),
                Expr::Throw("No match found".to_string(), InferredType::Unknown),
            )
            .add_infer_type(InferredType::Bool),
        )
        .add_infer_type(InferredType::Bool);

        assert_eq!(desugared_expr, expected);
    }

    #[test]
    fn test_desugar_pattern_match_with_literals() {
        let rib_expr = r#"
          match some(1) {
            some(2) => true,
            some(3) => false
          }
        "#;

        let function_type_registry = get_function_type_registry();

        let mut expr = Expr::from_text(rib_expr).unwrap();
        let _ = expr.infer_types(&function_type_registry).unwrap();

        let desugared_expr = match expr.clone() {
            Expr::PatternMatch(predicate, match_arms, _) => {
                desugar_pattern_match(predicate.deref(), &match_arms, expr.inferred_type()).unwrap()
            }
            _ => panic!("Expected a match expression"),
        };

        let expected = Expr::cond(
            Expr::equal_to(
                Expr::Unwrap(
                    Box::new(Expr::option(Some(Expr::Number(
                        Number { value: 1f64 },
                        InferredType::U64,
                    )))),
                    InferredType::Unknown,
                ),
                Expr::Number(Number { value: 2f64 }, InferredType::U64),
            ),
            Expr::boolean(true),
            Expr::cond(
                Expr::equal_to(
                    Expr::Unwrap(
                        Box::new(Expr::option(Some(Expr::Number(
                            Number { value: 1f64 },
                            InferredType::U64,
                        )))),
                        InferredType::Unknown,
                    ),
                    Expr::Number(Number { value: 3f64 }, InferredType::U64),
                ),
                Expr::boolean(false),
                Expr::Throw("No match found".to_string(), InferredType::Unknown),
            )
            .add_infer_type(InferredType::Bool),
        )
        .add_infer_type(InferredType::Bool);

        assert_eq!(desugared_expr, expected);
    }

    #[test]
    fn test_desugar_pattern_match_with_identifiers() {
        let rib_expr = r#"
          match some(1) {
            some(x) => x,
            some(y) => y
          }
        "#;

        let function_type_registry = get_function_type_registry();

        let mut expr = Expr::from_text(rib_expr).unwrap();
        let _ = expr.infer_types(&function_type_registry).unwrap();

        let desugared_expr = match expr.clone() {
            Expr::PatternMatch(predicate, match_arms, _) => {
                desugar_pattern_match(predicate.deref(), &match_arms, expr.inferred_type()).unwrap()
            }
            _ => panic!("Expected a match expression"),
        };

        let expected_let_binding1 = Expr::let_binding(
            "x",
            Expr::Unwrap(
                Box::new(Expr::option(Some(Expr::Number(
                    Number { value: 1f64 },
                    InferredType::U64,
                )))),
                InferredType::Unknown
            ),
        )
        .add_infer_type(InferredType::U64);

        let first_expected_block = Expr::multiple(vec![
            expected_let_binding1,
            Expr::Identifier(VariableId::local_with_no_id("x"), InferredType::U64),
        ]);

        let expected_let_binding2 = Expr::let_binding(
            "y",
            Expr::Unwrap(
                Box::new(Expr::option(Some(Expr::Number(
                    Number { value: 1f64 },
                    InferredType::U64,
                )))),
                InferredType::Unknown,
            ),
        )
        .add_infer_type(InferredType::U64);

        let second_expected_block = Expr::multiple(vec![
            expected_let_binding2.add_infer_type(InferredType::U64),
            Expr::Identifier(VariableId::local_with_no_id("y"), InferredType::U64),
        ]);

        let expected = Expr::cond(
            Expr::boolean(true),
            first_expected_block,
            Expr::cond(
                Expr::boolean(true),
                second_expected_block,
                Expr::Throw("No match found".to_string(), InferredType::Unknown),
            )
            .add_infer_type(InferredType::U64),
        )
        .add_infer_type(InferredType::U64);

        assert_eq!(desugared_expr, expected);
    }
}
