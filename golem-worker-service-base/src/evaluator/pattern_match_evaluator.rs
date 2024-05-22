use crate::evaluator::evaluator_context::EvaluationContext;
use crate::evaluator::{DefaultEvaluator, Evaluator};
use crate::evaluator::{EvaluationError, EvaluationResult};
use crate::expression::{ArmPattern, ConstructorTypeName, Expr, InBuiltConstructorInner, MatchArm};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::TypeAnnotatedValue;
use std::ops::Deref;
use std::sync::Arc;
use crate::worker_bridge_execution::WorkerRequestExecutor;

struct BindingVariable(String);

enum ArmPatternOutput {
    Matched(MatchResult),
    NoneMatched,
    TypeMisMatch(TypeMisMatchResult),
}

struct MatchResult {
    binding_variable: Option<BindingVariable>,
    result: TypeAnnotatedValue,
}

struct TypeMisMatchResult {
    expected_type: String,
    actual_type: String,
}

pub(crate) async fn evaluate_pattern_match(
    worker_executor: &Arc<dyn WorkerRequestExecutor>,
    match_expr: &Expr,
    arms: &Vec<MatchArm>,
    input: &mut EvaluationContext,
) -> Result<EvaluationResult, EvaluationError> {
    let evaluator = DefaultEvaluator::from_worker_request_executor(worker_executor.clone());

    let match_evaluated = evaluator.evaluate(match_expr, input).await?;

    let mut resolved: Option<EvaluationResult> = None;

    for arm in arms {
        let constructor_pattern = &arm.0 .0;
        let match_arm_evaluated = evaluate_arm_pattern(
            constructor_pattern,
            &match_evaluated
                .get_value()
                .ok_or("Unit cannot be part of match expression".to_string())?,
            input,
            None,
        )?;

        match match_arm_evaluated {
            ArmPatternOutput::Matched(match_result) => {
                if let Some(binding_variable) = &match_result.binding_variable {
                    let typ = AnalysedType::from(&match_result.result);

                    input.merge_variables(&TypeAnnotatedValue::Record {
                        value: vec![(binding_variable.0.clone(), match_result.result)],
                        typ: vec![(binding_variable.0.clone(), typ)],
                    });
                }

                let arm_body = &arm.0 .1;

                resolved = Some(evaluator.evaluate(arm_body, input).await?);
                break;
            }
            ArmPatternOutput::TypeMisMatch(type_mismatch) => {
                return Err(EvaluationError::Message(format!(
                    "Type mismatch. Expected: {}, Actual: {}",
                    type_mismatch.expected_type, type_mismatch.actual_type
                )));
            }

            ArmPatternOutput::NoneMatched => {}
        }
    }

    resolved.ok_or(EvaluationError::Message(
        "Pattern matching failed".to_string(),
    ))
}

