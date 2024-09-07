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

use crate::{AnalysedTypeWithUnit, ParsedFunctionName, VariableId};
use bincode::{Decode, Encode};
use golem_api_grpc::proto::golem::rib::rib_ir::Instruction;
use golem_api_grpc::proto::golem::rib::{
    CallInstruction, ConcatInstruction, EqualTo, GetTag, GreaterThan, GreaterThanOrEqualTo,
    JumpInstruction, LessThan, LessThanOrEqualTo, Negate, PushListInstruction, PushNoneInstruction,
    PushTupleInstruction, RibIr as ProtoRibIR,
};
use golem_wasm_ast::analysis::{AnalysedType, TypeStr};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use serde::{Deserialize, Serialize};

// To create any type, example, CreateOption, you have to feed a fully formed AnalysedType
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub enum RibIR {
    PushLit(TypeAnnotatedValue),
    AssignVar(VariableId),
    LoadVar(VariableId),
    CreateAndPushRecord(AnalysedType),
    UpdateRecord(String),
    PushList(AnalysedType, usize),
    PushTuple(AnalysedType, usize),
    PushSome(AnalysedType),
    PushNone(Option<AnalysedType>), // In certain cases, we don't need the type info
    PushOkResult(AnalysedType),
    PushErrResult(AnalysedType),
    PushFlag(TypeAnnotatedValue), // More or less like a literal, compiler can form the value directly
    SelectField(String),
    SelectIndex(usize),
    EqualTo,
    GreaterThan,
    LessThan,
    GreaterThanOrEqualTo,
    LessThanOrEqualTo,
    JumpIfFalse(InstructionId),
    Jump(InstructionId),
    Label(InstructionId),
    Deconstruct,
    InvokeFunction(ParsedFunctionName, usize, AnalysedTypeWithUnit),
    PushVariant(String, AnalysedType), // There is no arg size since the type of each variant case is only 1 from beginning
    PushEnum(String, AnalysedType),
    Throw(String),
    GetTag,
    Concat(usize),
    Negate,
}

// Every instruction can have a unique ID, and the compiler
// can assign this and label the start and end of byte code blocks.
// This is more efficient than assigning index to every instruction and incrementing it
// as we care about it only if we need to jump through instructions.
// Jumping to an ID is simply draining the stack until we find a Label instruction with the same ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct InstructionId {
    pub index: usize,
}

impl InstructionId {
    pub fn from(index: usize) -> Self {
        InstructionId { index }
    }

    pub fn init() -> Self {
        InstructionId { index: 0 }
    }

    pub fn increment(&self) -> InstructionId {
        InstructionId {
            index: self.index + 1,
        }
    }

    pub fn increment_mut(&mut self) -> InstructionId {
        self.index += 1;
        self.clone()
    }
}

impl TryFrom<ProtoRibIR> for RibIR {
    type Error = String;

