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
use std::borrow::Cow;

impl From<&RefType> for wasm_encoder::RefType {
    fn from(value: &RefType) -> Self {
        match value {
            RefType::FuncRef => wasm_encoder::RefType::EXTERNREF,
            RefType::ExternRef => wasm_encoder::RefType::EXTERNREF,
        }
    }
}

impl From<&ValType> for wasm_encoder::ValType {
    fn from(value: &ValType) -> Self {
        match value {
            ValType::Num(NumType::I32) => wasm_encoder::ValType::I32,
            ValType::Num(NumType::I64) => wasm_encoder::ValType::I64,
            ValType::Num(NumType::F32) => wasm_encoder::ValType::F32,
            ValType::Num(NumType::F64) => wasm_encoder::ValType::F64,
            ValType::Vec(VecType::V128) => wasm_encoder::ValType::V128,
            ValType::Ref(ref_type) => wasm_encoder::ValType::Ref(ref_type.into()),
        }
    }
}

impl From<&FuncType> for wasm_encoder::TypeSection {
    fn from(value: &FuncType) -> Self {
        let mut section = wasm_encoder::TypeSection::new();
        add_to_type_section(&mut section, value);
        section
    }
}

fn add_to_type_section(section: &mut wasm_encoder::TypeSection, value: &FuncType) {
    section.ty().function(
        value.input.values.iter().map(|v| v.into()),
        value.output.values.iter().map(|v| v.into()),
    );
}

impl From<&FuncTypeRef> for wasm_encoder::FunctionSection {
    fn from(value: &FuncTypeRef) -> Self {
        let mut section = wasm_encoder::FunctionSection::new();
        add_to_function_section(&mut section, value);
        section
    }
}

fn add_to_function_section(section: &mut wasm_encoder::FunctionSection, value: &FuncTypeRef) {
    section.function(value.type_idx);
}

impl<T: RetainsInstructions> TryFrom<&FuncCode<T>> for wasm_encoder::CodeSection {
    type Error = String;

    fn try_from(value: &FuncCode<T>) -> Result<Self, Self::Error> {
        let mut section = wasm_encoder::CodeSection::new();
        add_to_code_section(&mut section, value)?;
        Ok(section)
    }
}

fn add_to_code_section<T: RetainsInstructions>(
    section: &mut wasm_encoder::CodeSection,
    value: &FuncCode<T>,
) -> Result<(), String> {
    let mut function =
        wasm_encoder::Function::new_with_locals_types(value.locals.iter().map(|v| v.into()));
    encode_instructions(value.body.instructions(), &mut function)?;
    section.function(&function);
    Ok(())
}

impl From<&RefType> for wasm_encoder::HeapType {
    fn from(value: &RefType) -> Self {
        match value {
            RefType::ExternRef => wasm_encoder::HeapType::EXTERN,
            RefType::FuncRef => wasm_encoder::HeapType::FUNC,
        }
    }
}

impl From<&MemArg> for wasm_encoder::MemArg {
    fn from(value: &MemArg) -> Self {
        wasm_encoder::MemArg {
            offset: value.offset as u64,
            align: value.align as u32,
            memory_index: 0, // multi-memory proposal not supported
        }
    }
}

impl From<&BlockType> for wasm_encoder::BlockType {
    fn from(value: &BlockType) -> Self {
        match value {
            BlockType::None => wasm_encoder::BlockType::Empty,
            BlockType::Index(type_idx) => wasm_encoder::BlockType::FunctionType(*type_idx),
            BlockType::Value(val_type) => wasm_encoder::BlockType::Result(val_type.into()),
        }
    }
}

impl From<&TableType> for wasm_encoder::TableType {
    fn from(value: &TableType) -> Self {
        wasm_encoder::TableType {
            element_type: (&value.elements).into(),
            table64: false, // 64 bit tables are not supported yet
            minimum: value.limits.min,
            maximum: value.limits.max,
            shared: false, // shard-everything proposal is not supported yet
        }
    }
}

impl From<&Table> for wasm_encoder::TableSection {
    fn from(value: &Table) -> Self {
        let mut section = wasm_encoder::TableSection::new();
        add_to_table_section(&mut section, value);
        section
    }
}

fn add_to_table_section(section: &mut wasm_encoder::TableSection, value: &Table) {
    section.table((&value.table_type).into());
}

impl From<&MemType> for wasm_encoder::MemoryType {
    fn from(value: &MemType) -> Self {
        wasm_encoder::MemoryType {
            minimum: value.limits.min,
            maximum: value.limits.max,
            memory64: false,
            shared: false,
            page_size_log2: None, // custom-page-sizes proposal is not supported yet
        }
    }
}

impl From<&Mem> for wasm_encoder::MemorySection {
    fn from(value: &Mem) -> Self {
        let mut section = wasm_encoder::MemorySection::new();
        add_to_memory_section(&mut section, value);
        section
    }
}

fn add_to_memory_section(section: &mut wasm_encoder::MemorySection, value: &Mem) {
    section.memory((&value.mem_type).into());
}

impl TryFrom<&Expr> for wasm_encoder::ConstExpr {
    type Error = String;

    fn try_from(value: &Expr) -> Result<Self, Self::Error> {
        if value.instrs.len() != 1 {
            Err("Constant expression must consist of a single instruction".to_string())
        } else {
            match &value.instrs[0] {
                Instr::I32Const(value) => Ok(wasm_encoder::ConstExpr::i32_const(*value)),
                Instr::I64Const(value) => Ok(wasm_encoder::ConstExpr::i64_const(*value)),
                Instr::F32Const(value) => Ok(wasm_encoder::ConstExpr::f32_const((*value).into())),
                Instr::F64Const(value) => Ok(wasm_encoder::ConstExpr::f64_const((*value).into())),
                Instr::V128Const(value) => Ok(wasm_encoder::ConstExpr::v128_const(*value)),
                Instr::GlobalGet(global_idx) => {
                    Ok(wasm_encoder::ConstExpr::global_get(*global_idx))
                }
                Instr::RefNull(ref_type) => Ok(wasm_encoder::ConstExpr::ref_null(ref_type.into())),
                Instr::RefFunc(func_idx) => Ok(wasm_encoder::ConstExpr::ref_func(*func_idx)),
                _ => Err("Unsupported constant instruction".to_string()),
            }
        }
    }
}

impl From<&GlobalType> for wasm_encoder::GlobalType {
    fn from(value: &GlobalType) -> Self {
        wasm_encoder::GlobalType {
            val_type: (&value.val_type).into(),
            mutable: value.mutability == Mut::Var,
            shared: false, // shared-everything threads proposal is not supported yet
        }
    }
}

impl TryFrom<&Global> for wasm_encoder::GlobalSection {
    type Error = String;

    fn try_from(value: &Global) -> Result<Self, Self::Error> {
        let mut section = wasm_encoder::GlobalSection::new();
        add_to_global_section(&mut section, value)?;
        Ok(section)
    }
}

fn add_to_global_section(
    section: &mut wasm_encoder::GlobalSection,
    value: &Global,
) -> Result<(), String> {
    section.global((&value.global_type).into(), &(&value.init).try_into()?);
    Ok(())
}

impl<T: RetainsInstructions> TryFrom<&Elem<T>> for wasm_encoder::ElementSection {
    type Error = String;

    fn try_from(value: &Elem<T>) -> Result<Self, Self::Error> {
        let mut section = wasm_encoder::ElementSection::new();
        add_to_elem_section(&mut section, value)?;
        Ok(section)
    }
}

