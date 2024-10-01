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

use crate::compiler::byte_code::internal::ExprState;
use crate::compiler::ir::RibIR;
use crate::{Expr, InstructionId};
use bincode::{Decode, Encode};
use golem_api_grpc::proto::golem::rib::RibByteCode as ProtoRibByteCode;

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct RibByteCode {
    pub instructions: Vec<RibIR>,
}

impl RibByteCode {
    // Convert expression to bytecode instructions
    pub fn from_expr(expr: Expr) -> Result<RibByteCode, String> {
        let mut instructions = Vec::new();
        let mut stack: Vec<ExprState> = Vec::new();
        let mut instruction_id = InstructionId::init();
        stack.push(ExprState::from_expr(&expr));

        while let Some(remaining) = stack.pop() {
            match remaining {
                ExprState::Expr(expr) => {
                    internal::process_expr(
                        &expr,
                        &mut stack,
                        &mut instructions,
                        &mut instruction_id,
                    )?;
                }

                ExprState::Instruction(instruction) => {
                    instructions.push(instruction);
                }
            }
        }

        // Use VecDeque to avoid reversal, but ok as well since this is compilation
        Ok(RibByteCode {
            instructions: instructions.into_iter().rev().collect(),
        })
    }
}

impl TryFrom<ProtoRibByteCode> for RibByteCode {
    type Error = String;

    fn try_from(value: ProtoRibByteCode) -> Result<Self, Self::Error> {
        let proto_instructions = value.instructions;
        let mut instructions = Vec::new();

        for proto_instruction in proto_instructions {
            instructions.push(proto_instruction.try_into()?);
        }

        Ok(RibByteCode { instructions })
    }
}

impl From<RibByteCode> for ProtoRibByteCode {
    fn from(value: RibByteCode) -> Self {
        let mut instructions = Vec::new();
        for instruction in value.instructions {
            instructions.push(instruction.into());
        }

        ProtoRibByteCode { instructions }
    }
}