    fn try_from(value: ProtoRibIR) -> Result<Self, Self::Error> {
        let instruction = value
            .instruction
            .ok_or_else(|| "Missing instruction".to_string())?;

        match instruction {
            Instruction::PushLit(value) => Ok(RibIR::PushLit(
                value
                    .type_annotated_value
                    .ok_or("Missing type_annotated_value".to_string())?,
            )),
            Instruction::AssignVar(value) => Ok(RibIR::AssignVar(
                value
                    .try_into()
                    .map_err(|_| "Failed to convert AssignVar".to_string())?,
            )),
            Instruction::LoadVar(value) => Ok(RibIR::LoadVar(
                value
                    .try_into()
                    .map_err(|_| "Failed to convert LoadVar".to_string())?,
            )),
            Instruction::CreateAndPushRecord(value) => {
                Ok(RibIR::CreateAndPushRecord((&value).try_into().map_err(
                    |_| "Failed to convert CreateAndPushRecord".to_string(),
                )?))
            }
            Instruction::UpdateRecord(value) => Ok(RibIR::UpdateRecord(value)),
            Instruction::PushList(value) => Ok(RibIR::PushList(
                value
                    .list_type
                    .ok_or("List type not present".to_string())
                    .and_then(|t| {
                        (&t).try_into()
                            .map_err(|_| "Failed to convert AnalysedType".to_string())
                    })?,
                value.list_size as usize,
            )),
            Instruction::CreateSome(value) => Ok(RibIR::PushSome(
                (&value)
                    .try_into()
                    .map_err(|_| "Failed to convert CreateSome".to_string())?,
            )),
            Instruction::CreateNone(value) => match value.none_type {
                Some(v) => {
                    let optional_type = (&v)
                        .try_into()
                        .map_err(|_| "Failed to convert AnalysedType".to_string());
                    Ok(RibIR::PushNone(Some(optional_type?)))
                }
                None => Ok(RibIR::PushNone(None)),
            },
            Instruction::CreateOkResult(value) => {
                Ok(RibIR::PushOkResult((&value).try_into().map_err(|_| {
                    "Failed to convert CreateOkResult".to_string()
                })?))
            }
            Instruction::CreateErrResult(value) => {
                Ok(RibIR::PushErrResult((&value).try_into().map_err(|_| {
                    "Failed to convert CreateErrResult".to_string()
                })?))
            }
            Instruction::SelectField(value) => Ok(RibIR::SelectField(value)),
            Instruction::SelectIndex(value) => Ok(RibIR::SelectIndex(value as usize)),
            Instruction::EqualTo(_) => Ok(RibIR::EqualTo),
            Instruction::GreaterThan(_) => Ok(RibIR::GreaterThan),
            Instruction::LessThan(_) => Ok(RibIR::LessThan),
            Instruction::GreaterThanOrEqualTo(_) => Ok(RibIR::GreaterThanOrEqualTo),
            Instruction::LessThanOrEqualTo(_) => Ok(RibIR::LessThanOrEqualTo),
            Instruction::JumpIfFalse(value) => Ok(RibIR::JumpIfFalse(InstructionId::from(
                value.instruction_id as usize,
            ))),
            Instruction::Jump(value) => Ok(RibIR::Jump(InstructionId::from(
                value.instruction_id as usize,
            ))),
            Instruction::Label(value) => Ok(RibIR::Label(InstructionId::from(
                value.instruction_id as usize,
            ))),
            Instruction::Deconstruct(_) => Ok(RibIR::Deconstruct),
            Instruction::Call(call_instruction) => {
                let return_type = match call_instruction.return_type {
                    Some(return_type) => {
                        let analysed_type = (&return_type)
                            .try_into()
                            .map_err(|_| "Failed to convert AnalysedType".to_string())?;

                        AnalysedTypeWithUnit::Type(analysed_type)
                    }
                    None => AnalysedTypeWithUnit::Unit,
                };

                Ok(RibIR::InvokeFunction(
                    ParsedFunctionName::parse(call_instruction.function_name)
                        .map_err(|_| "Failed to convert ParsedFunctionName".to_string())?,
                    call_instruction.argument_count as usize,
                    return_type,
                ))
            }
            Instruction::VariantConstruction(variant_construction) => {
                let variant_type = variant_construction
                    .return_type
                    .ok_or("Missing return_type for variant construction".to_string())?;

                let analysed_variant_type = (&variant_type)
                    .try_into()
                    .map_err(|_| "Failed to convert AnalysedType".to_string())?;

                Ok(RibIR::PushVariant(
                    variant_construction.variant_name,
                    analysed_variant_type,
                ))
            }
            Instruction::EnumConstruction(enum_construction) => {
                let enum_type = enum_construction
                    .return_type
                    .ok_or("Missing return_type for enum construction".to_string())?;

                let analysed_enum_type = (&enum_type)
                    .try_into()
                    .map_err(|_| "Failed to convert AnalysedType".to_string())?;

                Ok(RibIR::PushEnum(
                    enum_construction.enum_name,
                    analysed_enum_type,
                ))
            }
            Instruction::Throw(value) => Ok(RibIR::Throw(value)),
            Instruction::PushFlag(flag) => Ok(RibIR::PushFlag(
                flag.type_annotated_value
                    .ok_or("Missing flag".to_string())?,
            )),
            Instruction::GetTag(_) => Ok(RibIR::GetTag),
            Instruction::PushTuple(tuple_instruction) => {
                let tuple_type = tuple_instruction
                    .tuple_type
                    .ok_or("Missing tuple_type".to_string())
                    .and_then(|t| {
                        (&t).try_into()
                            .map_err(|_| "Failed to convert AnalysedType".to_string())
                    })?;

                Ok(RibIR::PushTuple(
                    tuple_type,
                    tuple_instruction.tuple_size as usize,
                ))
            }
            Instruction::Negate(_) => Ok(RibIR::Negate),
            Instruction::Concat(concat_instruction) => {
                Ok(RibIR::Concat(concat_instruction.arg_size as usize))
            }
        }
    }
}