fn evaluate_arm_pattern(
    constructor_pattern: &ArmPattern,
    match_expr_result: &TypeAnnotatedValue,
    input: &mut EvaluationContext,
    binding_variable: Option<BindingVariable>,
) -> Result<ArmPatternOutput, EvaluationError> {
    match constructor_pattern {
        ArmPattern::WildCard => Ok(ArmPatternOutput::Matched(MatchResult {
            binding_variable: None,
            result: match_expr_result.clone(),
        })),
        ArmPattern::As(variable, arm_pattern) => {
            let binding_variable = variable.clone();
            evaluate_arm_pattern(
                arm_pattern,
                match_expr_result,
                input,
                Some(BindingVariable(binding_variable)),
            )
        }

        ArmPattern::Constructor(name, variables) => match name {
            ConstructorTypeName::InBuiltConstructor(in_built) => match in_built {
                InBuiltConstructorInner::Ok => handle_ok(
                    match_expr_result,
                    variables.first().ok_or(EvaluationError::Message(
                        "Ok constructor should have a variable".to_string(),
                    ))?,
                    binding_variable,
                    input,
                ),
                InBuiltConstructorInner::Err => handle_err(
                    match_expr_result,
                    variables.first().ok_or(EvaluationError::Message(
                        "Err constructor should have a value".to_string(),
                    ))?,
                    binding_variable,
                    input,
                ),
                InBuiltConstructorInner::None => handle_none(match_expr_result),
                InBuiltConstructorInner::Some => handle_some(
                    match_expr_result,
                    variables.first().ok_or(EvaluationError::Message(
                        "Some constructor should have a variable".to_string(),
                    ))?,
                    binding_variable,
                    input,
                ),
            },
            ConstructorTypeName::Identifier(name) => {
                if variables.is_empty() {
                    // TODO; Populate evaluation-context with type informations.
                    // The fact that if name is actually a variant (custom constructor)
                    // with zero parameters or enum value or a simple variable can now be available as a symbol
                    // table in evaluation context.
                    Ok(ArmPatternOutput::Matched(MatchResult {
                        binding_variable: Some(BindingVariable(name.clone())),
                        result: match_expr_result.clone(),
                    }))
                } else {
                    handle_variant(
                        name.as_str(),
                        match_expr_result,
                        variables,
                        binding_variable,
                        input,
                    )
                }
            }
        },
        ArmPattern::Literal(expr) => match expr.deref() {
            Expr::Identifier(variable) => Ok(ArmPatternOutput::Matched(MatchResult {
                binding_variable: Some(BindingVariable(variable.clone())),
                result: match_expr_result.clone(),
            })),

            expr => {
                let arm_pattern = ArmPattern::from_expr(expr.clone());
                evaluate_arm_pattern(&arm_pattern, match_expr_result, input, binding_variable)
            }
        },
    }
}

fn handle_ok(
    match_expr_result: &TypeAnnotatedValue,
    ok_variable: &ArmPattern,
    binding_variable: Option<BindingVariable>,
    input: &mut EvaluationContext,
) -> Result<ArmPatternOutput, EvaluationError> {
    match match_expr_result {
        result @ TypeAnnotatedValue::Result {
            value: Ok(ok_value),
            ..
        } => {
            let type_annotated_value_in_ok = *ok_value.clone().ok_or(EvaluationError::Message(
                "Ok constructor should have a value".to_string(),
            ))?;

            if let Some(bv) = binding_variable {
                input.merge_variables(&TypeAnnotatedValue::Record {
                    value: vec![(bv.0.clone(), result.clone())],
                    typ: vec![(bv.0.clone(), AnalysedType::from(result))],
                });
            }

            evaluate_arm_pattern(ok_variable, &type_annotated_value_in_ok, input, None)
        }

        TypeAnnotatedValue::Result { value: Err(_), .. } => Ok(ArmPatternOutput::NoneMatched),

        type_annotated_value => Ok(ArmPatternOutput::TypeMisMatch(TypeMisMatchResult {
            expected_type: "Result::Ok".to_string(),
            actual_type: format!("{:?}", AnalysedType::from(type_annotated_value)),
        })),
    }
}

fn handle_err(
    match_expr_result: &TypeAnnotatedValue,
    err_variable: &ArmPattern,
    binding_variable: Option<BindingVariable>,
    input: &mut EvaluationContext,
) -> Result<ArmPatternOutput, EvaluationError> {
    match match_expr_result {
        result @ TypeAnnotatedValue::Result {
            value: Err(err_value),
            ..
        } => {
            let type_annotated_value_in_err = err_value.clone().ok_or(EvaluationError::Message(
                "Err constructor should have a value".to_string(),
            ))?;

            if let Some(bv) = binding_variable {
                input.merge_variables(&TypeAnnotatedValue::Record {
                    value: vec![(bv.0.clone(), result.clone())],
                    typ: vec![(bv.0.clone(), AnalysedType::from(result))],
                });
            }

            evaluate_arm_pattern(err_variable, &type_annotated_value_in_err, input, None)
        }

        TypeAnnotatedValue::Result { value: Ok(_), .. } => Ok(ArmPatternOutput::NoneMatched),

        type_annotated_value => Ok(ArmPatternOutput::TypeMisMatch(TypeMisMatchResult {
            expected_type: "Result::Err".to_string(),
            actual_type: format!("{:?}", AnalysedType::from(type_annotated_value)),
        })),
    }
}