fn add_to_elem_section<T: RetainsInstructions>(
    section: &mut wasm_encoder::ElementSection,
    value: &Elem<T>,
) -> Result<(), String> {
    match value.ref_type {
        RefType::FuncRef => {
            let func_indices: Vec<u32> = value
                .init
                .iter()
                .flat_map(|expr| expr.instructions())
                .filter_map(|instr| match instr {
                    Instr::RefFunc(func_idx) => Some(func_idx),
                    _ => None,
                })
                .cloned()
                .collect();
            let elements = wasm_encoder::Elements::Functions(Cow::Owned(func_indices));
            match &value.mode {
                ElemMode::Passive => section.passive(elements),
                ElemMode::Active { table_idx, offset } => section.active(
                    if *table_idx == 0 {
                        None
                    } else {
                        Some(*table_idx)
                    },
                    &offset.try_into()?,
                    elements,
                ),
                ElemMode::Declarative => section.declared(elements),
            };
        }
        RefType::ExternRef => {
            let init: Vec<wasm_encoder::ConstExpr> = value
                .init
                .iter()
                .map(|expr| {
                    let instrs = expr.instructions().to_vec();
                    (&Expr { instrs }).try_into()
                })
                .collect::<Result<Vec<wasm_encoder::ConstExpr>, String>>()?;
            let elements = wasm_encoder::Elements::Expressions(
                wasm_encoder::RefType::EXTERNREF,
                Cow::Owned(init),
            );
            match &value.mode {
                ElemMode::Passive => section.passive(elements),
                ElemMode::Active { table_idx, offset } => section.active(
                    if *table_idx == 0 {
                        None
                    } else {
                        Some(*table_idx)
                    },
                    &offset.try_into()?,
                    elements,
                ),
                ElemMode::Declarative => section.declared(elements),
            };
        }
    };
    Ok(())
}

impl<T: Clone + RetainsInstructions> TryFrom<&Data<T>> for wasm_encoder::DataSection {
    type Error = String;

    fn try_from(value: &Data<T>) -> Result<Self, Self::Error> {
        let mut section = wasm_encoder::DataSection::new();
        add_to_data_section(&mut section, value.clone())?;
        Ok(section)
    }
}

fn add_to_data_section<T: Clone + RetainsInstructions>(
    section: &mut wasm_encoder::DataSection,
    value: Data<T>,
) -> Result<(), String> {
    match &value.mode {
        DataMode::Passive => section.passive(value.init),
        DataMode::Active { memory, offset } => {
            let offset = Expr {
                instrs: offset.instructions().to_vec(),
            };
            section.active(*memory, &(&offset).try_into()?, value.init)
        }
    };
    Ok(())
}

impl From<&Export> for wasm_encoder::ExportSection {
    fn from(value: &Export) -> Self {
        let mut section = wasm_encoder::ExportSection::new();
        add_to_export_section(&mut section, value);
        section
    }
}

fn add_to_export_section(section: &mut wasm_encoder::ExportSection, value: &Export) {
    let (kind, index) = match value.desc {
        ExportDesc::Func(func_idx) => (wasm_encoder::ExportKind::Func, func_idx),
        ExportDesc::Table(table_idx) => (wasm_encoder::ExportKind::Table, table_idx),
        ExportDesc::Mem(mem_idx) => (wasm_encoder::ExportKind::Memory, mem_idx),
        ExportDesc::Global(global_idx) => (wasm_encoder::ExportKind::Global, global_idx),
    };
    section.export(&value.name, kind, index);
}

impl From<&TypeRef> for wasm_encoder::EntityType {
    fn from(value: &TypeRef) -> Self {
        match value {
            TypeRef::Func(type_idx) => wasm_encoder::EntityType::Function(*type_idx),
            TypeRef::Table(table_type) => wasm_encoder::EntityType::Table(table_type.into()),
            TypeRef::Mem(mem_type) => wasm_encoder::EntityType::Memory(mem_type.into()),
            TypeRef::Global(global_type) => wasm_encoder::EntityType::Global(global_type.into()),
        }
    }
}

impl From<&Import> for wasm_encoder::ImportSection {
    fn from(value: &Import) -> Self {
        let mut section = wasm_encoder::ImportSection::new();
        add_to_import_section(&mut section, value);
        section
    }
}

fn add_to_import_section(section: &mut wasm_encoder::ImportSection, value: &Import) {
    let entity_type: wasm_encoder::EntityType = (&value.desc).into();
    section.import(&value.module, &value.name, entity_type);
}

impl From<Custom> for wasm_encoder::CustomSection<'_> {
    fn from(value: Custom) -> Self {
        wasm_encoder::CustomSection {
            name: value.name.into(),
            data: value.data.into(),
        }
    }
}

impl<Ast> TryFrom<Module<Ast>> for wasm_encoder::Module
where
    Ast: AstCustomization,
    Ast::Expr: RetainsInstructions,
    Ast::Data: Into<Data<Ast::Expr>>,
    Ast::Custom: Into<Custom>,
{
    type Error = String;

    fn try_from(value: Module<Ast>) -> Result<Self, Self::Error> {
        let mut module = wasm_encoder::Module::new();

        for (section_type, sections) in value.into_grouped() {
            match section_type {
                CoreSectionType::Type => {
                    let mut section = wasm_encoder::TypeSection::new();
                    for tpe in sections {
                        add_to_type_section(&mut section, tpe.as_type());
                    }
                    module.section(&section);
                }
                CoreSectionType::Func => {
                    let mut section = wasm_encoder::FunctionSection::new();
                    for func in sections {
                        add_to_function_section(&mut section, func.as_func());
                    }
                    module.section(&section);
                }
                CoreSectionType::Code => {
                    let mut section = wasm_encoder::CodeSection::new();
                    for code in sections {
                        add_to_code_section(&mut section, code.as_code())?;
                    }
                    module.section(&section);
                }
                CoreSectionType::Table => {
                    let mut section = wasm_encoder::TableSection::new();
                    for table in sections {
                        add_to_table_section(&mut section, table.as_table());
                    }
                    module.section(&section);
                }
                CoreSectionType::Mem => {
                    let mut section = wasm_encoder::MemorySection::new();
                    for mem in sections {
                        add_to_memory_section(&mut section, mem.as_mem());
                    }
                    module.section(&section);
                }
                CoreSectionType::Global => {
                    let mut section = wasm_encoder::GlobalSection::new();
                    for global in sections {
                        add_to_global_section(&mut section, global.as_global())?;
                    }
                    module.section(&section);
                }
                CoreSectionType::Elem => {
                    let mut section = wasm_encoder::ElementSection::new();
                    for elem in sections {
                        add_to_elem_section(&mut section, elem.as_elem())?;
                    }
                    module.section(&section);
                }
                CoreSectionType::Data => {
                    let mut section = wasm_encoder::DataSection::new();
                    for data in sections {
                        let data: Data<Ast::Expr> = data.as_data().clone().into();
                        add_to_data_section(&mut section, data)?;
                    }
                    module.section(&section);
                }
                CoreSectionType::DataCount => {
                    let count = sections.first().unwrap().as_data_count();
                    let section = wasm_encoder::DataCountSection { count: count.count };
                    module.section(&section);
                }
                CoreSectionType::Start => {
                    let start = sections.first().unwrap().as_start();
                    let section = wasm_encoder::StartSection {
                        function_index: start.func,
                    };
                    module.section(&section);
                }
                CoreSectionType::Export => {
                    let mut section = wasm_encoder::ExportSection::new();
                    for export in sections {
                        add_to_export_section(&mut section, export.as_export());
                    }
                    module.section(&section);
                }
                CoreSectionType::Import => {
                    let mut section = wasm_encoder::ImportSection::new();
                    for import in sections {
                        add_to_import_section(&mut section, import.as_import());
                    }
                    module.section(&section);
                }
                CoreSectionType::Custom => {
                    let custom = sections.first().unwrap().as_custom();
                    let custom: Custom = custom.clone().into();
                    let section: wasm_encoder::CustomSection = custom.into();
                    module.section(&section);
                }
            }
        }

        Ok(module)
    }
}

trait InstructionTarget {
    fn emit(&mut self, instr: wasm_encoder::Instruction);
}

impl InstructionTarget for wasm_encoder::Function {
    fn emit(&mut self, instr: wasm_encoder::Instruction) {
        self.instruction(&instr);
    }
}

fn encode_instructions<F: InstructionTarget>(
    instrs: &[Instr],
    target: &mut F,
) -> Result<(), String> {
    for instr in instrs {
        encode_instr(instr, target)?;
    }
    Ok(())
}