impl From<RibIR> for ProtoRibIR {
    fn from(value: RibIR) -> Self {
        let instruction = match value {
            RibIR::PushLit(value) => {
                Instruction::PushLit(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(value),
                })
            }
            RibIR::AssignVar(value) => Instruction::AssignVar(value.into()),
            RibIR::LoadVar(value) => Instruction::LoadVar(value.into()),
            RibIR::CreateAndPushRecord(value) => Instruction::CreateAndPushRecord((&value).into()),
            RibIR::UpdateRecord(value) => Instruction::UpdateRecord(value),
            RibIR::PushList(value, arg_size) => Instruction::PushList(PushListInstruction {
                list_type: Some((&value).into()),
                list_size: arg_size as u64,
            }),
            RibIR::PushSome(value) => Instruction::CreateSome((&value).into()),
            RibIR::PushNone(value) => {
                let push_none_instruction = PushNoneInstruction {
                    none_type: value.map(|t| (&t).into()),
                };
                Instruction::CreateNone(push_none_instruction)
            }
            RibIR::PushOkResult(value) => Instruction::CreateOkResult((&value).into()),
            RibIR::PushErrResult(value) => Instruction::CreateErrResult((&value).into()),
            RibIR::SelectField(value) => Instruction::SelectField(value),
            RibIR::SelectIndex(value) => Instruction::SelectIndex(value as u64),
            RibIR::EqualTo => Instruction::EqualTo(EqualTo {}),
            RibIR::GreaterThan => Instruction::GreaterThan(GreaterThan {}),
            RibIR::LessThan => Instruction::LessThan(LessThan {}),
            RibIR::GreaterThanOrEqualTo => {
                Instruction::GreaterThanOrEqualTo(GreaterThanOrEqualTo {})
            }
            RibIR::LessThanOrEqualTo => Instruction::LessThanOrEqualTo(LessThanOrEqualTo {}),
            RibIR::JumpIfFalse(value) => Instruction::JumpIfFalse(JumpInstruction {
                instruction_id: value.index as u64,
            }),
            RibIR::Jump(value) => Instruction::Jump(JumpInstruction {
                instruction_id: value.index as u64,
            }),
            RibIR::Label(value) => Instruction::Label(JumpInstruction {
                instruction_id: value.index as u64,
            }),
            RibIR::Deconstruct => Instruction::Deconstruct((&AnalysedType::Str(TypeStr)).into()), //TODO; remove type in deconstruct from protobuf
            RibIR::InvokeFunction(name, arg_count, return_type) => {
                let typ = match return_type {
                    AnalysedTypeWithUnit::Unit => None,
                    AnalysedTypeWithUnit::Type(analysed_type) => {
                        let typ = golem_wasm_ast::analysis::protobuf::Type::from(&analysed_type);
                        Some(typ)
                    }
                };

                Instruction::Call(CallInstruction {
                    function_name: name.to_string(),
                    argument_count: arg_count as u64,
                    return_type: typ,
                })
            }
            RibIR::PushVariant(name, return_type) => {
                let typ = golem_wasm_ast::analysis::protobuf::Type::from(&return_type);

                Instruction::VariantConstruction(
                    golem_api_grpc::proto::golem::rib::VariantConstructionInstruction {
                        variant_name: name,
                        return_type: Some(typ),
                    },
                )
            }
            RibIR::PushEnum(name, return_type) => {
                let typ = golem_wasm_ast::analysis::protobuf::Type::from(&return_type);

                Instruction::EnumConstruction(
                    golem_api_grpc::proto::golem::rib::EnumConstructionInstruction {
                        enum_name: name,
                        return_type: Some(typ),
                    },
                )
            }
            RibIR::Throw(msg) => Instruction::Throw(msg),
            RibIR::PushFlag(flag) => {
                Instruction::PushFlag(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(flag),
                })
            }
            RibIR::GetTag => Instruction::GetTag(GetTag {}),
            RibIR::PushTuple(analysed_type, size) => {
                let typ = golem_wasm_ast::analysis::protobuf::Type::from(&analysed_type);

                Instruction::PushTuple(PushTupleInstruction {
                    tuple_type: Some(typ),
                    tuple_size: size as u64,
                })
            }
            RibIR::Concat(concat) => Instruction::Concat(ConcatInstruction {
                arg_size: concat as u64,
            }),
            RibIR::Negate => Instruction::Negate(Negate {}),
        };

        ProtoRibIR {
            instruction: Some(instruction),
        }
    }
}