fn handle_some(
    match_expr_result: &TypeAnnotatedValue,
    some_variable: &ArmPattern,
    binding_variable: Option<BindingVariable>,
    input: &mut EvaluationContext,
) -> Result<ArmPatternOutput, EvaluationError> {
    match match_expr_result {
        result @ TypeAnnotatedValue::Option {
            value: Some(some_value),
            ..
        } => {
            let type_annotated_value_in_some = *some_value.clone();

            if let Some(bv) = binding_variable {
                input.merge_variables(&TypeAnnotatedValue::Record {
                    value: vec![(bv.0.clone(), result.clone())],
                    typ: vec![(bv.0.clone(), AnalysedType::from(result))],
                });
            }

            evaluate_arm_pattern(some_variable, &type_annotated_value_in_some, input, None)
        }

        TypeAnnotatedValue::Option { value: None, .. } => Ok(ArmPatternOutput::NoneMatched),

        type_annotated_value => Ok(ArmPatternOutput::TypeMisMatch(TypeMisMatchResult {
            expected_type: "Option::Some".to_string(),
            actual_type: format!("{:?}", AnalysedType::from(type_annotated_value)),
        })),
    }
}

fn handle_none(
    match_expr_result: &TypeAnnotatedValue,
) -> Result<ArmPatternOutput, EvaluationError> {
    match match_expr_result {
        TypeAnnotatedValue::Option { value: None, .. } => {
            Ok(ArmPatternOutput::Matched(MatchResult {
                binding_variable: None,
                result: TypeAnnotatedValue::Option {
                    value: None,
                    typ: AnalysedType::Str,
                },
            }))
        }

        TypeAnnotatedValue::Option { value: Some(_), .. } => Ok(ArmPatternOutput::NoneMatched),

        type_annotated_value => Ok(ArmPatternOutput::TypeMisMatch(TypeMisMatchResult {
            expected_type: "Option::None".to_string(),
            actual_type: format!("{:?}", AnalysedType::from(type_annotated_value)),
        })),
    }
}

fn handle_variant(
    variant_name: &str,
    match_expr_result: &TypeAnnotatedValue,
    variables: &[ArmPattern],
    binding_variable: Option<BindingVariable>,
    input: &mut EvaluationContext,
) -> Result<ArmPatternOutput, EvaluationError> {
    match match_expr_result {
        result @ TypeAnnotatedValue::Variant {
            case_name,
            case_value,
            ..
        } => {
            if case_name == variant_name {
                let type_annotated_value_in_case = *case_value.clone().ok_or(
                    EvaluationError::Message("Variant constructor should have a value".to_string()),
                )?;

                if let Some(bv) = binding_variable {
                    input.merge_variables(&TypeAnnotatedValue::Record {
                        value: vec![(bv.0.clone(), result.clone())],
                        typ: vec![(bv.0.clone(), AnalysedType::from(result))],
                    });
                }

                match variables.first() {
                    Some(variable) => {
                        evaluate_arm_pattern(variable, &type_annotated_value_in_case, input, None)
                    }
                    None => Ok(ArmPatternOutput::Matched(MatchResult {
                        binding_variable: None,
                        result: type_annotated_value_in_case,
                    })),
                }
            } else {
                Ok(ArmPatternOutput::NoneMatched)
            }
        }
        type_annotated_value => Ok(ArmPatternOutput::TypeMisMatch(TypeMisMatchResult {
            expected_type: format!("Variant::{}", variant_name),
            actual_type: format!("{:?}", AnalysedType::from(type_annotated_value)),
        })),
    }
}
