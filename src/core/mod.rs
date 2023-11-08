use crate::{IndexSpace, Section, SectionType, Sections};
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, Mutex};

#[cfg(feature = "parser")]
pub mod parser;
#[cfg(feature = "writer")]
pub mod writer;

pub type DataIdx = u32;
pub type ElemIdx = u32;
pub type ExportIdx = u32;
pub type FuncIdx = u32;
pub type GlobalIdx = u32;
pub type LabelIdx = u32;
pub type LocalIdx = u32;
pub type MemIdx = u32;
pub type TableIdx = u32;
pub type TypeIdx = u32;

#[derive(Debug, Clone, PartialEq)]
pub enum CoreSection<Expr> {
    Type(FuncType),
    Func(FuncTypeRef),
    Code(FuncCode<Expr>),
    Table(Table),
    Mem(Mem),
    Global(Global),
    Elem(Elem<Expr>),
    Data(Data<Expr>),
    DataCount(DataCount),
    Start(Start),
    Export(Export),
    Import(Import),
    Custom(Custom),
}

impl<Expr: Debug + Clone + PartialEq> Section<CoreIndexSpace, CoreSectionType>
    for CoreSection<Expr>
{
    fn index_space(&self) -> CoreIndexSpace {
        match self {
            CoreSection::Type(inner) => inner.index_space(),
            CoreSection::Func(inner) => inner.index_space(),
            CoreSection::Code(inner) => inner.index_space(),
            CoreSection::Table(inner) => inner.index_space(),
            CoreSection::Mem(inner) => inner.index_space(),
            CoreSection::Global(inner) => inner.index_space(),
            CoreSection::Elem(inner) => inner.index_space(),
            CoreSection::Data(inner) => inner.index_space(),
            CoreSection::DataCount(inner) => inner.index_space(),
            CoreSection::Start(inner) => inner.index_space(),
            CoreSection::Export(inner) => inner.index_space(),
            CoreSection::Import(inner) => inner.index_space(),
            CoreSection::Custom(inner) => inner.index_space(),
        }
    }

    fn section_type(&self) -> CoreSectionType {
        match self {
            CoreSection::Type(inner) => inner.section_type(),
            CoreSection::Func(inner) => inner.section_type(),
            CoreSection::Code(inner) => inner.section_type(),
            CoreSection::Table(inner) => inner.section_type(),
            CoreSection::Mem(inner) => inner.section_type(),
            CoreSection::Global(inner) => inner.section_type(),
            CoreSection::Elem(inner) => inner.section_type(),
            CoreSection::Data(inner) => inner.section_type(),
            CoreSection::DataCount(inner) => inner.section_type(),
            CoreSection::Start(inner) => inner.section_type(),
            CoreSection::Export(inner) => inner.section_type(),
            CoreSection::Import(inner) => inner.section_type(),
            CoreSection::Custom(inner) => inner.section_type(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CoreSectionType {
    Type,
    Func,
    Code,
    Table,
    Mem,
    Global,
    Elem,
    Data,
    DataCount,
    Start,
    Export,
    Import,
    Custom,
}

impl SectionType for CoreSectionType {
    fn allow_grouping(&self) -> bool {
        match self {
            CoreSectionType::Type => true,
            CoreSectionType::Func => true,
            CoreSectionType::Code => true,
            CoreSectionType::Table => true,
            CoreSectionType::Mem => true,
            CoreSectionType::Global => true,
            CoreSectionType::Elem => true,
            CoreSectionType::Data => true,
            CoreSectionType::DataCount => false,
            CoreSectionType::Start => false,
            CoreSectionType::Export => true,
            CoreSectionType::Import => true,
            CoreSectionType::Custom => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CoreIndexSpace {
    Type,
    Func,
    Table,
    Mem,
    Global,
    Elem,
    Data,
    Local,
    Label,
    Code,
    Export,
    Start,
    Custom,
}

impl IndexSpace for CoreIndexSpace {
    type Index = u32;
}

/// Number types classify numeric values.
///
/// The types i32 and i64 classify 32 and 64 bit integers, respectively. Integers are not inherently signed or unsigned,
/// their interpretation is determined by individual operations.
///
/// The types f32 and f64 classify 32 and 64 bit floating-point data, respectively. They correspond to the respective
/// binary floating-point representations, also known as single and double precision, as defined by the IEEE 754
/// standard (Section 3.3).
///
/// Number types are transparent, meaning that their bit patterns can be observed. Values of number type can be stored
/// in memories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumType {
    I32,
    I64,
    F32,
    F64,
}

/// Vector types classify vectors of numeric values processed by vector instructions (also known as SIMD instructions,
/// single instruction multiple data).
///
/// The type v128 corresponds to a 128 bit vector of packed integer or floating-point data. The packed data can be
/// interpreted as signed or unsigned integers, single or double precision floating-point values, or a single 128 bit
/// type. The interpretation is determined by individual operations.
///
/// Vector types, like number types are transparent, meaning that their bit patterns can be observed. Values of vector
/// type can be stored in memories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VecType {
    V128,
}

/// Reference types classify first-class references to objects in the runtime store.
///
/// The type funcref denotes the infinite union of all references to functions, regardless of their function types.
///
/// The type externref denotes the infinite union of all references to objects owned by the embedder and that can be
/// passed into WebAssembly under this type.
///
/// Reference types are opaque, meaning that neither their size nor their bit pattern can be observed. Values of
/// reference type can be stored in tables.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefType {
    FuncRef,
    ExternRef,
}

/// Value types classify the individual values that WebAssembly code can compute with and the values that a variable
/// accepts. They are either number types, vector types, or reference types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValType {
    Num(NumType),
    Vec(VecType),
    Ref(RefType),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumOrVecType {
    Num(NumType),
    Vec(VecType),
}

/// Result types classify the result of executing instructions or functions, which is a sequence of values, written with
/// brackets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultType {
    pub values: Vec<ValType>,
}

/// Function types classify the signature of functions, mapping a vector of parameters to a vector of results. They are
/// also used to classify the inputs and outputs of instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncType {
    pub input: ResultType,
    pub output: ResultType,
}

impl Section<CoreIndexSpace, CoreSectionType> for FuncType {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Type
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Type
    }
}

/// Limits classify the size range of resizeable storage associated with memory types and table types.
///
/// If no maximum is given, the respective storage can grow to any size.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    min: u32,
    max: Option<u32>,
}

