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

use crate::compiler::byte_code::internal::ExprState;
use crate::compiler::ir::RibIR;
use crate::{Expr, InferredExpr, InstructionId};
use bincode::{Decode, Encode};

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct RibByteCode {
    pub instructions: Vec<RibIR>,
}

impl RibByteCode {
    // Convert expression to bytecode instructions
    pub fn from_expr(inferred_expr: &InferredExpr) -> Result<RibByteCode, String> {
        let expr: &Expr = inferred_expr.get_expr();
        let mut instructions = Vec::new();
        let mut stack: Vec<ExprState> = Vec::new();
        let mut instruction_id = InstructionId::init();
        stack.push(ExprState::from_expr(expr));

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

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::RibByteCode;
    use golem_api_grpc::proto::golem::rib::RibByteCode as ProtoRibByteCode;

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

    impl TryFrom<RibByteCode> for ProtoRibByteCode {
        type Error = String;

        fn try_from(value: RibByteCode) -> Result<Self, Self::Error> {
            let mut instructions = Vec::new();
            for instruction in value.instructions {
                instructions.push(instruction.try_into()?);
            }

            Ok(ProtoRibByteCode { instructions })
        }
    }
}

mod internal {
    use crate::compiler::desugar::desugar_pattern_match;
    use crate::{
        AnalysedTypeWithUnit, DynamicParsedFunctionReference, Expr, FunctionReferenceType,
        InferredType, InstructionId, Range, RibIR, VariableId, WorkerNamePresence,
    };
    use golem_wasm_ast::analysis::{AnalysedType, TypeFlags, TypeRecord};
    use std::collections::HashSet;

    use crate::call_type::{CallType, InstanceCreationType};
    use golem_wasm_ast::analysis::analysed_type::{bool, field, option, record, u64};
    use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
    use std::ops::Deref;

