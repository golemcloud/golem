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

use crate::core::*;
use crate::AstCustomization;
use std::io::Write;
use wasmparser::{BinaryReader, Operator, OperatorsReader, Parser, Payload};

impl TryFrom<wasmparser::RefType> for RefType {
    type Error = String;

    fn try_from(value: wasmparser::RefType) -> Result<Self, Self::Error> {
        match value {
            wasmparser::RefType::FUNCREF => Ok(RefType::FuncRef),
            wasmparser::RefType::EXTERNREF => Ok(RefType::ExternRef),
            _ => Err("Unsupported reference type: {value:?}".to_string()),
        }
    }
}

impl TryFrom<wasmparser::ValType> for ValType {
    type Error = String;

    fn try_from(value: wasmparser::ValType) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ValType::I32 => Ok(ValType::Num(NumType::I32)),
            wasmparser::ValType::I64 => Ok(ValType::Num(NumType::I64)),
            wasmparser::ValType::F32 => Ok(ValType::Num(NumType::F32)),
            wasmparser::ValType::F64 => Ok(ValType::Num(NumType::F64)),
            wasmparser::ValType::V128 => Ok(ValType::Vec(VecType::V128)),
            wasmparser::ValType::Ref(r) => Ok(ValType::Ref(r.try_into()?)),
        }
    }
}

impl TryFrom<&[wasmparser::ValType]> for ResultType {
    type Error = String;

    fn try_from(value: &[wasmparser::ValType]) -> Result<Self, Self::Error> {
        let values = value
            .iter()
            .map(|v| (*v).try_into())
            .collect::<Result<Vec<ValType>, Self::Error>>()?;
        Ok(ResultType { values })
    }
}

impl TryFrom<wasmparser::FuncType> for FuncType {
    type Error = String;

    fn try_from(value: wasmparser::FuncType) -> Result<Self, Self::Error> {
        let params = value.params().try_into()?;
        let results = value.results().try_into()?;
        Ok(FuncType {
            input: params,
            output: results,
        })
    }
}

impl TryFrom<wasmparser::TableType> for TableType {
    type Error = String;

    fn try_from(value: wasmparser::TableType) -> Result<Self, Self::Error> {
        Ok(TableType {
            limits: Limits {
                min: value.initial,
                max: value.maximum,
            },
            elements: value.element_type.try_into()?,
        })
    }
}

impl TryFrom<wasmparser::MemoryType> for MemType {
    type Error = String;

    fn try_from(value: wasmparser::MemoryType) -> Result<Self, Self::Error> {
        if value.memory64 {
            Err("64-bit memories are not supported".to_string())
        } else {
            Ok(MemType {
                limits: Limits {
                    min: value.initial,
                    max: value.maximum,
                },
            })
        }
    }
}

impl TryFrom<wasmparser::GlobalType> for GlobalType {
    type Error = String;

    fn try_from(value: wasmparser::GlobalType) -> Result<Self, Self::Error> {
        Ok(GlobalType {
            mutability: if value.mutable { Mut::Var } else { Mut::Const },
            val_type: value.content_type.try_into()?,
        })
    }
}

impl TryFrom<wasmparser::TypeRef> for TypeRef {
    type Error = String;

    fn try_from(value: wasmparser::TypeRef) -> Result<Self, Self::Error> {
        match value {
            wasmparser::TypeRef::Func(func_idx) => Ok(TypeRef::Func(func_idx)),
            wasmparser::TypeRef::Table(table_type) => Ok(TypeRef::Table(table_type.try_into()?)),
            wasmparser::TypeRef::Memory(mem_type) => Ok(TypeRef::Mem(mem_type.try_into()?)),
            wasmparser::TypeRef::Global(global_type) => {
                Ok(TypeRef::Global(global_type.try_into()?))
            }
            wasmparser::TypeRef::Tag(_) => {
                Err("Exception handling proposal is not supported".to_string())
            }
        }
    }
}

impl TryFrom<wasmparser::Import<'_>> for Import {
    type Error = String;

    fn try_from(value: wasmparser::Import) -> Result<Self, Self::Error> {
        Ok(Import {
            module: value.module.to_string(),
            name: value.name.to_string(),
            desc: value.ty.try_into()?,
        })
    }
}

impl TryFrom<wasmparser::Table<'_>> for Table {
    type Error = String;

    fn try_from(value: wasmparser::Table) -> Result<Self, Self::Error> {
        Ok(Table {
            table_type: value.ty.try_into()?,
        })
    }
}

impl TryFrom<wasmparser::MemoryType> for Mem {
    type Error = String;

    fn try_from(value: wasmparser::MemoryType) -> Result<Self, Self::Error> {
        Ok(Mem {
            mem_type: value.try_into()?,
        })
    }
}

impl<'a> TryFrom<wasmparser::Global<'a>> for Global {
    type Error = String;

    fn try_from(value: wasmparser::Global<'a>) -> Result<Self, Self::Error> {
        let op_reader = value.init_expr.get_operators_reader();
        let init = op_reader.try_into()?;
        Ok(Global {
            global_type: value.ty.try_into()?,
            init,
        })
    }
}

impl TryFrom<wasmparser::BlockType> for BlockType {
    type Error = String;

    fn try_from(value: wasmparser::BlockType) -> Result<Self, Self::Error> {
        match value {
            wasmparser::BlockType::Empty => Ok(BlockType::None),
            wasmparser::BlockType::Type(val_type) => Ok(BlockType::Value(val_type.try_into()?)),
            wasmparser::BlockType::FuncType(type_idx) => Ok(BlockType::Index(type_idx)),
        }
    }
}

impl TryFrom<wasmparser::MemArg> for MemArg {
    type Error = String;

    fn try_from(value: wasmparser::MemArg) -> Result<Self, Self::Error> {
        if value.offset > (u32::MAX as u64) {
            Err("64-bit memories are not supported".to_string())
        } else {
            Ok(MemArg {
                offset: value.offset as u32,
                align: value.align,
            })
        }
    }
}

impl TryFrom<wasmparser::HeapType> for RefType {
    type Error = String;

    fn try_from(value: wasmparser::HeapType) -> Result<Self, Self::Error> {
        if value == wasmparser::HeapType::EXTERN {
            Ok(RefType::ExternRef)
        } else if value == wasmparser::HeapType::FUNC {
            Ok(RefType::FuncRef)
        } else {
            Err("GC proposal is not supported".to_string())
        }
    }
}

impl TryFrom<wasmparser::Export<'_>> for Export {
    type Error = String;

    fn try_from(value: wasmparser::Export) -> Result<Self, Self::Error> {
        let desc = match value.kind {
            wasmparser::ExternalKind::Func => Ok(ExportDesc::Func(value.index)),
            wasmparser::ExternalKind::Table => Ok(ExportDesc::Table(value.index)),
            wasmparser::ExternalKind::Memory => Ok(ExportDesc::Mem(value.index)),
            wasmparser::ExternalKind::Global => Ok(ExportDesc::Global(value.index)),
            wasmparser::ExternalKind::Tag => {
                Err("Exception handling proposal is not supported".to_string())
            }
        }?;
        Ok(Export {
            name: value.name.to_string(),
            desc,
        })
    }
}

impl TryFrom<wasmparser::ElementKind<'_>> for ElemMode {
    type Error = String;

    fn try_from(value: wasmparser::ElementKind) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ElementKind::Passive => Ok(ElemMode::Passive),
            wasmparser::ElementKind::Active {
                table_index,
                offset_expr,
            } => Ok(ElemMode::Active {
                table_idx: table_index.unwrap_or(0),
                offset: offset_expr.get_operators_reader().try_into()?,
            }),
            wasmparser::ElementKind::Declared => Ok(ElemMode::Declarative),
        }
    }
}

impl<T: TryFromExprSource> TryFrom<wasmparser::Element<'_>> for Elem<T> {
    type Error = String;

    fn try_from(value: wasmparser::Element) -> Result<Self, Self::Error> {
        let r: Result<(RefType, Vec<T>), String> = match value.items {
            wasmparser::ElementItems::Functions(indices) => {
                let mut init = Vec::new();
                for func_idx in indices {
                    let func_idx = func_idx
                        .map_err(|e| format!("Error parsing core module element: {:?}", e))?;
                    let expr_source = RefFuncExprSource::new(func_idx);
                    init.push(T::try_from(expr_source)?);
                }
                Ok((RefType::FuncRef, init))
            }
            wasmparser::ElementItems::Expressions(ref_type, exprs) => {
                let mut init = Vec::new();
                for expr in exprs {
                    let expr =
                        expr.map_err(|e| format!("Error parsing core module element: {:?}", e))?;
                    let expr_source = OperatorsReaderExprSource::new(expr.get_operators_reader());
                    let expr: T = T::try_from(expr_source)?;
                    init.push(expr);
                }
                Ok((ref_type.try_into()?, init))
            }
        };
        let (ref_type, init) = r?;

        Ok(Elem {
            ref_type,
            mode: value.kind.try_into()?,
            init,
        })
    }
}

impl<T: TryFromExprSource + Debug + Clone + PartialEq> TryFrom<wasmparser::DataKind<'_>>
    for DataMode<T>
{
    type Error = String;

    fn try_from(value: wasmparser::DataKind) -> Result<Self, Self::Error> {
        match value {
            wasmparser::DataKind::Passive => Ok(DataMode::Passive),
            wasmparser::DataKind::Active {
                memory_index,
                offset_expr,
            } => {
                let operators_reader =
                    OperatorsReaderExprSource::new(offset_expr.get_operators_reader());
                Ok(DataMode::Active {
                    memory: memory_index,
                    offset: T::try_from(operators_reader)?,
                })
            }
        }
    }
}

impl<T: TryFromExprSource + Debug + Clone + PartialEq> TryFrom<wasmparser::Data<'_>> for Data<T> {
    type Error = String;

    fn try_from(value: wasmparser::Data) -> Result<Self, Self::Error> {
        Ok(Data {
            init: value.data.to_vec(),
            mode: value.kind.try_into()?,
        })
    }
}