/// Memory types classify linear memories and their size range.
///
/// The limits constrain the minimum and optionally the maximum size of a memory. The limits are given in units of page
/// size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemType {
    pub limits: Limits,
}

/// Table types classify tables over elements of reference type within a size range.
///
/// Like memories, tables are constrained by limits for their minimum and optionally maximum size. The limits are given
/// in numbers of entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableType {
    pub limits: Limits,
    pub elements: RefType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mut {
    Const,
    Var,
}

/// Global types classify global variables, which hold a value and can either be mutable or immutable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalType {
    pub mutability: Mut,
    pub val_type: ValType,
}

/// External types classify imports and external values with their respective types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternType {
    Func(FuncType),
    Table(TableType),
    Mem(MemType),
    Global(GlobalType),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncTypeRef {
    pub type_idx: TypeIdx,
}

impl Section<CoreIndexSpace, CoreSectionType> for FuncTypeRef {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Func
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Func
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuncCode<Expr> {
    pub locals: Vec<ValType>,
    pub body: Expr,
}

impl<Expr: Debug + Clone + PartialEq> Section<CoreIndexSpace, CoreSectionType> for FuncCode<Expr> {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Func
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Code
    }
}

/// The funcs component of a module defines a vector of functions with the following structure.
///
/// Functions are referenced through function indices, starting with the smallest index not referencing a function
/// import.
///
/// `typ` is the type of a function declares its signature by reference to a type defined in the module. The parameters of the
/// function are referenced through 0-based local indices in the function’s body; they are mutable.
///
/// The `locals` declare a vector of mutable local variables and their types. These variables are referenced through
/// local indices in the function’s body. The index of the first local is the smallest index not referencing a
/// parameter.
///
/// The `body` is an instruction sequence that upon termination must produce a stack matching the function type’s result
/// type.
/// /
#[derive(Debug, Clone, PartialEq)]
pub struct Func<Expr> {
    pub type_idx: TypeIdx,
    pub locals: Vec<ValType>,
    pub body: Expr,
}

/// The tables component of a module defines a vector of tables described by their table type:
///
/// A table is a vector of opaque values of a particular reference type. The size in the limits of the table type
/// specifies the initial size of that table, while its max, if present, restricts the size to which it can grow later.
///
/// Tables can be initialized through element segments.
///
/// Tables are referenced through table indices, starting with the smallest index not referencing a table import. Most
/// constructs implicitly reference table index 0.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub table_type: TableType,
}

impl Section<CoreIndexSpace, CoreSectionType> for Table {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Table
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Table
    }
}

/// The mems component of a module defines a vector of linear memories (or memories for short) as described by their
/// memory type:
///
/// A memory is a vector of raw uninterpreted bytes. The size in the limits of the memory type specifies the initial
/// size of that memory, while its max, if present, restricts the size to which it can grow later. Both are in units of
/// page size.
///
/// Memories can be initialized through data segments.
///
/// Memories are referenced through memory indices, starting with the smallest index not referencing a memory import.
/// Most constructs implicitly reference memory index 0.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mem {
    pub mem_type: MemType,
}

impl Section<CoreIndexSpace, CoreSectionType> for Mem {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Mem
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Mem
    }
}

/// The globals component of a module defines a vector of global variables (or globals for short):
///
/// Each global stores a single value of the given global type. Its type also specifies whether a global is immutable or
/// mutable. Moreover, each global is initialized with an value given by a constant initializer expression.
///
/// Globals are referenced through global indices, starting with the smallest index not referencing a global import.
#[derive(Debug, Clone, PartialEq)]
pub struct Global {
    pub global_type: GlobalType,
    pub init: Expr,
}

