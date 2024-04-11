use crate::evaluator::EvaluationError;
use crate::evaluator::Evaluator;
use crate::expression::{ArmPattern, ConstructorTypeName, Expr, InBuiltConstructorInner, MatchArm};
use crate::merge::Merge;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::TypeAnnotatedValue;
use std::ops::Deref;

struct BindingVariable(String);

enum PatternMatchOutput {
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

pub(crate) fn evaluate_pattern_match(
    match_expr: &Expr,
    arms: &Vec<MatchArm>,
    input: &mut TypeAnnotatedValue,
) -> Result<TypeAnnotatedValue, EvaluationError> {
    let match_evaluated = match_expr.evaluate(input)?;

    let mut resolved: Option<TypeAnnotatedValue> = None;

    for arm in arms {
        let constructor_pattern = &arm.0 .0;
        let match_arm_evaluated = evaluate_arm_pattern(constructor_pattern, &match_evaluated)?;

        match match_arm_evaluated {
            PatternMatchOutput::Matched(match_result) => {
                if let Some(binding_variable) = &match_result.binding_variable {
                    let typ = AnalysedType::from(&match_result.result);

                    input.merge(&TypeAnnotatedValue::Record {
                        value: vec![(binding_variable.0.clone(), match_result.result)],
                        typ: vec![(binding_variable.0.clone(), typ)],
                    });
                }

                let arm_body = &arm.0 .1;

                resolved = Some(arm_body.evaluate(input)?);
                break;
            }
            PatternMatchOutput::TypeMisMatch(type_mismatch) => {
                return Err(EvaluationError::Message(format!(
                    "Type mismatch. Expected: {}, Actual: {}",
                    type_mismatch.expected_type, type_mismatch.actual_type
                )));
            }

            PatternMatchOutput::NoneMatched => {}
        }
    }

    resolved.ok_or(EvaluationError::Message(
        "Pattern matching failed".to_string(),
    ))
}

fn evaluate_arm_pattern(
    constructor_pattern: &ArmPattern,
    match_expr_result: &TypeAnnotatedValue,
) -> Result<PatternMatchOutput, EvaluationError> {
    match constructor_pattern {
        ArmPattern::WildCard => Ok(PatternMatchOutput::Matched(MatchResult {
            binding_variable: None,
            result: match_expr_result.clone(),
        })),
        ArmPattern::As(variable, arm_pattern) => {
            let binding_variable = variable.clone();
            let result = evaluate_arm_pattern(arm_pattern, match_expr_result)?;

            let result = match result {
                PatternMatchOutput::Matched(match_result) => {
                    PatternMatchOutput::Matched(MatchResult {
                        binding_variable: Some(BindingVariable(binding_variable)),
                        result: match_result.result,
                    })
                }
                value => value,
            };

            Ok(result)
        }
        ArmPattern::Constructor(name, variables) => match name {
            ConstructorTypeName::InBuiltConstructor(in_built) => match in_built {
                InBuiltConstructorInner::Ok => handle_ok(
                    match_expr_result,
                    variables.first().ok_or(EvaluationError::Message(
                        "Ok constructor should have a value".to_string(),
                    ))?,
                ),
                InBuiltConstructorInner::Err => handle_err(
                    match_expr_result,
                    variables.first().ok_or(EvaluationError::Message(
                        "Err constructor should have a value".to_string(),
                    ))?,
                ),
                InBuiltConstructorInner::None => handle_none(match_expr_result),
                InBuiltConstructorInner::Some => handle_some(
                    match_expr_result,
                    variables.first().ok_or(EvaluationError::Message(
                        "Some constructor should have a value".to_string(),
                    ))?,
                ),
            },
            ConstructorTypeName::CustomConstructor(name) => {
                handle_variant(name.as_str(), match_expr_result, variables)
            }
        },
        ArmPattern::Literal(expr) => match expr.deref() {
            Expr::Variable(variable) => Ok(PatternMatchOutput::Matched(MatchResult {
                binding_variable: Some(BindingVariable(variable.clone())),
                result: match_expr_result.clone(),
            })),
            _ => Err(EvaluationError::Message(
                "Currently only variable pattern is supported".to_string(),
            )),
        },
    }
}

fn handle_ok(
    match_expr_result: &TypeAnnotatedValue,
    ok_variable: &ArmPattern,
) -> Result<PatternMatchOutput, EvaluationError> {
    match match_expr_result {
        TypeAnnotatedValue::Result {
            value: Ok(ok_value),
            ..
        } => {
            let type_annotated_value_in_ok = *ok_value.clone().ok_or(EvaluationError::Message(
                "Ok constructor should have a value".to_string(),
            ))?;
            evaluate_arm_pattern(ok_variable, &type_annotated_value_in_ok)
        }

        TypeAnnotatedValue::Result { value: Err(_), .. } => Ok(PatternMatchOutput::NoneMatched),

        type_annotated_value => Ok(PatternMatchOutput::TypeMisMatch(TypeMisMatchResult {
            expected_type: "Result::Ok".to_string(),
            actual_type: format!("{:?}", AnalysedType::from(type_annotated_value)),
        })),
    }
}

fn handle_err(
    match_expr_result: &TypeAnnotatedValue,
    err_variable: &ArmPattern,
) -> Result<PatternMatchOutput, EvaluationError> {
    match match_expr_result {
        TypeAnnotatedValue::Result {
            value: Err(err_value),
            ..
        } => {
            let type_annotated_value_in_err = *err_value.clone().ok_or(
                EvaluationError::Message("Err constructor should have a value".to_string()),
            )?;
            evaluate_arm_pattern(err_variable, &type_annotated_value_in_err)
        }

        TypeAnnotatedValue::Result { value: Ok(_), .. } => Ok(PatternMatchOutput::NoneMatched),

        type_annotated_value => Ok(PatternMatchOutput::TypeMisMatch(TypeMisMatchResult {
            expected_type: "Result::Err".to_string(),
            actual_type: format!("{:?}", AnalysedType::from(type_annotated_value)),
        })),
    }
}

fn handle_some(
    match_expr_result: &TypeAnnotatedValue,
    some_variable: &ArmPattern,
) -> Result<PatternMatchOutput, EvaluationError> {
    match match_expr_result {
        TypeAnnotatedValue::Option {
            value: Some(some_value),
            ..
        } => {
            let type_annotated_value_in_some = *some_value.clone();
            evaluate_arm_pattern(some_variable, &type_annotated_value_in_some)
        }

        TypeAnnotatedValue::Option { value: None, .. } => Ok(PatternMatchOutput::NoneMatched),

        type_annotated_value => Ok(PatternMatchOutput::TypeMisMatch(TypeMisMatchResult {
            expected_type: "Option::Some".to_string(),
            actual_type: format!("{:?}", AnalysedType::from(type_annotated_value)),
        })),
    }
}

fn handle_none(
    match_expr_result: &TypeAnnotatedValue,
) -> Result<PatternMatchOutput, EvaluationError> {
    match match_expr_result {
        TypeAnnotatedValue::Option { value: None, .. } => {
            Ok(PatternMatchOutput::Matched(MatchResult {
                binding_variable: None,
                result: TypeAnnotatedValue::Option {
                    value: None,
                    typ: AnalysedType::Str,
                },
            }))
        }

        TypeAnnotatedValue::Option { value: Some(_), .. } => Ok(PatternMatchOutput::NoneMatched),

        type_annotated_value => Ok(PatternMatchOutput::TypeMisMatch(TypeMisMatchResult {
            expected_type: "Option::None".to_string(),
            actual_type: format!("{:?}", AnalysedType::from(type_annotated_value)),
        })),
    }
}

fn handle_variant(
    variant_name: &str,
    match_expr_result: &TypeAnnotatedValue,
    variables: &Vec<ArmPattern>,
) -> Result<PatternMatchOutput, EvaluationError> {
    match match_expr_result {
        TypeAnnotatedValue::Variant {
            case_name,
            case_value,
            ..
        } => {
            if case_name == variant_name {
                let type_annotated_value_in_case = *case_value.clone().ok_or(
                    EvaluationError::Message("Variant constructor should have a value".to_string()),
                )?;

                match variables.first() {
                    Some(variable) => evaluate_arm_pattern(variable, &type_annotated_value_in_case),
                    None => Ok(PatternMatchOutput::Matched(MatchResult {
                        binding_variable: None,
                        result: type_annotated_value_in_case,
                    })),
                }
            } else {
                Ok(PatternMatchOutput::NoneMatched)
            }
        }
        type_annotated_value => Ok(PatternMatchOutput::TypeMisMatch(TypeMisMatchResult {
            expected_type: format!("Variant::{}", variant_name),
            actual_type: format!("{:?}", AnalysedType::from(type_annotated_value)),
        })),
    }
}