impl<T: TryFromExprSource> TryFrom<wasmparser::FunctionBody<'_>> for FuncCode<T> {
    type Error = String;

    fn try_from(value: wasmparser::FunctionBody) -> Result<Self, Self::Error> {
        let mut locals = Vec::new();

        for local_groups in value
            .get_locals_reader()
            .map_err(|e| format!("Error parsing core module function body: {:?}", e))?
        {
            let (count, val_type) = local_groups
                .map_err(|e| format!("Error parsing core module function body: {:?}", e))?;
            let val_type: ValType = val_type.try_into()?;
            for _ in 0..count {
                locals.push(val_type.clone());
            }
        }

        let expr_source = OperatorsReaderExprSource::new(
            value
                .get_operators_reader()
                .map_err(|e| format!("Error parsing core module function body: {:?}", e))?,
        );
        let body: T = T::try_from(expr_source)?;

        Ok(FuncCode { locals, body })
    }
}

enum OperatorTarget {
    TopLevel(Vec<Instr>),
    Block(BlockType, Vec<Instr>),
    Loop(BlockType, Vec<Instr>),
    If(BlockType, Vec<Instr>),
    Else(BlockType, Vec<Instr>, Vec<Instr>),
}

impl OperatorTarget {
    pub fn push(&mut self, instr: Instr) {
        match self {
            OperatorTarget::TopLevel(instructions) => instructions.push(instr),
            OperatorTarget::Block(_, instructions) => instructions.push(instr),
            OperatorTarget::Loop(_, instructions) => instructions.push(instr),
            OperatorTarget::If(_, instructions) => instructions.push(instr),
            OperatorTarget::Else(_, _, instructions) => instructions.push(instr),
        }
    }
}