impl Section<CoreIndexSpace, CoreSectionType> for Global {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Global
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Global
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ElemMode {
    Passive,
    Active { table_idx: TableIdx, offset: Expr },
    Declarative,
}

/// The initial contents of a table is uninitialized. Element segments can be used to initialize a subrange of a table
/// from a static vector of elements.
///
/// The elems component of a module defines a vector of element segments. Each element segment defines a reference type
/// and a corresponding list of constant element expressions.
///
/// Element segments have a mode that identifies them as either passive, active, or declarative. A passive element
/// segment’s elements can be copied to a table using the table.init instruction. An active element segment copies its
/// elements into a table during instantiation, as specified by a table index and a constant expression defining an
/// offset into that table. A declarative element segment is not available at runtime but merely serves to
/// forward-declare references that are formed in code with instructions like ref.func.
///
/// Element segments are referenced through element indices.
///
#[derive(Debug, Clone, PartialEq)]
pub struct Elem<Expr> {
    pub ref_type: RefType,
    pub init: Vec<Expr>,
    pub mode: ElemMode,
}

impl<Expr: Debug + Clone + PartialEq> Section<CoreIndexSpace, CoreSectionType> for Elem<Expr> {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Elem
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Elem
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DataMode<Expr> {
    Passive,
    Active { memory: MemIdx, offset: Expr },
}

/// The initial contents of a memory are zero bytes. Data segments can be used to initialize a range of memory from a
/// static vector of bytes.
///
/// The datas component of a module defines a vector of data segments.
///
/// Like element segments, data segments have a mode that identifies them as either passive or active. A passive data
/// segment’s contents can be copied into a memory using the memory.init instruction. An active data segment copies its
/// contents into a memory during instantiation, as specified by a memory index and a constant expression defining an
/// offset into that memory.
///
/// Data segments are referenced through data indices.
///
#[derive(Debug, Clone, PartialEq)]
pub struct Data<Expr> {
    init: Vec<u8>,
    mode: DataMode<Expr>,
}

impl<Expr: Debug + Clone + PartialEq> Section<CoreIndexSpace, CoreSectionType> for Data<Expr> {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Data
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Data
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataCount {
    pub count: u32,
}

impl Section<CoreIndexSpace, CoreSectionType> for DataCount {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Data
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::DataCount
    }
}

/// The start component of a module declares the function index of a start function that is automatically invoked when
/// the module is instantiated, after tables and memories have been initialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub func: FuncIdx,
}

impl Section<CoreIndexSpace, CoreSectionType> for Start {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Start
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Start
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportDesc {
    Func(FuncIdx),
    Table(TableIdx),
    Mem(MemIdx),
    Global(GlobalIdx),
}

/// The exports component of a module defines a set of exports that become accessible to the host environment once the
/// module has been instantiated.
///
/// Each export is labeled by a unique name. Exportable definitions are functions, tables, memories, and globals, which
/// are referenced through a respective descriptor.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub name: String,
    pub desc: ExportDesc,
}

impl Section<CoreIndexSpace, CoreSectionType> for Export {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Export
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Export
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    Func(TypeIdx),
    Table(TableType),
    Mem(MemType),
    Global(GlobalType),
}

/// The imports component of a module defines a set of imports that are required for instantiation.
///
/// Each import is labeled by a two-level name space, consisting of a module name and a name for an entity within that
/// module. Importable definitions are functions, tables, memories, and globals. Each import is specified by a
/// descriptor with a respective type that a definition provided during instantiation is required to match.
///
/// Every import defines an index in the respective index space. In each index space, the indices of imports go before
/// the first index of any definition contained in the module itself.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub desc: TypeRef,
}

impl Section<CoreIndexSpace, CoreSectionType> for Import {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Func
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Import
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Custom {
    pub name: String,
    pub data: Vec<u8>,
}

impl Section<CoreIndexSpace, CoreSectionType> for Custom {
    fn index_space(&self) -> CoreIndexSpace {
        CoreIndexSpace::Custom
    }