mod internal {
    use crate::compiler::desugar::desugar_pattern_match;
    use crate::{
        AnalysedTypeWithUnit, DynamicParsedFunctionReference, Expr, FunctionReferenceType,
        InferredType, InstructionId, RibIR,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

    use crate::call_type::CallType;
    use golem_wasm_rpc::protobuf::TypedFlags;
    use std::ops::Deref;

    pub(crate) fn process_expr(
        expr: &Expr,
        stack: &mut Vec<ExprState>,
        instructions: &mut Vec<RibIR>,
        instruction_id: &mut InstructionId,
    ) -> Result<(), String> {
        match expr {
            Expr::Unwrap(inner_expr, _) => {
                stack.push(ExprState::from_expr(inner_expr.deref()));
                instructions.push(RibIR::Deconstruct);
            }
            Expr::Throw(msg, _) => {
                instructions.push(RibIR::Throw(msg.to_string()));
            }
            Expr::Identifier(variable_id, _) => {
                instructions.push(RibIR::LoadVar(variable_id.clone()));
            }
            Expr::Literal(str, _) => {
                let type_annotated_value = TypeAnnotatedValue::Str(str.clone());
                instructions.push(RibIR::PushLit(type_annotated_value));
            }
            Expr::Number(num, _, inferred_type) => {
                let analysed_type = convert_to_analysed_type_for(expr, inferred_type)?;

                let type_annotated_value = num.to_val(&analysed_type).ok_or(format!(
                    "Internal error: convert a number to wasm value using {:?}",
                    analysed_type
                ))?;

                instructions.push(RibIR::PushLit(type_annotated_value));
            }
            Expr::EqualTo(lhs, rhs, _) => {
                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::EqualTo);
            }
            Expr::GreaterThan(lhs, rhs, _) => {
                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::GreaterThan);
            }
            Expr::LessThan(lhs, rhs, _) => {
                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::LessThan);
            }
            Expr::GreaterThanOrEqualTo(lhs, rhs, _) => {
                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::GreaterThanOrEqualTo);
            }
            Expr::LessThanOrEqualTo(lhs, rhs, _) => {
                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::LessThanOrEqualTo);
            }
            Expr::And(lhs, rhs, _) => {
                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::And);
            }
            Expr::Record(fields, inferred_type) => {
                // Push field instructions in reverse order
                for (field_name, field_expr) in fields.iter().rev() {
                    stack.push(ExprState::from_expr(field_expr.as_ref()));
                    instructions.push(RibIR::UpdateRecord(field_name.clone()));
                }
                // Push record creation instruction
                let analysed_type = convert_to_analysed_type_for(expr, inferred_type);
                instructions.push(RibIR::CreateAndPushRecord(analysed_type?));
            }
            Expr::Sequence(exprs, inferred_type) => {
                // Push all expressions in reverse order
                for expr in exprs.iter().rev() {
                    stack.push(ExprState::from_expr(expr));
                }

                let analysed_type = convert_to_analysed_type_for(expr, inferred_type)?;
                instructions.push(RibIR::PushList(analysed_type, exprs.len()));
            }
            Expr::Multiple(exprs, _) => {
                // Push all expressions in reverse order
                for expr in exprs.iter() {
                    stack.push(ExprState::from_expr(expr));
                }
            }
            Expr::Let(variable_id, _, inner_expr, _) => {
                stack.push(ExprState::from_expr(inner_expr.deref()));
                instructions.push(RibIR::AssignVar(variable_id.clone()));
            }
            Expr::PatternMatch(pred, match_arms, inferred_type) => {
                let desugared_pattern_match =
                    desugar_pattern_match(pred.deref(), match_arms, inferred_type.clone())
                        .ok_or("Desugar pattern match failed".to_string())?;
                stack.push(ExprState::from_expr(&desugared_pattern_match));
            }
            Expr::Cond(if_expr, then_expr, else_expr, _) => {
                handle_if_condition(
                    instruction_id,
                    if_expr.deref(),
                    then_expr.deref(),
                    else_expr.deref(),
                    stack,
                );
            }

            Expr::SelectField(record_expr, field_name, _) => {
                stack.push(ExprState::from_expr(record_expr.deref()));
                instructions.push(RibIR::SelectField(field_name.clone()));
            }
            Expr::SelectIndex(sequence_expr, index, _) => {
                stack.push(ExprState::from_expr(sequence_expr.deref()));
                instructions.push(RibIR::SelectIndex(*index));
            }
            Expr::Option(Some(inner_expr), inferred_type) => {
                stack.push(ExprState::from_expr(inner_expr.deref()));
                instructions.push(RibIR::PushSome(convert_to_analysed_type_for(
                    expr,
                    inferred_type,
                )?));
            }

            Expr::Option(None, inferred_type) => {
                let optional = convert_to_analysed_type_for(expr, inferred_type);
                instructions.push(RibIR::PushNone(optional.ok()));
            }

            Expr::Result(Ok(inner_expr), inferred_type) => {
                stack.push(ExprState::from_expr(inner_expr.deref()));
                instructions.push(RibIR::PushOkResult(convert_to_analysed_type_for(
                    expr,
                    inferred_type,
                )?));
            }

            Expr::Result(Err(inner_expr), inferred_type) => {
                stack.push(ExprState::from_expr(inner_expr.deref()));
                instructions.push(RibIR::PushErrResult(convert_to_analysed_type_for(
                    expr,
                    inferred_type,
                )?));
            }

            Expr::Call(invocation_name, arguments, inferred_type) => {
                for expr in arguments.iter().rev() {
                    stack.push(ExprState::from_expr(expr));
                }

                match invocation_name {
                    CallType::Function(parsed_function_name) => {
                        let function_result_type = if inferred_type.is_unit() {
                            AnalysedTypeWithUnit::Unit
                        } else {
                            AnalysedTypeWithUnit::Type(convert_to_analysed_type_for(
                                expr,
                                inferred_type,
                            )?)
                        };

                        instructions
                            .push(RibIR::InvokeFunction(arguments.len(), function_result_type));

                        let site = parsed_function_name.site.clone();

                        match &parsed_function_name.function {
                            DynamicParsedFunctionReference::Function { function } => instructions
                                .push(RibIR::CreateFunctionName(
                                    site,
                                    FunctionReferenceType::Function {
                                        function: function.clone(),
                                    },
                                )),

                            DynamicParsedFunctionReference::RawResourceConstructor { resource } => {
                                instructions.push(RibIR::CreateFunctionName(
                                    site,
                                    FunctionReferenceType::RawResourceConstructor {
                                        resource: resource.clone(),
                                    },
                                ))
                            }
                            DynamicParsedFunctionReference::RawResourceDrop { resource } => {
                                instructions.push(RibIR::CreateFunctionName(
                                    site,
                                    FunctionReferenceType::RawResourceDrop {
                                        resource: resource.clone(),
                                    },
                                ))
                            }
                            DynamicParsedFunctionReference::RawResourceMethod {
                                resource,
                                method,
                            } => instructions.push(RibIR::CreateFunctionName(
                                site,
                                FunctionReferenceType::RawResourceMethod {
                                    resource: resource.clone(),
                                    method: method.clone(),
                                },
                            )),
                            DynamicParsedFunctionReference::RawResourceStaticMethod {
                                resource,
                                method,
                            } => instructions.push(RibIR::CreateFunctionName(
                                site,
                                FunctionReferenceType::RawResourceStaticMethod {
                                    resource: resource.clone(),
                                    method: method.clone(),
                                },
                            )),
                            DynamicParsedFunctionReference::IndexedResourceConstructor {
                                resource,
                                resource_params,
                            } => {
                                for param in resource_params {
                                    stack.push(ExprState::from_expr(param));
                                }
                                instructions.push(RibIR::CreateFunctionName(
                                    site,
                                    FunctionReferenceType::IndexedResourceConstructor {
                                        resource: resource.clone(),
                                        arg_size: resource_params.len(),
                                    },
                                ))
                            }
                            DynamicParsedFunctionReference::IndexedResourceMethod {
                                resource,
                                resource_params,
                                method,
                            } => {
                                for param in resource_params {
                                    stack.push(ExprState::from_expr(param));
                                }
                                instructions.push(RibIR::CreateFunctionName(
                                    site,
                                    FunctionReferenceType::IndexedResourceMethod {
                                        resource: resource.clone(),
                                        arg_size: resource_params.len(),
                                        method: method.clone(),
                                    },
                                ))
                            }
                            DynamicParsedFunctionReference::IndexedResourceStaticMethod {
                                resource,
                                resource_params,
                                method,
                            } => {
                                for param in resource_params {
                                    stack.push(ExprState::from_expr(param));
                                }
                                instructions.push(RibIR::CreateFunctionName(
                                    site,
                                    FunctionReferenceType::IndexedResourceStaticMethod {
                                        resource: resource.clone(),
                                        arg_size: resource_params.len(),
                                        method: method.clone(),
                                    },
                                ))
                            }
                            DynamicParsedFunctionReference::IndexedResourceDrop {
                                resource,
                                resource_params,
                            } => {
                                for param in resource_params {
                                    stack.push(ExprState::from_expr(param));
                                }
                                instructions.push(RibIR::CreateFunctionName(
                                    site,
                                    FunctionReferenceType::IndexedResourceDrop {
                                        resource: resource.clone(),
                                        arg_size: resource_params.len(),
                                    },
                                ))
                            }
                        }
                    }

                    CallType::VariantConstructor(variant_name) => {
                        instructions.push(RibIR::PushVariant(
                            variant_name.clone(),
                            convert_to_analysed_type_for(expr, inferred_type)?,
                        ));
                    }
                    CallType::EnumConstructor(enmum_name) => {
                        instructions.push(RibIR::PushEnum(
                            enmum_name.clone(),
                            convert_to_analysed_type_for(expr, inferred_type)?,
                        ));
                    }
                }
            }

            Expr::Flags(flag_values, inferred_type) => match inferred_type {
                InferredType::Flags(all_flags) => {
                    instructions.push(RibIR::PushFlag(TypeAnnotatedValue::Flags(TypedFlags {
                        typ: all_flags.clone(),
                        values: flag_values.clone(),
                    })));
                }
                inferred_type => {
                    return Err(format!(
                        "Flags should have inferred type Flags {:?}",
                        inferred_type
                    ));
                }
            },
            Expr::Boolean(bool, _) => {
                instructions.push(RibIR::PushLit(TypeAnnotatedValue::Bool(*bool)));
            }
            Expr::GetTag(expr, _) => {
                stack.push(ExprState::from_expr(expr.deref()));
                stack.push(ExprState::from_ir(RibIR::GetTag));
            }

            Expr::Concat(exprs, _) => {
                for expr in exprs.iter().rev() {
                    stack.push(ExprState::from_expr(expr));
                }
                instructions.push(RibIR::Concat(exprs.len()));
            }

            Expr::Not(expr, _) => {
                stack.push(ExprState::from_expr(expr.deref()));
                instructions.push(RibIR::Negate);
            }

            Expr::Tuple(exprs, analysed_type) => {
                for expr in exprs.iter().rev() {
                    stack.push(ExprState::from_expr(expr));
                }
                let analysed_type = convert_to_analysed_type_for(expr, analysed_type)?;
                instructions.push(RibIR::PushTuple(analysed_type, exprs.len()));
            }
        }

        Ok(())
    }

    pub(crate) fn convert_to_analysed_type_for(
        expr: &Expr,
        inferred_type: &InferredType,
    ) -> Result<AnalysedType, String> {
        AnalysedType::try_from(inferred_type).map_err(|e| {
            format!(
                "Invalid Rib {}. Error converting {:?} to AnalysedType: {:?}",
                expr, inferred_type, e
            )
        })
    }

    // We create a temporary stack of expressions that we pop one by one,
    // while injecting some pre-defined IRs such as Jump in certain cases
    // This injection of new IRs in a stack can be found cumbersome compared
    // to simple recursion where we create the instructions earlier, and add an IR on
    // to the final instruction set, however, comes with the cost of stack safety
    pub(crate) enum ExprState {
        Expr(Expr),
        Instruction(RibIR),
    }

    impl ExprState {
        pub(crate) fn from_expr(expr: &Expr) -> Self {
            ExprState::Expr(expr.clone())
        }

        pub(crate) fn from_ir(ir: RibIR) -> Self {
            ExprState::Instruction(ir)
        }
    }

    fn handle_if_condition(
        instruction_id: &mut InstructionId,
        if_expr: &Expr,
        then_expr: &Expr,
        else_expr: &Expr,
        stack: &mut Vec<ExprState>,
    ) {
        instruction_id.increment_mut();
        let else_beginning_id = instruction_id.clone();
        instruction_id.increment_mut();
        let else_ending_id = instruction_id.clone();

        stack.push(ExprState::from_expr(if_expr));

        stack.push(ExprState::from_ir(RibIR::JumpIfFalse(
            else_beginning_id.clone(),
        )));

        stack.push(ExprState::from_expr(then_expr));

        stack.push(ExprState::from_ir(RibIR::Jump(else_ending_id.clone())));

        stack.push(ExprState::from_ir(RibIR::Label(else_beginning_id.clone())));

        stack.push(ExprState::from_expr(else_expr));

        stack.push(ExprState::from_ir(RibIR::Label(else_ending_id.clone())));
    }
}