impl TryFrom<OperatorsReader<'_>> for Expr {
    type Error = String;

    fn try_from(value: OperatorsReader) -> Result<Self, Self::Error> {
        let mut stack = vec![OperatorTarget::TopLevel(Vec::new())];

        for op in value {
            let op = op.map_err(|e| format!("Error parsing core module instruction: {:?}", e))?;

            let instr = match op {
                Operator::Unreachable => Some(Instr::Unreachable),
                Operator::Nop => Some(Instr::Nop),
                Operator::Block { blockty } => {
                    let block_type = blockty.try_into()?;
                    stack.push(OperatorTarget::Block(block_type, Vec::new()));
                    None
                }
                Operator::Loop { blockty } => {
                    let block_type = blockty.try_into()?;
                    stack.push(OperatorTarget::Loop(block_type, Vec::new()));
                    None
                }
                Operator::If { blockty } => {
                    let block_type = blockty.try_into()?;
                    stack.push(OperatorTarget::If(block_type, Vec::new()));
                    None
                }
                Operator::Else => {
                    match stack.pop() {
                        Some(OperatorTarget::If(block_type, true_instrs)) => {
                            stack.push(OperatorTarget::Else(block_type, true_instrs, Vec::new()));
                        }
                        _ => {
                            return Err(
                                "Else operator must be preceded by an if operator".to_string()
                            );
                        }
                    }
                    None
                }
                Operator::Try { .. } => {
                    return Err("Exception handling proposal is not supported".to_string());
                }
                Operator::Catch { .. } => {
                    return Err("Exception handling proposal is not supported".to_string());
                }
                Operator::Throw { .. } => {
                    return Err("Exception handling proposal is not supported".to_string());
                }
                Operator::Rethrow { .. } => {
                    return Err("Exception handling proposal is not supported".to_string());
                }
                Operator::End => match stack.pop() {
                    Some(OperatorTarget::Block(block_type, instrs)) => {
                        Some(Instr::Block(block_type, instrs))
                    }
                    Some(OperatorTarget::Loop(block_type, instrs)) => {
                        Some(Instr::Loop(block_type, instrs))
                    }
                    Some(OperatorTarget::If(block_type, true_instrs)) => {
                        Some(Instr::If(block_type, true_instrs, Vec::new()))
                    }
                    Some(OperatorTarget::Else(block_type, true_instrs, false_instrs)) => {
                        Some(Instr::If(block_type, true_instrs, false_instrs))
                    }
                    Some(OperatorTarget::TopLevel(instrs)) => {
                        stack.push(OperatorTarget::TopLevel(instrs));
                        None
                    }
                    None => {
                        return Err(
                            "End operator must be preceded by a block, loop, or if operator"
                                .to_string(),
                        );
                    }
                },
                Operator::Br { relative_depth } => Some(Instr::Br(relative_depth)),
                Operator::BrIf { relative_depth } => Some(Instr::BrIf(relative_depth)),
                Operator::BrTable { targets } => {
                    let labels: Vec<LabelIdx> = targets
                        .targets()
                        .map(|r| {
                            r.map_err(|err| format!("Failed to read brtable labels: {:?}", err))
                        })
                        .collect::<Result<Vec<LabelIdx>, String>>()?;
                    Some(Instr::BrTable(labels, targets.default()))
                }
                Operator::Return => Some(Instr::Return),
                Operator::Call { function_index } => Some(Instr::Call(function_index)),
                Operator::CallIndirect {
                    type_index,
                    table_index,
                    ..
                } => Some(Instr::CallIndirect(table_index, type_index)),
                Operator::ReturnCall { .. } => {
                    return Err("Tail call proposal is not supported".to_string());
                }
                Operator::ReturnCallIndirect { .. } => {
                    return Err("Tail call proposal is not supported".to_string());
                }
                Operator::Delegate { .. } => {
                    return Err("Exception handling proposal is not supported".to_string());
                }
                Operator::CatchAll => {
                    return Err("Exception handling proposal is not supported".to_string());
                }
                Operator::Drop => Some(Instr::Drop),
                Operator::Select => Some(Instr::Select(None)),
                Operator::TypedSelect { ty } => Some(Instr::Select(Some(vec![ty.try_into()?]))),
                Operator::LocalGet { local_index } => Some(Instr::LocalGet(local_index)),
                Operator::LocalSet { local_index } => Some(Instr::LocalSet(local_index)),
                Operator::LocalTee { local_index } => Some(Instr::LocalTee(local_index)),
                Operator::GlobalGet { global_index } => Some(Instr::GlobalGet(global_index)),
                Operator::GlobalSet { global_index } => Some(Instr::GlobalSet(global_index)),
                Operator::I32Load { memarg } => Some(Instr::Load(
                    NumOrVecType::Num(NumType::I32),
                    memarg.try_into()?,
                )),
                Operator::I64Load { memarg } => Some(Instr::Load(
                    NumOrVecType::Num(NumType::I64),
                    memarg.try_into()?,
                )),
                Operator::F32Load { memarg } => Some(Instr::Load(
                    NumOrVecType::Num(NumType::F32),
                    memarg.try_into()?,
                )),
                Operator::F64Load { memarg } => Some(Instr::Load(
                    NumOrVecType::Num(NumType::F64),
                    memarg.try_into()?,
                )),
                Operator::I32Load8S { memarg } => Some(Instr::Load8(
                    NumType::I32,
                    Signedness::Signed,
                    memarg.try_into()?,
                )),
                Operator::I32Load8U { memarg } => Some(Instr::Load8(
                    NumType::I32,
                    Signedness::Unsigned,
                    memarg.try_into()?,
                )),
                Operator::I32Load16S { memarg } => Some(Instr::Load16(
                    NumType::I32,
                    Signedness::Signed,
                    memarg.try_into()?,
                )),
                Operator::I32Load16U { memarg } => Some(Instr::Load16(
                    NumType::I32,
                    Signedness::Unsigned,
                    memarg.try_into()?,
                )),
                Operator::I64Load8S { memarg } => Some(Instr::Load8(
                    NumType::I64,
                    Signedness::Signed,
                    memarg.try_into()?,
                )),
                Operator::I64Load8U { memarg } => Some(Instr::Load8(
                    NumType::I64,
                    Signedness::Unsigned,
                    memarg.try_into()?,
                )),
                Operator::I64Load16S { memarg } => Some(Instr::Load16(
                    NumType::I64,
                    Signedness::Signed,
                    memarg.try_into()?,
                )),
                Operator::I64Load16U { memarg } => Some(Instr::Load16(
                    NumType::I64,
                    Signedness::Unsigned,
                    memarg.try_into()?,
                )),
                Operator::I64Load32S { memarg } => {
                    Some(Instr::Load32(Signedness::Signed, memarg.try_into()?))
                }
                Operator::I64Load32U { memarg } => {
                    Some(Instr::Load32(Signedness::Unsigned, memarg.try_into()?))
                }
                Operator::I32Store { memarg } => Some(Instr::Store(
                    NumOrVecType::Num(NumType::I32),
                    memarg.try_into()?,
                )),
                Operator::I64Store { memarg } => Some(Instr::Store(
                    NumOrVecType::Num(NumType::I64),
                    memarg.try_into()?,
                )),
                Operator::F32Store { memarg } => Some(Instr::Store(
                    NumOrVecType::Num(NumType::F32),
                    memarg.try_into()?,
                )),
                Operator::F64Store { memarg } => Some(Instr::Store(
                    NumOrVecType::Num(NumType::F64),
                    memarg.try_into()?,
                )),
                Operator::I32Store8 { memarg } => {
                    Some(Instr::Store8(NumType::I32, memarg.try_into()?))
                }
                Operator::I32Store16 { memarg } => {
                    Some(Instr::Store16(NumType::I32, memarg.try_into()?))
                }
                Operator::I64Store8 { memarg } => {
                    Some(Instr::Store8(NumType::I64, memarg.try_into()?))
                }
                Operator::I64Store16 { memarg } => {
                    Some(Instr::Store16(NumType::I64, memarg.try_into()?))
                }
                Operator::I64Store32 { memarg } => Some(Instr::Store32(memarg.try_into()?)),
                Operator::MemorySize { .. } => Some(Instr::MemorySize),
                Operator::MemoryGrow { .. } => Some(Instr::MemoryGrow),
                Operator::I32Const { value } => Some(Instr::I32Const(value)),
                Operator::I64Const { value } => Some(Instr::I64Const(value)),
                Operator::F32Const { value } => Some(Instr::F32Const(f32::from_bits(value.bits()))),
                Operator::F64Const { value } => Some(Instr::F64Const(f64::from_bits(value.bits()))),
                Operator::RefNull { hty } => Some(Instr::RefNull(hty.try_into()?)),
                Operator::RefIsNull => Some(Instr::RefIsNull),
                Operator::RefFunc { function_index } => Some(Instr::RefFunc(function_index)),
                Operator::I32Eqz => Some(Instr::IEqz(IntWidth::I32)),
                Operator::I32Eq => Some(Instr::IEq(IntWidth::I32)),
                Operator::I32Ne => Some(Instr::INe(IntWidth::I32)),
                Operator::I32LtS => Some(Instr::ILt(IntWidth::I32, Signedness::Signed)),
                Operator::I32LtU => Some(Instr::ILt(IntWidth::I32, Signedness::Unsigned)),
                Operator::I32GtS => Some(Instr::IGt(IntWidth::I32, Signedness::Signed)),
                Operator::I32GtU => Some(Instr::IGt(IntWidth::I32, Signedness::Unsigned)),
                Operator::I32LeS => Some(Instr::ILe(IntWidth::I32, Signedness::Signed)),
                Operator::I32LeU => Some(Instr::ILe(IntWidth::I32, Signedness::Unsigned)),
                Operator::I32GeS => Some(Instr::IGe(IntWidth::I32, Signedness::Signed)),
                Operator::I32GeU => Some(Instr::IGe(IntWidth::I32, Signedness::Unsigned)),
                Operator::I64Eqz => Some(Instr::IEqz(IntWidth::I64)),
                Operator::I64Eq => Some(Instr::IEq(IntWidth::I64)),
                Operator::I64Ne => Some(Instr::INe(IntWidth::I64)),
                Operator::I64LtS => Some(Instr::ILt(IntWidth::I64, Signedness::Signed)),
                Operator::I64LtU => Some(Instr::ILt(IntWidth::I64, Signedness::Unsigned)),
                Operator::I64GtS => Some(Instr::IGt(IntWidth::I64, Signedness::Signed)),
                Operator::I64GtU => Some(Instr::IGt(IntWidth::I64, Signedness::Unsigned)),
                Operator::I64LeS => Some(Instr::ILe(IntWidth::I64, Signedness::Signed)),
                Operator::I64LeU => Some(Instr::ILe(IntWidth::I64, Signedness::Unsigned)),
                Operator::I64GeS => Some(Instr::IGe(IntWidth::I64, Signedness::Signed)),
                Operator::I64GeU => Some(Instr::IGe(IntWidth::I64, Signedness::Unsigned)),
                Operator::F32Eq => Some(Instr::FEq(FloatWidth::F32)),
                Operator::F32Ne => Some(Instr::FNe(FloatWidth::F32)),
                Operator::F32Lt => Some(Instr::FLt(FloatWidth::F32)),
                Operator::F32Gt => Some(Instr::FGt(FloatWidth::F32)),
                Operator::F32Le => Some(Instr::FLe(FloatWidth::F32)),
                Operator::F32Ge => Some(Instr::FGe(FloatWidth::F32)),
                Operator::F64Eq => Some(Instr::FEq(FloatWidth::F64)),
                Operator::F64Ne => Some(Instr::FNe(FloatWidth::F64)),
                Operator::F64Lt => Some(Instr::FLt(FloatWidth::F64)),
                Operator::F64Gt => Some(Instr::FGt(FloatWidth::F64)),
                Operator::F64Le => Some(Instr::FLe(FloatWidth::F64)),
                Operator::F64Ge => Some(Instr::FGe(FloatWidth::F64)),
                Operator::I32Clz => Some(Instr::IClz(IntWidth::I32)),
                Operator::I32Ctz => Some(Instr::ICtz(IntWidth::I32)),
                Operator::I32Popcnt => Some(Instr::IPopCnt(IntWidth::I32)),
                Operator::I32Add => Some(Instr::IAdd(IntWidth::I32)),
                Operator::I32Sub => Some(Instr::ISub(IntWidth::I32)),
                Operator::I32Mul => Some(Instr::IMul(IntWidth::I32)),
                Operator::I32DivS => Some(Instr::IDiv(IntWidth::I32, Signedness::Signed)),
                Operator::I32DivU => Some(Instr::IDiv(IntWidth::I32, Signedness::Unsigned)),
                Operator::I32RemS => Some(Instr::IRem(IntWidth::I32, Signedness::Signed)),
                Operator::I32RemU => Some(Instr::IRem(IntWidth::I32, Signedness::Unsigned)),
                Operator::I32And => Some(Instr::IAnd(IntWidth::I32)),
                Operator::I32Or => Some(Instr::IOr(IntWidth::I32)),
                Operator::I32Xor => Some(Instr::IXor(IntWidth::I32)),
                Operator::I32Shl => Some(Instr::IShl(IntWidth::I32)),
                Operator::I32ShrS => Some(Instr::IShr(IntWidth::I32, Signedness::Signed)),
                Operator::I32ShrU => Some(Instr::IShr(IntWidth::I32, Signedness::Unsigned)),
                Operator::I32Rotl => Some(Instr::IRotL(IntWidth::I32)),
                Operator::I32Rotr => Some(Instr::IRotR(IntWidth::I32)),
                Operator::I64Clz => Some(Instr::IClz(IntWidth::I64)),
                Operator::I64Ctz => Some(Instr::ICtz(IntWidth::I64)),
                Operator::I64Popcnt => Some(Instr::IPopCnt(IntWidth::I64)),
                Operator::I64Add => Some(Instr::IAdd(IntWidth::I64)),
                Operator::I64Sub => Some(Instr::ISub(IntWidth::I64)),
                Operator::I64Mul => Some(Instr::IMul(IntWidth::I64)),
                Operator::I64DivS => Some(Instr::IDiv(IntWidth::I64, Signedness::Signed)),
                Operator::I64DivU => Some(Instr::IDiv(IntWidth::I64, Signedness::Unsigned)),
                Operator::I64RemS => Some(Instr::IRem(IntWidth::I64, Signedness::Signed)),
                Operator::I64RemU => Some(Instr::IRem(IntWidth::I64, Signedness::Unsigned)),
                Operator::I64And => Some(Instr::IAnd(IntWidth::I64)),
                Operator::I64Or => Some(Instr::IOr(IntWidth::I64)),
                Operator::I64Xor => Some(Instr::IXor(IntWidth::I64)),
                Operator::I64Shl => Some(Instr::IShl(IntWidth::I64)),
                Operator::I64ShrS => Some(Instr::IShr(IntWidth::I64, Signedness::Signed)),
                Operator::I64ShrU => Some(Instr::IShr(IntWidth::I64, Signedness::Unsigned)),
                Operator::I64Rotl => Some(Instr::IRotL(IntWidth::I64)),
                Operator::I64Rotr => Some(Instr::IRotR(IntWidth::I64)),
                Operator::F32Abs => Some(Instr::FAbs(FloatWidth::F32)),
                Operator::F32Neg => Some(Instr::FNeg(FloatWidth::F32)),
                Operator::F32Ceil => Some(Instr::FCeil(FloatWidth::F32)),
                Operator::F32Floor => Some(Instr::FFloor(FloatWidth::F32)),
                Operator::F32Trunc => Some(Instr::FTrunc(FloatWidth::F32)),
                Operator::F32Nearest => Some(Instr::FNearest(FloatWidth::F32)),
                Operator::F32Sqrt => Some(Instr::FSqrt(FloatWidth::F32)),
                Operator::F32Add => Some(Instr::FAdd(FloatWidth::F32)),
                Operator::F32Sub => Some(Instr::FSub(FloatWidth::F32)),
                Operator::F32Mul => Some(Instr::FMul(FloatWidth::F32)),
                Operator::F32Div => Some(Instr::FDiv(FloatWidth::F32)),
                Operator::F32Min => Some(Instr::FMin(FloatWidth::F32)),
                Operator::F32Max => Some(Instr::FMax(FloatWidth::F32)),
                Operator::F32Copysign => Some(Instr::FCopySign(FloatWidth::F32)),
                Operator::F64Abs => Some(Instr::FAbs(FloatWidth::F64)),
                Operator::F64Neg => Some(Instr::FNeg(FloatWidth::F64)),
                Operator::F64Ceil => Some(Instr::FCeil(FloatWidth::F64)),
                Operator::F64Floor => Some(Instr::FFloor(FloatWidth::F64)),
                Operator::F64Trunc => Some(Instr::FTrunc(FloatWidth::F64)),
                Operator::F64Nearest => Some(Instr::FNearest(FloatWidth::F64)),
                Operator::F64Sqrt => Some(Instr::FSqrt(FloatWidth::F64)),
                Operator::F64Add => Some(Instr::FAdd(FloatWidth::F64)),
                Operator::F64Sub => Some(Instr::FSub(FloatWidth::F64)),
                Operator::F64Mul => Some(Instr::FMul(FloatWidth::F64)),
                Operator::F64Div => Some(Instr::FDiv(FloatWidth::F64)),
                Operator::F64Min => Some(Instr::FMin(FloatWidth::F64)),
                Operator::F64Max => Some(Instr::FMax(FloatWidth::F64)),
                Operator::F64Copysign => Some(Instr::FCopySign(FloatWidth::F64)),
                Operator::I32WrapI64 => Some(Instr::I32WrapI64),
                Operator::I32TruncF32S => Some(Instr::ITruncF(
                    IntWidth::I32,
                    FloatWidth::F32,
                    Signedness::Signed,
                )),
                Operator::I32TruncF32U => Some(Instr::ITruncF(
                    IntWidth::I32,
                    FloatWidth::F32,
                    Signedness::Unsigned,
                )),
                Operator::I32TruncF64S => Some(Instr::ITruncF(
                    IntWidth::I32,
                    FloatWidth::F64,
                    Signedness::Signed,
                )),
                Operator::I32TruncF64U => Some(Instr::ITruncF(
                    IntWidth::I32,
                    FloatWidth::F64,
                    Signedness::Unsigned,
                )),
                Operator::I64ExtendI32S => Some(Instr::I64ExtendI32(Signedness::Signed)),
                Operator::I64ExtendI32U => Some(Instr::I64ExtendI32(Signedness::Unsigned)),
                Operator::I64TruncF32S => Some(Instr::ITruncF(
                    IntWidth::I64,
                    FloatWidth::F32,
                    Signedness::Signed,
                )),
                Operator::I64TruncF32U => Some(Instr::ITruncF(
                    IntWidth::I64,
                    FloatWidth::F32,
                    Signedness::Unsigned,
                )),
                Operator::I64TruncF64S => Some(Instr::ITruncF(
                    IntWidth::I64,
                    FloatWidth::F64,
                    Signedness::Signed,
                )),
                Operator::I64TruncF64U => Some(Instr::ITruncF(
                    IntWidth::I64,
                    FloatWidth::F64,
                    Signedness::Unsigned,
                )),
                Operator::F32ConvertI32S => Some(Instr::FConvertI(
                    FloatWidth::F32,
                    IntWidth::I32,
                    Signedness::Signed,
                )),
                Operator::F32ConvertI32U => Some(Instr::FConvertI(
                    FloatWidth::F32,
                    IntWidth::I32,
                    Signedness::Unsigned,
                )),
                Operator::F32ConvertI64S => Some(Instr::FConvertI(
                    FloatWidth::F32,
                    IntWidth::I64,
                    Signedness::Signed,
                )),
                Operator::F32ConvertI64U => Some(Instr::FConvertI(
                    FloatWidth::F32,
                    IntWidth::I64,
                    Signedness::Unsigned,
                )),
                Operator::F32DemoteF64 => Some(Instr::F32DemoteF64),
                Operator::F64ConvertI32S => Some(Instr::FConvertI(
                    FloatWidth::F64,
                    IntWidth::I32,
                    Signedness::Signed,
                )),
                Operator::F64ConvertI32U => Some(Instr::FConvertI(
                    FloatWidth::F64,
                    IntWidth::I32,
                    Signedness::Unsigned,
                )),
                Operator::F64ConvertI64S => Some(Instr::FConvertI(
                    FloatWidth::F64,
                    IntWidth::I64,
                    Signedness::Signed,
                )),
                Operator::F64ConvertI64U => Some(Instr::FConvertI(
                    FloatWidth::F64,
                    IntWidth::I64,
                    Signedness::Unsigned,
                )),
                Operator::F64PromoteF32 => Some(Instr::F64PromoteF32),
                Operator::I32ReinterpretF32 => Some(Instr::IReinterpretF(IntWidth::I32)),
                Operator::I64ReinterpretF64 => Some(Instr::IReinterpretF(IntWidth::I64)),
                Operator::F32ReinterpretI32 => Some(Instr::FReinterpretI(FloatWidth::F32)),
                Operator::F64ReinterpretI64 => Some(Instr::FReinterpretI(FloatWidth::F64)),
                Operator::I32Extend8S => Some(Instr::IExtend8S(IntWidth::I32)),
                Operator::I32Extend16S => Some(Instr::IExtend16S(IntWidth::I32)),
                Operator::I64Extend8S => Some(Instr::IExtend8S(IntWidth::I64)),
                Operator::I64Extend16S => Some(Instr::IExtend16S(IntWidth::I64)),
                Operator::I64Extend32S => Some(Instr::I64Extend32S),
                Operator::RefI31 => {
                    return Err("GC proposal is not supported".to_string());
                }
                Operator::I31GetS => {
                    return Err("GC proposal is not supported".to_string());
                }
                Operator::I31GetU => {
                    return Err("GC proposal is not supported".to_string());
                }
                Operator::I32TruncSatF32S => Some(Instr::ITruncSatF(
                    IntWidth::I32,
                    FloatWidth::F32,
                    Signedness::Signed,
                )),
                Operator::I32TruncSatF32U => Some(Instr::ITruncSatF(
                    IntWidth::I32,
                    FloatWidth::F32,
                    Signedness::Unsigned,
                )),
                Operator::I32TruncSatF64S => Some(Instr::ITruncSatF(
                    IntWidth::I32,
                    FloatWidth::F64,
                    Signedness::Signed,
                )),
                Operator::I32TruncSatF64U => Some(Instr::ITruncSatF(
                    IntWidth::I32,
                    FloatWidth::F64,
                    Signedness::Unsigned,
                )),
                Operator::I64TruncSatF32S => Some(Instr::ITruncSatF(
                    IntWidth::I64,
                    FloatWidth::F32,
                    Signedness::Signed,
                )),
                Operator::I64TruncSatF32U => Some(Instr::ITruncSatF(
                    IntWidth::I64,
                    FloatWidth::F32,
                    Signedness::Unsigned,
                )),
                Operator::I64TruncSatF64S => Some(Instr::ITruncSatF(
                    IntWidth::I64,
                    FloatWidth::F64,
                    Signedness::Signed,
                )),
                Operator::I64TruncSatF64U => Some(Instr::ITruncSatF(
                    IntWidth::I64,
                    FloatWidth::F64,
                    Signedness::Unsigned,
                )),
                Operator::MemoryInit { data_index, .. } => Some(Instr::MemoryInit(data_index)),
                Operator::DataDrop { data_index } => Some(Instr::DataDrop(data_index)),
                Operator::MemoryCopy { .. } => Some(Instr::MemoryCopy),
                Operator::MemoryFill { .. } => Some(Instr::MemoryFill),
                Operator::TableInit { elem_index, table } => {
                    Some(Instr::TableInit(table, elem_index))
                }
                Operator::ElemDrop { elem_index } => Some(Instr::ElemDrop(elem_index)),
                Operator::TableCopy {
                    dst_table,
                    src_table,
                } => Some(Instr::TableCopy {
                    source: src_table,
                    destination: dst_table,
                }),
                Operator::TableFill { table } => Some(Instr::TableFill(table)),
                Operator::TableGet { table } => Some(Instr::TableGet(table)),
                Operator::TableSet { table } => Some(Instr::TableSet(table)),
                Operator::TableGrow { table } => Some(Instr::TableGrow(table)),
                Operator::TableSize { table } => Some(Instr::TableSize(table)),
                Operator::MemoryDiscard { .. } => {
                    return Err(
                        "Fine grained control of memory proposal is not supported".to_string()
                    );
                }
                Operator::MemoryAtomicNotify { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::MemoryAtomicWait32 { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::MemoryAtomicWait64 { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::AtomicFence => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicLoad { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicLoad { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicLoad8U { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicLoad16U { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicLoad8U { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicLoad16U { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicLoad32U { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicStore { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicStore { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicStore8 { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicStore16 { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicStore8 { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicStore16 { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicStore32 { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmwAdd { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmwAdd { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw8AddU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw16AddU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw8AddU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw16AddU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw32AddU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmwSub { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmwSub { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw8SubU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw16SubU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw8SubU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw16SubU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw32SubU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmwAnd { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmwAnd { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw8AndU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw16AndU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw8AndU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw16AndU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw32AndU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmwOr { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmwOr { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw8OrU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw16OrU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw8OrU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw16OrU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw32OrU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmwXor { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmwXor { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw8XorU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw16XorU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw8XorU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw16XorU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw32XorU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmwXchg { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmwXchg { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw8XchgU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw16XchgU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw8XchgU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw16XchgU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw32XchgU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmwCmpxchg { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmwCmpxchg { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw8CmpxchgU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I32AtomicRmw16CmpxchgU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw8CmpxchgU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw16CmpxchgU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::I64AtomicRmw32CmpxchgU { .. } => {
                    return Err("Threads proposal is not supported".to_string());
                }
                Operator::V128Load { memarg } => Some(Instr::Load(
                    NumOrVecType::Vec(VecType::V128),
                    memarg.try_into()?,
                )),
                Operator::V128Load8x8S { memarg } => {
                    Some(Instr::V128Load8x8(Signedness::Signed, memarg.try_into()?))
                }
                Operator::V128Load8x8U { memarg } => {
                    Some(Instr::V128Load8x8(Signedness::Unsigned, memarg.try_into()?))
                }
                Operator::V128Load16x4S { memarg } => {
                    Some(Instr::V128Load16x4(Signedness::Signed, memarg.try_into()?))
                }
                Operator::V128Load16x4U { memarg } => Some(Instr::V128Load16x4(
                    Signedness::Unsigned,
                    memarg.try_into()?,
                )),
                Operator::V128Load32x2S { memarg } => {
                    Some(Instr::V128Load32x2(Signedness::Signed, memarg.try_into()?))
                }
                Operator::V128Load32x2U { memarg } => Some(Instr::V128Load32x2(
                    Signedness::Unsigned,
                    memarg.try_into()?,
                )),
                Operator::V128Load8Splat { memarg } => Some(Instr::V128LoadSplat(
                    VectorLoadShape::WW8,
                    memarg.try_into()?,
                )),
                Operator::V128Load16Splat { memarg } => Some(Instr::V128LoadSplat(
                    VectorLoadShape::WW16,
                    memarg.try_into()?,
                )),
                Operator::V128Load32Splat { memarg } => Some(Instr::V128LoadSplat(
                    VectorLoadShape::WW32,
                    memarg.try_into()?,
                )),
                Operator::V128Load64Splat { memarg } => Some(Instr::V128LoadSplat(
                    VectorLoadShape::WW64,
                    memarg.try_into()?,
                )),
                Operator::V128Load32Zero { memarg } => {
                    Some(Instr::V128Load32Zero(memarg.try_into()?))
                }
                Operator::V128Load64Zero { memarg } => {
                    Some(Instr::V128Load64Zero(memarg.try_into()?))
                }
                Operator::V128Store { memarg } => Some(Instr::Store(
                    NumOrVecType::Vec(VecType::V128),
                    memarg.try_into()?,
                )),
                Operator::V128Load8Lane { memarg, lane } => Some(Instr::V128LoadLane(
                    VectorLoadShape::WW8,
                    memarg.try_into()?,
                    lane,
                )),
                Operator::V128Load16Lane { memarg, lane } => Some(Instr::V128LoadLane(
                    VectorLoadShape::WW16,
                    memarg.try_into()?,
                    lane,
                )),
                Operator::V128Load32Lane { memarg, lane } => Some(Instr::V128LoadLane(
                    VectorLoadShape::WW32,
                    memarg.try_into()?,
                    lane,
                )),
                Operator::V128Load64Lane { memarg, lane } => Some(Instr::V128LoadLane(
                    VectorLoadShape::WW64,
                    memarg.try_into()?,
                    lane,
                )),
                Operator::V128Store8Lane { memarg, lane } => Some(Instr::V128StoreLane(
                    VectorLoadShape::WW8,
                    memarg.try_into()?,
                    lane,
                )),
                Operator::V128Store16Lane { memarg, lane } => Some(Instr::V128StoreLane(
                    VectorLoadShape::WW16,
                    memarg.try_into()?,
                    lane,
                )),
                Operator::V128Store32Lane { memarg, lane } => Some(Instr::V128StoreLane(
                    VectorLoadShape::WW32,
                    memarg.try_into()?,
                    lane,
                )),
                Operator::V128Store64Lane { memarg, lane } => Some(Instr::V128StoreLane(
                    VectorLoadShape::WW64,
                    memarg.try_into()?,
                    lane,
                )),
                Operator::V128Const { value } => Some(Instr::V128Const(value.i128())),
                Operator::I8x16Shuffle { lanes } => Some(Instr::VI8x16Shuffle(lanes)),
                Operator::I8x16ExtractLaneS { lane } => {
                    Some(Instr::VI8x16ExtractLane(Signedness::Signed, lane))
                }
                Operator::I8x16ExtractLaneU { lane } => {
                    Some(Instr::VI8x16ExtractLane(Signedness::Unsigned, lane))
                }
                Operator::I8x16ReplaceLane { lane } => {
                    Some(Instr::VReplaceLane(Shape::Int(IShape::I8x16), lane))
                }
                Operator::I16x8ExtractLaneS { lane } => {
                    Some(Instr::VI16x8ExtractLane(Signedness::Signed, lane))
                }
                Operator::I16x8ExtractLaneU { lane } => {
                    Some(Instr::VI16x8ExtractLane(Signedness::Unsigned, lane))
                }
                Operator::I16x8ReplaceLane { lane } => {
                    Some(Instr::VReplaceLane(Shape::Int(IShape::I16x8), lane))
                }
                Operator::I32x4ExtractLane { lane } => Some(Instr::VI32x4ExtractLane(lane)),
                Operator::I32x4ReplaceLane { lane } => {
                    Some(Instr::VReplaceLane(Shape::Int(IShape::I32x4), lane))
                }
                Operator::I64x2ExtractLane { lane } => Some(Instr::VI64x2ExtractLane(lane)),
                Operator::I64x2ReplaceLane { lane } => {
                    Some(Instr::VReplaceLane(Shape::Int(IShape::I64x2), lane))
                }
                Operator::F32x4ExtractLane { lane } => {
                    Some(Instr::VFExtractLane(FShape::F32x4, lane))
                }
                Operator::F32x4ReplaceLane { lane } => {
                    Some(Instr::VReplaceLane(Shape::Float(FShape::F32x4), lane))
                }
                Operator::F64x2ExtractLane { lane } => {
                    Some(Instr::VFExtractLane(FShape::F64x2, lane))
                }
                Operator::F64x2ReplaceLane { lane } => {
                    Some(Instr::VReplaceLane(Shape::Float(FShape::F64x2), lane))
                }
                Operator::I8x16Swizzle => Some(Instr::VI18x16Swizzle),
                Operator::I8x16Splat => Some(Instr::VSplat(Shape::Int(IShape::I8x16))),
                Operator::I16x8Splat => Some(Instr::VSplat(Shape::Int(IShape::I16x8))),
                Operator::I32x4Splat => Some(Instr::VSplat(Shape::Int(IShape::I32x4))),
                Operator::I64x2Splat => Some(Instr::VSplat(Shape::Int(IShape::I64x2))),
                Operator::F32x4Splat => Some(Instr::VSplat(Shape::Float(FShape::F32x4))),
                Operator::F64x2Splat => Some(Instr::VSplat(Shape::Float(FShape::F64x2))),
                Operator::I8x16Eq => Some(Instr::VIEq(IShape::I8x16)),
                Operator::I8x16Ne => Some(Instr::VIEq(IShape::I8x16)),
                Operator::I8x16LtS => Some(Instr::VILt(IShape::I8x16, Signedness::Signed)),
                Operator::I8x16LtU => Some(Instr::VILt(IShape::I8x16, Signedness::Unsigned)),
                Operator::I8x16GtS => Some(Instr::VIGt(IShape::I8x16, Signedness::Signed)),
                Operator::I8x16GtU => Some(Instr::VIGt(IShape::I8x16, Signedness::Unsigned)),
                Operator::I8x16LeS => Some(Instr::VILe(IShape::I8x16, Signedness::Signed)),
                Operator::I8x16LeU => Some(Instr::VILe(IShape::I8x16, Signedness::Unsigned)),
                Operator::I8x16GeS => Some(Instr::VIGe(IShape::I8x16, Signedness::Signed)),
                Operator::I8x16GeU => Some(Instr::VIGe(IShape::I8x16, Signedness::Unsigned)),
                Operator::I16x8Eq => Some(Instr::VIEq(IShape::I16x8)),
                Operator::I16x8Ne => Some(Instr::VIEq(IShape::I16x8)),
                Operator::I16x8LtS => Some(Instr::VILt(IShape::I16x8, Signedness::Signed)),
                Operator::I16x8LtU => Some(Instr::VILt(IShape::I16x8, Signedness::Unsigned)),
                Operator::I16x8GtS => Some(Instr::VIGt(IShape::I16x8, Signedness::Signed)),
                Operator::I16x8GtU => Some(Instr::VIGt(IShape::I16x8, Signedness::Unsigned)),
                Operator::I16x8LeS => Some(Instr::VILe(IShape::I16x8, Signedness::Signed)),
                Operator::I16x8LeU => Some(Instr::VILe(IShape::I16x8, Signedness::Unsigned)),
                Operator::I16x8GeS => Some(Instr::VIGe(IShape::I16x8, Signedness::Signed)),
                Operator::I16x8GeU => Some(Instr::VIGe(IShape::I16x8, Signedness::Unsigned)),
                Operator::I32x4Eq => Some(Instr::VIEq(IShape::I32x4)),
                Operator::I32x4Ne => Some(Instr::VIEq(IShape::I32x4)),
                Operator::I32x4LtS => Some(Instr::VILt(IShape::I32x4, Signedness::Signed)),
                Operator::I32x4LtU => Some(Instr::VILt(IShape::I32x4, Signedness::Unsigned)),
                Operator::I32x4GtS => Some(Instr::VIGt(IShape::I32x4, Signedness::Signed)),
                Operator::I32x4GtU => Some(Instr::VIGt(IShape::I32x4, Signedness::Unsigned)),
                Operator::I32x4LeS => Some(Instr::VILe(IShape::I32x4, Signedness::Signed)),
                Operator::I32x4LeU => Some(Instr::VILe(IShape::I32x4, Signedness::Unsigned)),
                Operator::I32x4GeS => Some(Instr::VIGe(IShape::I32x4, Signedness::Signed)),
                Operator::I32x4GeU => Some(Instr::VIGe(IShape::I32x4, Signedness::Unsigned)),
                Operator::I64x2Eq => Some(Instr::VIEq(IShape::I64x2)),
                Operator::I64x2Ne => Some(Instr::VIEq(IShape::I64x2)),
                Operator::I64x2LtS => Some(Instr::VILt(IShape::I64x2, Signedness::Signed)),
                Operator::I64x2GtS => Some(Instr::VIGt(IShape::I64x2, Signedness::Signed)),
                Operator::I64x2LeS => Some(Instr::VILe(IShape::I64x2, Signedness::Signed)),
                Operator::I64x2GeS => Some(Instr::VIGe(IShape::I64x2, Signedness::Signed)),
                Operator::F32x4Eq => Some(Instr::VFEq(FShape::F32x4)),
                Operator::F32x4Ne => Some(Instr::VFEq(FShape::F32x4)),
                Operator::F32x4Lt => Some(Instr::VFLt(FShape::F32x4)),
                Operator::F32x4Gt => Some(Instr::VFGt(FShape::F32x4)),
                Operator::F32x4Le => Some(Instr::VFLe(FShape::F32x4)),
                Operator::F32x4Ge => Some(Instr::VFGe(FShape::F32x4)),
                Operator::F64x2Eq => Some(Instr::VFEq(FShape::F64x2)),
                Operator::F64x2Ne => Some(Instr::VFEq(FShape::F64x2)),
                Operator::F64x2Lt => Some(Instr::VFLt(FShape::F64x2)),
                Operator::F64x2Gt => Some(Instr::VFGt(FShape::F64x2)),
                Operator::F64x2Le => Some(Instr::VFLe(FShape::F64x2)),
                Operator::F64x2Ge => Some(Instr::VFGe(FShape::F64x2)),
                Operator::V128Not => Some(Instr::V128Not),
                Operator::V128And => Some(Instr::V128And),
                Operator::V128AndNot => Some(Instr::V128AndNot),
                Operator::V128Or => Some(Instr::V128Or),
                Operator::V128Xor => Some(Instr::V128XOr),
                Operator::V128Bitselect => Some(Instr::V128BitSelect),
                Operator::V128AnyTrue => Some(Instr::V128AnyTrue),
                Operator::I8x16Abs => Some(Instr::VIAbs(IShape::I8x16)),
                Operator::I8x16Neg => Some(Instr::VINeg(IShape::I8x16)),
                Operator::I8x16Popcnt => Some(Instr::VI8x16PopCnt),
                Operator::I8x16AllTrue => Some(Instr::VIAllTrue(IShape::I8x16)),
                Operator::I8x16Bitmask => Some(Instr::VIBitMask(IShape::I8x16)),
                Operator::I8x16NarrowI16x8S => Some(Instr::VI8x16NarrowI16x8(Signedness::Signed)),
                Operator::I8x16NarrowI16x8U => Some(Instr::VI8x16NarrowI16x8(Signedness::Unsigned)),
                Operator::I8x16Shl => Some(Instr::VIShl(IShape::I8x16)),
                Operator::I8x16ShrS => Some(Instr::VIShr(IShape::I8x16, Signedness::Signed)),
                Operator::I8x16ShrU => Some(Instr::VIShr(IShape::I8x16, Signedness::Unsigned)),
                Operator::I8x16Add => Some(Instr::VIAdd(IShape::I8x16)),
                Operator::I8x16AddSatS => Some(Instr::VIAddSat(IShape::I8x16, Signedness::Signed)),
                Operator::I8x16AddSatU => {
                    Some(Instr::VIAddSat(IShape::I8x16, Signedness::Unsigned))
                }
                Operator::I8x16Sub => Some(Instr::VISub(IShape::I8x16)),
                Operator::I8x16SubSatS => Some(Instr::VISubSat(IShape::I8x16, Signedness::Signed)),
                Operator::I8x16SubSatU => {
                    Some(Instr::VISubSat(IShape::I8x16, Signedness::Unsigned))
                }
                Operator::I8x16MinS => Some(Instr::VIMin(IShape::I8x16, Signedness::Signed)),
                Operator::I8x16MinU => Some(Instr::VIMin(IShape::I8x16, Signedness::Unsigned)),
                Operator::I8x16MaxS => Some(Instr::VIMax(IShape::I8x16, Signedness::Signed)),
                Operator::I8x16MaxU => Some(Instr::VIMax(IShape::I8x16, Signedness::Unsigned)),
                Operator::I8x16AvgrU => Some(Instr::VIAvgr(IShape::I8x16)),
                Operator::I16x8ExtAddPairwiseI8x16S => {
                    Some(Instr::VIExtAddPairwise(IShape::I16x8, Signedness::Signed))
                }
                Operator::I16x8ExtAddPairwiseI8x16U => {
                    Some(Instr::VIExtAddPairwise(IShape::I16x8, Signedness::Unsigned))
                }
                Operator::I16x8Abs => Some(Instr::VIAbs(IShape::I16x8)),
                Operator::I16x8Neg => Some(Instr::VINeg(IShape::I16x8)),
                Operator::I16x8Q15MulrSatS => Some(Instr::VI16x8Q15MulrSat),
                Operator::I16x8AllTrue => Some(Instr::VIAllTrue(IShape::I16x8)),
                Operator::I16x8Bitmask => Some(Instr::VIBitMask(IShape::I16x8)),
                Operator::I16x8NarrowI32x4S => Some(Instr::VI16x8NarrowI32x4(Signedness::Signed)),
                Operator::I16x8NarrowI32x4U => Some(Instr::VI16x8NarrowI32x4(Signedness::Unsigned)),
                Operator::I16x8ExtendLowI8x16S => {
                    Some(Instr::VI16x8ExtendI8x16(Half::Low, Signedness::Signed))
                }
                Operator::I16x8ExtendHighI8x16S => {
                    Some(Instr::VI16x8ExtendI8x16(Half::High, Signedness::Signed))
                }
                Operator::I16x8ExtendLowI8x16U => {
                    Some(Instr::VI16x8ExtendI8x16(Half::Low, Signedness::Unsigned))
                }
                Operator::I16x8ExtendHighI8x16U => {
                    Some(Instr::VI16x8ExtendI8x16(Half::High, Signedness::Unsigned))
                }
                Operator::I16x8Shl => Some(Instr::VIShl(IShape::I16x8)),
                Operator::I16x8ShrS => Some(Instr::VIShr(IShape::I16x8, Signedness::Signed)),
                Operator::I16x8ShrU => Some(Instr::VIShr(IShape::I16x8, Signedness::Unsigned)),
                Operator::I16x8Add => Some(Instr::VIAdd(IShape::I16x8)),
                Operator::I16x8AddSatS => Some(Instr::VIAddSat(IShape::I16x8, Signedness::Signed)),
                Operator::I16x8AddSatU => {
                    Some(Instr::VIAddSat(IShape::I16x8, Signedness::Unsigned))
                }
                Operator::I16x8Sub => Some(Instr::VISub(IShape::I16x8)),
                Operator::I16x8SubSatS => Some(Instr::VISubSat(IShape::I16x8, Signedness::Signed)),
                Operator::I16x8SubSatU => {
                    Some(Instr::VISubSat(IShape::I16x8, Signedness::Unsigned))
                }
                Operator::I16x8Mul => Some(Instr::VIMul(IShape::I16x8)),
                Operator::I16x8MinS => Some(Instr::VIMin(IShape::I16x8, Signedness::Signed)),
                Operator::I16x8MinU => Some(Instr::VIMin(IShape::I16x8, Signedness::Unsigned)),
                Operator::I16x8MaxS => Some(Instr::VIMax(IShape::I16x8, Signedness::Signed)),
                Operator::I16x8MaxU => Some(Instr::VIMax(IShape::I16x8, Signedness::Unsigned)),
                Operator::I16x8AvgrU => Some(Instr::VIAvgr(IShape::I16x8)),
                Operator::I16x8ExtMulLowI8x16S => Some(Instr::VIExtMul(
                    IShape::I16x8,
                    Half::Low,
                    Signedness::Signed,
                )),
                Operator::I16x8ExtMulHighI8x16S => Some(Instr::VIExtMul(
                    IShape::I16x8,
                    Half::High,
                    Signedness::Signed,
                )),
                Operator::I16x8ExtMulLowI8x16U => Some(Instr::VIExtMul(
                    IShape::I16x8,
                    Half::Low,
                    Signedness::Unsigned,
                )),
                Operator::I16x8ExtMulHighI8x16U => Some(Instr::VIExtMul(
                    IShape::I16x8,
                    Half::High,
                    Signedness::Unsigned,
                )),
                Operator::I32x4ExtAddPairwiseI16x8S => {
                    Some(Instr::VIExtAddPairwise(IShape::I32x4, Signedness::Signed))
                }
                Operator::I32x4ExtAddPairwiseI16x8U => {
                    Some(Instr::VIExtAddPairwise(IShape::I32x4, Signedness::Unsigned))
                }
                Operator::I32x4Abs => Some(Instr::VIAbs(IShape::I32x4)),
                Operator::I32x4Neg => Some(Instr::VINeg(IShape::I32x4)),
                Operator::I32x4AllTrue => Some(Instr::VIAllTrue(IShape::I32x4)),
                Operator::I32x4Bitmask => Some(Instr::VIBitMask(IShape::I32x4)),
                Operator::I32x4ExtendLowI16x8S => {
                    Some(Instr::VI32x4ExtendI16x8(Half::Low, Signedness::Signed))
                }
                Operator::I32x4ExtendHighI16x8S => {
                    Some(Instr::VI32x4ExtendI16x8(Half::High, Signedness::Signed))
                }
                Operator::I32x4ExtendLowI16x8U => {
                    Some(Instr::VI32x4ExtendI16x8(Half::Low, Signedness::Unsigned))
                }
                Operator::I32x4ExtendHighI16x8U => {
                    Some(Instr::VI32x4ExtendI16x8(Half::High, Signedness::Unsigned))
                }
                Operator::I32x4Shl => Some(Instr::VIShl(IShape::I32x4)),
                Operator::I32x4ShrS => Some(Instr::VIShr(IShape::I32x4, Signedness::Signed)),
                Operator::I32x4ShrU => Some(Instr::VIShr(IShape::I32x4, Signedness::Unsigned)),
                Operator::I32x4Add => Some(Instr::VIAdd(IShape::I32x4)),
                Operator::I32x4Sub => Some(Instr::VISub(IShape::I32x4)),
                Operator::I32x4Mul => Some(Instr::VIMul(IShape::I32x4)),
                Operator::I32x4MinS => Some(Instr::VIMin(IShape::I32x4, Signedness::Signed)),
                Operator::I32x4MinU => Some(Instr::VIMin(IShape::I32x4, Signedness::Unsigned)),
                Operator::I32x4MaxS => Some(Instr::VIMax(IShape::I32x4, Signedness::Signed)),
                Operator::I32x4MaxU => Some(Instr::VIMax(IShape::I32x4, Signedness::Unsigned)),
                Operator::I32x4DotI16x8S => Some(Instr::VI32x4DotI16x8),
                Operator::I32x4ExtMulLowI16x8S => Some(Instr::VIExtMul(
                    IShape::I32x4,
                    Half::Low,
                    Signedness::Signed,
                )),
                Operator::I32x4ExtMulHighI16x8S => Some(Instr::VIExtMul(
                    IShape::I32x4,
                    Half::High,
                    Signedness::Signed,
                )),
                Operator::I32x4ExtMulLowI16x8U => Some(Instr::VIExtMul(
                    IShape::I32x4,
                    Half::Low,
                    Signedness::Unsigned,
                )),
                Operator::I32x4ExtMulHighI16x8U => Some(Instr::VIExtMul(
                    IShape::I32x4,
                    Half::High,
                    Signedness::Unsigned,
                )),
                Operator::I64x2Abs => Some(Instr::VIAbs(IShape::I64x2)),
                Operator::I64x2Neg => Some(Instr::VINeg(IShape::I64x2)),
                Operator::I64x2AllTrue => Some(Instr::VIAllTrue(IShape::I64x2)),
                Operator::I64x2Bitmask => Some(Instr::VIBitMask(IShape::I64x2)),
                Operator::I64x2ExtendLowI32x4S => {
                    Some(Instr::VI64x2ExtendI32x4(Half::Low, Signedness::Signed))
                }
                Operator::I64x2ExtendHighI32x4S => {
                    Some(Instr::VI64x2ExtendI32x4(Half::High, Signedness::Signed))
                }
                Operator::I64x2ExtendLowI32x4U => {
                    Some(Instr::VI64x2ExtendI32x4(Half::Low, Signedness::Unsigned))
                }
                Operator::I64x2ExtendHighI32x4U => {
                    Some(Instr::VI64x2ExtendI32x4(Half::High, Signedness::Unsigned))
                }
                Operator::I64x2Shl => Some(Instr::VIShl(IShape::I64x2)),
                Operator::I64x2ShrS => Some(Instr::VIShr(IShape::I64x2, Signedness::Signed)),
                Operator::I64x2ShrU => Some(Instr::VIShr(IShape::I64x2, Signedness::Unsigned)),
                Operator::I64x2Add => Some(Instr::VIAdd(IShape::I64x2)),
                Operator::I64x2Sub => Some(Instr::VISub(IShape::I64x2)),
                Operator::I64x2Mul => Some(Instr::VIMul(IShape::I64x2)),
                Operator::I64x2ExtMulLowI32x4S => Some(Instr::VIExtMul(
                    IShape::I64x2,
                    Half::Low,
                    Signedness::Signed,
                )),
                Operator::I64x2ExtMulHighI32x4S => Some(Instr::VIExtMul(
                    IShape::I64x2,
                    Half::High,
                    Signedness::Signed,
                )),
                Operator::I64x2ExtMulLowI32x4U => Some(Instr::VIExtMul(
                    IShape::I64x2,
                    Half::Low,
                    Signedness::Unsigned,
                )),
                Operator::I64x2ExtMulHighI32x4U => Some(Instr::VIExtMul(
                    IShape::I64x2,
                    Half::High,
                    Signedness::Unsigned,
                )),
                Operator::F32x4Ceil => Some(Instr::VFCeil(FShape::F32x4)),
                Operator::F32x4Floor => Some(Instr::VFFloor(FShape::F32x4)),
                Operator::F32x4Trunc => Some(Instr::VFTrunc(FShape::F32x4)),
                Operator::F32x4Nearest => Some(Instr::VFNearest(FShape::F32x4)),
                Operator::F32x4Abs => Some(Instr::VFAbs(FShape::F32x4)),
                Operator::F32x4Neg => Some(Instr::VFNeg(FShape::F32x4)),
                Operator::F32x4Sqrt => Some(Instr::VFSqrt(FShape::F32x4)),
                Operator::F32x4Add => Some(Instr::VFAdd(FShape::F32x4)),
                Operator::F32x4Sub => Some(Instr::VFSub(FShape::F32x4)),
                Operator::F32x4Mul => Some(Instr::VFMul(FShape::F32x4)),
                Operator::F32x4Div => Some(Instr::VFDiv(FShape::F32x4)),
                Operator::F32x4Min => Some(Instr::VFMin(FShape::F32x4)),
                Operator::F32x4Max => Some(Instr::VFMax(FShape::F32x4)),
                Operator::F32x4PMin => Some(Instr::VFPMin(FShape::F32x4)),
                Operator::F32x4PMax => Some(Instr::VFPMax(FShape::F32x4)),
                Operator::F64x2Ceil => Some(Instr::VFCeil(FShape::F64x2)),
                Operator::F64x2Floor => Some(Instr::VFFloor(FShape::F64x2)),
                Operator::F64x2Trunc => Some(Instr::VFTrunc(FShape::F64x2)),
                Operator::F64x2Nearest => Some(Instr::VFNearest(FShape::F64x2)),
                Operator::F64x2Abs => Some(Instr::VFAbs(FShape::F64x2)),
                Operator::F64x2Neg => Some(Instr::VFNeg(FShape::F64x2)),
                Operator::F64x2Sqrt => Some(Instr::VFSqrt(FShape::F64x2)),
                Operator::F64x2Add => Some(Instr::VFAdd(FShape::F64x2)),
                Operator::F64x2Sub => Some(Instr::VFSub(FShape::F64x2)),
                Operator::F64x2Mul => Some(Instr::VFMul(FShape::F64x2)),
                Operator::F64x2Div => Some(Instr::VFDiv(FShape::F64x2)),
                Operator::F64x2Min => Some(Instr::VFMin(FShape::F64x2)),
                Operator::F64x2Max => Some(Instr::VFMax(FShape::F64x2)),
                Operator::F64x2PMin => Some(Instr::VFPMin(FShape::F64x2)),
                Operator::F64x2PMax => Some(Instr::VFPMax(FShape::F64x2)),
                Operator::I32x4TruncSatF32x4S => Some(Instr::ITruncSatF(
                    IntWidth::I32,
                    FloatWidth::F32,
                    Signedness::Signed,
                )),
                Operator::I32x4TruncSatF32x4U => Some(Instr::ITruncSatF(
                    IntWidth::I32,
                    FloatWidth::F32,
                    Signedness::Unsigned,
                )),
                Operator::F32x4ConvertI32x4S => Some(Instr::FConvertI(
                    FloatWidth::F32,
                    IntWidth::I32,
                    Signedness::Signed,
                )),
                Operator::F32x4ConvertI32x4U => Some(Instr::FConvertI(
                    FloatWidth::F32,
                    IntWidth::I32,
                    Signedness::Unsigned,
                )),
                Operator::I32x4TruncSatF64x2SZero => Some(Instr::ITruncSatF(
                    IntWidth::I32,
                    FloatWidth::F64,
                    Signedness::Signed,
                )),
                Operator::I32x4TruncSatF64x2UZero => Some(Instr::ITruncSatF(
                    IntWidth::I32,
                    FloatWidth::F64,
                    Signedness::Unsigned,
                )),
                Operator::F64x2ConvertLowI32x4S => Some(Instr::FConvertI(
                    FloatWidth::F64,
                    IntWidth::I32,
                    Signedness::Signed,
                )),
                Operator::F64x2ConvertLowI32x4U => Some(Instr::FConvertI(
                    FloatWidth::F64,
                    IntWidth::I32,
                    Signedness::Unsigned,
                )),
                Operator::F32x4DemoteF64x2Zero => Some(Instr::F32DemoteF64),
                Operator::F64x2PromoteLowF32x4 => Some(Instr::F64PromoteF32),
                Operator::I8x16RelaxedSwizzle => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::I32x4RelaxedTruncF32x4S => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::I32x4RelaxedTruncF32x4U => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::I32x4RelaxedTruncF64x2SZero => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::I32x4RelaxedTruncF64x2UZero => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::F32x4RelaxedMadd => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::F32x4RelaxedNmadd => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::F64x2RelaxedMadd => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::F64x2RelaxedNmadd => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::I8x16RelaxedLaneselect => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::I16x8RelaxedLaneselect => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::I32x4RelaxedLaneselect => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::I64x2RelaxedLaneselect => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::F32x4RelaxedMin => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::F32x4RelaxedMax => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::F64x2RelaxedMin => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::F64x2RelaxedMax => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::I16x8RelaxedQ15mulrS => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::I16x8RelaxedDotI8x16I7x16S => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::I32x4RelaxedDotI8x16I7x16AddS => {
                    return Err("Relaxed SIMD instructions are not supported".to_string());
                }
                Operator::CallRef { .. } => {
                    return Err("Function Reference Types Proposal is not supported".to_string());
                }
                Operator::ReturnCallRef { .. } => {
                    return Err("Function Reference Types Proposal is not supported".to_string());
                }
                Operator::RefAsNonNull => {
                    return Err("Function Reference Types Proposal is not supported".to_string());
                }
                Operator::BrOnNull { .. } => {
                    return Err("Function Reference Types Proposal is not supported".to_string());
                }
                Operator::BrOnNonNull { .. } => {
                    return Err("Function Reference Types Proposal is not supported".to_string());
                }
                Operator::TryTable { .. } => {
                    return Err("Exception Handling Proposal is not supported".to_string());
                }
                Operator::ThrowRef { .. } => {
                    return Err("Exception Handling Proposal is not supported".to_string());
                }
                Operator::RefEq => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::StructNew { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::StructNewDefault { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::StructGet { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::StructGetS { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::StructGetU { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::StructSet { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayNew { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayNewDefault { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayNewFixed { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayNewData { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayNewElem { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayGet { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayGetS { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayGetU { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArraySet { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayLen => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayFill { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayCopy { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayInitData { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ArrayInitElem { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::RefTestNonNull { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::RefTestNullable { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::RefCastNonNull { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::RefCastNullable { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::BrOnCast { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::BrOnCastFail { .. } => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::AnyConvertExtern => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::ExternConvertAny => {
                    return Err("GC Proposal is not supported".to_string());
                }
                Operator::GlobalAtomicGet { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::GlobalAtomicSet { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::GlobalAtomicRmwAdd { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::GlobalAtomicRmwSub { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::GlobalAtomicRmwAnd { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::GlobalAtomicRmwOr { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::GlobalAtomicRmwXor { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::GlobalAtomicRmwXchg { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::GlobalAtomicRmwCmpxchg { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::TableAtomicGet { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::TableAtomicSet { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::TableAtomicRmwXchg { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::TableAtomicRmwCmpxchg { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::StructAtomicGet { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::StructAtomicGetS { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::StructAtomicGetU { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::StructAtomicSet { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::StructAtomicRmwAdd { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::StructAtomicRmwSub { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::StructAtomicRmwAnd { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::StructAtomicRmwOr { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::StructAtomicRmwXor { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::StructAtomicRmwXchg { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::StructAtomicRmwCmpxchg { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::ArrayAtomicGet { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::ArrayAtomicGetS { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::ArrayAtomicGetU { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::ArrayAtomicSet { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::ArrayAtomicRmwAdd { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::ArrayAtomicRmwSub { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::ArrayAtomicRmwAnd { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::ArrayAtomicRmwOr { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::ArrayAtomicRmwXor { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::ArrayAtomicRmwXchg { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::ArrayAtomicRmwCmpxchg { .. } => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::RefI31Shared => {
                    return Err("Shared Everything Threads proposal is not supported".to_string());
                }
                Operator::ContNew { .. } => {
                    return Err("Task Switching proposal is not supported".to_string());
                }
                Operator::ContBind { .. } => {
                    return Err("Task Switching proposal is not supported".to_string());
                }
                Operator::Suspend { .. } => {
                    return Err("Task Switching proposal is not supported".to_string());
                }
                Operator::Resume { .. } => {
                    return Err("Task Switching proposal is not supported".to_string());
                }
                Operator::ResumeThrow { .. } => {
                    return Err("Task Switching proposal is not supported".to_string());
                }
                Operator::Switch { .. } => {
                    return Err("Task Switching proposal is not supported".to_string());
                }
                Operator::I64Add128 => {
                    return Err("Wide Arithmetic proposal is not supported".to_string());
                }
                Operator::I64Sub128 => {
                    return Err("Wide Arithmetic proposal is not supported".to_string());
                }
                Operator::I64MulWideS => {
                    return Err("Wide Arithmetic proposal is not supported".to_string());
                }
                Operator::I64MulWideU => {
                    return Err("Wide Arithmetic proposal is not supported".to_string());
                }
                _ => return Err(format!("Unsupported operator: {op:?}")),
            };

            if let Some(instr) = instr {
                stack.last_mut().unwrap().push(instr);
            }
        }

        match stack.into_iter().next() {
            Some(OperatorTarget::TopLevel(instrs)) => Ok(Expr { instrs }),
            _ => Err("Unexpected stack state".to_string()),
        }
    }
}

impl<Ast> TryFrom<(Parser, &[u8])> for Sections<CoreIndexSpace, CoreSectionType, CoreSection<Ast>>
where
    Ast: AstCustomization,
    Ast::Expr: TryFromExprSource,
    Ast::Data: From<Data<Ast::Expr>>,
    Ast::Custom: From<Custom>,
{
    type Error = String;

    fn try_from(value: (Parser, &[u8])) -> Result<Self, Self::Error> {
        let (parser, data) = value;
        let mut sections = Vec::new();
        for payload in parser.parse_all(data) {
            let payload = payload.map_err(|e| format!("Error parsing core module: {:?}", e))?;
            match payload {
                Payload::Version { .. } => {}
                Payload::TypeSection(reader) => {
                    for tpe in reader.into_iter_err_on_gc_types() {
                        let tpe = tpe.map_err(|e| format!("Error parsing core module type section: {:?}", e))?;
                        sections.push(CoreSection::Type(tpe.try_into()?));
                    }
                }
                Payload::ImportSection(reader) => {
                    for import in reader {
                        let import = import.map_err(|e| format!("Error parsing core module import section: {:?}", e))?;
                        sections.push(CoreSection::Import(import.try_into()?))
                    }
                }
                Payload::FunctionSection(reader) => {
                    for function in reader {
                        let type_idx = function.map_err(|e| format!("Error parsing core module function section: {:?}", e))?;
                        sections.push(CoreSection::Func(FuncTypeRef { type_idx }));
                    }
                }
                Payload::TableSection(reader) => {
                    for table in reader {
                        let table = table.map_err(|e| format!("Error parsing core module table section: {:?}", e))?;
                        sections.push(CoreSection::Table(table.try_into()?))
                    }
                }
                Payload::MemorySection(reader) => {
                    for mem_type in reader {
                        let memory = mem_type.map_err(|e| format!("Error parsing core module memory section: {:?}", e))?;
                        sections.push(CoreSection::Mem(memory.try_into()?))
                    }
                }
                Payload::TagSection(_) =>
                    return Err("Unexpected tag section in core module; exception handling proposal is not supported".to_string()),
                Payload::GlobalSection(reader) => {
                    for global in reader {
                        let global = global.map_err(|e| format!("Error parsing core module global section: {:?}", e))?;
                        sections.push(CoreSection::Global(global.try_into()?))
                    }
                }
                Payload::ExportSection(reader) => {
                    for export in reader {
                        let export = export.map_err(|e| format!("Error parsing core module export section: {:?}", e))?;
                        sections.push(CoreSection::Export(export.try_into()?))
                    }
                }
                Payload::StartSection { func, .. } => {
                    sections.push(CoreSection::Start(Start { func }))
                }
                Payload::ElementSection(reader) => {
                    for element in reader {
                        let element = element.map_err(|e| format!("Error parsing core module element section: {:?}", e))?;
                        sections.push(CoreSection::Elem(element.try_into()?))
                    }
                }
                Payload::DataCountSection { count, .. } => {
                    sections.push(CoreSection::DataCount(DataCount { count }))
                }
                Payload::DataSection(reader) => {
                    for data in reader {
                        let data = data.map_err(|e| format!("Error parsing core module data section: {:?}", e))?;
                        let data: Data<Ast::Expr> = data.try_into()?;
                        sections.push(CoreSection::Data(data.into()));
                    }
                }
                Payload::CodeSectionStart { .. } => {
                    // this is just a marker that the next payload will be CodeSectionEntry
                }
                Payload::CodeSectionEntry(function_body) => {
                    sections.push(CoreSection::Code(function_body.try_into()?))
                }
                Payload::CustomSection(reader) => {
                    sections.push(CoreSection::Custom(Custom {
                        name: reader.name().to_string(),
                        data: reader.data().to_vec(),
                    }.into()))
                }
                Payload::End(_) => {}
                Payload::InstanceSection(_) =>
                    return Err("Unexpected component section in core module".to_string()),
                Payload::CoreTypeSection(_) =>
                    return Err("Unexpected component section in core module".to_string()),
                Payload::ModuleSection { .. } =>
                    return Err("Unexpected module section in core module".to_string()),
                Payload::ComponentSection { .. } =>
                    return Err("Unexpected component section in core module".to_string()),
                Payload::ComponentInstanceSection(_) =>
                    return Err("Unexpected component instance section in core module".to_string()),
                Payload::ComponentAliasSection(_) =>
                    return Err("Unexpected component alias section in core module".to_string()),
                Payload::ComponentTypeSection(_) =>
                    return Err("Unexpected component type section in core module".to_string()),
                Payload::ComponentCanonicalSection(_) =>
                    return Err("Unexpected component canonical section in core module".to_string()),
                Payload::ComponentStartSection { .. } =>
                    return Err("Unexpected component start section in core module".to_string()),
                Payload::ComponentImportSection(_) =>
                    return Err("Unexpected component import section in core module".to_string()),
                Payload::ComponentExportSection(_) =>
                    return Err("Unexpected component export section in core module".to_string()),
                Payload::UnknownSection { .. } =>
                    return Err("Unexpected unknown section in core module".to_string()),
                _ => return Err("Unexpected payload in core module".to_string()),
            }
        }
        Ok(Sections::from_flat(sections))
    }
}

impl<Ast> TryFrom<(Parser, &[u8])> for Module<Ast>
where
    Ast: AstCustomization,
    Ast::Expr: TryFromExprSource,
    Ast::Data: From<Data<Ast::Expr>>,
    Ast::Custom: From<Custom>,
{
    type Error = String;

    fn try_from(value: (Parser, &[u8])) -> Result<Self, Self::Error> {
        let sections =
            Sections::<CoreIndexSpace, CoreSectionType, CoreSection<Ast>>::try_from(value)?;
        Ok(sections.into())
    }
}

struct OperatorsReaderExprSource<'a> {
    reader: OperatorsReader<'a>,
}

impl<'a> OperatorsReaderExprSource<'a> {
    pub fn new(reader: OperatorsReader<'a>) -> Self {
        Self { reader }
    }
}

impl ExprSource for OperatorsReaderExprSource<'_> {
    fn unparsed(self) -> Result<Vec<u8>, String> {
        let binary_reader: BinaryReader = self.reader.get_binary_reader();
        let range = binary_reader.range();
        let bytes = self.reader.get_binary_reader().read_bytes(range.count());
        bytes
            .map_err(|e| format!("Error reading bytes from binary reader: {:?}", e))
            .map(|bytes| bytes.to_vec())
    }
}

impl IntoIterator for OperatorsReaderExprSource<'_> {
    type Item = Result<Instr, String>;
    type IntoIter = Box<dyn Iterator<Item = Result<Instr, String>>>;

    fn into_iter(self) -> Self::IntoIter {
        // TODO: parse incrementally
        let expr: Result<Expr, String> = self.reader.try_into();
        match expr {
            Err(err) => Box::new(vec![Err(err)].into_iter()),
            Ok(expr) => Box::new(expr.instrs.into_iter().map(Ok)),
        }
    }
}

struct RefFuncExprSource {
    func_idx: FuncIdx,
}

impl RefFuncExprSource {
    pub fn new(func_idx: FuncIdx) -> Self {
        Self { func_idx }
    }
}

impl ExprSource for RefFuncExprSource {
    fn unparsed(self) -> Result<Vec<u8>, String> {
        let mut result: Vec<u8> = Vec::new();
        result.write_all(&[0xd2u8]).unwrap();
        leb128::write::unsigned(&mut result, self.func_idx as u64).unwrap();
        result.write_all(&[0x0bu8]).unwrap();
        Ok(result)
    }
}

impl IntoIterator for RefFuncExprSource {
    type Item = Result<Instr, String>;
    type IntoIter = Box<dyn Iterator<Item = Result<Instr, String>>>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(vec![Instr::RefFunc(self.func_idx)].into_iter().map(Ok))
    }
}