    fn section_type(&self) -> CoreSectionType {
        CoreSectionType::Custom
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub instrs: Vec<Instr>,
}

pub trait ExprSource: IntoIterator<Item = Result<Instr, String>> {
    fn unparsed(self) -> Result<Vec<u8>, String>;
}

pub trait ExprSink: IntoIterator<Item = Instr> {}

pub trait TryFromExprSource {
    fn try_from<S: ExprSource>(value: S) -> Result<Self, String>
    where
        Self: Sized;
}

impl TryFromExprSource for Expr {
    fn try_from<S: ExprSource>(value: S) -> Result<Self, String> {
        let instrs = value.into_iter().collect::<Result<Vec<Instr>, String>>()?;
        Ok(Self { instrs })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntWidth {
    I32,
    I64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FloatWidth {
    F32,
    F64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signedness {
    Signed,
    Unsigned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IShape {
    I8x16,
    I16x8,
    I32x4,
    I64x2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FShape {
    F32x4,
    F64x2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    Int(IShape),
    Float(FShape),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Half {
    Low,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemArg {
    pub align: u8,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VectorLoadShape {
    WW8,
    WW16,
    WW32,
    WW64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockType {
    None,
    Index(TypeIdx),
    Value(ValType),
}

pub type LaneIdx = u8;

#[derive(Debug, Clone, PartialEq)]
pub enum Instr {
    // NumericInstr
    I32Const(i32),
    I64Const(i64),
    F32Const(f32),
    F64Const(f64),

    // ITestOp
    IEqz(IntWidth),

    // IRelOp
    IEq(IntWidth),
    INe(IntWidth),
    ILt(IntWidth, Signedness),
    IGt(IntWidth, Signedness),
    ILe(IntWidth, Signedness),
    IGe(IntWidth, Signedness),

    // FRelOp
    FEq(FloatWidth),
    FNe(FloatWidth),
    FLt(FloatWidth),
    FGt(FloatWidth),
    FLe(FloatWidth),
    FGe(FloatWidth),

    // IUnOp
    IClz(IntWidth),
    ICtz(IntWidth),
    IPopCnt(IntWidth),

    // IBinOp
    IAdd(IntWidth),
    ISub(IntWidth),
    IMul(IntWidth),
    IDiv(IntWidth, Signedness),
    IRem(IntWidth, Signedness),
    IAnd(IntWidth),
    IOr(IntWidth),
    IXor(IntWidth),
    IShl(IntWidth),
    IShr(IntWidth, Signedness),
    IRotL(IntWidth),
    IRotR(IntWidth),

    // FUnOp
    FAbs(FloatWidth),
    FNeg(FloatWidth),
    FCeil(FloatWidth),
    FFloor(FloatWidth),
    FTrunc(FloatWidth),
    FNearest(FloatWidth),

    // FBinOp
    FSqrt(FloatWidth),
    FAdd(FloatWidth),
    FSub(FloatWidth),
    FMul(FloatWidth),
    FDiv(FloatWidth),
    FMin(FloatWidth),
    FMax(FloatWidth),
    FCopySign(FloatWidth),

    I32WrapI64,

    ITruncF(IntWidth, FloatWidth, Signedness),

    I64ExtendI32(Signedness),
    I64Extend32S,
    IExtend8S(IntWidth),
    IExtend16S(IntWidth),

    FConvertI(FloatWidth, IntWidth, Signedness),

    F32DemoteF64,
    F64PromoteF32,

    IReinterpretF(IntWidth),
    FReinterpretI(FloatWidth),

    ITruncSatF(IntWidth, FloatWidth, Signedness),

    // VectorInstr
    V128Const(i128),

    // VVUnOp
    V128Not,

    // VVBinOp
    V128And,
    V128AndNot,
    V128Or,
    V128XOr,

    // VVTernOp
    V128BitSelect,

    // VVTestOp
    V128AnyTrue,

    VI8x16Shuffle([LaneIdx; 16]),

    VI18x16Swizzle,
    VSplat(Shape),
    VI8x16ExtractLane(Signedness, LaneIdx),
    VI16x8ExtractLane(Signedness, LaneIdx),
    VI32x4ExtractLane(LaneIdx),
    VI64x2ExtractLane(LaneIdx),
    VFExtractLane(FShape, LaneIdx),
    VReplaceLane(Shape, LaneIdx),

    // VIRelOp
    VIEq(IShape),
    VINe(IShape),
    VILt(IShape, Signedness),
    VIGt(IShape, Signedness),
    VILe(IShape, Signedness),
    VIGe(IShape, Signedness),
    VI64x2Lt,
    VI64x2Gt,
    VI64x2Le,
    VI64x2Ge,

    // VFRelOp
    VFEq(FShape),
    VFNe(FShape),
    VFLt(FShape),
    VFGt(FShape),
    VFLe(FShape),
    VFGe(FShape),

    // VIUnOp
    VIAbs(IShape),
    VINeg(IShape),

    VI8x16PopCnt,
    VI16x8Q15MulrSat,
    VI32x4DotI16x8,

    // VFUnOp
    VFAbs(FShape),
    VFNeg(FShape),
    VFSqrt(FShape),
    VFCeil(FShape),
    VFFloor(FShape),
    VFTrunc(FShape),
    VFNearest(FShape),

    // VITestOp
    VIAllTrue(IShape),

    VIBitMask(IShape),

    VI8x16NarrowI16x8(Signedness),
    VI16x8NarrowI32x4(Signedness),

    VI16x8ExtendI8x16(Half, Signedness),
    VI32x4ExtendI16x8(Half, Signedness),
    VI64x2ExtendI32x4(Half, Signedness),

    // VIShiftOp
    VIShl(IShape),
    VIShr(IShape, Signedness),

    // VIBinOp
    VIAdd(IShape),
    VISub(IShape),

    // VIMinMaxOp
    VIMin(IShape, Signedness),
    VIMax(IShape, Signedness),

    // VISatBinOp
    VIAddSat(IShape, Signedness),
    VISubSat(IShape, Signedness),

    VIMul(IShape),
    VIAvgr(IShape),
    VIExtMul(IShape, Half, Signedness),
    VIExtAddPairwise(IShape, Signedness),

    // VFBinOp
    VFAdd(FShape),
    VFSub(FShape),
    VFMul(FShape),
    VFDiv(FShape),
    VFMin(FShape),
    VFMax(FShape),
    VFPMin(FShape),
    VFPMax(FShape),

    VI32x4TruncSatF32x4(Signedness),
    VI32x4TruncSatF64x2Zero(Signedness),
    VI32x4ConvertI32x4(Signedness),
    VF32x4DemoteF64x2Zero,
    VF64x2ConvertLowI32x4(Signedness),
    VF64x2PromoteLowI32x4,

    // ReferenceInstr
    RefNull(RefType),
    RefIsNull,
    RefFunc(FuncIdx),

    // ParametricInstr
    Drop,
    Select(Option<Vec<ValType>>),

    // VariableInstr
    LocalGet(LocalIdx),
    LocalSet(LocalIdx),
    LocalTee(LocalIdx),
    GlobalGet(GlobalIdx),
    GlobalSet(GlobalIdx),

    // TableInstr
    TableGet(TableIdx),
    TableSet(TableIdx),
    TableSize(TableIdx),
    TableGrow(TableIdx),
    TableFill(TableIdx),
    TableCopy {
        source: TableIdx,
        destination: TableIdx,
    },
    TableInit(TableIdx, ElemIdx),
    ElemDrop(ElemIdx),

    // MemoryInstr
    Load(NumOrVecType, MemArg),
    Store(NumOrVecType, MemArg),
    Load8(NumType, Signedness, MemArg),
    Load16(NumType, Signedness, MemArg),
    Load32(Signedness, MemArg),
    Store8(NumType, MemArg),
    Store16(NumType, MemArg),
    Store32(MemArg),
    V128Load8x8(Signedness, MemArg),
    V128Load16x4(Signedness, MemArg),
    V128Load32x2(Signedness, MemArg),
    V128Load32Zero(MemArg),
    V128Load64Zero(MemArg),
    V128LoadSplat(VectorLoadShape, MemArg),
    V128LoadLane(VectorLoadShape, MemArg, LaneIdx),
    V128StoreLane(VectorLoadShape, MemArg, LaneIdx),
    MemorySize,
    MemoryGrow,
    MemoryFill,
    MemoryCopy,
    MemoryInit(DataIdx),
    DataDrop(DataIdx),

    // ControlInstr
    Nop,
    Unreachable,
    Block(BlockType, Vec<Instr>),
    Loop(BlockType, Vec<Instr>),
    If(BlockType, Vec<Instr>, Vec<Instr>),
    Br(LabelIdx),
    BrIf(LabelIdx),
    BrTable(Vec<LabelIdx>, LabelIdx),
    Return,
    Call(FuncIdx),
    CallIndirect(TableIdx, TypeIdx),
}

#[derive(Debug, Clone)]
pub enum ImportOrFunc<Expr> {
    Import(Import),
    Func(Func<Expr>),
}

struct ModuleInner<Expr: Debug + Clone + PartialEq> {
    sections: Sections<CoreIndexSpace, CoreSectionType, CoreSection<Expr>>,

    types: Option<Vec<FuncType>>,
    funcs: Option<Vec<Func<Expr>>>,
    tables: Option<Vec<Table>>,
    mems: Option<Vec<Mem>>,
    globals: Option<Vec<Global>>,
    elems: Option<Vec<Elem<Expr>>>,
    datas: Option<Vec<Data<Expr>>>,
    start: Option<Start>,
    imports: Option<Vec<Import>>,
    exports: Option<Vec<Export>>,
    customs: Option<Vec<Custom>>,

    type_index: Option<HashMap<u32, CoreSection<Expr>>>,
    func_index: Option<HashMap<u32, CoreSection<Expr>>>,
    code_index: Option<HashMap<u32, CoreSection<Expr>>>,
    table_index: Option<HashMap<u32, CoreSection<Expr>>>,
    mem_index: Option<HashMap<u32, CoreSection<Expr>>>,
    global_index: Option<HashMap<u32, CoreSection<Expr>>>,
    elem_index: Option<HashMap<u32, CoreSection<Expr>>>,
    data_index: Option<HashMap<u32, CoreSection<Expr>>>,
    export_index: Option<HashMap<u32, CoreSection<Expr>>>,
    import_index: Option<HashMap<u32, CoreSection<Expr>>>,
    custom_index: Option<HashMap<u32, CoreSection<Expr>>>,
}

impl<Expr: Debug + Clone + PartialEq> ModuleInner<Expr> {
    pub fn new(sections: Sections<CoreIndexSpace, CoreSectionType, CoreSection<Expr>>) -> Self {
        Self {
            sections,
            types: None,
            funcs: None,
            tables: None,
            mems: None,
            globals: None,
            elems: None,
            datas: None,
            start: None,
            imports: None,
            exports: None,
            customs: None,
            type_index: None,
            func_index: None,
            code_index: None,
            table_index: None,
            mem_index: None,
            global_index: None,
            elem_index: None,
            data_index: None,
            export_index: None,
            import_index: None,
            custom_index: None,
        }
    }

    pub fn types(&mut self) -> Vec<FuncType> {
        self.ensure_types();
        self.types.clone().unwrap()
    }
    pub fn funcs(&mut self) -> Vec<Func<Expr>> {
        self.ensure_funcs();
        self.funcs.clone().unwrap()
    }
    pub fn tables(&mut self) -> Vec<Table> {
        self.ensure_tables();
        self.tables.clone().unwrap()
    }
    pub fn mems(&mut self) -> Vec<Mem> {
        self.ensure_mems();
        self.mems.clone().unwrap()
    }
    pub fn globals(&mut self) -> Vec<Global> {
        self.ensure_globals();
        self.globals.clone().unwrap()
    }
    pub fn elems(&mut self) -> Vec<Elem<Expr>> {
        self.ensure_elems();
        self.elems.clone().unwrap()
    }
    pub fn datas(&mut self) -> Vec<Data<Expr>> {
        self.ensure_datas();
        self.datas.clone().unwrap()
    }
    pub fn start(&mut self) -> Option<Start> {
        self.ensure_start();
        self.start.clone()
    }
    pub fn imports(&mut self) -> Vec<Import> {
        self.ensure_imports();
        self.imports.clone().unwrap()
    }
    pub fn exports(&mut self) -> Vec<Export> {
        self.ensure_exports();
        self.exports.clone().unwrap()
    }
    pub fn customs(&mut self) -> Vec<Custom> {
        self.ensure_customs();
        self.customs.clone().unwrap()
    }

    pub fn add_data(&mut self, data: Data<Expr>) {
        self.invalidate();
        self.sections.add_to_last_group(CoreSection::Data(data));
        self.sections
            .map_section_by_section_type(CoreSectionType::DataCount, |section| {
                if let CoreSection::DataCount(DataCount { count }) = section {
                    *count += 1;
                }
            });
    }

    pub fn add_elem(&mut self, elem: Elem<Expr>) {
        self.invalidate();
        self.sections.add_to_last_group(CoreSection::Elem(elem));
    }

    pub fn add_export(&mut self, export: Export) {
        self.invalidate();
        self.sections.add_to_last_group(CoreSection::Export(export));
    }

    pub fn add_function(
        &mut self,
        func_type: FuncType,
        locals: Vec<ValType>,
        body: Expr,
    ) -> FuncIdx {
        self.ensure_types();
        let existing_type_idx = self.type_idx_of(&func_type);
        let type_idx = match existing_type_idx {
            Some(idx) => idx as TypeIdx,
            None => {
                let idx = self.types.as_ref().unwrap().len() as TypeIdx;
                self.sections
                    .add_to_last_group(CoreSection::Type(func_type));
                idx
            }
        };
        let func_type_ref = FuncTypeRef { type_idx };
        let func_code = FuncCode { locals, body };
        self.sections
            .add_to_last_group(CoreSection::Func(func_type_ref));
        self.sections
            .add_to_last_group(CoreSection::Code(func_code));
        self.invalidate();
        self.ensure_funcs();
        (self.funcs.as_ref().unwrap().len() as u32) - 1 as FuncIdx
    }

    pub fn add_global(&mut self, global: Global) {
        self.invalidate();
        self.sections.add_to_last_group(CoreSection::Global(global));
    }

    pub fn add_memory(&mut self, mem: Mem) {
        self.invalidate();
        self.sections.add_to_last_group(CoreSection::Mem(mem));
    }

    pub fn add_table(&mut self, table: Table) {
        self.invalidate();
        self.sections.add_to_last_group(CoreSection::Table(table));
    }

    pub fn add_type(&mut self, func_type: FuncType) {
        self.invalidate();
        self.sections
            .add_to_last_group(CoreSection::Type(func_type));
    }

    pub fn get_code(&mut self, func_idx: FuncIdx) -> Option<FuncCode<Expr>> {
        self.ensure_code_index();
        match self.code_index.as_ref().unwrap().get(&func_idx) {
            Some(CoreSection::Code(code)) => Some(code.clone()),
            _ => None,
        }
    }

    pub fn get_data(&mut self, data_idx: DataIdx) -> Option<Data<Expr>> {
        self.ensure_data_index();
        match self.data_index.as_ref().unwrap().get(&data_idx) {
            Some(CoreSection::Data(data)) => Some(data.clone()),
            _ => None,
        }
    }

    pub fn get_elem(&mut self, elem_idx: ElemIdx) -> Option<Elem<Expr>> {
        self.ensure_elem_index();
        match self.elem_index.as_ref().unwrap().get(&elem_idx) {
            Some(CoreSection::Elem(elem)) => Some(elem.clone()),
            _ => None,
        }
    }

    pub fn get_export(&mut self, export_idx: ExportIdx) -> Option<Export> {
        self.ensure_export_index();
        match self.export_index.as_ref().unwrap().get(&export_idx) {
            Some(CoreSection::Export(export)) => Some(export.clone()),
            _ => None,
        }
    }

    pub fn get_function(&mut self, func_idx: FuncIdx) -> Option<ImportOrFunc<Expr>> {
        self.ensure_func_index();
        match self.func_index.as_ref().unwrap().get(&func_idx) {
            Some(CoreSection::Func(FuncTypeRef { type_idx })) => {
                let type_idx = *type_idx;
                let code = self.get_code(func_idx).unwrap();
                Some(ImportOrFunc::Func(Func {
                    type_idx,
                    locals: code.locals.clone(),
                    body: code.body.clone(),
                }))
            }
            Some(CoreSection::Import(import)) => Some(ImportOrFunc::Import(import.clone())),
            _ => None,
        }
    }

    pub fn get_global(&mut self, global_idx: GlobalIdx) -> Option<Global> {
        self.ensure_global_index();
        match self.global_index.as_ref().unwrap().get(&global_idx) {
            Some(CoreSection::Global(global)) => Some(global.clone()),
            _ => None,
        }
    }

    pub fn get_memory(&mut self, mem_idx: MemIdx) -> Option<Mem> {
        self.ensure_mem_index();
        match self.mem_index.as_ref().unwrap().get(&mem_idx) {
            Some(CoreSection::Mem(mem)) => Some(mem.clone()),
            _ => None,
        }
    }

    pub fn get_table(&mut self, table_idx: TableIdx) -> Option<Table> {
        self.ensure_table_index();
        match self.table_index.as_ref().unwrap().get(&table_idx) {
            Some(CoreSection::Table(table)) => Some(table.clone()),
            _ => None,
        }
    }

    pub fn type_idx_of(&self, func_type: &FuncType) -> Option<TypeIdx> {
        self.types
            .as_ref()
            .unwrap()
            .iter()
            .position(|ft| ft == func_type)
            .map(|idx| idx as TypeIdx)
    }

    fn ensure_code_index(&mut self) {
        if self.code_index.is_none() {
            self.code_index = Some(self.sections.indexed(CoreIndexSpace::Code));
        }
    }

    fn ensure_customs(&mut self) {
        if self.customs.is_none() {
            self.customs = Some(
                self.sections
                    .filter_by_section_type(CoreSectionType::Custom)
                    .into_iter()
                    .map(|section| {
                        if let CoreSection::Custom(custom) = section {
                            custom
                        } else {
                            unreachable!()
                        }
                    })
                    .collect(),
            );
        }
    }

    fn ensure_data_index(&mut self) {
        if self.data_index.is_none() {
            self.data_index = Some(self.sections.indexed(CoreIndexSpace::Data));
        }
    }

    fn ensure_datas(&mut self) {
        if self.datas.is_none() {
            self.datas = Some(
                self.sections
                    .filter_by_section_type(CoreSectionType::Data)
                    .into_iter()
                    .map(|section| {
                        if let CoreSection::Data(data) = section {
                            data
                        } else {
                            unreachable!()
                        }
                    })
                    .collect(),
            );
        }
    }

    fn ensure_elem_index(&mut self) {
        if self.elem_index.is_none() {
            self.elem_index = Some(self.sections.indexed(CoreIndexSpace::Elem));
        }
    }

    fn ensure_elems(&mut self) {
        if self.elems.is_none() {
            self.elems = Some(
                self.sections
                    .filter_by_section_type(CoreSectionType::Elem)
                    .into_iter()
                    .map(|section| {
                        if let CoreSection::Elem(elem) = section {
                            elem
                        } else {
                            unreachable!()
                        }
                    })
                    .collect(),
            );
        }
    }

    fn ensure_export_index(&mut self) {
        if self.export_index.is_none() {
            self.export_index = Some(self.sections.indexed(CoreIndexSpace::Export));
        }
    }

    fn ensure_exports(&mut self) {
        if self.exports.is_none() {
            self.exports = Some(
                self.sections
                    .filter_by_section_type(CoreSectionType::Export)
                    .into_iter()
                    .map(|section| {
                        if let CoreSection::Export(export) = section {
                            export
                        } else {
                            unreachable!()
                        }
                    })
                    .collect(),
            );
        }
    }

    fn ensure_func_index(&mut self) {
        if self.func_index.is_none() {
            self.func_index = Some(self.sections.indexed(CoreIndexSpace::Func));
        }
    }

    fn ensure_funcs(&mut self) {
        if self.funcs.is_none() {
            let func_types = self.sections.filter_by_section_type(CoreSectionType::Func);
            let codes = self.sections.filter_by_section_type(CoreSectionType::Code);

            self.funcs = Some(
                func_types
                    .into_iter()
                    .zip(codes.into_iter())
                    .map(|(func_type, code)| match (func_type, code) {
                        (CoreSection::Func(func_type), CoreSection::Code(code)) => Func {
                            type_idx: func_type.type_idx,
                            locals: code.locals,
                            body: code.body,
                        },
                        _ => unreachable!(),
                    })
                    .collect(),
            );
        }
    }

    fn ensure_global_index(&mut self) {
        if self.global_index.is_none() {
            self.global_index = Some(self.sections.indexed(CoreIndexSpace::Global));
        }
    }

    fn ensure_globals(&mut self) {
        if self.globals.is_none() {
            self.globals = Some(
                self.sections
                    .filter_by_section_type(CoreSectionType::Global)
                    .into_iter()
                    .map(|section| {
                        if let CoreSection::Global(global) = section {
                            global
                        } else {
                            unreachable!()
                        }
                    })
                    .collect(),
            );
        }
    }

    fn ensure_imports(&mut self) {
        if self.imports.is_none() {
            self.imports = Some(
                self.sections
                    .filter_by_section_type(CoreSectionType::Import)
                    .into_iter()
                    .map(|section| {
                        if let CoreSection::Import(import) = section {
                            import
                        } else {
                            unreachable!()
                        }
                    })
                    .collect(),
            );
        }
    }

    fn ensure_mem_index(&mut self) {
        if self.mem_index.is_none() {
            self.mem_index = Some(self.sections.indexed(CoreIndexSpace::Mem));
        }
    }

    fn ensure_mems(&mut self) {
        if self.mems.is_none() {
            self.mems = Some(
                self.sections
                    .filter_by_section_type(CoreSectionType::Mem)
                    .into_iter()
                    .map(|section| {
                        if let CoreSection::Mem(mem) = section {
                            mem
                        } else {
                            unreachable!()
                        }
                    })
                    .collect(),
            );
        }
    }

    fn ensure_start(&mut self) {
        if self.start.is_none() {
            self.start = self
                .sections
                .filter_by_section_type(CoreSectionType::Start)
                .into_iter()
                .next()
                .map(|section| {
                    if let CoreSection::Start(start) = section {
                        start
                    } else {
                        unreachable!()
                    }
                });
        }
    }

    fn ensure_table_index(&mut self) {
        if self.funcs.is_none() {
            self.table_index = Some(self.sections.indexed(CoreIndexSpace::Table));
        }
    }

    fn ensure_tables(&mut self) {
        if self.tables.is_none() {
            self.tables = Some(
                self.sections
                    .filter_by_section_type(CoreSectionType::Table)
                    .into_iter()
                    .map(|section| {
                        if let CoreSection::Table(table) = section {
                            table
                        } else {
                            unreachable!()
                        }
                    })
                    .collect(),
            );
        }
    }

    fn ensure_types(&mut self) {
        if self.types.is_none() {
            self.types = Some(
                self.sections
                    .filter_by_section_type(CoreSectionType::Type)
                    .into_iter()
                    .map(|section| {
                        if let CoreSection::Type(func_type) = section {
                            func_type
                        } else {
                            unreachable!()
                        }
                    })
                    .collect(),
            );
        }
    }

    fn invalidate(&mut self) {
        self.types = None;
        self.funcs = None;
        self.tables = None;
        self.mems = None;
        self.globals = None;
        self.elems = None;
        self.datas = None;
        self.start = None;
        self.imports = None;
        self.exports = None;
        self.customs = None;
        self.type_index = None;
        self.func_index = None;
        self.code_index = None;
        self.table_index = None;
        self.mem_index = None;
        self.global_index = None;
        self.elem_index = None;
        self.data_index = None;
        self.export_index = None;
        self.import_index = None;
        self.custom_index = None;
    }
}

impl<Expr: Debug + Clone + PartialEq> Debug for ModuleInner<Expr> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.sections.fmt(f)
    }
}

impl<Expr: Debug + Clone + PartialEq> PartialEq for ModuleInner<Expr> {
    fn eq(&self, other: &Self) -> bool {
        self.sections.eq(&other.sections)
    }
}

// TODO: parametric Expr type
#[derive(Debug, Clone)]
pub struct Module<Expr: Debug + Clone + PartialEq> {
    inner: Arc<Mutex<ModuleInner<Expr>>>,
}

impl<Expr: Debug + Clone + PartialEq> Module<Expr> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ModuleInner::new(Sections::new()))),
        }
    }

    pub fn from_flat(sections: Vec<CoreSection<Expr>>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ModuleInner::new(Sections::from_flat(sections)))),
        }
    }