#[cfg(test)]
mod compiler_tests {
    use super::*;
    use crate::{compiler, ArmPattern, InferredType, MatchArm, Number, VariableId};
    use golem_wasm_ast::analysis::{AnalysedType, NameTypePair, TypeRecord, TypeStr, TypeU32};
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

    #[test]
    fn test_instructions_for_literal() {
        let literal = Expr::Literal("hello".to_string(), InferredType::Str);

        let instructions = RibByteCode::from_expr(literal).unwrap();

        let instruction_set = vec![RibIR::PushLit(TypeAnnotatedValue::Str("hello".to_string()))];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_for_identifier() {
        let inferred_input_type = InferredType::Str;
        let variable_id = VariableId::local("request", 0);
        let expr = Expr::Identifier(variable_id.clone(), inferred_input_type);

        let instructions = RibByteCode::from_expr(expr).unwrap();

        let instruction_set = vec![RibIR::LoadVar(variable_id)];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_assign_variable() {
        let literal = Expr::Literal("hello".to_string(), InferredType::Str);

        let variable_id = VariableId::local("request", 0);

        let expr = Expr::Let(
            variable_id.clone(),
            None,
            Box::new(literal),
            InferredType::Unknown,
        );

        let instructions = RibByteCode::from_expr(expr).unwrap();

        let instruction_set = vec![
            RibIR::PushLit(TypeAnnotatedValue::Str("hello".to_string())),
            RibIR::AssignVar(variable_id),
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_equal_to() {
        let number_f32 = Expr::Number(Number { value: 1f64 }, None, InferredType::F32);
        let number_u32 = Expr::Number(Number { value: 1f64 }, None, InferredType::U32);

        let expr = Expr::equal_to(number_f32, number_u32);

        let instructions = RibByteCode::from_expr(expr).unwrap();

        let type_annotated_value1 = TypeAnnotatedValue::F32(1.0);
        let type_annotated_value2 = TypeAnnotatedValue::U32(1);

        let instruction_set = vec![
            RibIR::PushLit(type_annotated_value2),
            RibIR::PushLit(type_annotated_value1),
            RibIR::EqualTo,
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_greater_than() {
        let number_f32 = Expr::Number(Number { value: 1f64 }, None, InferredType::F32);
        let number_u32 = Expr::Number(Number { value: 2f64 }, None, InferredType::U32);

        let expr = Expr::greater_than(number_f32, number_u32);

        let instructions = RibByteCode::from_expr(expr).unwrap();

        let type_annotated_value1 = TypeAnnotatedValue::F32(1.0);
        let type_annotated_value2 = TypeAnnotatedValue::U32(2);

        let instruction_set = vec![
            RibIR::PushLit(type_annotated_value2),
            RibIR::PushLit(type_annotated_value1),
            RibIR::GreaterThan,
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_less_than() {
        let number_f32 = Expr::Number(Number { value: 1f64 }, None, InferredType::F32);
        let number_u32 = Expr::Number(Number { value: 1f64 }, None, InferredType::U32);

        let expr = Expr::less_than(number_f32, number_u32);

        let instructions = RibByteCode::from_expr(expr).unwrap();

        let type_annotated_value1 = TypeAnnotatedValue::F32(1.0);
        let type_annotated_value2 = TypeAnnotatedValue::U32(1);

        let instruction_set = vec![
            RibIR::PushLit(type_annotated_value2),
            RibIR::PushLit(type_annotated_value1),
            RibIR::LessThan,
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_greater_than_or_equal_to() {
        let number_f32 = Expr::Number(Number { value: 1f64 }, None, InferredType::F32);
        let number_u32 = Expr::Number(Number { value: 1f64 }, None, InferredType::U32);

        let expr = Expr::greater_than_or_equal_to(number_f32, number_u32);

        let instructions = RibByteCode::from_expr(expr).unwrap();

        let type_annotated_value1 = TypeAnnotatedValue::F32(1.0);
        let type_annotated_value2 = TypeAnnotatedValue::U32(1);

        let instruction_set = vec![
            RibIR::PushLit(type_annotated_value2),
            RibIR::PushLit(type_annotated_value1),
            RibIR::GreaterThanOrEqualTo,
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_less_than_or_equal_to() {
        let number_f32 = Expr::Number(Number { value: 1f64 }, None, InferredType::F32);
        let number_u32 = Expr::Number(Number { value: 1f64 }, None, InferredType::U32);

        let expr = Expr::less_than_or_equal_to(number_f32, number_u32);

        let instructions = RibByteCode::from_expr(expr).unwrap();

        let type_annotated_value1 = TypeAnnotatedValue::F32(1.0);
        let type_annotated_value2 = TypeAnnotatedValue::U32(1);

        let instruction_set = vec![
            RibIR::PushLit(type_annotated_value2),
            RibIR::PushLit(type_annotated_value1),
            RibIR::LessThanOrEqualTo,
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_for_record() {
        let record = Expr::Record(
            vec![
                (
                    "foo_key".to_string(),
                    Box::new(Expr::Literal("foo_value".to_string(), InferredType::Str)),
                ),
                (
                    "bar_key".to_string(),
                    Box::new(Expr::Literal("bar_value".to_string(), InferredType::Str)),
                ),
            ],
            InferredType::Record(vec![
                (String::from("foo_key"), InferredType::Str),
                (String::from("bar_key"), InferredType::Str),
            ]),
        );

        let instructions = RibByteCode::from_expr(record).unwrap();

        let bar_value = TypeAnnotatedValue::Str("bar_value".to_string());
        let foo_value = TypeAnnotatedValue::Str("foo_value".to_string());

        let instruction_set = vec![
            RibIR::PushLit(bar_value),
            RibIR::PushLit(foo_value),
            RibIR::CreateAndPushRecord(AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "foo_key".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    NameTypePair {
                        name: "bar_key".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
            })),
            RibIR::UpdateRecord("foo_key".to_string()),
            RibIR::UpdateRecord("bar_key".to_string()),
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_for_multiple() {
        let multiple = Expr::Multiple(
            vec![
                Expr::Literal("foo".to_string(), InferredType::Str),
                Expr::Literal("bar".to_string(), InferredType::Str),
            ],
            InferredType::Unknown,
        );

        let instructions = RibByteCode::from_expr(multiple).unwrap();

        let instruction_set = vec![
            RibIR::PushLit(TypeAnnotatedValue::Str("foo".to_string())),
            RibIR::PushLit(TypeAnnotatedValue::Str("bar".to_string())),
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_if_conditional() {
        let if_expr = Expr::Literal("pred".to_string(), InferredType::Str);
        let then_expr = Expr::Literal("then".to_string(), InferredType::Str);
        let else_expr = Expr::Literal("else".to_string(), InferredType::Str);

        let expr = Expr::Cond(
            Box::new(if_expr),
            Box::new(then_expr),
            Box::new(else_expr),
            InferredType::Str,
        );

        let instructions = RibByteCode::from_expr(expr).unwrap();

        let instruction_set = vec![
            RibIR::PushLit(TypeAnnotatedValue::Str("pred".to_string())),
            RibIR::JumpIfFalse(InstructionId { index: 1 }), // jumps to the next label having Id 1 (which is else block)
            RibIR::PushLit(TypeAnnotatedValue::Str("then".to_string())),
            RibIR::Jump(InstructionId { index: 2 }), // Once if is executed then jump to the end of the else block with id 2
            RibIR::Label(InstructionId { index: 1 }),
            RibIR::PushLit(TypeAnnotatedValue::Str("else".to_string())),
            RibIR::Label(InstructionId { index: 2 }),
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_for_nested_if_else() {
        let if_expr = Expr::Literal("if-pred1".to_string(), InferredType::Str);
        let then_expr = Expr::Literal("then1".to_string(), InferredType::Str);
        let else_expr = Expr::Cond(
            Box::new(Expr::Literal("else-pred2".to_string(), InferredType::Str)),
            Box::new(Expr::Literal("else-then2".to_string(), InferredType::Str)),
            Box::new(Expr::Literal("else-else2".to_string(), InferredType::Str)),
            InferredType::Str,
        );

        let expr = Expr::Cond(
            Box::new(if_expr),
            Box::new(then_expr),
            Box::new(else_expr),
            InferredType::Str,
        );

        let instructions = RibByteCode::from_expr(expr).unwrap();

        let instruction_set = vec![
            // if case
            RibIR::PushLit(TypeAnnotatedValue::Str("if-pred1".to_string())),
            RibIR::JumpIfFalse(InstructionId { index: 1 }), // jumps to the next label having Id 1 (which is else block)
            RibIR::PushLit(TypeAnnotatedValue::Str("then1".to_string())),
            RibIR::Jump(InstructionId { index: 2 }), // Once if is executed then jump to the end of the else block with id 3
            RibIR::Label(InstructionId { index: 1 }),
            RibIR::PushLit(TypeAnnotatedValue::Str("else-pred2".to_string())),
            RibIR::JumpIfFalse(InstructionId { index: 3 }), // jumps to the next label having Id 2 (which is else block)
            RibIR::PushLit(TypeAnnotatedValue::Str("else-then2".to_string())),
            RibIR::Jump(InstructionId { index: 4 }), // Once if is executed then jump to the end of the else block with id 3
            RibIR::Label(InstructionId { index: 3 }),
            RibIR::PushLit(TypeAnnotatedValue::Str("else-else2".to_string())),
            RibIR::Label(InstructionId { index: 4 }),
            RibIR::Label(InstructionId { index: 2 }),
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_for_select_field() {
        let record = Expr::Record(
            vec![
                (
                    "foo_key".to_string(),
                    Box::new(Expr::Literal("foo_value".to_string(), InferredType::Str)),
                ),
                (
                    "bar_key".to_string(),
                    Box::new(Expr::Literal("bar_value".to_string(), InferredType::Str)),
                ),
            ],
            InferredType::Record(vec![
                (String::from("foo_key"), InferredType::Str),
                (String::from("bar_key"), InferredType::Str),
            ]),
        );

        let select_field =
            Expr::SelectField(Box::new(record), "bar_key".to_string(), InferredType::Str);

        let instructions = RibByteCode::from_expr(select_field).unwrap();

        let bar_value = TypeAnnotatedValue::Str("bar_value".to_string());
        let foo_value = TypeAnnotatedValue::Str("foo_value".to_string());

        let instruction_set = vec![
            RibIR::PushLit(bar_value),
            RibIR::PushLit(foo_value),
            RibIR::CreateAndPushRecord(AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "foo_key".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    NameTypePair {
                        name: "bar_key".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
            })),
            RibIR::UpdateRecord("foo_key".to_string()), // next pop is foo_value
            RibIR::UpdateRecord("bar_key".to_string()), // last pop is bar_value
            RibIR::SelectField("bar_key".to_string()),
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_for_select_index() {
        let sequence = Expr::Sequence(
            vec![
                Expr::Literal("foo".to_string(), InferredType::Str),
                Expr::Literal("bar".to_string(), InferredType::Str),
            ],
            InferredType::Str,
        );

        let select_index = Expr::SelectIndex(Box::new(sequence), 1, InferredType::Str);

        let instructions = RibByteCode::from_expr(select_index).unwrap();

        let instruction_set = vec![
            RibIR::PushLit(TypeAnnotatedValue::Str("bar".to_string())),
            RibIR::PushLit(TypeAnnotatedValue::Str("foo".to_string())),
            RibIR::PushList(AnalysedType::Str(TypeStr), 2),
            RibIR::SelectIndex(1),
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_for_expr_arm_pattern_match() {
        let expr = Expr::PatternMatch(
            Box::new(Expr::Literal("pred".to_string(), InferredType::Str)),
            vec![
                MatchArm::new(
                    ArmPattern::Literal(Box::new(Expr::Literal(
                        "arm1_pattern_expr".to_string(),
                        InferredType::Str,
                    ))),
                    Expr::Literal("arm1_resolution_expr".to_string(), InferredType::Str),
                ),
                MatchArm::new(
                    ArmPattern::Literal(Box::new(Expr::Literal(
                        "arm2_pattern_expr".to_string(),
                        InferredType::Str,
                    ))),
                    Expr::Literal("arm2_resolution_expr".to_string(), InferredType::Str),
                ),
                MatchArm::new(
                    ArmPattern::Literal(Box::new(Expr::Literal(
                        "arm3_pattern_expr".to_string(),
                        InferredType::Str,
                    ))),
                    Expr::Literal("arm3_resolution_expr".to_string(), InferredType::Str),
                ),
            ],
            InferredType::Str,
        );

        let instructions = RibByteCode::from_expr(expr).unwrap();

        // instructions will correspond to an if-else statement
        let instruction_set = vec![
            RibIR::PushLit(TypeAnnotatedValue::Str("arm1_pattern_expr".to_string())),
            RibIR::PushLit(TypeAnnotatedValue::Str("pred".to_string())),
            RibIR::EqualTo,
            RibIR::JumpIfFalse(InstructionId { index: 1 }),
            RibIR::PushLit(TypeAnnotatedValue::Str("arm1_resolution_expr".to_string())),
            RibIR::Jump(InstructionId { index: 2 }),
            RibIR::Label(InstructionId { index: 1 }),
            RibIR::PushLit(TypeAnnotatedValue::Str("arm2_pattern_expr".to_string())),
            RibIR::PushLit(TypeAnnotatedValue::Str("pred".to_string())),
            RibIR::EqualTo,
            RibIR::JumpIfFalse(InstructionId { index: 3 }),
            RibIR::PushLit(TypeAnnotatedValue::Str("arm2_resolution_expr".to_string())),
            RibIR::Jump(InstructionId { index: 4 }),
            RibIR::Label(InstructionId { index: 3 }),
            RibIR::PushLit(TypeAnnotatedValue::Str("arm3_pattern_expr".to_string())),
            RibIR::PushLit(TypeAnnotatedValue::Str("pred".to_string())),
            RibIR::EqualTo,
            RibIR::JumpIfFalse(InstructionId { index: 5 }),
            RibIR::PushLit(TypeAnnotatedValue::Str("arm3_resolution_expr".to_string())),
            RibIR::Jump(InstructionId { index: 6 }),
            RibIR::Label(InstructionId { index: 5 }),
            RibIR::Throw("No match found".to_string()),
            RibIR::Label(InstructionId { index: 6 }),
            RibIR::Label(InstructionId { index: 4 }),
            RibIR::Label(InstructionId { index: 2 }),
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[cfg(test)]
    mod global_input_tests {
        use crate::compiler::byte_code::compiler_tests::internal;
        use crate::{compiler, Expr};
        use golem_wasm_ast::analysis::{
            AnalysedType, NameOptionTypePair, NameTypePair, TypeEnum, TypeList, TypeOption,
            TypeRecord, TypeResult, TypeStr, TypeTuple, TypeU32, TypeU64, TypeVariant,
        };

        #[tokio::test]
        async fn test_variant_type_info() {
            let request_value_type = AnalysedType::Variant(TypeVariant {
                cases: vec![
                    NameOptionTypePair {
                        name: "register-user".to_string(),
                        typ: Some(AnalysedType::U64(TypeU64)),
                    },
                    NameOptionTypePair {
                        name: "process-user".to_string(),
                        typ: Some(AnalysedType::Str(TypeStr)),
                    },
                    NameOptionTypePair {
                        name: "validate".to_string(),
                        typ: None,
                    },
                ],
            });

            let output_analysed_type = AnalysedType::Str(TypeStr);

            let analysed_exports = internal::get_component_metadata(
                "my-worker-function",
                vec![request_value_type.clone()],
                output_analysed_type,
            );

            // x = request, implies we are expecting a global variable
            // called request as the  input to Rib.
            // my-worker-function is a function that takes a Variant as input,
            // implies the type of request is a Variant.
            // This means the rib interpreter env has to have a request variable in it,
            // with a value that should be of the type Variant
            let expr = r#"
               my-worker-function(request);
               match request {
                 process-user(user) => user,
                 _ => "default"
               }
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
            let expected_type_info =
                internal::rib_input_type_info(vec![("request", request_value_type)]);

            assert_eq!(compiled.global_input_type_info, expected_type_info);
        }

        #[tokio::test]
        async fn test_result_type_info() {
            let request_value_type = AnalysedType::Result(TypeResult {
                ok: Some(Box::new(AnalysedType::U64(TypeU64))),
                err: Some(Box::new(AnalysedType::Str(TypeStr))),
            });

            let output_analysed_type = AnalysedType::Str(TypeStr);

            let analysed_exports = internal::get_component_metadata(
                "my-worker-function",
                vec![request_value_type.clone()],
                output_analysed_type,
            );

            // x = request, implies we are expecting a global variable
            // called request as the  input to Rib.
            // my-worker-function is a function that takes a Result as input,
            // implies the type of request is a Result.
            // This means the rib interpreter env has to have a request variable in it,
            // with a value that should be of the type Result
            let expr = r#"
               my-worker-function(request);
               match request {
                 ok(x) => "${x}",
                 err(msg) => msg
               }
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
            let expected_type_info =
                internal::rib_input_type_info(vec![("request", request_value_type)]);

            assert_eq!(compiled.global_input_type_info, expected_type_info);
        }

        #[tokio::test]
        async fn test_option_type_info() {
            let request_value_type = AnalysedType::Option(TypeOption {
                inner: Box::new(AnalysedType::Str(TypeStr)),
            });

            let output_analysed_type = AnalysedType::Str(TypeStr);

            let analysed_exports = internal::get_component_metadata(
                "my-worker-function",
                vec![request_value_type.clone()],
                output_analysed_type,
            );

            // x = request, implies we are expecting a global variable
            // called request as the input to Rib.
            // my-worker-function is a function that takes a Option as input,
            // implies the type of request is a Result.
            // This means the rib interpreter env has to have a request variable in it,
            // with a value that should be of the type Option
            let expr = r#"
               my-worker-function(request);
               match request {
                 some(x) => x,
                 none => "error"
               }
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
            let expected_type_info =
                internal::rib_input_type_info(vec![("request", request_value_type)]);

            assert_eq!(compiled.global_input_type_info, expected_type_info);
        }

        #[tokio::test]
        async fn test_enum_type_info() {
            let request_value_type = AnalysedType::Enum(TypeEnum {
                cases: vec!["prod".to_string(), "dev".to_string(), "test".to_string()],
            });

            let output_analysed_type = AnalysedType::Str(TypeStr);

            let analysed_exports = internal::get_component_metadata(
                "my-worker-function",
                vec![request_value_type.clone()],
                output_analysed_type,
            );

            // x = request, implies we are expecting a global variable
            // called request as the input to Rib.
            // my-worker-function is a function that takes a Option as input,
            // implies the type of request is a Result.
            // This means the rib interpreter env has to have a request variable in it,
            // with a value that should be of the type Option
            let expr = r#"
               my-worker-function(request);
               match request {
                 prod  => "p",
                 dev => "d",
                 test => "t"
               }
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
            let expected_type_info =
                internal::rib_input_type_info(vec![("request", request_value_type)]);

            assert_eq!(compiled.global_input_type_info, expected_type_info);
        }

        #[tokio::test]
        async fn test_record_global_input() {
            let request_value_type = AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "path".to_string(),
                    typ: AnalysedType::Record(TypeRecord {
                        fields: vec![NameTypePair {
                            name: "user".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        }],
                    }),
                }],
            });

            let output_analysed_type = AnalysedType::Str(TypeStr);

            let analysed_exports = internal::get_component_metadata(
                "my-worker-function",
                vec![request_value_type.clone()],
                output_analysed_type,
            );

            // x = request, implies we are expecting a global variable
            // called request as the  input to Rib.
            // my-worker-function is a function that takes a Record of path -> user -> str as input
            // implies the type of request is a Record.
            // This means the rib interpreter env has to have a request variable in it,
            // with a value that should be of the type Record
            let expr = r#"
               let x = request;
               my-worker-function(x);

               let name = x.path.user;

               match x {
                 { path : { user : some_name } } => some_name,
                 _ => name
               }
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
            let expected_type_info =
                internal::rib_input_type_info(vec![("request", request_value_type)]);

            assert_eq!(compiled.global_input_type_info, expected_type_info);
        }

        #[tokio::test]
        async fn test_tuple_global_input() {
            let request_value_type = AnalysedType::Tuple(TypeTuple {
                items: vec![
                    AnalysedType::Str(TypeStr),
                    AnalysedType::U32(TypeU32),
                    AnalysedType::Record(TypeRecord {
                        fields: vec![NameTypePair {
                            name: "user".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        }],
                    }),
                ],
            });

            let output_analysed_type = AnalysedType::Str(TypeStr);

            let analysed_exports = internal::get_component_metadata(
                "my-worker-function",
                vec![request_value_type.clone()],
                output_analysed_type,
            );

            // x = request, implies we are expecting a global variable
            // called request as the  input to Rib.
            // my-worker-function is a function that takes a Tuple,
            // implies the type of request is a Tuple.
            let expr = r#"
               let x = request;
               my-worker-function(x);
               match x {
                (_, _, record) =>  record.user,
                 _ => "fallback"
               }
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
            let expected_type_info =
                internal::rib_input_type_info(vec![("request", request_value_type)]);

            assert_eq!(compiled.global_input_type_info, expected_type_info);
        }

        #[tokio::test]
        async fn test_list_global_input() {
            let request_value_type = AnalysedType::List(TypeList {
                inner: Box::new(AnalysedType::Str(TypeStr)),
            });

            let output_analysed_type = AnalysedType::Str(TypeStr);

            let analysed_exports = internal::get_component_metadata(
                "my-worker-function",
                vec![request_value_type.clone()],
                output_analysed_type,
            );

            // x = request, implies we are expecting a global variable
            // called request as the  input to Rib.
            // my-worker-function is a function that takes a List,
            // implies the type of request should be a List
            let expr = r#"
               let x = request;
               my-worker-function(x);
               match x {
               [a, b, c]  => a,
                 _ => "fallback"
               }
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
            let expected_type_info =
                internal::rib_input_type_info(vec![("request", request_value_type)]);

            assert_eq!(compiled.global_input_type_info, expected_type_info);
        }
    }

    #[tokio::test]
    async fn test_str_global_input() {
        let request_value_type = AnalysedType::Str(TypeStr);

        let output_analysed_type = AnalysedType::Str(TypeStr);

        let analysed_exports = internal::get_component_metadata(
            "my-worker-function",
            vec![request_value_type.clone()],
            output_analysed_type,
        );

        let expr = r#"
               let x = request;
               my-worker-function(x);
               match x {
                "foo"  => "success",
                 _ => "fallback"
               }
            "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
        let expected_type_info =
            internal::rib_input_type_info(vec![("request", request_value_type)]);

        assert_eq!(compiled.global_input_type_info, expected_type_info);
    }

    #[tokio::test]
    async fn test_number_global_input() {
        let request_value_type = AnalysedType::U32(TypeU32);

        let output_analysed_type = AnalysedType::Str(TypeStr);

        let analysed_exports = internal::get_component_metadata(
            "my-worker-function",
            vec![request_value_type.clone()],
            output_analysed_type,
        );

        let expr = r#"
               let x = request;
               my-worker-function(x);
               match x {
                1  => "success",
                0 => "failure"
               }
            "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
        let expected_type_info =
            internal::rib_input_type_info(vec![("request", request_value_type)]);

        assert_eq!(compiled.global_input_type_info, expected_type_info);
    }

    mod internal {
        use crate::RibInputTypeInfo;
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            AnalysedType,
        };
        use std::collections::HashMap;

        pub(crate) fn get_component_metadata(
            function_name: &str,
            input_types: Vec<AnalysedType>,
            output: AnalysedType,
        ) -> Vec<AnalysedExport> {
            let analysed_function_parameters = input_types
                .into_iter()
                .enumerate()
                .map(|(index, typ)| AnalysedFunctionParameter {
                    name: format!("param{}", index),
                    typ,
                })
                .collect();

            vec![AnalysedExport::Function(AnalysedFunction {
                name: function_name.to_string(),
                parameters: analysed_function_parameters,
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: output,
                }],
            })]
        }

        pub(crate) fn rib_input_type_info(types: Vec<(&str, AnalysedType)>) -> RibInputTypeInfo {
            let mut type_info = HashMap::new();
            for (name, typ) in types {
                type_info.insert(name.to_string(), typ);
            }
            RibInputTypeInfo { types: type_info }
        }
    }
}