fn encode_instr<F: InstructionTarget>(instr: &Instr, target: &mut F) -> Result<(), String> {
    match instr {
        Instr::I32Const(value) => target.emit(wasm_encoder::Instruction::I32Const(*value)),
        Instr::I64Const(value) => target.emit(wasm_encoder::Instruction::I64Const(*value)),
        Instr::F32Const(value) => target.emit(wasm_encoder::Instruction::F32Const((*value).into())),
        Instr::F64Const(value) => target.emit(wasm_encoder::Instruction::F64Const((*value).into())),
        Instr::IEqz(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Eqz),
        Instr::IEqz(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Eqz),
        Instr::IEq(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Eq),
        Instr::IEq(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Eq),
        Instr::INe(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Ne),
        Instr::INe(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Ne),
        Instr::ILt(IntWidth::I32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32LtS)
        }
        Instr::ILt(IntWidth::I32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32LtU)
        }
        Instr::ILt(IntWidth::I64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64LtS)
        }
        Instr::ILt(IntWidth::I64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64LtU)
        }
        Instr::IGt(IntWidth::I32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32GtS)
        }
        Instr::IGt(IntWidth::I32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32GtU)
        }
        Instr::IGt(IntWidth::I64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64GtS)
        }
        Instr::IGt(IntWidth::I64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64GtU)
        }
        Instr::ILe(IntWidth::I32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32LeS)
        }
        Instr::ILe(IntWidth::I32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32LeU)
        }
        Instr::ILe(IntWidth::I64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64LeS)
        }
        Instr::ILe(IntWidth::I64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64LeU)
        }
        Instr::IGe(IntWidth::I32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32GeS)
        }
        Instr::IGe(IntWidth::I32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32GeU)
        }
        Instr::IGe(IntWidth::I64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64GeS)
        }
        Instr::IGe(IntWidth::I64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64GeU)
        }
        Instr::FEq(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Eq),
        Instr::FEq(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Eq),
        Instr::FNe(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Ne),
        Instr::FNe(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Ne),
        Instr::FLt(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Lt),
        Instr::FLt(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Lt),
        Instr::FGt(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Gt),
        Instr::FGt(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Gt),
        Instr::FLe(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Le),
        Instr::FLe(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Le),
        Instr::FGe(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Ge),
        Instr::FGe(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Ge),
        Instr::IClz(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Clz),
        Instr::IClz(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Clz),
        Instr::ICtz(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Ctz),
        Instr::ICtz(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Ctz),
        Instr::IPopCnt(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Popcnt),
        Instr::IPopCnt(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Popcnt),
        Instr::IAdd(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Add),
        Instr::IAdd(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Add),
        Instr::ISub(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Sub),
        Instr::ISub(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Sub),
        Instr::IMul(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Mul),
        Instr::IMul(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Mul),
        Instr::IDiv(IntWidth::I32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32DivS)
        }
        Instr::IDiv(IntWidth::I32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32DivU)
        }
        Instr::IDiv(IntWidth::I64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64DivS)
        }
        Instr::IDiv(IntWidth::I64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64DivU)
        }
        Instr::IRem(IntWidth::I32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32RemS)
        }
        Instr::IRem(IntWidth::I32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32RemU)
        }
        Instr::IRem(IntWidth::I64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64RemS)
        }
        Instr::IRem(IntWidth::I64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64RemU)
        }
        Instr::IAnd(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32And),
        Instr::IAnd(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64And),
        Instr::IOr(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Or),
        Instr::IOr(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Or),
        Instr::IXor(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Xor),
        Instr::IXor(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Xor),
        Instr::IShl(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Shl),
        Instr::IShl(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Shl),
        Instr::IShr(IntWidth::I32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32ShrS)
        }
        Instr::IShr(IntWidth::I32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32ShrU)
        }
        Instr::IShr(IntWidth::I64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64ShrS)
        }
        Instr::IShr(IntWidth::I64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64ShrU)
        }
        Instr::IRotL(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Rotl),
        Instr::IRotL(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Rotl),
        Instr::IRotR(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Rotr),
        Instr::IRotR(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Rotr),
        Instr::FAbs(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Abs),
        Instr::FAbs(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Abs),
        Instr::FNeg(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Neg),
        Instr::FNeg(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Neg),
        Instr::FCeil(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Ceil),
        Instr::FCeil(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Ceil),
        Instr::FFloor(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Floor),
        Instr::FFloor(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Floor),
        Instr::FTrunc(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Trunc),
        Instr::FTrunc(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Trunc),
        Instr::FNearest(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Nearest),
        Instr::FNearest(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Nearest),
        Instr::FSqrt(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Sqrt),
        Instr::FSqrt(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Sqrt),
        Instr::FAdd(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Add),
        Instr::FAdd(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Add),
        Instr::FSub(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Sub),
        Instr::FSub(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Sub),
        Instr::FMul(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Mul),
        Instr::FMul(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Mul),
        Instr::FDiv(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Div),
        Instr::FDiv(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Div),
        Instr::FMin(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Min),
        Instr::FMin(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Min),
        Instr::FMax(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Max),
        Instr::FMax(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Max),
        Instr::FCopySign(FloatWidth::F32) => target.emit(wasm_encoder::Instruction::F32Copysign),
        Instr::FCopySign(FloatWidth::F64) => target.emit(wasm_encoder::Instruction::F64Copysign),
        Instr::I32WrapI64 => target.emit(wasm_encoder::Instruction::I32WrapI64),
        Instr::ITruncF(IntWidth::I32, FloatWidth::F32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32TruncF32S)
        }
        Instr::ITruncF(IntWidth::I32, FloatWidth::F32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32TruncF32U)
        }
        Instr::ITruncF(IntWidth::I32, FloatWidth::F64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32TruncF64S)
        }
        Instr::ITruncF(IntWidth::I32, FloatWidth::F64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32TruncF64U)
        }
        Instr::ITruncF(IntWidth::I64, FloatWidth::F32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64TruncF32S)
        }
        Instr::ITruncF(IntWidth::I64, FloatWidth::F32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64TruncF32U)
        }
        Instr::ITruncF(IntWidth::I64, FloatWidth::F64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64TruncF64S)
        }
        Instr::ITruncF(IntWidth::I64, FloatWidth::F64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64TruncF64U)
        }
        Instr::I64ExtendI32(Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64ExtendI32S)
        }
        Instr::I64ExtendI32(Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64ExtendI32U)
        }
        Instr::I64Extend32S => target.emit(wasm_encoder::Instruction::I64Extend8S),
        Instr::IExtend8S(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Extend8S),
        Instr::IExtend8S(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Extend8S),
        Instr::IExtend16S(IntWidth::I32) => target.emit(wasm_encoder::Instruction::I32Extend16S),
        Instr::IExtend16S(IntWidth::I64) => target.emit(wasm_encoder::Instruction::I64Extend16S),
        Instr::FConvertI(FloatWidth::F32, IntWidth::I32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::F32ConvertI32S)
        }
        Instr::FConvertI(FloatWidth::F32, IntWidth::I32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::F32ConvertI32U)
        }
        Instr::FConvertI(FloatWidth::F32, IntWidth::I64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::F32ConvertI64S)
        }
        Instr::FConvertI(FloatWidth::F32, IntWidth::I64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::F32ConvertI64U)
        }
        Instr::FConvertI(FloatWidth::F64, IntWidth::I32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::F64ConvertI32S)
        }
        Instr::FConvertI(FloatWidth::F64, IntWidth::I32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::F64ConvertI32U)
        }
        Instr::FConvertI(FloatWidth::F64, IntWidth::I64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::F64ConvertI64S)
        }
        Instr::FConvertI(FloatWidth::F64, IntWidth::I64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::F64ConvertI64U)
        }
        Instr::F32DemoteF64 => target.emit(wasm_encoder::Instruction::F32DemoteF64),
        Instr::F64PromoteF32 => target.emit(wasm_encoder::Instruction::F64PromoteF32),
        Instr::IReinterpretF(IntWidth::I32) => {
            target.emit(wasm_encoder::Instruction::I32ReinterpretF32)
        }
        Instr::IReinterpretF(IntWidth::I64) => {
            target.emit(wasm_encoder::Instruction::I64ReinterpretF64)
        }
        Instr::FReinterpretI(FloatWidth::F32) => {
            target.emit(wasm_encoder::Instruction::F32ReinterpretI32)
        }
        Instr::FReinterpretI(FloatWidth::F64) => {
            target.emit(wasm_encoder::Instruction::F64ReinterpretI64)
        }
        Instr::ITruncSatF(IntWidth::I32, FloatWidth::F32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32TruncSatF32S)
        }
        Instr::ITruncSatF(IntWidth::I32, FloatWidth::F32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32TruncSatF32U)
        }
        Instr::ITruncSatF(IntWidth::I32, FloatWidth::F64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32TruncSatF64S)
        }
        Instr::ITruncSatF(IntWidth::I32, FloatWidth::F64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32TruncSatF64U)
        }
        Instr::ITruncSatF(IntWidth::I64, FloatWidth::F32, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64TruncSatF32S)
        }
        Instr::ITruncSatF(IntWidth::I64, FloatWidth::F32, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64TruncSatF32U)
        }
        Instr::ITruncSatF(IntWidth::I64, FloatWidth::F64, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64TruncSatF64S)
        }
        Instr::ITruncSatF(IntWidth::I64, FloatWidth::F64, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64TruncSatF64U)
        }
        Instr::V128Const(value) => target.emit(wasm_encoder::Instruction::V128Const(*value)),
        Instr::V128Not => target.emit(wasm_encoder::Instruction::V128Not),
        Instr::V128And => target.emit(wasm_encoder::Instruction::V128And),
        Instr::V128AndNot => target.emit(wasm_encoder::Instruction::V128AndNot),
        Instr::V128Or => target.emit(wasm_encoder::Instruction::V128Or),
        Instr::V128XOr => target.emit(wasm_encoder::Instruction::V128Xor),
        Instr::V128BitSelect => target.emit(wasm_encoder::Instruction::V128Bitselect),
        Instr::V128AnyTrue => target.emit(wasm_encoder::Instruction::V128AnyTrue),
        Instr::VI8x16Shuffle(lanes) => target.emit(wasm_encoder::Instruction::I8x16Shuffle(*lanes)),
        Instr::VI18x16Swizzle => target.emit(wasm_encoder::Instruction::I8x16Swizzle),
        Instr::VSplat(Shape::Int(IShape::I8x16)) => {
            target.emit(wasm_encoder::Instruction::I8x16Splat)
        }
        Instr::VSplat(Shape::Int(IShape::I16x8)) => {
            target.emit(wasm_encoder::Instruction::I16x8Splat)
        }
        Instr::VSplat(Shape::Int(IShape::I32x4)) => {
            target.emit(wasm_encoder::Instruction::I32x4Splat)
        }
        Instr::VSplat(Shape::Int(IShape::I64x2)) => {
            target.emit(wasm_encoder::Instruction::I64x2Splat)
        }
        Instr::VSplat(Shape::Float(FShape::F32x4)) => {
            target.emit(wasm_encoder::Instruction::F32x4Splat)
        }
        Instr::VSplat(Shape::Float(FShape::F64x2)) => {
            target.emit(wasm_encoder::Instruction::F64x2Splat)
        }
        Instr::VI8x16ExtractLane(Signedness::Signed, lane_idx) => {
            target.emit(wasm_encoder::Instruction::I8x16ExtractLaneS(*lane_idx))
        }
        Instr::VI8x16ExtractLane(Signedness::Unsigned, lane_idx) => {
            target.emit(wasm_encoder::Instruction::I8x16ExtractLaneU(*lane_idx))
        }
        Instr::VI16x8ExtractLane(Signedness::Signed, lane_idx) => {
            target.emit(wasm_encoder::Instruction::I16x8ExtractLaneS(*lane_idx))
        }
        Instr::VI16x8ExtractLane(Signedness::Unsigned, lane_idx) => {
            target.emit(wasm_encoder::Instruction::I16x8ExtractLaneU(*lane_idx))
        }
        Instr::VI32x4ExtractLane(lane_idx) => {
            target.emit(wasm_encoder::Instruction::I32x4ExtractLane(*lane_idx))
        }
        Instr::VI64x2ExtractLane(lane_idx) => {
            target.emit(wasm_encoder::Instruction::I64x2ExtractLane(*lane_idx))
        }
        Instr::VFExtractLane(FShape::F32x4, lane_idx) => {
            target.emit(wasm_encoder::Instruction::F32x4ExtractLane(*lane_idx))
        }
        Instr::VFExtractLane(FShape::F64x2, lane_idx) => {
            target.emit(wasm_encoder::Instruction::F64x2ExtractLane(*lane_idx))
        }
        Instr::VReplaceLane(Shape::Int(IShape::I8x16), lane_idx) => {
            target.emit(wasm_encoder::Instruction::I8x16ReplaceLane(*lane_idx))
        }
        Instr::VReplaceLane(Shape::Int(IShape::I16x8), lane_idx) => {
            target.emit(wasm_encoder::Instruction::I16x8ReplaceLane(*lane_idx))
        }
        Instr::VReplaceLane(Shape::Int(IShape::I32x4), lane_idx) => {
            target.emit(wasm_encoder::Instruction::I32x4ReplaceLane(*lane_idx))
        }
        Instr::VReplaceLane(Shape::Int(IShape::I64x2), lane_idx) => {
            target.emit(wasm_encoder::Instruction::I64x2ReplaceLane(*lane_idx))
        }
        Instr::VReplaceLane(Shape::Float(FShape::F32x4), lane_idx) => {
            target.emit(wasm_encoder::Instruction::F32x4ReplaceLane(*lane_idx))
        }
        Instr::VReplaceLane(Shape::Float(FShape::F64x2), lane_idx) => {
            target.emit(wasm_encoder::Instruction::F64x2ReplaceLane(*lane_idx))
        }
        Instr::VIEq(IShape::I8x16) => target.emit(wasm_encoder::Instruction::I8x16Eq),
        Instr::VIEq(IShape::I16x8) => target.emit(wasm_encoder::Instruction::I16x8Eq),
        Instr::VIEq(IShape::I32x4) => target.emit(wasm_encoder::Instruction::I32x4Eq),
        Instr::VIEq(IShape::I64x2) => target.emit(wasm_encoder::Instruction::I64x2Eq),
        Instr::VINe(IShape::I8x16) => target.emit(wasm_encoder::Instruction::I8x16Ne),
        Instr::VINe(IShape::I16x8) => target.emit(wasm_encoder::Instruction::I16x8Ne),
        Instr::VINe(IShape::I32x4) => target.emit(wasm_encoder::Instruction::I32x4Ne),
        Instr::VINe(IShape::I64x2) => target.emit(wasm_encoder::Instruction::I64x2Ne),
        Instr::VILt(IShape::I8x16, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I8x16LtS)
        }
        Instr::VILt(IShape::I8x16, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I8x16LtU)
        }
        Instr::VILt(IShape::I16x8, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8LtS)
        }
        Instr::VILt(IShape::I16x8, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8LtU)
        }
        Instr::VILt(IShape::I32x4, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4LtS)
        }
        Instr::VILt(IShape::I32x4, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4LtU)
        }
        Instr::VILt(IShape::I64x2, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64x2LtS)
        }
        Instr::VILt(IShape::I64x2, Signedness::Unsigned) => {
            return Err("invalid instruction: VI64x2LtU".to_string());
        }
        Instr::VIGt(IShape::I8x16, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I8x16GtS)
        }
        Instr::VIGt(IShape::I8x16, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I8x16GtU)
        }
        Instr::VIGt(IShape::I16x8, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8GtS)
        }
        Instr::VIGt(IShape::I16x8, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8GtU)
        }
        Instr::VIGt(IShape::I32x4, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4GtS)
        }
        Instr::VIGt(IShape::I32x4, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4GtU)
        }
        Instr::VIGt(IShape::I64x2, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64x2GtS)
        }
        Instr::VIGt(IShape::I64x2, Signedness::Unsigned) => {
            return Err("invalid instruction: VI64x2GtU".to_string());
        }
        Instr::VILe(IShape::I8x16, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I8x16LeS)
        }
        Instr::VILe(IShape::I8x16, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I8x16LeU)
        }
        Instr::VILe(IShape::I16x8, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8LeS)
        }
        Instr::VILe(IShape::I16x8, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8LeU)
        }
        Instr::VILe(IShape::I32x4, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4LeS)
        }
        Instr::VILe(IShape::I32x4, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4LeU)
        }
        Instr::VILe(IShape::I64x2, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64x2LeS)
        }
        Instr::VILe(IShape::I64x2, Signedness::Unsigned) => {
            return Err("invalid instruction: VI64x2LeU".to_string());
        }
        Instr::VIGe(IShape::I8x16, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I8x16GeS)
        }
        Instr::VIGe(IShape::I8x16, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I8x16GeU)
        }
        Instr::VIGe(IShape::I16x8, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8GeS)
        }
        Instr::VIGe(IShape::I16x8, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8GeU)
        }
        Instr::VIGe(IShape::I32x4, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4GeS)
        }
        Instr::VIGe(IShape::I32x4, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4GeU)
        }
        Instr::VIGe(IShape::I64x2, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64x2GeS)
        }
        Instr::VIGe(IShape::I64x2, Signedness::Unsigned) => {
            return Err("invalid instruction: VI64x2GeU".to_string());
        }
        Instr::VI64x2Lt => target.emit(wasm_encoder::Instruction::I64x2LtS),
        Instr::VI64x2Gt => target.emit(wasm_encoder::Instruction::I64x2GtS),
        Instr::VI64x2Le => target.emit(wasm_encoder::Instruction::I64x2LeS),
        Instr::VI64x2Ge => target.emit(wasm_encoder::Instruction::I64x2GeS),
        Instr::VFEq(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Eq),
        Instr::VFEq(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Eq),
        Instr::VFNe(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Ne),
        Instr::VFNe(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Ne),
        Instr::VFLt(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Lt),
        Instr::VFLt(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Lt),
        Instr::VFGt(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Gt),
        Instr::VFGt(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Gt),
        Instr::VFLe(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Le),
        Instr::VFLe(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Le),
        Instr::VFGe(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Ge),
        Instr::VFGe(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Ge),
        Instr::VIAbs(IShape::I8x16) => target.emit(wasm_encoder::Instruction::I8x16Abs),
        Instr::VIAbs(IShape::I16x8) => target.emit(wasm_encoder::Instruction::I16x8Abs),
        Instr::VIAbs(IShape::I32x4) => target.emit(wasm_encoder::Instruction::I32x4Abs),
        Instr::VIAbs(IShape::I64x2) => target.emit(wasm_encoder::Instruction::I64x2Abs),
        Instr::VINeg(IShape::I8x16) => target.emit(wasm_encoder::Instruction::I8x16Neg),
        Instr::VINeg(IShape::I16x8) => target.emit(wasm_encoder::Instruction::I16x8Neg),
        Instr::VINeg(IShape::I32x4) => target.emit(wasm_encoder::Instruction::I32x4Neg),
        Instr::VINeg(IShape::I64x2) => target.emit(wasm_encoder::Instruction::I64x2Neg),
        Instr::VI8x16PopCnt => target.emit(wasm_encoder::Instruction::I8x16Popcnt),
        Instr::VI16x8Q15MulrSat => target.emit(wasm_encoder::Instruction::I16x8Q15MulrSatS),
        Instr::VI32x4DotI16x8 => target.emit(wasm_encoder::Instruction::I32x4DotI16x8S),
        Instr::VFAbs(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Abs),
        Instr::VFAbs(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Abs),
        Instr::VFNeg(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Neg),
        Instr::VFNeg(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Neg),
        Instr::VFSqrt(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Sqrt),
        Instr::VFSqrt(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Sqrt),
        Instr::VFCeil(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Ceil),
        Instr::VFCeil(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Ceil),
        Instr::VFFloor(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Floor),
        Instr::VFFloor(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Floor),
        Instr::VFTrunc(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Trunc),
        Instr::VFTrunc(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Trunc),
        Instr::VFNearest(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Nearest),
        Instr::VFNearest(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Nearest),
        Instr::VIAllTrue(IShape::I8x16) => target.emit(wasm_encoder::Instruction::I8x16AllTrue),
        Instr::VIAllTrue(IShape::I16x8) => target.emit(wasm_encoder::Instruction::I16x8AllTrue),
        Instr::VIAllTrue(IShape::I32x4) => target.emit(wasm_encoder::Instruction::I32x4AllTrue),
        Instr::VIAllTrue(IShape::I64x2) => target.emit(wasm_encoder::Instruction::I64x2AllTrue),
        Instr::VIBitMask(IShape::I8x16) => target.emit(wasm_encoder::Instruction::I8x16Bitmask),
        Instr::VIBitMask(IShape::I16x8) => target.emit(wasm_encoder::Instruction::I16x8Bitmask),
        Instr::VIBitMask(IShape::I32x4) => target.emit(wasm_encoder::Instruction::I32x4Bitmask),
        Instr::VIBitMask(IShape::I64x2) => target.emit(wasm_encoder::Instruction::I64x2Bitmask),
        Instr::VI8x16NarrowI16x8(Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I8x16NarrowI16x8S)
        }
        Instr::VI8x16NarrowI16x8(Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I8x16NarrowI16x8U)
        }
        Instr::VI16x8NarrowI32x4(Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8NarrowI32x4S)
        }
        Instr::VI16x8NarrowI32x4(Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8NarrowI32x4U)
        }
        Instr::VI16x8ExtendI8x16(Half::Low, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8ExtendLowI8x16S)
        }
        Instr::VI16x8ExtendI8x16(Half::Low, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8ExtendLowI8x16U)
        }
        Instr::VI16x8ExtendI8x16(Half::High, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8ExtendHighI8x16S)
        }
        Instr::VI16x8ExtendI8x16(Half::High, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8ExtendHighI8x16U)
        }
        Instr::VI32x4ExtendI16x8(Half::Low, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4ExtendLowI16x8S)
        }
        Instr::VI32x4ExtendI16x8(Half::Low, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4ExtendLowI16x8U)
        }
        Instr::VI32x4ExtendI16x8(Half::High, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4ExtendHighI16x8S)
        }
        Instr::VI32x4ExtendI16x8(Half::High, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4ExtendHighI16x8U)
        }
        Instr::VI64x2ExtendI32x4(Half::Low, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64x2ExtendLowI32x4S)
        }
        Instr::VI64x2ExtendI32x4(Half::Low, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64x2ExtendLowI32x4U)
        }
        Instr::VI64x2ExtendI32x4(Half::High, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64x2ExtendHighI32x4S)
        }
        Instr::VI64x2ExtendI32x4(Half::High, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64x2ExtendHighI32x4U)
        }
        Instr::VIShl(IShape::I8x16) => target.emit(wasm_encoder::Instruction::I8x16Shl),
        Instr::VIShl(IShape::I16x8) => target.emit(wasm_encoder::Instruction::I16x8Shl),
        Instr::VIShl(IShape::I32x4) => target.emit(wasm_encoder::Instruction::I32x4Shl),
        Instr::VIShl(IShape::I64x2) => target.emit(wasm_encoder::Instruction::I64x2Shl),
        Instr::VIShr(IShape::I8x16, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I8x16ShrS)
        }
        Instr::VIShr(IShape::I8x16, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I8x16ShrU)
        }
        Instr::VIShr(IShape::I16x8, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8ShrS)
        }
        Instr::VIShr(IShape::I16x8, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8ShrU)
        }
        Instr::VIShr(IShape::I32x4, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4ShrS)
        }
        Instr::VIShr(IShape::I32x4, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4ShrU)
        }
        Instr::VIShr(IShape::I64x2, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64x2ShrS)
        }
        Instr::VIShr(IShape::I64x2, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64x2ShrU)
        }
        Instr::VIAdd(IShape::I8x16) => target.emit(wasm_encoder::Instruction::I8x16Add),
        Instr::VIAdd(IShape::I16x8) => target.emit(wasm_encoder::Instruction::I16x8Add),
        Instr::VIAdd(IShape::I32x4) => target.emit(wasm_encoder::Instruction::I32x4Add),
        Instr::VIAdd(IShape::I64x2) => target.emit(wasm_encoder::Instruction::I64x2Add),
        Instr::VISub(IShape::I8x16) => target.emit(wasm_encoder::Instruction::I8x16Sub),
        Instr::VISub(IShape::I16x8) => target.emit(wasm_encoder::Instruction::I16x8Sub),
        Instr::VISub(IShape::I32x4) => target.emit(wasm_encoder::Instruction::I32x4Sub),
        Instr::VISub(IShape::I64x2) => target.emit(wasm_encoder::Instruction::I64x2Sub),
        Instr::VIMin(IShape::I8x16, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I8x16MinS)
        }
        Instr::VIMin(IShape::I8x16, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I8x16MinU)
        }
        Instr::VIMin(IShape::I16x8, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8MinS)
        }
        Instr::VIMin(IShape::I16x8, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8MinU)
        }
        Instr::VIMin(IShape::I32x4, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4MinS)
        }
        Instr::VIMin(IShape::I32x4, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4MinU)
        }
        Instr::VIMin(IShape::I64x2, Signedness::Signed) => {
            return Err("invalid instruction: VI64x2MinS".to_string());
        }
        Instr::VIMin(IShape::I64x2, Signedness::Unsigned) => {
            return Err("invalid instruction: VI64x2MinU".to_string());
        }
        Instr::VIMax(IShape::I8x16, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I8x16MaxS)
        }
        Instr::VIMax(IShape::I8x16, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I8x16MaxU)
        }
        Instr::VIMax(IShape::I16x8, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8MaxS)
        }
        Instr::VIMax(IShape::I16x8, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8MaxU)
        }
        Instr::VIMax(IShape::I32x4, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4MaxS)
        }
        Instr::VIMax(IShape::I32x4, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4MaxU)
        }
        Instr::VIMax(IShape::I64x2, Signedness::Signed) => {
            return Err("invalid instruction: VI64x2MaxS".to_string());
        }
        Instr::VIMax(IShape::I64x2, Signedness::Unsigned) => {
            return Err("invalid instruction: VI64x2MaxU".to_string());
        }
        Instr::VIAddSat(IShape::I8x16, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I8x16AddSatS)
        }
        Instr::VIAddSat(IShape::I8x16, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I8x16AddSatU)
        }
        Instr::VIAddSat(IShape::I16x8, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8AddSatS)
        }
        Instr::VIAddSat(IShape::I16x8, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8AddSatU)
        }
        Instr::VIAddSat(IShape::I32x4, Signedness::Signed) => {
            return Err("invalid instruction: VI32x4AddSatS".to_string());
        }
        Instr::VIAddSat(IShape::I32x4, Signedness::Unsigned) => {
            return Err("invalid instruction: VI32x4AddSatU".to_string());
        }
        Instr::VIAddSat(IShape::I64x2, Signedness::Signed) => {
            return Err("invalid instruction: VI64x2AddSatS".to_string());
        }
        Instr::VIAddSat(IShape::I64x2, Signedness::Unsigned) => {
            return Err("invalid instruction: VI64x2AddSatU".to_string());
        }
        Instr::VISubSat(IShape::I8x16, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I8x16SubSatS)
        }
        Instr::VISubSat(IShape::I8x16, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I8x16SubSatU)
        }
        Instr::VISubSat(IShape::I16x8, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8SubSatS)
        }
        Instr::VISubSat(IShape::I16x8, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8SubSatU)
        }
        Instr::VISubSat(IShape::I32x4, Signedness::Signed) => {
            return Err("invalid instruction: VI32x4SubSatS".to_string());
        }
        Instr::VISubSat(IShape::I32x4, Signedness::Unsigned) => {
            return Err("invalid instruction: VI32x4SubSatU".to_string());
        }
        Instr::VISubSat(IShape::I64x2, Signedness::Signed) => {
            return Err("invalid instruction: VI64x2SubSatS".to_string());
        }
        Instr::VISubSat(IShape::I64x2, Signedness::Unsigned) => {
            return Err("invalid instruction: VI64x2SubSatU".to_string());
        }
        Instr::VIMul(IShape::I8x16) => return Err("invalid instruction: VI8x16Mul".to_string()),
        Instr::VIMul(IShape::I16x8) => target.emit(wasm_encoder::Instruction::I16x8Mul),
        Instr::VIMul(IShape::I32x4) => target.emit(wasm_encoder::Instruction::I32x4Mul),
        Instr::VIMul(IShape::I64x2) => target.emit(wasm_encoder::Instruction::I64x2Mul),
        Instr::VIAvgr(IShape::I8x16) => target.emit(wasm_encoder::Instruction::I8x16AvgrU),
        Instr::VIAvgr(IShape::I16x8) => target.emit(wasm_encoder::Instruction::I16x8AvgrU),
        Instr::VIAvgr(IShape::I32x4) => return Err("invalid instruction: VI32x4AvgrU".to_string()),
        Instr::VIAvgr(IShape::I64x2) => return Err("invalid instruction: VI64x2AvgrU".to_string()),
        Instr::VIExtMul(IShape::I8x16, Half::Low, Signedness::Signed) => {
            return Err("invalid instruction: VI8x16ExtMulLowI8x16S".to_string());
        }
        Instr::VIExtMul(IShape::I8x16, Half::Low, Signedness::Unsigned) => {
            return Err("invalid instruction: VI8x16ExtMulLowI8x16U".to_string());
        }
        Instr::VIExtMul(IShape::I16x8, Half::Low, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8ExtMulLowI8x16S)
        }
        Instr::VIExtMul(IShape::I16x8, Half::Low, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8ExtMulLowI8x16U)
        }
        Instr::VIExtMul(IShape::I32x4, Half::Low, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4ExtMulLowI16x8S)
        }
        Instr::VIExtMul(IShape::I32x4, Half::Low, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4ExtMulLowI16x8U)
        }
        Instr::VIExtMul(IShape::I64x2, Half::Low, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64x2ExtMulLowI32x4S)
        }
        Instr::VIExtMul(IShape::I64x2, Half::Low, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64x2ExtMulLowI32x4U)
        }
        Instr::VIExtMul(IShape::I8x16, Half::High, Signedness::Signed) => {
            return Err("invalid instruction: VI8x16ExtMulHighI8x16S".to_string());
        }
        Instr::VIExtMul(IShape::I8x16, Half::High, Signedness::Unsigned) => {
            return Err("invalid instruction: VI8x16ExtMulHighI8x16U".to_string());
        }
        Instr::VIExtMul(IShape::I16x8, Half::High, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8ExtMulHighI8x16S)
        }
        Instr::VIExtMul(IShape::I16x8, Half::High, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8ExtMulHighI8x16U)
        }
        Instr::VIExtMul(IShape::I32x4, Half::High, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4ExtMulHighI16x8S)
        }
        Instr::VIExtMul(IShape::I32x4, Half::High, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4ExtMulHighI16x8U)
        }
        Instr::VIExtMul(IShape::I64x2, Half::High, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I64x2ExtMulHighI32x4S)
        }
        Instr::VIExtMul(IShape::I64x2, Half::High, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I64x2ExtMulHighI32x4U)
        }
        Instr::VIExtAddPairwise(IShape::I8x16, Signedness::Signed) => {
            return Err("invalid instruction: VI8x16ExtAddPairwiseI8x16S".to_string());
        }
        Instr::VIExtAddPairwise(IShape::I8x16, Signedness::Unsigned) => {
            return Err("invalid instruction: VI8x16ExtAddPairwiseI8x16U".to_string());
        }
        Instr::VIExtAddPairwise(IShape::I16x8, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I16x8ExtAddPairwiseI8x16S)
        }
        Instr::VIExtAddPairwise(IShape::I16x8, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I16x8ExtAddPairwiseI8x16U)
        }
        Instr::VIExtAddPairwise(IShape::I32x4, Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4ExtAddPairwiseI16x8S)
        }
        Instr::VIExtAddPairwise(IShape::I32x4, Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4ExtAddPairwiseI16x8U)
        }
        Instr::VIExtAddPairwise(IShape::I64x2, Signedness::Signed) => {
            return Err("invalid instruction: VI64x2ExtAddPairwiseI32x4S".to_string());
        }
        Instr::VIExtAddPairwise(IShape::I64x2, Signedness::Unsigned) => {
            return Err("invalid instruction: VI64x2ExtAddPairwiseI32x4U".to_string());
        }
        Instr::VFAdd(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Add),
        Instr::VFAdd(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Add),
        Instr::VFSub(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Sub),
        Instr::VFSub(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Sub),
        Instr::VFMul(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Mul),
        Instr::VFMul(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Mul),
        Instr::VFDiv(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Div),
        Instr::VFDiv(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Div),
        Instr::VFMin(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Min),
        Instr::VFMin(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Min),
        Instr::VFMax(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4Max),
        Instr::VFMax(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2Max),
        Instr::VFPMin(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4PMin),
        Instr::VFPMin(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2PMin),
        Instr::VFPMax(FShape::F32x4) => target.emit(wasm_encoder::Instruction::F32x4PMax),
        Instr::VFPMax(FShape::F64x2) => target.emit(wasm_encoder::Instruction::F64x2PMax),
        Instr::VI32x4TruncSatF32x4(Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4TruncSatF32x4S)
        }
        Instr::VI32x4TruncSatF32x4(Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4TruncSatF32x4U)
        }
        Instr::VI32x4TruncSatF64x2Zero(Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::I32x4TruncSatF64x2SZero)
        }
        Instr::VI32x4TruncSatF64x2Zero(Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::I32x4TruncSatF64x2UZero)
        }
        Instr::VI32x4ConvertI32x4(Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::F32x4ConvertI32x4S)
        }
        Instr::VI32x4ConvertI32x4(Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::F32x4ConvertI32x4U)
        }
        Instr::VF32x4DemoteF64x2Zero => {
            target.emit(wasm_encoder::Instruction::F32x4DemoteF64x2Zero)
        }
        Instr::VF64x2ConvertLowI32x4(Signedness::Signed) => {
            target.emit(wasm_encoder::Instruction::F64x2ConvertLowI32x4S)
        }
        Instr::VF64x2ConvertLowI32x4(Signedness::Unsigned) => {
            target.emit(wasm_encoder::Instruction::F64x2ConvertLowI32x4U)
        }
        Instr::VF64x2PromoteLowI32x4 => {
            target.emit(wasm_encoder::Instruction::F64x2PromoteLowF32x4)
        }
        Instr::RefNull(ref_type) => {
            target.emit(wasm_encoder::Instruction::RefNull(ref_type.into()))
        }
        Instr::RefIsNull => target.emit(wasm_encoder::Instruction::RefIsNull),
        Instr::RefFunc(func_idx) => target.emit(wasm_encoder::Instruction::RefFunc(*func_idx)),
        Instr::Drop => target.emit(wasm_encoder::Instruction::Drop),
        Instr::Select(None) => target.emit(wasm_encoder::Instruction::Select),
        Instr::Select(Some(ty)) => match ty.first() {
            Some(ty) => target.emit(wasm_encoder::Instruction::TypedSelect(ty.into())),
            None => target.emit(wasm_encoder::Instruction::Select),
        },
        Instr::LocalGet(local_idx) => target.emit(wasm_encoder::Instruction::LocalGet(*local_idx)),
        Instr::LocalSet(local_idx) => target.emit(wasm_encoder::Instruction::LocalSet(*local_idx)),
        Instr::LocalTee(local_idx) => target.emit(wasm_encoder::Instruction::LocalTee(*local_idx)),
        Instr::GlobalGet(global_idx) => {
            target.emit(wasm_encoder::Instruction::GlobalGet(*global_idx))
        }
        Instr::GlobalSet(global_idx) => {
            target.emit(wasm_encoder::Instruction::GlobalSet(*global_idx))
        }
        Instr::TableGet(table_idx) => target.emit(wasm_encoder::Instruction::TableGet(*table_idx)),
        Instr::TableSet(table_idx) => target.emit(wasm_encoder::Instruction::TableSet(*table_idx)),
        Instr::TableSize(table_idx) => {
            target.emit(wasm_encoder::Instruction::TableSize(*table_idx))
        }
        Instr::TableGrow(table_idx) => {
            target.emit(wasm_encoder::Instruction::TableGrow(*table_idx))
        }
        Instr::TableFill(table_idx) => {
            target.emit(wasm_encoder::Instruction::TableFill(*table_idx))
        }
        Instr::TableCopy {
            source,
            destination,
        } => target.emit(wasm_encoder::Instruction::TableCopy {
            src_table: *source,
            dst_table: *destination,
        }),
        Instr::TableInit(table_idx, elem_idx) => {
            target.emit(wasm_encoder::Instruction::TableInit {
                table: *table_idx,
                elem_index: *elem_idx,
            })
        }
        Instr::ElemDrop(elem_idx) => target.emit(wasm_encoder::Instruction::ElemDrop(*elem_idx)),
        Instr::Load(NumOrVecType::Num(NumType::I32), mem_arg) => {
            target.emit(wasm_encoder::Instruction::I32Load(mem_arg.into()))
        }
        Instr::Load(NumOrVecType::Num(NumType::I64), mem_arg) => {
            target.emit(wasm_encoder::Instruction::I64Load(mem_arg.into()))
        }
        Instr::Load(NumOrVecType::Num(NumType::F32), mem_arg) => {
            target.emit(wasm_encoder::Instruction::F32Load(mem_arg.into()))
        }
        Instr::Load(NumOrVecType::Num(NumType::F64), mem_arg) => {
            target.emit(wasm_encoder::Instruction::F64Load(mem_arg.into()))
        }
        Instr::Load(NumOrVecType::Vec(VecType::V128), mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load(mem_arg.into()))
        }
        Instr::Store(NumOrVecType::Num(NumType::I32), mem_arg) => {
            target.emit(wasm_encoder::Instruction::I32Store(mem_arg.into()))
        }
        Instr::Store(NumOrVecType::Num(NumType::I64), mem_arg) => {
            target.emit(wasm_encoder::Instruction::I64Store(mem_arg.into()))
        }
        Instr::Store(NumOrVecType::Num(NumType::F32), mem_arg) => {
            target.emit(wasm_encoder::Instruction::F32Store(mem_arg.into()))
        }
        Instr::Store(NumOrVecType::Num(NumType::F64), mem_arg) => {
            target.emit(wasm_encoder::Instruction::F64Store(mem_arg.into()))
        }
        Instr::Store(NumOrVecType::Vec(VecType::V128), mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Store(mem_arg.into()))
        }
        Instr::Load8(NumType::I32, Signedness::Signed, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I32Load8S(mem_arg.into()))
        }
        Instr::Load8(NumType::I32, Signedness::Unsigned, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I32Load8U(mem_arg.into()))
        }
        Instr::Load8(NumType::I64, Signedness::Signed, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I64Load8S(mem_arg.into()))
        }
        Instr::Load8(NumType::I64, Signedness::Unsigned, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I64Load8U(mem_arg.into()))
        }
        Instr::Load8(NumType::F32, Signedness::Signed, _mem_arg) => {
            return Err("invalid instruction: F32Load8S".to_string());
        }
        Instr::Load8(NumType::F32, Signedness::Unsigned, _mem_arg) => {
            return Err("invalid instruction: F32Load8U".to_string());
        }
        Instr::Load8(NumType::F64, Signedness::Signed, _mem_arg) => {
            return Err("invalid instruction: F64Load8S".to_string());
        }
        Instr::Load8(NumType::F64, Signedness::Unsigned, _mem_arg) => {
            return Err("invalid instruction: F64Load8U".to_string());
        }
        Instr::Load16(NumType::I32, Signedness::Signed, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I32Load16S(mem_arg.into()))
        }
        Instr::Load16(NumType::I32, Signedness::Unsigned, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I32Load16U(mem_arg.into()))
        }
        Instr::Load16(NumType::I64, Signedness::Signed, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I64Load16S(mem_arg.into()))
        }
        Instr::Load16(NumType::I64, Signedness::Unsigned, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I64Load16U(mem_arg.into()))
        }
        Instr::Load16(NumType::F32, Signedness::Signed, _mem_arg) => {
            return Err("invalid instruction: F32Load16S".to_string());
        }
        Instr::Load16(NumType::F32, Signedness::Unsigned, _mem_arg) => {
            return Err("invalid instruction: F32Load16U".to_string());
        }
        Instr::Load16(NumType::F64, Signedness::Signed, _mem_arg) => {
            return Err("invalid instruction: F64Load16S".to_string());
        }
        Instr::Load16(NumType::F64, Signedness::Unsigned, _mem_arg) => {
            return Err("invalid instruction: F64Load16U".to_string());
        }
        Instr::Load32(Signedness::Signed, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I64Load32S(mem_arg.into()))
        }
        Instr::Load32(Signedness::Unsigned, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I64Load32U(mem_arg.into()))
        }
        Instr::Store8(NumType::I32, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I32Store8(mem_arg.into()))
        }
        Instr::Store8(NumType::I64, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I64Store8(mem_arg.into()))
        }
        Instr::Store8(NumType::F32, _mem_arg) => {
            return Err("invalid instruction: F32Store8".to_string());
        }
        Instr::Store8(NumType::F64, _mem_arg) => {
            return Err("invalid instruction: F64Store8".to_string());
        }
        Instr::Store16(NumType::I32, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I32Store16(mem_arg.into()))
        }
        Instr::Store16(NumType::I64, mem_arg) => {
            target.emit(wasm_encoder::Instruction::I64Store16(mem_arg.into()))
        }
        Instr::Store16(NumType::F32, _mem_arg) => {
            return Err("invalid instruction: F32Store16".to_string());
        }
        Instr::Store16(NumType::F64, _mem_arg) => {
            return Err("invalid instruction: F64Store16".to_string());
        }
        Instr::Store32(mem_arg) => {
            target.emit(wasm_encoder::Instruction::I64Store32(mem_arg.into()))
        }
        Instr::V128Load8x8(Signedness::Signed, mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load8x8S(mem_arg.into()))
        }
        Instr::V128Load8x8(Signedness::Unsigned, mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load8x8U(mem_arg.into()))
        }
        Instr::V128Load16x4(Signedness::Signed, mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load16x4S(mem_arg.into()))
        }
        Instr::V128Load16x4(Signedness::Unsigned, mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load16x4U(mem_arg.into()))
        }
        Instr::V128Load32x2(Signedness::Signed, mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load32x2S(mem_arg.into()))
        }
        Instr::V128Load32x2(Signedness::Unsigned, mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load32x2U(mem_arg.into()))
        }
        Instr::V128Load32Zero(mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load32Zero(mem_arg.into()))
        }
        Instr::V128Load64Zero(mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load64Zero(mem_arg.into()))
        }
        Instr::V128LoadSplat(VectorLoadShape::WW8, mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load8Splat(mem_arg.into()))
        }
        Instr::V128LoadSplat(VectorLoadShape::WW16, mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load16Splat(mem_arg.into()))
        }
        Instr::V128LoadSplat(VectorLoadShape::WW32, mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load32Splat(mem_arg.into()))
        }
        Instr::V128LoadSplat(VectorLoadShape::WW64, mem_arg) => {
            target.emit(wasm_encoder::Instruction::V128Load64Splat(mem_arg.into()))
        }
        Instr::V128LoadLane(VectorLoadShape::WW8, mem_arg, lane_idx) => {
            target.emit(wasm_encoder::Instruction::V128Load8Lane {
                memarg: mem_arg.into(),
                lane: *lane_idx,
            })
        }
        Instr::V128LoadLane(VectorLoadShape::WW16, mem_arg, lane_idx) => {
            target.emit(wasm_encoder::Instruction::V128Load16Lane {
                memarg: mem_arg.into(),
                lane: *lane_idx,
            })
        }
        Instr::V128LoadLane(VectorLoadShape::WW32, mem_arg, lane_idx) => {
            target.emit(wasm_encoder::Instruction::V128Load32Lane {
                memarg: mem_arg.into(),
                lane: *lane_idx,
            })
        }
        Instr::V128LoadLane(VectorLoadShape::WW64, mem_arg, lane_idx) => {
            target.emit(wasm_encoder::Instruction::V128Load64Lane {
                memarg: mem_arg.into(),
                lane: *lane_idx,
            })
        }
        Instr::V128StoreLane(VectorLoadShape::WW8, mem_arg, lane_idx) => {
            target.emit(wasm_encoder::Instruction::V128Store8Lane {
                memarg: mem_arg.into(),
                lane: *lane_idx,
            })
        }
        Instr::V128StoreLane(VectorLoadShape::WW16, mem_arg, lane_idx) => {
            target.emit(wasm_encoder::Instruction::V128Store16Lane {
                memarg: mem_arg.into(),
                lane: *lane_idx,
            })
        }
        Instr::V128StoreLane(VectorLoadShape::WW32, mem_arg, lane_idx) => {
            target.emit(wasm_encoder::Instruction::V128Store32Lane {
                memarg: mem_arg.into(),
                lane: *lane_idx,
            })
        }
        Instr::V128StoreLane(VectorLoadShape::WW64, mem_arg, lane_idx) => {
            target.emit(wasm_encoder::Instruction::V128Store64Lane {
                memarg: mem_arg.into(),
                lane: *lane_idx,
            })
        }
        Instr::MemorySize => target.emit(wasm_encoder::Instruction::MemorySize(0)),
        Instr::MemoryGrow => target.emit(wasm_encoder::Instruction::MemoryGrow(0)),
        Instr::MemoryFill => target.emit(wasm_encoder::Instruction::MemoryFill(0)),
        Instr::MemoryCopy => target.emit(wasm_encoder::Instruction::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        }),
        Instr::MemoryInit(data_idx) => target.emit(wasm_encoder::Instruction::MemoryInit {
            mem: 0,
            data_index: *data_idx,
        }),
        Instr::DataDrop(data_idx) => target.emit(wasm_encoder::Instruction::DataDrop(*data_idx)),
        Instr::Nop => target.emit(wasm_encoder::Instruction::Nop),
        Instr::Unreachable => target.emit(wasm_encoder::Instruction::Unreachable),
        Instr::Block(block_type, instrs) => {
            target.emit(wasm_encoder::Instruction::Block(block_type.into()));
            for instr in instrs {
                encode_instr(instr, target)?;
            }
            target.emit(wasm_encoder::Instruction::End);
        }
        Instr::Loop(block_type, instrs) => {
            target.emit(wasm_encoder::Instruction::Loop(block_type.into()));
            for instr in instrs {
                encode_instr(instr, target)?;
            }
            target.emit(wasm_encoder::Instruction::End);
        }
        Instr::If(block_type, true_instrs, false_instrs) => {
            target.emit(wasm_encoder::Instruction::If(block_type.into()));
            for instr in true_instrs {
                encode_instr(instr, target)?;
            }
            if false_instrs.is_empty() {
                target.emit(wasm_encoder::Instruction::Else);
                for instr in false_instrs {
                    encode_instr(instr, target)?;
                }
            }
            target.emit(wasm_encoder::Instruction::End);
        }
        Instr::Br(label_idx) => target.emit(wasm_encoder::Instruction::Br(*label_idx)),
        Instr::BrIf(label_idx) => target.emit(wasm_encoder::Instruction::BrIf(*label_idx)),
        Instr::BrTable(labels, default) => target.emit(wasm_encoder::Instruction::BrTable(
            Cow::from(labels),
            *default,
        )),
        Instr::Return => target.emit(wasm_encoder::Instruction::Return),
        Instr::Call(func_idx) => target.emit(wasm_encoder::Instruction::Call(*func_idx)),
        Instr::CallIndirect(table_idx, type_idx) => {
            target.emit(wasm_encoder::Instruction::CallIndirect {
                table_index: *table_idx,
                type_index: *type_idx,
            })
        }
    }
    Ok(())
}