    pub fn from_grouped(groups: Vec<(CoreSectionType, Vec<CoreSection<Expr>>)>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ModuleInner::new(Sections::from_grouped(groups)))),
        }
    }

    pub fn types(&self) -> Vec<FuncType> {
        self.inner.lock().unwrap().types()
    }

    pub fn funcs(&self) -> Vec<Func<Expr>> {
        self.inner.lock().unwrap().funcs()
    }

    pub fn tables(&self) -> Vec<Table> {
        self.inner.lock().unwrap().tables()
    }

    pub fn mems(&self) -> Vec<Mem> {
        self.inner.lock().unwrap().mems()
    }

    pub fn globals(&self) -> Vec<Global> {
        self.inner.lock().unwrap().globals()
    }

    pub fn elems(&self) -> Vec<Elem<Expr>> {
        self.inner.lock().unwrap().elems()
    }

    pub fn datas(&self) -> Vec<Data<Expr>> {
        self.inner.lock().unwrap().datas()
    }

    pub fn start(&self) -> Option<Start> {
        self.inner.lock().unwrap().start()
    }

    pub fn imports(&self) -> Vec<Import> {
        self.inner.lock().unwrap().imports()
    }

    pub fn exports(&self) -> Vec<Export> {
        self.inner.lock().unwrap().exports()
    }

    pub fn customs(&self) -> Vec<Custom> {
        self.inner.lock().unwrap().customs()
    }

    pub fn add_data(&self, data: Data<Expr>) {
        self.inner.lock().unwrap().add_data(data);
    }

    pub fn add_elem(&self, elem: Elem<Expr>) {
        self.inner.lock().unwrap().add_elem(elem);
    }

    pub fn add_export(&self, export: Export) {
        self.inner.lock().unwrap().add_export(export);
    }

    pub fn add_function(&self, func_type: FuncType, locals: Vec<ValType>, body: Expr) -> FuncIdx {
        self.inner
            .lock()
            .unwrap()
            .add_function(func_type, locals, body)
    }

    pub fn add_global(&self, global: Global) {
        self.inner.lock().unwrap().add_global(global);
    }

    pub fn add_memory(&self, mem: Mem) {
        self.inner.lock().unwrap().add_memory(mem);
    }

    pub fn add_table(&self, table: Table) {
        self.inner.lock().unwrap().add_table(table);
    }

    pub fn add_type(&self, func_type: FuncType) {
        self.inner.lock().unwrap().add_type(func_type);
    }

    pub fn get_code(&self, func_idx: FuncIdx) -> Option<FuncCode<Expr>> {
        self.inner.lock().unwrap().get_code(func_idx)
    }

    pub fn get_data(&self, data_idx: DataIdx) -> Option<Data<Expr>> {
        self.inner.lock().unwrap().get_data(data_idx)
    }

    pub fn get_elem(&self, elem_idx: ElemIdx) -> Option<Elem<Expr>> {
        self.inner.lock().unwrap().get_elem(elem_idx)
    }

    pub fn get_export(&self, export_idx: ExportIdx) -> Option<Export> {
        self.inner.lock().unwrap().get_export(export_idx)
    }

    pub fn get_function(&self, func_idx: FuncIdx) -> Option<ImportOrFunc<Expr>> {
        self.inner.lock().unwrap().get_function(func_idx)
    }

    pub fn get_global(&self, global_idx: GlobalIdx) -> Option<Global> {
        self.inner.lock().unwrap().get_global(global_idx)
    }

    pub fn get_memory(&self, mem_idx: MemIdx) -> Option<Mem> {
        self.inner.lock().unwrap().get_memory(mem_idx)
    }

    pub fn get_table(&self, table_idx: TableIdx) -> Option<Table> {
        self.inner.lock().unwrap().get_table(table_idx)
    }

    pub fn into_sections(self) -> Vec<CoreSection<Expr>> {
        self.inner.lock().unwrap().sections.take_all()
    }

    pub fn type_idx_of(&self, func_type: &FuncType) -> Option<TypeIdx> {
        self.inner.lock().unwrap().type_idx_of(func_type)
    }

    // TODO: metadata section support
}

impl<Expr: Debug + Clone + PartialEq>
    From<Sections<CoreIndexSpace, CoreSectionType, CoreSection<Expr>>> for Module<Expr>
{
    fn from(sections: Sections<CoreIndexSpace, CoreSectionType, CoreSection<Expr>>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ModuleInner::new(sections))),
        }
    }
}

impl<Expr: Debug + Clone + PartialEq> PartialEq for Module<Expr> {
    fn eq(&self, other: &Self) -> bool {
        let inner = self.inner.lock().unwrap();
        let other_inner = other.inner.lock().unwrap();
        inner.eq(&other_inner)
    }
}

#[cfg(feature = "component")]
impl<Expr: Debug + Clone + PartialEq>
    Section<crate::component::ComponentIndexSpace, crate::component::ComponentSectionType>
    for Module<Expr>
{
    fn index_space(&self) -> crate::component::ComponentIndexSpace {
        crate::component::ComponentIndexSpace::Module
    }

    fn section_type(&self) -> crate::component::ComponentSectionType {
        crate::component::ComponentSectionType::Component
    }
}
