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

use crate::{AnalysedTypeWithUnit, ParsedFunctionSite, VariableId};
use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::ValueAndType;
use serde::{Deserialize, Serialize};

// To create any type, example, CreateOption, you have to feed a fully formed AnalysedType
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum RibIR {
    PushLit(ValueAndType),
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
    PushFlag(ValueAndType), // More or less like a literal, compiler can form the value directly
    SelectField(String),
    SelectIndex(usize), // Kept for backward compatibility. Cannot read old SelectIndex(usize) as a SelectIndexV1
    SelectIndexV1,
    EqualTo,
    GreaterThan,
    And,
    Or,
    LessThan,
    GreaterThanOrEqualTo,
    LessThanOrEqualTo,
    IsEmpty,
    JumpIfFalse(InstructionId),
    Jump(InstructionId),
    Label(InstructionId),
    Deconstruct,
    CreateFunctionName(ParsedFunctionSite, FunctionReferenceType),
    InvokeFunction(WorkerNamePresence, usize, AnalysedTypeWithUnit),
    PushVariant(String, AnalysedType), // There is no arg size since the type of each variant case is only 1 from beginning
    PushEnum(String, AnalysedType),
    Throw(String),
    GetTag,
    Concat(usize),
    Plus(AnalysedType),
    Minus(AnalysedType),
    Divide(AnalysedType),
    Multiply(AnalysedType),
    Negate,
    ToIterator,
    CreateSink(AnalysedType),
    AdvanceIterator,
    PushToSink,
    SinkToList,
    Length,
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum WorkerNamePresence {
    Present,
    Absent,
}

impl From<golem_api_grpc::proto::golem::rib::WorkerNamePresence> for WorkerNamePresence {
    fn from(value: golem_api_grpc::proto::golem::rib::WorkerNamePresence) -> Self {
        match value {
            golem_api_grpc::proto::golem::rib::WorkerNamePresence::Present => {
                WorkerNamePresence::Present
            }
            golem_api_grpc::proto::golem::rib::WorkerNamePresence::Absent => {
                WorkerNamePresence::Absent
            }
        }
    }
}

impl From<WorkerNamePresence> for golem_api_grpc::proto::golem::rib::WorkerNamePresence {
    fn from(value: WorkerNamePresence) -> Self {
        match value {
            WorkerNamePresence::Present => {
                golem_api_grpc::proto::golem::rib::WorkerNamePresence::Present
            }
            WorkerNamePresence::Absent => {
                golem_api_grpc::proto::golem::rib::WorkerNamePresence::Absent
            }
        }
    }
}

impl RibIR {
    pub fn get_instruction_id(&self) -> Option<InstructionId> {
        match self {
            RibIR::Label(id) => Some(id.clone()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub enum FunctionReferenceType {
    Function {
        function: String,
    },
    RawResourceConstructor {
        resource: String,
    },
    RawResourceDrop {
        resource: String,
    },
    RawResourceMethod {
        resource: String,
        method: String,
    },
    RawResourceStaticMethod {
        resource: String,
        method: String,
    },
    IndexedResourceConstructor {
        resource: String,
        arg_size: usize,
    },
    IndexedResourceMethod {
        resource: String,
        arg_size: usize,
        method: String,
    },
    IndexedResourceStaticMethod {
        resource: String,
        arg_size: usize,
        method: String,
    },
    IndexedResourceDrop {
        resource: String,
        arg_size: usize,
    },
}

// Every instruction can have a unique ID, and the compiler
// can assign this and label the start and end of byte code blocks.
// This is more efficient than assigning index to every instruction and incrementing it
// as we care about it only if we need to jump through instructions.
// Jumping to an ID is simply draining the stack until we find a Label instruction with the same ID.
#[derive(Debug, Clone, PartialEq, Hash, Eq, Serialize, Deserialize, Encode, Decode)]
pub struct InstructionId {
    pub index: usize,
}

impl InstructionId {
    pub fn new(index: usize) -> Self {
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

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::{
        AnalysedTypeWithUnit, FunctionReferenceType, InstructionId, ParsedFunctionSite, RibIR,
        WorkerNamePresence,
    };
    use golem_api_grpc::proto::golem::rib::rib_ir::Instruction;
    use golem_api_grpc::proto::golem::rib::{
        And, CallInstruction, ConcatInstruction, CreateFunctionNameInstruction, EqualTo, GetTag,
        GreaterThan, GreaterThanOrEqualTo, IsEmpty, JumpInstruction, LessThan, LessThanOrEqualTo,
        Negate, Or, PushListInstruction, PushNoneInstruction, PushTupleInstruction,
        RibIr as ProtoRibIR,
    };
    use golem_wasm_ast::analysis::{AnalysedType, TypeStr};
    use golem_wasm_rpc::ValueAndType;

    impl TryFrom<golem_api_grpc::proto::golem::rib::FunctionReferenceType> for FunctionReferenceType {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::rib::FunctionReferenceType,
        ) -> Result<Self, Self::Error> {
            let value = value.r#type.ok_or("Missing type".to_string())?;
            let function_reference_type = match value {
                golem_api_grpc::proto::golem::rib::function_reference_type::Type::Function(name) => FunctionReferenceType::Function {
                    function: name.name
                },
                golem_api_grpc::proto::golem::rib::function_reference_type::Type::RawResourceConstructor(name) =>
                    FunctionReferenceType::RawResourceConstructor {
                        resource: name.resource_name
                    },
                golem_api_grpc::proto::golem::rib::function_reference_type::Type::RawResourceDrop(name) => FunctionReferenceType::RawResourceDrop {
                    resource: name.resource_name
                },
                golem_api_grpc::proto::golem::rib::function_reference_type::Type::RawResourceMethod(raw_resource_method) => {
                    let resource = raw_resource_method.resource_name;
                    let method = raw_resource_method.method_name;
                    FunctionReferenceType::RawResourceMethod { resource, method }
                }
                golem_api_grpc::proto::golem::rib::function_reference_type::Type::RawResourceStaticMethod(raw_resource_static_method) => {
                    let resource = raw_resource_static_method.resource_name;
                    let method = raw_resource_static_method.method_name;
                    FunctionReferenceType::RawResourceStaticMethod { resource, method }
                }
                golem_api_grpc::proto::golem::rib::function_reference_type::Type::IndexedResourceConstructor(indexed_resource_constructor) => {
                    let resource = indexed_resource_constructor.resource_name;
                    let arg_size = indexed_resource_constructor.arg_size;
                    FunctionReferenceType::IndexedResourceConstructor { resource, arg_size: arg_size as usize }
                }
                golem_api_grpc::proto::golem::rib::function_reference_type::Type::IndexedResourceMethod(indexed_resource_method) => {
                    let resource = indexed_resource_method.resource_name;
                    let arg_size = indexed_resource_method.arg_size;
                    let method = indexed_resource_method.method_name;
                    FunctionReferenceType::IndexedResourceMethod { resource, arg_size: arg_size as usize, method }
                }
                golem_api_grpc::proto::golem::rib::function_reference_type::Type::IndexedResourceStaticMethod(indexed_resource_static_method) => {
                    let resource = indexed_resource_static_method.resource_name;
                    let arg_size = indexed_resource_static_method.arg_size;
                    let method = indexed_resource_static_method.method_name;
                    FunctionReferenceType::IndexedResourceStaticMethod { resource, arg_size: arg_size as usize, method }
                }
                golem_api_grpc::proto::golem::rib::function_reference_type::Type::IndexedResourceDrop(indexed_resource_drop) => {
                    let resource = indexed_resource_drop.resource_name;
                    let arg_size = indexed_resource_drop.arg_size;
                    FunctionReferenceType::IndexedResourceDrop { resource, arg_size: arg_size as usize }
                }
            };
            Ok(function_reference_type)
        }
    }

    impl From<FunctionReferenceType> for golem_api_grpc::proto::golem::rib::FunctionReferenceType {
        fn from(value: FunctionReferenceType) -> Self {
            match value {
                FunctionReferenceType::Function { function } => golem_api_grpc::proto::golem::rib::FunctionReferenceType {
                    r#type: Some(golem_api_grpc::proto::golem::rib::function_reference_type::Type::Function(golem_api_grpc::proto::golem::rib::Function {
                        name: function
                    }))
                },
                FunctionReferenceType::RawResourceConstructor { resource } => golem_api_grpc::proto::golem::rib::FunctionReferenceType {
                    r#type: Some(golem_api_grpc::proto::golem::rib::function_reference_type::Type::RawResourceConstructor(golem_api_grpc::proto::golem::rib::RawResourceConstructor {
                        resource_name: resource
                    }))
                },
                FunctionReferenceType::RawResourceDrop { resource } => golem_api_grpc::proto::golem::rib::FunctionReferenceType {
                    r#type: Some(golem_api_grpc::proto::golem::rib::function_reference_type::Type::RawResourceDrop(golem_api_grpc::proto::golem::rib::RawResourceDrop {
                        resource_name: resource
                    }))
                },
                FunctionReferenceType::RawResourceMethod { resource, method } => golem_api_grpc::proto::golem::rib::FunctionReferenceType {
                    r#type: Some(golem_api_grpc::proto::golem::rib::function_reference_type::Type::RawResourceMethod(golem_api_grpc::proto::golem::rib::RawResourceMethod {
                        resource_name: resource,
                        method_name: method,
                    }))
                },
                FunctionReferenceType::RawResourceStaticMethod { resource, method } => golem_api_grpc::proto::golem::rib::FunctionReferenceType {
                    r#type: Some(golem_api_grpc::proto::golem::rib::function_reference_type::Type::RawResourceStaticMethod(golem_api_grpc::proto::golem::rib::RawResourceStaticMethod {
                        resource_name: resource,
                        method_name: method,
                    }))
                },
                FunctionReferenceType::IndexedResourceConstructor { resource, arg_size } => golem_api_grpc::proto::golem::rib::FunctionReferenceType {
                    r#type: Some(golem_api_grpc::proto::golem::rib::function_reference_type::Type::IndexedResourceConstructor(golem_api_grpc::proto::golem::rib::IndexedResourceConstructor {
                        resource_name: resource,
                        arg_size: arg_size as u32,
                    }))
                },
                FunctionReferenceType::IndexedResourceMethod { resource, arg_size, method } => golem_api_grpc::proto::golem::rib::FunctionReferenceType {
                    r#type: Some(golem_api_grpc::proto::golem::rib::function_reference_type::Type::IndexedResourceMethod(golem_api_grpc::proto::golem::rib::IndexedResourceMethod {
                        resource_name: resource,
                        arg_size: arg_size as u32,
                        method_name: method,
                    }))
                },
                FunctionReferenceType::IndexedResourceStaticMethod { resource, arg_size, method } => golem_api_grpc::proto::golem::rib::FunctionReferenceType {
                    r#type: Some(golem_api_grpc::proto::golem::rib::function_reference_type::Type::IndexedResourceStaticMethod(golem_api_grpc::proto::golem::rib::IndexedResourceStaticMethod {
                        resource_name: resource,
                        arg_size: arg_size as u32,
                        method_name: method,
                    }))
                },
                FunctionReferenceType::IndexedResourceDrop { resource, arg_size } => golem_api_grpc::proto::golem::rib::FunctionReferenceType {
                    r#type: Some(golem_api_grpc::proto::golem::rib::function_reference_type::Type::IndexedResourceDrop(golem_api_grpc::proto::golem::rib::IndexedResourceDrop {
                        resource_name: resource,
                        arg_size: arg_size as u32,
                    }))
                }
            }
        }
    }

    impl TryFrom<ProtoRibIR> for RibIR {
        type Error = String;

        fn try_from(value: ProtoRibIR) -> Result<Self, Self::Error> {
            let instruction = value
                .instruction
                .ok_or_else(|| "Missing instruction".to_string())?;

            match instruction {
                Instruction::PushLit(value) => {
                    let value: ValueAndType = value.try_into()?;
                    Ok(RibIR::PushLit(value))
                }
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
                Instruction::Plus(value) => {
                    Ok(RibIR::Plus((&value).try_into().map_err(|_| {
                        "Failed to convert CreateAndPushRecord".to_string()
                    })?))
                }
                Instruction::Multiply(value) => {
                    Ok(RibIR::Multiply((&value).try_into().map_err(|_| {
                        "Failed to convert CreateAndPushRecord".to_string()
                    })?))
                }
                Instruction::Minus(value) => {
                    Ok(RibIR::Minus((&value).try_into().map_err(|_| {
                        "Failed to convert CreateAndPushRecord".to_string()
                    })?))
                }
                Instruction::Divide(value) => {
                    Ok(RibIR::Divide((&value).try_into().map_err(|_| {
                        "Failed to convert CreateAndPushRecord".to_string()
                    })?))
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
                Instruction::Length(_) => Ok(RibIR::Length),
                Instruction::SelectField(value) => Ok(RibIR::SelectField(value)),
                Instruction::SelectIndex(value) => Ok(RibIR::SelectIndex(value as usize)),
                Instruction::SelectIndexV1(_) => Ok(RibIR::SelectIndexV1),
                Instruction::EqualTo(_) => Ok(RibIR::EqualTo),
                Instruction::GreaterThan(_) => Ok(RibIR::GreaterThan),
                Instruction::LessThan(_) => Ok(RibIR::LessThan),
                Instruction::GreaterThanOrEqualTo(_) => Ok(RibIR::GreaterThanOrEqualTo),
                Instruction::LessThanOrEqualTo(_) => Ok(RibIR::LessThanOrEqualTo),
                Instruction::And(_) => Ok(RibIR::And),
                Instruction::IsEmpty(_) => Ok(RibIR::IsEmpty),
                Instruction::Or(_) => Ok(RibIR::Or),
                Instruction::JumpIfFalse(value) => Ok(RibIR::JumpIfFalse(InstructionId::new(
                    value.instruction_id as usize,
                ))),
                Instruction::Jump(value) => Ok(RibIR::Jump(InstructionId::new(
                    value.instruction_id as usize,
                ))),
                Instruction::Label(value) => Ok(RibIR::Label(InstructionId::new(
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

                    let worker_name_presence = call_instruction
                        .worker_name_presence
                        .map(|x| {
                            golem_api_grpc::proto::golem::rib::WorkerNamePresence::try_from(x)
                                .map_err(|err| err.to_string())
                        })
                        .transpose()?;

                    // Default is absent because old rib scripts don't have worker name in it
                    let worker_name_presence = worker_name_presence
                        .map(|x| x.into())
                        .unwrap_or(WorkerNamePresence::Absent);

                    Ok(RibIR::InvokeFunction(
                        worker_name_presence,
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
                Instruction::PushFlag(flag) => {
                    let flag: ValueAndType = flag.try_into()?;
                    Ok(RibIR::PushFlag(flag))
                }
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
                Instruction::CreateFunctionName(instruction) => {
                    let parsed_site = instruction.site.ok_or("Missing site".to_string())?;
                    let parsed_function_site = ParsedFunctionSite::try_from(parsed_site)?;

                    let reference_type = instruction
                        .function_reference_details
                        .ok_or("Missing reference_type".to_string())?;
                    let function_reference_type = reference_type.try_into()?;

                    Ok(RibIR::CreateFunctionName(
                        parsed_function_site,
                        function_reference_type,
                    ))
                }
                Instruction::ListToIterator(_) => Ok(RibIR::ToIterator),
                Instruction::CreateSink(create_sink) => {
                    let result = create_sink
                        .list_type
                        .ok_or("Sink list type not present".to_string())
                        .and_then(|t| {
                            (&t).try_into()
                                .map_err(|_| "Failed to convert AnalysedType".to_string())
                        })?;

                    Ok(RibIR::CreateSink(result))
                }
                Instruction::AdvanceIterator(_) => Ok(RibIR::AdvanceIterator),
                Instruction::SinkToList(_) => Ok(RibIR::SinkToList),
                Instruction::PushToSink(_) => Ok(RibIR::PushToSink),
            }
        }
    }

    impl TryFrom<RibIR> for ProtoRibIR {
        type Error = String;

        fn try_from(value: RibIR) -> Result<Self, Self::Error> {
            let instruction = match value {
                RibIR::PushLit(value) => {
                    Instruction::PushLit(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                        type_annotated_value: Some(
                            value
                                .try_into()
                                .map_err(|errs: Vec<String>| errs.join(", "))?,
                        ),
                    })
                }
                RibIR::And => Instruction::And(And {}),
                RibIR::IsEmpty => Instruction::IsEmpty(IsEmpty {}),
                RibIR::Or => Instruction::Or(Or {}),
                RibIR::AssignVar(value) => Instruction::AssignVar(value.into()),
                RibIR::LoadVar(value) => Instruction::LoadVar(value.into()),
                RibIR::CreateAndPushRecord(value) => {
                    Instruction::CreateAndPushRecord((&value).into())
                }
                RibIR::Plus(value) => Instruction::Plus((&value).into()),
                RibIR::Minus(value) => Instruction::Minus((&value).into()),
                RibIR::Multiply(value) => Instruction::Multiply((&value).into()),
                RibIR::Divide(value) => Instruction::Divide((&value).into()),
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
                RibIR::Length => Instruction::Length(golem_api_grpc::proto::golem::rib::Length {}),
                RibIR::SelectIndexV1 => {
                    Instruction::SelectIndexV1(golem_api_grpc::proto::golem::rib::SelectIndexV1 {})
                }
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
                RibIR::Deconstruct => {
                    Instruction::Deconstruct((&AnalysedType::Str(TypeStr)).into())
                } //TODO; remove type in deconstruct from protobuf
                RibIR::InvokeFunction(worker_name_presence, arg_count, return_type) => {
                    let typ = match return_type {
                        AnalysedTypeWithUnit::Unit => None,
                        AnalysedTypeWithUnit::Type(analysed_type) => {
                            let typ =
                                golem_wasm_ast::analysis::protobuf::Type::from(&analysed_type);
                            Some(typ)
                        }
                    };

                    let worker_name_presence: golem_api_grpc::proto::golem::rib::WorkerNamePresence =
                        worker_name_presence.into();

                    Instruction::Call(CallInstruction {
                        argument_count: arg_count as u64,
                        return_type: typ,
                        worker_name_presence: Some(worker_name_presence.into()),
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
                        type_annotated_value: Some(
                            flag.try_into()
                                .map_err(|errs: Vec<String>| errs.join(", "))?,
                        ),
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
                RibIR::CreateFunctionName(site, reference_type) => {
                    Instruction::CreateFunctionName(CreateFunctionNameInstruction {
                        site: Some(site.into()),
                        function_reference_details: Some(reference_type.into()),
                    })
                }

                RibIR::ToIterator => Instruction::ListToIterator(
                    golem_api_grpc::proto::golem::rib::ListToIterator {},
                ),
                RibIR::CreateSink(analysed_type) => {
                    Instruction::CreateSink(golem_api_grpc::proto::golem::rib::CreateSink {
                        list_type: Some((&analysed_type).into()),
                    })
                }
                RibIR::AdvanceIterator => Instruction::AdvanceIterator(
                    golem_api_grpc::proto::golem::rib::AdvanceIterator {},
                ),
                RibIR::PushToSink => {
                    Instruction::PushToSink(golem_api_grpc::proto::golem::rib::PushToSink {})
                }
                RibIR::SinkToList => {
                    Instruction::SinkToList(golem_api_grpc::proto::golem::rib::SinkToList {})
                }
            };

            Ok(ProtoRibIR {
                instruction: Some(instruction),
            })
        }
    }
}