    pub(crate) fn process_expr(
        expr: &Expr,
        stack: &mut Vec<ExprState>,
        instructions: &mut Vec<RibIR>,
        instruction_id: &mut InstructionId,
    ) -> Result<(), String> {
        match expr {
            Expr::Unwrap { expr, .. } => {
                stack.push(ExprState::from_expr(expr.deref()));
                instructions.push(RibIR::Deconstruct);
            }
            Expr::Throw { message, .. } => {
                instructions.push(RibIR::Throw(message.to_string()));
            }
            Expr::Identifier { variable_id, .. } => {
                instructions.push(RibIR::LoadVar(variable_id.clone()));
            }
            Expr::Literal { value, .. } => {
                let value_and_type = value.clone().into_value_and_type();
                instructions.push(RibIR::PushLit(value_and_type));
            }
            Expr::Number {
                number,
                inferred_type,
                ..
            } => {
                let analysed_type = convert_to_analysed_type(expr, inferred_type)?;

                let value_and_type = number.to_val(&analysed_type).ok_or(format!(
                    "Internal error: convert a number to wasm value using {:?}",
                    analysed_type
                ))?;

                instructions.push(RibIR::PushLit(value_and_type));
            }
            Expr::EqualTo { lhs, rhs, .. } => {
                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::EqualTo);
            }
            Expr::GreaterThan { lhs, rhs, .. } => {
                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::GreaterThan);
            }
            Expr::LessThan { lhs, rhs, .. } => {
                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::LessThan);
            }
            Expr::GreaterThanOrEqualTo { lhs, rhs, .. } => {
                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::GreaterThanOrEqualTo);
            }
            Expr::LessThanOrEqualTo { lhs, rhs, .. } => {
                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::LessThanOrEqualTo);
            }
            Expr::Plus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                let analysed_type = convert_to_analysed_type(expr, inferred_type)?;
                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::Plus(analysed_type));
            }
            Expr::Minus {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                let analysed_type = convert_to_analysed_type(expr, inferred_type)?;

                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::Minus(analysed_type));
            }
            Expr::Divide {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                let analysed_type = convert_to_analysed_type(expr, inferred_type)?;

                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::Divide(analysed_type));
            }
            Expr::Multiply {
                lhs,
                rhs,
                inferred_type,
                ..
            } => {
                let analysed_type = convert_to_analysed_type(expr, inferred_type)?;

                stack.push(ExprState::from_expr(rhs.deref()));
                stack.push(ExprState::from_expr(lhs.deref()));
                instructions.push(RibIR::Multiply(analysed_type));
            }
            Expr::And { lhs, rhs, .. } => {
                // This optimization isn't optional, it's required for the correct functioning of the interpreter
                let optimised_expr = Expr::cond(
                    Expr::equal_to(lhs.deref().clone(), Expr::boolean(true)),
                    Expr::equal_to(rhs.deref().clone(), Expr::boolean(true)),
                    Expr::boolean(false),
                );

                stack.push(ExprState::from_expr(&optimised_expr));
            }

            Expr::Or { lhs, rhs, .. } => {
                let optimised_expr = Expr::cond(
                    Expr::equal_to(lhs.deref().clone(), Expr::boolean(true)),
                    Expr::boolean(true),
                    Expr::equal_to(rhs.deref().clone(), Expr::boolean(true)),
                );

                stack.push(ExprState::from_expr(&optimised_expr));
            }

            Expr::Record {
                exprs,
                inferred_type,
                ..
            } => {
                // Push field instructions in reverse order
                for (field_name, field_expr) in exprs.iter().rev() {
                    stack.push(ExprState::from_expr(field_expr.as_ref()));
                    instructions.push(RibIR::UpdateRecord(field_name.clone()));
                }
                // Push record creation instruction
                let analysed_type = convert_to_analysed_type(expr, inferred_type);
                instructions.push(RibIR::CreateAndPushRecord(analysed_type?));
            }
            Expr::Sequence {
                exprs,
                inferred_type,
                ..
            } => {
                // Push all expressions in reverse order
                for expr in exprs.iter().rev() {
                    stack.push(ExprState::from_expr(expr));
                }

                let analysed_type = convert_to_analysed_type(expr, inferred_type)?;
                instructions.push(RibIR::PushList(analysed_type, exprs.len()));
            }
            Expr::ExprBlock { exprs, .. } => {
                // Push all expressions in reverse order
                for expr in exprs.iter() {
                    stack.push(ExprState::from_expr(expr));
                }
            }
            Expr::Let {
                variable_id, expr, ..
            } => {
                stack.push(ExprState::from_expr(expr.deref()));
                instructions.push(RibIR::AssignVar(variable_id.clone()));
            }
            Expr::PatternMatch {
                predicate,
                match_arms,
                inferred_type,
                ..
            } => {
                let desugared_pattern_match =
                    desugar_pattern_match(predicate.deref(), match_arms, inferred_type.clone())
                        .ok_or("Desugar pattern match failed".to_string())?;
                stack.push(ExprState::from_expr(&desugared_pattern_match));
            }
            Expr::Cond { cond, lhs, rhs, .. } => {
                handle_if_condition(
                    instruction_id,
                    cond.deref(),
                    lhs.deref(),
                    rhs.deref(),
                    stack,
                );
            }

            Expr::SelectField { expr, field, .. } => {
                stack.push(ExprState::from_expr(expr.deref()));
                instructions.push(RibIR::SelectField(field.clone()));
            }

            Expr::SelectDynamic {expr, index, ..} => {
                stack.push(ExprState::from_expr(index.deref()));
                stack.push(ExprState::from_expr(expr.deref()));
                instructions.push(RibIR::SelectDynamic);
            }

            Expr::SelectIndex { expr, index, .. } => {
                stack.push(ExprState::from_expr(expr.deref()));
                instructions.push(RibIR::SelectIndex(*index));
            }
            Expr::Option {
                expr: Some(inner_expr),
                inferred_type,
                ..
            } => {
                stack.push(ExprState::from_expr(inner_expr.deref()));
                instructions.push(RibIR::PushSome(convert_to_analysed_type(
                    expr,
                    inferred_type,
                )?));
            }

            Expr::Option { inferred_type, .. } => {
                let optional = convert_to_analysed_type(expr, inferred_type);
                instructions.push(RibIR::PushNone(optional.ok()));
            }

            Expr::Result {
                expr: Ok(inner_expr),
                inferred_type,
                ..
            } => {
                stack.push(ExprState::from_expr(inner_expr.deref()));
                instructions.push(RibIR::PushOkResult(convert_to_analysed_type(
                    expr,
                    inferred_type,
                )?));
            }

            Expr::Result {
                expr: Err(inner_expr),
                inferred_type,
                ..
            } => {
                stack.push(ExprState::from_expr(inner_expr.deref()));
                instructions.push(RibIR::PushErrResult(convert_to_analysed_type(
                    expr,
                    inferred_type,
                )?));
            }

            Expr::Call {
                call_type,
                args,
                inferred_type,
                ..
            } => {
                for expr in args.iter().rev() {
                    stack.push(ExprState::from_expr(expr));
                }

                match call_type {
                    CallType::Function {
                        function_name,
                        worker,
                    } => {
                        let function_result_type = if inferred_type.is_unit() {
                            AnalysedTypeWithUnit::Unit
                        } else {
                            AnalysedTypeWithUnit::Type(convert_to_analysed_type(
                                expr,
                                inferred_type,
                            )?)
                        };

                        // To be pushed to interpreter stack later
                        let worker_name = match worker {
                            Some(_) => WorkerNamePresence::Absent,
                            None => WorkerNamePresence::Present,
                        };

                        instructions.push(RibIR::InvokeFunction(
                            worker_name,
                            args.len(),
                            function_result_type,
                        ));

                        if let Some(worker_expr) = worker {
                            stack.push(ExprState::from_expr(worker_expr));
                        }

                        let site = function_name.site.clone();

                        // Resolve the function name and update stack
                        match &function_name.function {
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

                    // There is nothing to do as such for instance creation
                    CallType::InstanceCreation(instance_creation) => {
                        instructions.push(RibIR::PushLit(match instance_creation {
                            InstanceCreationType::Worker { .. } => {
                                "create_worker".into_value_and_type()
                            }
                            InstanceCreationType::Resource { .. } => {
                                "create_resource".into_value_and_type()
                            }
                        }));
                    }

                    CallType::VariantConstructor(variant_name) => {
                        instructions.push(RibIR::PushVariant(
                            variant_name.clone(),
                            convert_to_analysed_type(expr, inferred_type)?,
                        ));
                    }
                    CallType::EnumConstructor(enum_name) => {
                        instructions.push(RibIR::PushEnum(
                            enum_name.clone(),
                            convert_to_analysed_type(expr, inferred_type)?,
                        ));
                    }
                }
            }

            Expr::Flags {
                flags,
                inferred_type,
                ..
            } => match inferred_type {
                InferredType::Flags(all_flags) => {
                    let mut bitmap = Vec::new();
                    let flag_values_set: HashSet<&String> = HashSet::from_iter(flags.iter());
                    for flag in all_flags.iter() {
                        bitmap.push(flag_values_set.contains(flag));
                    }
                    instructions.push(RibIR::PushFlag(ValueAndType {
                        value: Value::Flags(bitmap),
                        typ: AnalysedType::Flags(TypeFlags {
                            names: all_flags.iter().map(|n| n.to_string()).collect(),
                        }),
                    }));
                }
                inferred_type => {
                    return Err(format!(
                        "Flags should have inferred type Flags {:?}",
                        inferred_type
                    ));
                }
            },
            Expr::Boolean { value, .. } => {
                instructions.push(RibIR::PushLit(value.into_value_and_type()));
            }
            Expr::GetTag { expr, .. } => {
                stack.push(ExprState::from_expr(expr.deref()));
                stack.push(ExprState::from_ir(RibIR::GetTag));
            }

            Expr::Concat { exprs, .. } => {
                for expr in exprs.iter().rev() {
                    stack.push(ExprState::from_expr(expr));
                }
                instructions.push(RibIR::Concat(exprs.len()));
            }

            Expr::Not { expr, .. } => {
                stack.push(ExprState::from_expr(expr.deref()));
                instructions.push(RibIR::Negate);
            }

            Expr::Tuple {
                exprs,
                inferred_type,
                ..
            } => {
                for expr in exprs.iter().rev() {
                    stack.push(ExprState::from_expr(expr));
                }
                let analysed_type = convert_to_analysed_type(expr, inferred_type)?;
                instructions.push(RibIR::PushTuple(analysed_type, exprs.len()));
            }

            Expr::ListComprehension {
                iterated_variable,
                iterable_expr,
                yield_expr,
                inferred_type,
                ..
            } => {
                let analysed_type = convert_to_analysed_type(expr, inferred_type)?;
                handle_list_comprehension(
                    instruction_id,
                    stack,
                    iterable_expr,
                    yield_expr,
                    iterated_variable,
                    &analysed_type,
                )
            }

            range_expr @ Expr::Range {
                range,
                inferred_type,
                ..
            } => match inferred_type {
                InferredType::Range { .. } => {
                    let analysed_type = convert_to_analysed_type(range_expr, inferred_type)?;

                    handle_range(range, stack, analysed_type, instructions);
                }

                _ => {
                    return Err(format!(
                        "Range should have inferred type Range {:?}",
                        inferred_type
                    ));
                }
            },

            // Invoke is always handled by the CallType::Function branch
            Expr::InvokeMethodLazy { .. } => {}

            Expr::ListReduce {
                reduce_variable,
                iterated_variable,
                iterable_expr,
                init_value_expr,
                yield_expr,
                ..
            } => handle_list_reduce(
                instruction_id,
                stack,
                reduce_variable,
                iterated_variable,
                iterable_expr,
                init_value_expr,
                yield_expr,
            ),
        }

        Ok(())
    }

    pub(crate) fn convert_to_analysed_type(
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
    // A stack is required on one side to maintain the order of expressions
    // As soon a `Expr` becomes `Instruction` the instruction stack will be in order.
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

    fn handle_range(
        range: &Range,
        stack: &mut Vec<ExprState>,
        analysed_type: AnalysedType,
        instructions: &mut Vec<RibIR>,
    ) {
        let from = range.from();
        let to = range.to();
        let inclusive = range.inclusive();

        if let Some(from) = from {
            stack.push(ExprState::from_expr(from));
            instructions.push(RibIR::UpdateRecord("from".to_string()));
        }

        if let Some(to) = to {
            stack.push(ExprState::from_expr(to));
            instructions.push(RibIR::UpdateRecord("to".to_string()));
        }

        stack.push(ExprState::from_ir(RibIR::PushLit(ValueAndType::new(
            Value::Bool(inclusive),
            bool(),
        ))));

        instructions.push(RibIR::UpdateRecord("inclusive".to_string()));

        instructions.push(RibIR::CreateAndPushRecord(analysed_type));
    }

    fn handle_list_comprehension(
        instruction_id: &mut InstructionId,
        stack: &mut Vec<ExprState>,
        iterable_expr: &Expr,
        yield_expr: &Expr,
        variable_id: &VariableId,
        sink_type: &AnalysedType,
    ) {
        stack.push(ExprState::from_expr(iterable_expr));

        stack.push(ExprState::from_ir(RibIR::ToIterator));

        stack.push(ExprState::from_ir(RibIR::CreateSink(sink_type.clone())));

        let loop_start_label = instruction_id.increment_mut();

        stack.push(ExprState::from_ir(RibIR::Label(loop_start_label.clone())));

        let exit_label = instruction_id.increment_mut();

        stack.push(ExprState::from_ir(RibIR::IsEmpty));

        stack.push(ExprState::from_ir(RibIR::JumpIfFalse(exit_label.clone())));

        stack.push(ExprState::from_ir(RibIR::AdvanceIterator));

        stack.push(ExprState::from_ir(RibIR::AssignVar(variable_id.clone())));

        stack.push(ExprState::from_expr(yield_expr));

        stack.push(ExprState::from_ir(RibIR::PushToSink));

        stack.push(ExprState::from_ir(RibIR::Jump(loop_start_label)));

        stack.push(ExprState::from_ir(RibIR::Label(exit_label)));

        stack.push(ExprState::from_ir(RibIR::SinkToList))
    }

    fn handle_list_reduce(
        instruction_id: &mut InstructionId,
        stack: &mut Vec<ExprState>,
        reduce_variable: &VariableId,
        iterated_variable: &VariableId,
        iterable_expr: &Expr,
        initial_value_expr: &Expr,
        yield_expr: &Expr,
    ) {
        stack.push(ExprState::from_expr(iterable_expr));

        stack.push(ExprState::from_expr(initial_value_expr));

        stack.push(ExprState::from_ir(RibIR::AssignVar(
            reduce_variable.clone(),
        )));

        stack.push(ExprState::from_ir(RibIR::ToIterator));

        let loop_start_label = instruction_id.increment_mut();

        stack.push(ExprState::from_ir(RibIR::Label(loop_start_label.clone())));

        let exit_label = instruction_id.increment_mut();

        stack.push(ExprState::from_ir(RibIR::IsEmpty));

        stack.push(ExprState::from_ir(RibIR::JumpIfFalse(exit_label.clone())));

        stack.push(ExprState::from_ir(RibIR::AdvanceIterator));

        stack.push(ExprState::from_ir(RibIR::AssignVar(
            iterated_variable.clone(),
        )));

        stack.push(ExprState::from_expr(yield_expr));

        stack.push(ExprState::from_ir(RibIR::AssignVar(
            reduce_variable.clone(),
        )));

        stack.push(ExprState::from_ir(RibIR::Jump(loop_start_label)));

        stack.push(ExprState::from_ir(RibIR::Label(exit_label)));

        stack.push(ExprState::from_ir(RibIR::LoadVar(reduce_variable.clone())))
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
    use bigdecimal::BigDecimal;
    use test_r::test;

    use super::*;
    use crate::{ArmPattern, FunctionTypeRegistry, InferredType, MatchArm, VariableId};
    use golem_wasm_ast::analysis::analysed_type::{list, str};
    use golem_wasm_ast::analysis::{AnalysedType, NameTypePair, TypeRecord, TypeStr};
    use golem_wasm_rpc::IntoValueAndType;

    #[test]
    fn test_instructions_for_literal() {
        let literal = Expr::literal("hello");
        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&literal, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let instruction_set = vec![RibIR::PushLit("hello".into_value_and_type())];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_for_identifier() {
        let inferred_input_type = InferredType::Str;
        let variable_id = VariableId::local("request", 0);
        let empty_registry = FunctionTypeRegistry::empty();
        let expr = Expr::identifier_with_variable_id(variable_id.clone(), None)
            .with_inferred_type(inferred_input_type);
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let instruction_set = vec![RibIR::LoadVar(variable_id)];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_assign_variable() {
        let literal = Expr::literal("hello");

        let variable_id = VariableId::local("request", 0);

        let expr = Expr::let_binding_with_variable_id(variable_id.clone(), literal, None);

        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let instruction_set = vec![
            RibIR::PushLit("hello".into_value_and_type()),
            RibIR::AssignVar(variable_id),
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_equal_to() {
        let number_f32 = Expr::number(BigDecimal::from(1), None, InferredType::F32);
        let number_u32 = Expr::number(BigDecimal::from(1), None, InferredType::U32);

        let expr = Expr::equal_to(number_f32, number_u32);
        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let value_and_type1 = 1.0f32.into_value_and_type();
        let value_and_type2 = 1u32.into_value_and_type();

        let instruction_set = vec![
            RibIR::PushLit(value_and_type2),
            RibIR::PushLit(value_and_type1),
            RibIR::EqualTo,
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_greater_than() {
        let number_f32 = Expr::number(BigDecimal::from(1), None, InferredType::F32);
        let number_u32 = Expr::number(BigDecimal::from(2), None, InferredType::U32);

        let expr = Expr::greater_than(number_f32, number_u32);
        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let value_and_type1 = 1.0f32.into_value_and_type();
        let value_and_type2 = 2u32.into_value_and_type();

        let instruction_set = vec![
            RibIR::PushLit(value_and_type2),
            RibIR::PushLit(value_and_type1),
            RibIR::GreaterThan,
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_less_than() {
        let number_f32 = Expr::number(BigDecimal::from(1), None, InferredType::F32);
        let number_u32 = Expr::number(BigDecimal::from(1), None, InferredType::U32);

        let expr = Expr::less_than(number_f32, number_u32);
        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let value_and_type1 = 1.0f32.into_value_and_type();
        let value_and_type2 = 1u32.into_value_and_type();

        let instruction_set = vec![
            RibIR::PushLit(value_and_type2),
            RibIR::PushLit(value_and_type1),
            RibIR::LessThan,
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_greater_than_or_equal_to() {
        let number_f32 = Expr::number(BigDecimal::from(1), None, InferredType::F32);
        let number_u32 = Expr::number(BigDecimal::from(1), None, InferredType::U32);

        let expr = Expr::greater_than_or_equal_to(number_f32, number_u32);
        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let value_and_type1 = 1.0f32.into_value_and_type();
        let value_and_type2 = 1u32.into_value_and_type();

        let instruction_set = vec![
            RibIR::PushLit(value_and_type2),
            RibIR::PushLit(value_and_type1),
            RibIR::GreaterThanOrEqualTo,
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_less_than_or_equal_to() {
        let number_f32 = Expr::number(BigDecimal::from(1), None, InferredType::F32);
        let number_u32 = Expr::number(BigDecimal::from(1), None, InferredType::U32);

        let expr = Expr::less_than_or_equal_to(number_f32, number_u32);
        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let value_and_type1 = 1.0f32.into_value_and_type();
        let value_and_type2 = 1u32.into_value_and_type();

        let instruction_set = vec![
            RibIR::PushLit(value_and_type2),
            RibIR::PushLit(value_and_type1),
            RibIR::LessThanOrEqualTo,
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_for_record() {
        let expr = Expr::record(vec![
            ("foo_key".to_string(), Expr::literal("foo_value")),
            ("bar_key".to_string(), Expr::literal("bar_value")),
        ])
        .with_inferred_type(InferredType::Record(vec![
            (String::from("foo_key"), InferredType::Str),
            (String::from("bar_key"), InferredType::Str),
        ]));

        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let bar_value = "bar_value".into_value_and_type();
        let foo_value = "foo_value".into_value_and_type();

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
        let expr = Expr::expr_block(vec![Expr::literal("foo"), Expr::literal("bar")]);

        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let instruction_set = vec![
            RibIR::PushLit("foo".into_value_and_type()),
            RibIR::PushLit("bar".into_value_and_type()),
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_if_conditional() {
        let if_expr = Expr::literal("pred").with_inferred_type(InferredType::Bool);
        let then_expr = Expr::literal("then");
        let else_expr = Expr::literal("else");

        let expr = Expr::cond(if_expr, then_expr, else_expr).with_inferred_type(InferredType::Str);

        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let instruction_set = vec![
            RibIR::PushLit("pred".into_value_and_type()),
            RibIR::JumpIfFalse(InstructionId { index: 1 }), // jumps to the next label having Id 1 (which is else block)
            RibIR::PushLit("then".into_value_and_type()),
            RibIR::Jump(InstructionId { index: 2 }), // Once if is executed then jump to the end of the else block with id 2
            RibIR::Label(InstructionId { index: 1 }),
            RibIR::PushLit("else".into_value_and_type()),
            RibIR::Label(InstructionId { index: 2 }),
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_for_nested_if_else() {
        let if_expr = Expr::literal("if-pred1").with_inferred_type(InferredType::Bool);
        let then_expr = Expr::literal("then1").with_inferred_type(InferredType::Str);
        let else_expr = Expr::cond(
            Expr::literal("else-pred2").with_inferred_type(InferredType::Bool),
            Expr::literal("else-then2"),
            Expr::literal("else-else2"),
        )
        .with_inferred_type(InferredType::Str);

        let expr = Expr::cond(if_expr, then_expr, else_expr).with_inferred_type(InferredType::Str);

        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let instruction_set = vec![
            // if case
            RibIR::PushLit("if-pred1".into_value_and_type()),
            RibIR::JumpIfFalse(InstructionId { index: 1 }), // jumps to the next label having Id 1 (which is else block)
            RibIR::PushLit("then1".into_value_and_type()),
            RibIR::Jump(InstructionId { index: 2 }), // Once if is executed then jump to the end of the else block with id 3
            RibIR::Label(InstructionId { index: 1 }),
            RibIR::PushLit("else-pred2".into_value_and_type()),
            RibIR::JumpIfFalse(InstructionId { index: 3 }), // jumps to the next label having Id 2 (which is else block)
            RibIR::PushLit("else-then2".into_value_and_type()),
            RibIR::Jump(InstructionId { index: 4 }), // Once if is executed then jump to the end of the else block with id 3
            RibIR::Label(InstructionId { index: 3 }),
            RibIR::PushLit("else-else2".into_value_and_type()),
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
        let record = Expr::record(vec![
            ("foo_key".to_string(), Expr::literal("foo_value")),
            ("bar_key".to_string(), Expr::literal("bar_value")),
        ])
        .with_inferred_type(InferredType::Record(vec![
            (String::from("foo_key"), InferredType::Str),
            (String::from("bar_key"), InferredType::Str),
        ]));

        let expr =
            Expr::select_field(record, "bar_key", None).with_inferred_type(InferredType::Str);

        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let bar_value = "bar_value".into_value_and_type();
        let foo_value = "foo_value".into_value_and_type();

        let instruction_set = vec![
            RibIR::PushLit(bar_value),
            RibIR::PushLit(foo_value),
            RibIR::CreateAndPushRecord(AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "bar_key".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    NameTypePair {
                        name: "foo_key".to_string(),
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
        let sequence = Expr::sequence(vec![Expr::literal("foo"), Expr::literal("bar")], None)
            .with_inferred_type(InferredType::List(Box::new(InferredType::Str)));

        let expr = Expr::select_index(sequence, 1).with_inferred_type(InferredType::Str);

        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        let instruction_set = vec![
            RibIR::PushLit("bar".into_value_and_type()),
            RibIR::PushLit("foo".into_value_and_type()),
            RibIR::PushList(list(str()), 2),
            RibIR::SelectIndex(1),
        ];

        let expected_instructions = RibByteCode {
            instructions: instruction_set,
        };

        assert_eq!(instructions, expected_instructions);
    }

    #[test]
    fn test_instructions_for_expr_arm_pattern_match() {
        let expr = Expr::pattern_match(
            Expr::literal("pred"),
            vec![
                MatchArm::new(
                    ArmPattern::Literal(Box::new(Expr::literal("arm1_pattern_expr"))),
                    Expr::literal("arm1_resolution_expr"),
                ),
                MatchArm::new(
                    ArmPattern::Literal(Box::new(Expr::literal("arm2_pattern_expr"))),
                    Expr::literal("arm2_resolution_expr"),
                ),
                MatchArm::new(
                    ArmPattern::Literal(Box::new(Expr::literal("arm3_pattern_expr"))),
                    Expr::literal("arm3_resolution_expr"),
                ),
            ],
        )
        .with_inferred_type(InferredType::Str);

        let empty_registry = FunctionTypeRegistry::empty();
        let inferred_expr = InferredExpr::from_expr(&expr, &empty_registry, &vec![]).unwrap();

        let instructions = RibByteCode::from_expr(&inferred_expr).unwrap();

        // instructions will correspond to an if-else statement
        let instruction_set = vec![
            RibIR::PushLit("arm1_pattern_expr".into_value_and_type()),
            RibIR::PushLit("pred".into_value_and_type()),
            RibIR::EqualTo,
            RibIR::JumpIfFalse(InstructionId { index: 1 }),
            RibIR::PushLit("arm1_resolution_expr".into_value_and_type()),
            RibIR::Jump(InstructionId { index: 2 }),
            RibIR::Label(InstructionId { index: 1 }),
            RibIR::PushLit("arm2_pattern_expr".into_value_and_type()),
            RibIR::PushLit("pred".into_value_and_type()),
            RibIR::EqualTo,
            RibIR::JumpIfFalse(InstructionId { index: 3 }),
            RibIR::PushLit("arm2_resolution_expr".into_value_and_type()),
            RibIR::Jump(InstructionId { index: 4 }),
            RibIR::Label(InstructionId { index: 3 }),
            RibIR::PushLit("arm3_pattern_expr".into_value_and_type()),
            RibIR::PushLit("pred".into_value_and_type()),
            RibIR::EqualTo,
            RibIR::JumpIfFalse(InstructionId { index: 5 }),
            RibIR::PushLit("arm3_resolution_expr".into_value_and_type()),
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
    mod invalid_function_invoke_tests {
        use test_r::test;

        use crate::compiler::byte_code::compiler_tests::internal;
        use crate::{compiler, Expr};
        use golem_wasm_ast::analysis::{AnalysedType, TypeStr};

        #[test]
        fn test_unknown_function() {
            let expr = r#"
               foo(request);
               "success"
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiler_error = compiler::compile(&expr, &vec![]).unwrap_err().to_string();

            assert_eq!(compiler_error, "error in the following rib found at line 2, column 16\n`foo(request)`\ncause: invalid function call `foo`\nunknown function\n");
        }

        #[test]
        fn test_unknown_resource_constructor() {
            let metadata = internal::metadata_with_resource_methods();
            let expr = r#"
               let user_id = "user";
               golem:it/api.{cart(user_id).add-item}("apple");
               golem:it/api.{cart0(user_id).add-item}("apple");
                "success"
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiler_error = compiler::compile(&expr, &metadata).unwrap_err().to_string();
            assert_eq!(
                compiler_error,
                "error in the following rib found at line 4, column 16\n`golem:it/api.{cart0(user_id).add-item}(\"apple\")`\ncause: invalid function call `[constructor]cart0`\nunknown function\n"
            );
        }

        #[test]
        fn test_unknown_resource_method() {
            let metadata = internal::metadata_with_resource_methods();
            let expr = r#"
               let user_id = "user";
               golem:it/api.{cart(user_id).add-item}("apple");
               golem:it/api.{cart(user_id).foo}("apple");
                "success"
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiler_error = compiler::compile(&expr, &metadata).unwrap_err().to_string();
            assert_eq!(
                compiler_error,
                "error in the following rib found at line 4, column 16\n`golem:it/api.{cart(user_id).foo}(\"apple\")`\ncause: invalid function call `[method]cart.foo`\nunknown function\n"
            );
        }

        #[test]
        fn test_invalid_arg_size_function() {
            let metadata = internal::get_component_metadata(
                "foo",
                vec![AnalysedType::Str(TypeStr)],
                AnalysedType::Str(TypeStr),
            );

            let expr = r#"
               let user_id = "user";
               let result = foo(user_id, user_id);
               result
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiler_error = compiler::compile(&expr, &metadata).unwrap_err().to_string();
            assert_eq!(
                compiler_error,
                "error in the following rib found at line 3, column 29\n`foo(user_id, user_id)`\ncause: invalid argument size for function `foo`. expected 1 arguments, found 2\n"
            );
        }

        #[test]
        fn test_invalid_arg_size_resource_constructor() {
            let metadata = internal::metadata_with_resource_methods();
            let expr = r#"
               let user_id = "user";
               golem:it/api.{cart(user_id, user_id).add-item}("apple");
                "success"
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiler_error = compiler::compile(&expr, &metadata).unwrap_err().to_string();
            assert_eq!(
                compiler_error,
                "error in the following rib found at line 3, column 16\n`golem:it/api.{cart(user_id, user_id).add-item}(\"apple\")`\ncause: invalid argument size for function `[constructor]cart`. expected 1 arguments, found 2\n"
            );
        }

        #[test]
        fn test_invalid_arg_size_resource_method() {
            let metadata = internal::metadata_with_resource_methods();
            let expr = r#"
               let user_id = "user";
               golem:it/api.{cart(user_id).add-item}("apple", "samsung");
                "success"
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiler_error = compiler::compile(&expr, &metadata).unwrap_err().to_string();
            assert_eq!(
                compiler_error,
                "error in the following rib found at line 3, column 16\n`golem:it/api.{cart(user_id).add-item}(\"apple\", \"samsung\")`\ncause: invalid argument size for function `[method]cart.add-item`. expected 1 arguments, found 2\n"
            );
        }

        #[test]
        fn test_invalid_arg_size_variants() {
            let metadata = internal::metadata_with_variants();

            let expr = r#"
               let regiser_user_action = register-user(1, "foo");
               let result = golem:it/api.{foo}(regiser_user_action);
               result
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiler_error = compiler::compile(&expr, &metadata).unwrap_err().to_string();
            assert_eq!(
                compiler_error,
                "error in the following rib found at line 0, column 0\n`register-user(1, \"foo\")`\ncause: invalid argument size for function `register-user`. expected 1 arguments, found 2\n"
            );
        }

        #[test]
        fn test_invalid_arg_types_function() {
            let metadata = internal::get_component_metadata(
                "foo",
                vec![AnalysedType::Str(TypeStr)],
                AnalysedType::Str(TypeStr),
            );

            let expr = r#"
               let result = foo(1u64);
               result
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiler_error = compiler::compile(&expr, &metadata).unwrap_err().to_string();
            assert_eq!(
                compiler_error,
                "error in the following rib found at line 2, column 33\n`1: u64`\nfound within:\n`foo(1: u64)`\ncause: type mismatch. expected string. found u64\ninvalid argument to the function `foo`\n"
            );
        }

        #[test]
        fn test_invalid_arg_types_resource_method() {
            let metadata = internal::metadata_with_resource_methods();
            let expr = r#"
               let user_id = "user";
               golem:it/api.{cart(user_id).add-item}("apple");
                "success"
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiler_error = compiler::compile(&expr, &metadata).unwrap_err().to_string();
            assert_eq!(
                compiler_error,
                "error in the following rib found at line 3, column 54\n`\"apple\"`\nfound within:\n`golem:it/api.{cart(user_id).add-item}(\"apple\")`\ncause: type mismatch. expected record{name: string}. found string\ninvalid argument to the function `[method]cart.add-item`\n"
            );
        }

        #[test]
        fn test_invalid_arg_types_resource_constructor() {
            let metadata = internal::metadata_with_resource_methods();
            let expr = r#"
               golem:it/api.{cart({foo : "bar"}).add-item}("apple");
                "success"
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiler_error = compiler::compile(&expr, &metadata).unwrap_err().to_string();
            assert_eq!(
                compiler_error,
                "error in the following rib found at line 1, column 1\n`{foo: \"bar\"}`\nfound within:\n`golem:it/api.{cart({foo: \"bar\"}).add-item}(\"apple\")`\ncause: type mismatch. expected string. found record{foo: string}\ninvalid argument to the function `[constructor]cart`\n"
            );
        }

        #[test]
        fn test_invalid_arg_types_variants() {
            let metadata = internal::metadata_with_variants();

            let expr = r#"
               let regiser_user_action = register-user("foo");
               let result = golem:it/api.{foo}(regiser_user_action);
               result
            "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiler_error = compiler::compile(&expr, &metadata).unwrap_err().to_string();
            assert_eq!(
                compiler_error,
                "error in the following rib found at line 2, column 56\n`\"foo\"`\nfound within:\n`register-user(\"foo\")`\ncause: type mismatch. expected u64. found string\ninvalid argument to the function `register-user`\n"
            );
        }
    }

    #[cfg(test)]
    mod global_input_tests {
        use test_r::test;

        use crate::compiler::byte_code::compiler_tests::internal;
        use crate::{compiler, Expr};
        use golem_wasm_ast::analysis::{
            AnalysedType, NameOptionTypePair, NameTypePair, TypeEnum, TypeList, TypeOption,
            TypeRecord, TypeResult, TypeStr, TypeTuple, TypeU32, TypeU64, TypeVariant,
        };

        #[test]
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

            assert_eq!(compiled.rib_input_type_info, expected_type_info);
        }

        #[test]
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

            assert_eq!(compiled.rib_input_type_info, expected_type_info);
        }

        #[test]
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

            assert_eq!(compiled.rib_input_type_info, expected_type_info);
        }

        #[test]
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

            assert_eq!(compiled.rib_input_type_info, expected_type_info);
        }

        #[test]
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

            assert_eq!(compiled.rib_input_type_info, expected_type_info);
        }

        #[test]
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

            assert_eq!(compiled.rib_input_type_info, expected_type_info);
        }

        #[test]
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

            assert_eq!(compiled.rib_input_type_info, expected_type_info);
        }

        #[test]
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

            assert_eq!(compiled.rib_input_type_info, expected_type_info);
        }

        #[test]
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

            assert_eq!(compiled.rib_input_type_info, expected_type_info);
        }
    }

    mod internal {
        use crate::RibInputTypeInfo;
        use golem_wasm_ast::analysis::*;
        use std::collections::HashMap;

        pub(crate) fn metadata_with_variants() -> Vec<AnalysedExport> {
            let instance = AnalysedExport::Instance(AnalysedInstance {
                name: "golem:it/api".to_string(),
                functions: vec![AnalysedFunction {
                    name: "foo".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "param1".to_string(),
                        typ: AnalysedType::Variant(TypeVariant {
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
                        }),
                    }],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: AnalysedType::Handle(TypeHandle {
                            resource_id: AnalysedResourceId(0),
                            mode: AnalysedResourceMode::Owned,
                        }),
                    }],
                }],
            });

            vec![instance]
        }

        pub(crate) fn metadata_with_resource_methods() -> Vec<AnalysedExport> {
            let instance = AnalysedExport::Instance(AnalysedInstance {
                name: "golem:it/api".to_string(),
                functions: vec![
                    AnalysedFunction {
                        name: "[constructor]cart".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "param1".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        }],
                        results: vec![AnalysedFunctionResult {
                            name: None,
                            typ: AnalysedType::Handle(TypeHandle {
                                resource_id: AnalysedResourceId(0),
                                mode: AnalysedResourceMode::Owned,
                            }),
                        }],
                    },
                    AnalysedFunction {
                        name: "[method]cart.add-item".to_string(),
                        parameters: vec![
                            AnalysedFunctionParameter {
                                name: "self".to_string(),
                                typ: AnalysedType::Handle(TypeHandle {
                                    resource_id: AnalysedResourceId(0),
                                    mode: AnalysedResourceMode::Borrowed,
                                }),
                            },
                            AnalysedFunctionParameter {
                                name: "item".to_string(),
                                typ: AnalysedType::Record(TypeRecord {
                                    fields: vec![NameTypePair {
                                        name: "name".to_string(),
                                        typ: AnalysedType::Str(TypeStr),
                                    }],
                                }),
                            },
                        ],
                        results: vec![],
                    },
                ],
            });

            vec![instance]
        }
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
